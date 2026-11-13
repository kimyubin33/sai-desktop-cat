// 업무 배분 제안 (개발 방향 문서 7장). Electron·네트워크 의존성 없음.
//
// "공정"을 같은 개수로 보지 않는다. 다음을 함께 본다.
//  - 가용 시간 대비 부담 (검토·수정 몫 20%를 먼저 남겨 둔다)
//  - 필요한 역량과 경험, 배우고 싶은 일 (배우는 사람은 시간을 더 잡는다)
//  - 어려운 일·반복적인 일이 한 사람에게 쏠리지 않는지
//  - 선행 작업 순서와 마감까지의 여유
//  - 당사자가 이의를 제기한 일은 그 사람에게 다시 배정하지 않는다
// 결과에는 배정 이유와 추정의 불확실성을 함께 담는다. 실제 배정은 사람이 확인한 뒤에만 바뀐다.

const DAY = 24 * 60 * 60 * 1000;
const REVIEW_RESERVE = 0.2;
const LEVEL_LABEL = { 0: '경험 없음', 1: '배우는 중', 2: '할 수 있음', 3: '능숙' };

const norm = (s) => String(s || '').trim().toLowerCase();

function parseDate(d) {
  if (!d) return null;
  if (typeof d === 'number') return d;
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(d);
  if (!m) return null;
  // 마감일 하루 끝(현지 23:59)까지로 본다
  return new Date(Number(m[1]), Number(m[2]) - 1, Number(m[3]), 23, 59).getTime();
}

function fmtHours(h) {
  return Number.isInteger(h) ? `${h}` : h.toFixed(1);
}

function fmtDate(t) {
  const d = new Date(t);
  return `${d.getMonth() + 1}월 ${d.getDate()}일`;
}

/** 선행 작업 기준 위상 정렬. 순환이 있으면 cycle 에 해당 작업 id 를 담는다 */
function topoOrder(tasks) {
  const byId = new Map(tasks.map((t) => [t.id, t]));
  const state = new Map();
  const order = [];
  const cycle = new Set();
  const visit = (t, stack) => {
    const s = state.get(t.id);
    if (s === 'done') return;
    if (s === 'visiting') {
      for (const id of stack.slice(stack.indexOf(t.id))) cycle.add(id);
      return;
    }
    state.set(t.id, 'visiting');
    for (const dep of t.dependsOn || []) if (byId.has(dep)) visit(byId.get(dep), [...stack, t.id]);
    state.set(t.id, 'done');
    order.push(t);
  };
  for (const t of tasks) visit(t, [t.id]);
  return { order, cycle: [...cycle] };
}

function memberInfo(m, weeks) {
  const weekly = Math.max(0, Number(m.weeklyHours) || 0);
  const capacity = weekly * weeks;
  return {
    uid: m.uid,
    name: m.name || '팀원',
    weeklyHours: weekly,
    dailyHours: weekly / 7,
    capacity,
    usable: capacity * (1 - REVIEW_RESERVE),
    skills: Object.fromEntries(Object.entries(m.skills || {}).map(([k, v]) => [norm(k), Number(v) || 0])),
    learn: new Set((m.learn || []).map(norm)),
    load: 0,
    hard: 0,
    easy: 0,
    count: 0,
  };
}

function fitOf(member, task) {
  const req = (task.skills || []).map(norm).filter(Boolean);
  if (!req.length) return { fit: 0.5, levels: [], learnMatch: false, req };
  const levels = req.map((s) => ({ skill: s, level: member.skills[s] || 0 }));
  const fit = levels.reduce((a, l) => a + l.level, 0) / (3 * req.length);
  const learnMatch = levels.some((l) => l.level < 2 && member.learn.has(l.skill));
  return { fit, levels, learnMatch, req };
}

/** 역량이 낮을수록, 배우는 일일수록 시간을 더 잡는다 */
function effectiveHours(est, fit, learnMatch, hasReq) {
  if (!hasReq) return est;
  if (fit < 0.34) return est * (learnMatch ? 1.3 : 1.5);
  if (fit < 0.67) return est * 1.15;
  return est;
}

/**
 * members: [{ uid, name, weeklyHours, skills: {이름: 0~3}, learn: [이름] }]
 * tasks:   [{ id, title, estimateHours, difficulty(1~3), skills: [], dependsOn: [], assignee, accepted, status, objections: {uid: 사유} }]
 */
function plan({ members, tasks, deadline, now = Date.now() }) {
  const warnings = [];
  const deadlineAt = parseDate(deadline);
  const days = deadlineAt ? Math.max(1, (deadlineAt - now) / DAY) : 14;
  if (!deadlineAt) warnings.push({ kind: 'no-deadline', text: '마감일이 없어 2주를 기준으로 계산했어요.' });
  const weeks = days / 7;

  const ms = members.map((m) => memberInfo(m, weeks));
  const byUid = new Map(ms.map((m) => [m.uid, m]));
  if (!ms.length) return { assignments: [], members: [], warnings: [{ kind: 'no-members', text: '팀원 정보가 없어요.' }] };
  for (const m of ms) {
    if (!m.weeklyHours) warnings.push({ kind: 'no-hours', text: `${m.name}님의 주간 가능 시간이 비어 있어 배정에서 빠졌어요.` });
  }

  const open = tasks.filter((t) => t.status !== 'done');
  const { order, cycle } = topoOrder(open);
  if (cycle.length) {
    const names = cycle.map((id) => open.find((t) => t.id === id)?.title).filter(Boolean);
    warnings.push({ kind: 'cycle', text: `선행 작업이 서로 꼬여 있어요: ${names.join(' ↔ ')}. 순서를 다시 정해 주세요.` });
  }
  for (const t of open) {
    if (!(Number(t.estimateHours) > 0)) warnings.push({ kind: 'no-estimate', text: `"${t.title}"의 예상 시간이 없어 1시간으로 계산했어요.` });
  }

  // 이미 수락된 배정은 그대로 두고 부담에 넣는다
  const fixed = [];
  for (const t of open) {
    const m = t.accepted && byUid.get(t.assignee);
    if (!m) continue;
    const f = fitOf(m, t);
    const eff = effectiveHours(Number(t.estimateHours) || 1, f.fit, f.learnMatch, f.req.length > 0);
    m.load += eff;
    m.count++;
    if (Number(t.difficulty) === 3) m.hard++;
    if (Number(t.difficulty) === 1) m.easy++;
    fixed.push({ taskId: t.id, uid: m.uid, effectiveHours: eff, fixed: true, reasons: ['이미 수락한 배정이에요.'] });
  }

  // 어려운 일, 큰 일부터 나눈다 (작은 일로 마지막 균형을 맞추기 쉽도록)
  const todo = open
    .filter((t) => !(t.accepted && byUid.get(t.assignee)))
    .sort((a, b) => (Number(b.difficulty) || 2) - (Number(a.difficulty) || 2) || (Number(b.estimateHours) || 1) - (Number(a.estimateHours) || 1));

  const assignments = [...fixed];
  for (const t of todo) {
    const est = Number(t.estimateHours) || 1;
    const diff = Number(t.difficulty) || 2;
    const objected = new Set(Object.keys(t.objections || {}));
    let best = null;
    for (const m of ms) {
      if (!m.weeklyHours || objected.has(m.uid)) continue;
      const f = fitOf(m, t);
      const eff = effectiveHours(est, f.fit, f.learnMatch, f.req.length > 0);
      const ratio = m.usable > 0 ? (m.load + eff) / m.usable : Infinity;
      const score =
        3 * f.fit +
        (f.learnMatch ? 0.8 : 0) -
        4 * ratio -
        (diff === 3 ? 1.2 * m.hard : 0) -
        (diff === 1 ? 0.6 * m.easy : 0) -
        (ratio > 1 ? 10 : 0);
      if (!best || score > best.score + 1e-9 || (Math.abs(score - best.score) < 1e-9 && ratio < best.ratio)) {
        best = { m, f, eff, ratio, score };
      }
    }
    if (!best) {
      assignments.push({ taskId: t.id, uid: null, effectiveHours: est, reasons: ['배정할 수 있는 팀원이 없어요 (가능 시간이 없거나 모두 이의를 제기함).'] });
      warnings.push({ kind: 'unassigned', text: `"${t.title}"을 맡을 수 있는 사람이 없어요.` });
      continue;
    }
    const { m, f, eff, ratio } = best;
    m.load += eff;
    m.count++;
    if (diff === 3) m.hard++;
    if (diff === 1) m.easy++;
    assignments.push({ taskId: t.id, uid: m.uid, effectiveHours: eff, reasons: reasonsFor(m, t, f, eff, ratio, ms) });
  }

  // 역량 공백·부담 초과·전체 시간 부족
  for (const t of open) {
    const req = (t.skills || []).map(norm).filter(Boolean);
    for (const s of req) {
      if (!ms.some((m) => (m.skills[s] || 0) >= 2)) {
        warnings.push({ kind: 'missing-skill', text: `"${t.title}"에 필요한 "${s}"를 할 수 있다고 적은 사람이 없어요. 배울 시간과 도움 받을 곳을 함께 정해 주세요.` });
      }
    }
  }
  for (const m of ms) {
    if (m.usable > 0 && m.load > m.usable) {
      warnings.push({
        kind: 'overload',
        uid: m.uid,
        text: `${m.name}님은 마감까지 쓸 수 있는 ${fmtHours(round1(m.usable))}시간보다 ${fmtHours(round1(m.load - m.usable))}시간 많이 맡게 돼요.`,
      });
    }
  }
  const need = assignments.reduce((a, x) => a + x.effectiveHours, 0);
  const have = ms.reduce((a, m) => a + m.usable, 0);
  if (need > have) {
    warnings.push({
      kind: 'shortfall',
      text: `필요한 시간이 검토 몫을 뺀 가능 시간보다 약 ${fmtHours(round1(need - have))}시간 많아요. 범위를 줄이거나, 마감을 늦추거나, 함께할 사람을 늘려야 해요.`,
    });
  }

  // 일정 추정: 선행 작업과 한 사람이 동시에 두 일을 못 한다는 점을 반영
  const sched = schedule(order, assignments, byUid, now);
  let slackDays = null;
  if (deadlineAt && sched.finishAt) {
    slackDays = (deadlineAt - sched.finishAt) / DAY;
    const reviewDays = Math.max(1, days * 0.1);
    if (slackDays < 0) {
      warnings.push({
        kind: 'late',
        text: `지금 계획대로면 ${fmtDate(sched.finishAt)}쯤 끝나 마감보다 ${Math.ceil(-slackDays)}일 늦어요. 범위·담당·일정을 다시 맞춰 볼까요?`,
      });
    } else if (slackDays < reviewDays) {
      warnings.push({
        kind: 'tight',
        text: `${fmtDate(sched.finishAt)}쯤 끝나 마감 전 여유가 ${Math.floor(slackDays)}일뿐이에요. 검토·수정 시간이 부족할 수 있어요.`,
      });
    }
  }

  return {
    weeks: round1(weeks),
    assignments: assignments.map((a) => ({ ...a, effectiveHours: round1(a.effectiveHours), ...(sched.byTask[a.taskId] || {}) })),
    members: ms.map((m) => ({
      uid: m.uid,
      name: m.name,
      weeklyHours: m.weeklyHours,
      capacity: round1(m.capacity),
      usable: round1(m.usable),
      load: round1(m.load),
      ratio: m.usable > 0 ? round2(m.load / m.usable) : null,
      count: m.count,
      hard: m.hard,
    })),
    warnings,
    finishAt: sched.finishAt,
    slackDays: slackDays == null ? null : round1(slackDays),
    note: '자기소개와 예상 시간을 바탕으로 한 추정이에요. 실제로 해 보면서 고쳐 주세요.',
  };
}

function reasonsFor(m, t, f, eff, ratio, all) {
  const r = [];
  const est = Number(t.estimateHours) || 1;
  if (f.req.length) {
    const lv = f.levels.map((l) => `${l.skill}: ${LEVEL_LABEL[l.level]}`).join(', ');
    r.push(`필요한 역량(${lv})을 자기소개 기준으로 봤어요.`);
  }
  if (f.learnMatch) r.push('배우고 싶다고 적은 일이라, 배우는 시간을 더해 계산했어요.');
  if (eff > est + 0.01) r.push(`익숙하지 않은 부분을 고려해 ${fmtHours(est)}시간 → ${fmtHours(round1(eff))}시간으로 잡았어요.`);
  r.push(
    `마감까지 쓸 수 있는 ${fmtHours(round1(m.capacity))}시간 중 검토 몫을 남기고 ${fmtHours(round1(m.load))}시간(${Math.round(ratio * 100)}%)을 맡게 돼요.`,
  );
  const others = all.filter((x) => x.uid !== m.uid && x.usable > 0);
  if (Number(t.difficulty) === 3 && others.some((x) => x.hard > m.hard - 1)) r.push('어려운 일이 한 사람에게 몰리지 않게 나눴어요.');
  if (ratio > 1) r.push('그래도 가능 시간을 넘어요. 범위를 나누거나 도와줄 사람을 붙여 주세요.');
  return r;
}

function schedule(order, assignments, byUid, now) {
  const byTask = {};
  const asg = new Map(assignments.map((a) => [a.taskId, a]));
  const finish = new Map();
  const free = new Map();
  let finishAt = null;
  for (const t of order) {
    const a = asg.get(t.id);
    const m = a && a.uid && byUid.get(a.uid);
    if (!m || !m.dailyHours) continue;
    const depEnd = Math.max(now, ...(t.dependsOn || []).map((d) => finish.get(d) || now));
    const start = Math.max(depEnd, free.get(m.uid) || now);
    const end = start + (a.effectiveHours / m.dailyHours) * DAY;
    finish.set(t.id, end);
    free.set(m.uid, end);
    byTask[t.id] = { startAt: Math.round(start), finishAt: Math.round(end) };
    finishAt = Math.max(finishAt || 0, end);
  }
  return { byTask, finishAt: finishAt && Math.round(finishAt) };
}

const round1 = (x) => Math.round(x * 10) / 10;
const round2 = (x) => Math.round(x * 100) / 100;

module.exports = { plan, topoOrder, effectiveHours, parseDate, LEVEL_LABEL, REVIEW_RESERVE };
