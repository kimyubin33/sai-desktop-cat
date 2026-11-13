// 맡은 작업을 실제 시간 블록(몇 시부터 몇 시까지)으로 배치한다. Electron·네트워크 의존성 없음.
//
// 기준
//  - 하루 중 작업할 수 있는 시간대(기본 9~22시) 안에서, 캘린더의 바쁜 시간과 "안 되는 시간"을 피한다
//  - 마감이 빠른 작업부터, 먼저 끝나야 하는 작업 뒤에 배치한다
//  - 한 번에 최대 2시간, 블록 사이와 다른 일정 앞뒤에 15분 쉬는 시간, 하루 최대 6시간
//  - 사용자가 직접 정한(고정한) 블록은 그대로 두고 남은 시간만 채운다
//  - 마감 안에 다 못 넣으면 마감 뒤에 넣고 알려 준다

const MIN = 60 * 1000;
const HOUR = 60 * MIN;
const DAY = 24 * HOUR;
const STEP = 30 * MIN;

const DEFAULT_PREFS = { dayStart: 9, dayEnd: 22, maxBlockHours: 2, minBlockMinutes: 30, gapMinutes: 15, dailyMaxHours: 6 };

function normPrefs(p = {}) {
  const x = { ...DEFAULT_PREFS, ...p };
  x.dayStart = Math.min(23, Math.max(0, Number(x.dayStart)));
  x.dayEnd = Math.min(24, Math.max(x.dayStart + 1, Number(x.dayEnd)));
  x.maxBlockHours = Math.min(8, Math.max(0.5, Number(x.maxBlockHours)));
  x.minBlockMinutes = Math.min(120, Math.max(15, Number(x.minBlockMinutes)));
  x.gapMinutes = Math.min(60, Math.max(0, Number(x.gapMinutes)));
  x.dailyMaxHours = Math.min(16, Math.max(0.5, Number(x.dailyMaxHours)));
  return x;
}

function dayKey(t) {
  const d = new Date(t);
  return `${d.getFullYear()}-${d.getMonth() + 1}-${d.getDate()}`;
}

function dayStartMs(t) {
  const d = new Date(t);
  d.setHours(0, 0, 0, 0);
  return d.getTime();
}

function merge(intervals) {
  const s = intervals.filter((i) => i.end > i.start).sort((a, b) => a.start - b.start);
  const out = [];
  for (const i of s) {
    const last = out[out.length - 1];
    if (last && i.start <= last.end) last.end = Math.max(last.end, i.end);
    else out.push({ start: i.start, end: i.end });
  }
  return out;
}

/** [from,to) 에서 occupied 를 뺀 빈 구간 */
function gaps(from, to, occupied) {
  const out = [];
  let cur = from;
  for (const o of occupied) {
    if (o.end <= cur) continue;
    if (o.start >= to) break;
    if (o.start > cur) out.push({ start: cur, end: Math.min(o.start, to) });
    cur = Math.max(cur, o.end);
    if (cur >= to) break;
  }
  if (cur < to) out.push({ start: cur, end: to });
  return out;
}

/** 선행 작업을 먼저, 그다음 마감이 빠른 순 */
function orderItems(items) {
  const byId = new Map(items.map((i) => [i.taskId, i]));
  const sorted = [...items].sort((a, b) => (a.deadline || Infinity) - (b.deadline || Infinity) || (a.order || 0) - (b.order || 0));
  const out = [];
  const seen = new Set();
  const visit = (it, stack = new Set()) => {
    if (seen.has(it.taskId) || stack.has(it.taskId)) return;
    stack.add(it.taskId);
    for (const d of it.deps || []) if (byId.has(d)) visit(byId.get(d), stack);
    seen.add(it.taskId);
    out.push(it);
  };
  for (const it of sorted) visit(it);
  return out;
}

/**
 * items: [{ taskId, title, hours(남은 시간), deadline(ms|null), after(ms|null), deps:[taskId], order }]
 * busy:  [{ start, end }]  캘린더의 다른 일정 + 안 되는 시간
 * fixed: [{ taskId, start, end }]  이미 정해진 블록(그대로 둠)
 * 돌려주는 값: { blocks: [{ taskId, title, start, end, late }], warnings: [{ taskId, kind, text, hours }] }
 */
function planBlocks({ items, busy = [], fixed = [], from = Date.now(), horizonDays = 42, prefs }) {
  const p = normPrefs(prefs);
  const gap = p.gapMinutes * MIN;
  const maxBlock = p.maxBlockHours * HOUR;
  const minBlock = p.minBlockMinutes * MIN;
  const dailyMax = p.dailyMaxHours * HOUR;
  const start0 = Math.ceil(from / STEP) * STEP;
  const until = dayStartMs(start0) + horizonDays * DAY;

  // 다른 일정 앞뒤로 쉬는 시간을 둔다
  let occupied = merge([...busy, ...fixed].map((i) => ({ start: i.start - gap, end: i.end + gap })));
  const used = new Map();
  for (const f of fixed) used.set(dayKey(f.start), (used.get(dayKey(f.start)) || 0) + (f.end - f.start));
  const endOf = new Map();
  for (const f of fixed) endOf.set(f.taskId, Math.max(endOf.get(f.taskId) || 0, f.end));

  const blocks = [];
  const warnings = [];
  for (const it of orderItems(items)) {
    let remaining = Math.ceil(((Number(it.hours) || 0) * HOUR) / STEP) * STEP;
    if (remaining <= 0) continue;
    const depEnd = Math.max(0, ...(it.deps || []).map((d) => endOf.get(d) || 0));
    let earliest = Math.max(start0, it.after || 0, depEnd ? depEnd + gap : 0);
    earliest = Math.ceil(earliest / STEP) * STEP;
    for (let day = dayStartMs(earliest); day < until && remaining > 0; day = dayStartMs(day + DAY + 2 * HOUR)) {
      const ws = Math.max(earliest, day + p.dayStart * HOUR);
      const we = day + p.dayEnd * HOUR;
      if (we <= ws) continue;
      for (const g of gaps(ws, we, occupied)) {
        let s = Math.ceil(g.start / STEP) * STEP;
        while (remaining > 0) {
          const capLeft = dailyMax - (used.get(dayKey(day)) || 0);
          let len = Math.min(g.end - s, maxBlock, remaining, capLeft);
          len = Math.floor(len / STEP) * STEP;
          if (len <= 0 || len < Math.min(minBlock, remaining)) break;
          const b = { taskId: it.taskId, title: it.title, start: s, end: s + len, late: !!(it.deadline && s + len > it.deadline) };
          blocks.push(b);
          remaining -= len;
          used.set(dayKey(day), (used.get(dayKey(day)) || 0) + len);
          endOf.set(it.taskId, Math.max(endOf.get(it.taskId) || 0, b.end));
          occupied = merge([...occupied, { start: b.start - gap, end: b.end + gap }]);
          s = Math.ceil((b.end + gap) / STEP) * STEP;
        }
        if (remaining <= 0) break;
      }
    }
    const late = blocks.filter((b) => b.taskId === it.taskId && b.late);
    if (remaining > 0) {
      warnings.push({ taskId: it.taskId, kind: 'unplaced', hours: remaining / HOUR, text: `"${it.title}"의 ${fmtH(remaining / HOUR)}시간은 앞으로 ${horizonDays}일 안에 넣을 빈 시간이 없어요.` });
    } else if (late.length) {
      const h = late.reduce((a, b) => a + Math.min(b.end - b.start, b.end - it.deadline), 0) / HOUR;
      warnings.push({ taskId: it.taskId, kind: 'late', hours: h, text: `"${it.title}"은 마감 전에 ${fmtH(h)}시간이 모자라 마감 뒤에도 잡혔어요. 마감·시간·담당을 조정해 주세요.` });
    }
  }
  return { blocks, warnings, prefs: p };
}

const fmtH = (h) => (Math.round(h * 10) / 10).toString();

module.exports = { planBlocks, merge, gaps, orderItems, normPrefs, DEFAULT_PREFS, dayKey, HOUR, DAY };
