// 내 작업 시간표: 배치 계산, 사용자의 수정(옮기기·빼기·추가·시간 변경·안 되는 시간), 캘린더 반영 차이 계산.
// 시간표는 이 기기에만 저장된다. 팀에 보이는 것은 작업 상태뿐이다.
const { planBlocks, normPrefs, DEFAULT_PREFS, HOUR } = require('./time-plan');
const { fmtRange, startOfDay } = require('./korean-time');

const KEEP_PAST_MS = 14 * 24 * HOUR;
const BUSY_CACHE_MS = 2 * 60 * 1000;

let seq = 0;
const newId = () => `b${Date.now().toString(36)}${(seq++).toString(36)}`;

const DEFAULT_STATE = { prefs: { ...DEFAULT_PREFS }, unavailable: [], hours: {}, blocks: [], applied: {}, warnings: [], lastPlanAt: 0, calendarNote: null };

class ScheduleBook {
  /**
   * store: { get(), update(fn) }  (JSON 파일)
   * getItems(): [{ taskId, projectId, teamId, title, hours, deadline, after, deps, order }] — 내가 맡은(수락한) 끝나지 않은 작업
   * getBusy({ from, to }): Promise<{ busy: [{start,end}], ok, error }> — 캘린더의 바쁜 시간 (사이가 만든 일정 포함)
   */
  constructor({ store, getItems, getBusy, now = () => Date.now() }) {
    this.store = store;
    this.getItems = getItems;
    this.getBusy = getBusy;
    this.now = now;
    this.busyCache = null;
  }

  state() {
    const s = this.store.get();
    if (!s.schedule) this.store.update((x) => (x.schedule = structuredClone(DEFAULT_STATE)));
    return this.store.get().schedule;
  }

  update(fn) {
    this.state();
    this.store.update((x) => fn(x.schedule));
  }

  // ---------- 계산 ----------
  async busy(from, to, { fresh = false } = {}) {
    const c = this.busyCache;
    if (!fresh && c && c.from <= from && c.to >= to && this.now() - c.at < BUSY_CACHE_MS) return c.res;
    let res;
    try {
      res = await this.getBusy({ from, to });
    } catch (e) {
      res = { busy: [], ok: false, error: String(e.message || e) };
    }
    this.busyCache = { from, to, at: this.now(), res };
    return res;
  }

  invalidateBusy() {
    this.busyCache = null;
  }

  /** 맡은 작업 기준으로 다시 배치한다. 지난 블록과 직접 정한 블록은 그대로 둔다. */
  async replan({ fresh = false } = {}) {
    const now = this.now();
    const st = this.state();
    const prefs = normPrefs(st.prefs);
    const items = this.getItems();
    const itemIds = new Set(items.map((i) => i.taskId));

    // 지난 블록(기록)과, 아직 맡고 있는 작업의 고정 블록은 남긴다
    const past = st.blocks.filter((b) => b.end <= now && b.end > now - KEEP_PAST_MS);
    const pinned = st.blocks.filter((b) => b.end > now && b.pinned && itemIds.has(b.taskId));
    // 진행 중인 블록(이미 시작함)도 고정으로 본다
    const running = st.blocks.filter((b) => b.start <= now && b.end > now && !b.pinned && itemIds.has(b.taskId));
    const keep = [...pinned, ...running];

    const spent = new Map();
    for (const b of [...past, ...keep]) if (itemIds.has(b.taskId)) spent.set(b.taskId, (spent.get(b.taskId) || 0) + (b.end - b.start) / HOUR);

    const horizonDays = 42;
    const to = startOfDay(now) + horizonDays * 24 * HOUR;
    const busyRes = await this.busy(now, to, { fresh });
    // 사이가 캘린더에 넣은 일정은 바쁜 시간에서 뺀다 (다시 배치할 대상이므로)
    const own = Object.values(st.applied || {}).map((a) => ({ start: a.start, end: a.end }));
    const busy = subtract(busyRes.busy || [], own);
    const unavailable = (st.unavailable || []).filter((u) => u.end > now).map((u) => ({ start: u.start, end: u.end }));

    const planItems = items.map((i) => ({
      ...i,
      hours: Math.max(0, needHours(st, i) - (spent.get(i.taskId) || 0)),
    }));
    const { blocks, warnings } = planBlocks({ items: planItems, busy: [...busy, ...unavailable], fixed: keep, from: now, horizonDays, prefs });

    // 같은 작업의 예전 블록 id 를 이어 받는다 → 캘린더에서는 새로 만들지 않고 시간만 고친다
    const reuse = new Map();
    for (const b of st.blocks.filter((x) => x.end > now && !x.pinned && !running.includes(x)).sort((a, z) => a.start - z.start)) {
      if (!reuse.has(b.taskId)) reuse.set(b.taskId, []);
      reuse.get(b.taskId).push(b.id);
    }
    const meta = new Map(items.map((i) => [i.taskId, i]));
    const fresh_ = blocks.map((b) => {
      const ids = reuse.get(b.taskId) || [];
      const m = meta.get(b.taskId) || {};
      return { id: ids.shift() || newId(), taskId: b.taskId, projectId: m.projectId, teamId: m.teamId, title: b.title, start: b.start, end: b.end, pinned: false, late: b.late };
    });

    const titleOf = new Map(items.map((i) => [i.taskId, i.title]));
    const relabel = (b) => (titleOf.has(b.taskId) ? { ...b, title: titleOf.get(b.taskId) } : b);
    this.update((s) => {
      s.blocks = [...past, ...keep.map(relabel), ...fresh_].sort((a, z) => a.start - z.start);
      s.warnings = [
        ...(busyRes.ok === false ? [{ kind: 'calendar', text: `캘린더를 읽지 못해 다른 일정은 피하지 못했어요 (${busyRes.error || '연결 안 됨'}). 설정 → 외부 연결에서 Google 로그인을 확인해 주세요.` }] : []),
        ...warnings,
      ];
      s.calendarNote = busyRes.ok === false ? 'no-calendar' : null;
      s.lastPlanAt = now;
    });
    return this.view();
  }

  // ---------- 사용자의 수정 ----------
  /** "2번" 같은 번호(오늘 이후 블록 순서, 1부터) 또는 id 로 블록을 찾는다 */
  findBlock(ref) {
    const list = this.upcoming();
    if (typeof ref === 'number' || /^\d+$/.test(String(ref))) return list[Number(ref) - 1] || null;
    return this.state().blocks.find((b) => b.id === ref) || null;
  }

  upcoming() {
    const now = this.now();
    return this.state().blocks.filter((b) => b.end > now).sort((a, z) => a.start - z.start);
  }

  async moveBlock(ref, start, end) {
    const b = this.findBlock(ref);
    if (!b) throw new Error('그 번호의 일정을 찾지 못했어요.');
    if (!(end > start)) end = start + (b.end - b.start);
    if (start < this.now() - 5 * 60 * 1000) throw new Error('지난 시간으로는 옮길 수 없어요.');
    this.update((s) => Object.assign(s.blocks.find((x) => x.id === b.id), { start, end, pinned: true, late: false }));
    await this.replan();
    return this.state().blocks.find((x) => x.id === b.id);
  }

  /** 블록을 빼면 그 시간은 비워 두고, 작업 시간은 다른 빈 시간으로 옮긴다 */
  async removeBlock(ref) {
    const b = this.findBlock(ref);
    if (!b) throw new Error('그 번호의 일정을 찾지 못했어요.');
    this.update((s) => {
      s.blocks = s.blocks.filter((x) => x.id !== b.id);
      s.unavailable = [...(s.unavailable || []), { id: newId(), start: b.start, end: b.end, note: `${b.title} 뺀 시간` }];
    });
    await this.replan();
    return b;
  }

  async addBlock(taskId, start, end) {
    const it = this.getItems().find((i) => i.taskId === taskId);
    if (!it) throw new Error('내가 맡은 작업 중에서 찾지 못했어요.');
    if (!(end > start)) throw new Error('시작과 끝 시간을 알려 주세요.');
    this.update((s) => {
      s.blocks.push({ id: newId(), taskId, projectId: it.projectId, teamId: it.teamId, title: it.title, start, end, pinned: true, late: false });
      // 직접 추가한 시간만큼 전체 필요 시간이 늘어난 게 아니라면, 나머지 블록이 줄어든다
    });
    await this.replan();
  }

  async setHours(taskId, hours) {
    const h = Math.max(0, Math.min(200, Number(hours)));
    if (!Number.isFinite(h)) throw new Error('시간을 숫자로 알려 주세요.');
    this.update((s) => (s.hours = { ...s.hours, [taskId]: h }));
    await this.replan();
  }

  needOf(taskId) {
    const it = this.getItems().find((i) => i.taskId === taskId);
    return it ? needHours(this.state(), it) : Number(this.state().hours[taskId]) || 0;
  }

  async addUnavailable(start, end, note = '') {
    if (!(end > start)) throw new Error('언제부터 언제까지인지 알려 주세요.');
    this.update((s) => {
      s.unavailable = [...(s.unavailable || []), { id: newId(), start, end, note: String(note).slice(0, 40) }];
      // 그 시간에 직접 정해 둔 블록도 다시 배치되게 고정을 푼다
      for (const b of s.blocks) if (b.pinned && b.start < end && b.end > start) b.pinned = false;
      s.blocks = s.blocks.filter((b) => !(b.start < end && b.end > start && b.start > this.now()));
    });
    await this.replan();
  }

  async removeUnavailable(id) {
    this.update((s) => (s.unavailable = (s.unavailable || []).filter((u) => u.id !== id)));
    await this.replan();
  }

  async setPrefs(patch) {
    this.update((s) => (s.prefs = normPrefs({ ...s.prefs, ...patch })));
    await this.replan();
  }

  /** 직접 정한 블록을 모두 풀고 처음부터 다시 */
  async reset() {
    this.update((s) => {
      for (const b of s.blocks) b.pinned = false;
      s.unavailable = (s.unavailable || []).filter((u) => !/뺀 시간$/.test(u.note || ''));
    });
    return this.replan({ fresh: true });
  }

  // ---------- 캘린더 반영 ----------
  /** 캘린더와 다른 점: 만들 것·고칠 것·지울 것 */
  diff() {
    const now = this.now();
    const st = this.state();
    const applied = st.applied || {};
    const future = st.blocks.filter((b) => b.end > now);
    const ids = new Set(future.map((b) => b.id));
    const create = future.filter((b) => !applied[b.id]);
    const patch = future.filter((b) => applied[b.id] && (applied[b.id].start !== b.start || applied[b.id].end !== b.end || applied[b.id].title !== b.title));
    const remove = Object.entries(applied)
      .filter(([id, a]) => !ids.has(id) && a.end > now)
      .map(([id, a]) => ({ id, ...a }));
    return { create, patch, remove, count: create.length + patch.length + remove.length };
  }

  /** cal: { create(block) → eventId, patch(eventId, block), remove(eventId) } */
  async apply(cal) {
    const d = this.diff();
    const done = { created: 0, patched: 0, removed: 0, failed: 0, errors: [] };
    const rec = (id, b, eventId) =>
      this.update((s) => (s.applied = { ...s.applied, [id]: { eventId, start: b.start, end: b.end, title: b.title } }));
    for (const b of d.create) {
      try {
        rec(b.id, b, await cal.create(b));
        done.created++;
      } catch (e) {
        done.failed++;
        done.errors.push(String(e.message || e));
      }
    }
    for (const b of d.patch) {
      const a = this.state().applied[b.id];
      try {
        await cal.patch(a.eventId, b);
        rec(b.id, b, a.eventId);
        done.patched++;
      } catch (e) {
        // 캘린더에서 지운 일정이면 새로 만든다
        try {
          rec(b.id, b, await cal.create(b));
          done.created++;
        } catch (e2) {
          done.failed++;
          done.errors.push(String(e2.message || e2));
        }
      }
    }
    for (const r of d.remove) {
      try {
        await cal.remove(r.eventId);
      } catch {
        /* 이미 지워진 일정 */
      }
      this.update((s) => {
        const a = { ...s.applied };
        delete a[r.id];
        s.applied = a;
      });
      done.removed++;
    }
    // 오래된 기록 정리
    const cutoff = this.now() - KEEP_PAST_MS;
    this.update((s) => {
      s.applied = Object.fromEntries(Object.entries(s.applied || {}).filter(([, a]) => a.end > cutoff));
      s.unavailable = (s.unavailable || []).filter((u) => u.end > this.now());
    });
    this.invalidateBusy();
    return done;
  }

  // ---------- 보기 ----------
  view() {
    const st = this.state();
    const now = this.now();
    const items = this.getItems();
    const applied = st.applied || {};
    const up = this.upcoming();
    const placed = new Map();
    for (const b of st.blocks) placed.set(b.taskId, (placed.get(b.taskId) || 0) + (b.end - b.start) / HOUR);
    const d = this.diff();
    return {
      prefs: normPrefs(st.prefs),
      blocks: up.map((b, i) => ({ ...b, no: i + 1, label: fmtRange(b.start, b.end), synced: !!applied[b.id] && applied[b.id].start === b.start && applied[b.id].end === b.end, running: b.start <= now })),
      tasks: items.map((i) => ({
        taskId: i.taskId,
        title: i.title,
        project: i.projectTitle,
        need: round1(needHours(st, i)),
        placed: round1(placed.get(i.taskId) || 0),
        deadline: i.deadline,
        custom: st.hours[i.taskId] != null,
      })),
      unavailable: (st.unavailable || []).filter((u) => u.end > now).map((u) => ({ ...u, label: fmtRange(u.start, u.end) })),
      warnings: st.warnings || [],
      pending: d.count,
      pendingDetail: { create: d.create.length, patch: d.patch.length, remove: d.remove.length },
      lastPlanAt: st.lastPlanAt,
      calendarNote: st.calendarNote,
    };
  }

  /** AI·규칙 응답에 붙일 짧은 텍스트 */
  summaryText(limit = 12) {
    const v = this.view();
    if (!v.blocks.length) return v.tasks.length ? '아직 잡힌 시간이 없어요.' : '맡은 작업이 없어요.';
    const lines = v.blocks.slice(0, limit).map((b) => `${b.no}. ${b.label} ${b.title}${b.pinned ? ' (고정)' : ''}${b.late ? ' (마감 뒤)' : ''}`);
    if (v.blocks.length > limit) lines.push(`… 외 ${v.blocks.length - limit}개`);
    return lines.join('\n');
  }
}

const needHours = (st, i) => (st.hours && st.hours[i.taskId] != null ? Number(st.hours[i.taskId]) : Number(i.hours) || 0);

function subtract(intervals, cuts) {
  let out = intervals.map((i) => ({ start: i.start, end: i.end }));
  for (const c of cuts) {
    const next = [];
    for (const i of out) {
      if (c.end <= i.start || c.start >= i.end) next.push(i);
      else {
        if (c.start > i.start) next.push({ start: i.start, end: c.start });
        if (c.end < i.end) next.push({ start: c.end, end: i.end });
      }
    }
    out = next;
  }
  return out;
}

const round1 = (x) => Math.round(x * 10) / 10;

module.exports = { ScheduleBook, subtract, DEFAULT_STATE };
