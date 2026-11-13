// 고양이가 업무 진행을 챙기는 말 (개발 방향 문서 7.3).
// 원칙: 지연을 꾸짖기 전에 진행을 막는 이유를 묻고, 불가능한 일정은 무엇을 바꾸면 되는지 말한다.
const { parseDate } = require('./planner');

const HOUR = 60 * 60 * 1000;
const DAY = 24 * HOUR;
const REPEAT_MS = 4 * HOUR;
const STUCK_REPEAT_MS = 12 * HOUR;

function fmtH(h) {
  const r = Math.round(h * 10) / 10;
  return Number.isInteger(r) ? `${r}` : r.toFixed(1);
}

const A = {
  later: { id: 'later', label: '나중에' },
  open: { id: 'open-collab', label: '협업 탭 열기' },
  accept: { id: 'task-accept', label: '맡을게요', primary: true },
  object: { id: 'task-object', label: '이의 있어요' },
  start: { id: 'task-start', label: '시작할게요', primary: true },
  ok: { id: 'task-ok', label: '잘 되고 있어요' },
  blocked: { id: 'task-blocked', label: '막혔어요' },
  done: { id: 'task-done', label: '끝났어요', primary: true },
};

/**
 * 지금 건넬 진행 확인 한마디를 고른다 (없으면 null).
 * recent: { key: 마지막으로 말한 시각 }
 * planWarnings: planner.plan() 의 warnings (late/tight 를 본다)
 */
function nextNudge({ myUid, project, tasks, profile, planWarnings = [], effective = {}, now = Date.now(), recent = {} }) {
  if (!myUid || !project) return null;
  const byId = new Map(tasks.map((t) => [t.id, t]));
  const mine = tasks.filter((t) => t.assignee === myUid && t.status !== 'done');
  const daily = Math.max(0.5, (Number(profile && profile.weeklyHours) || 7) / 7);
  const eff = (t) => effective[t.id] || Number(t.estimateHours) || 1;
  const depsLeft = (t) => (t.dependsOn || []).map((d) => byId.get(d)).filter((d) => d && d.status !== 'done');
  const ok = (key, wait = REPEAT_MS) => !recent[key] || now - recent[key] > wait;
  const out = (key, taskId, text, actions) => ({ key, taskId, text, actions, source: `협업 · ${project.title}` });

  // 1) 배정 확인
  for (const t of mine.filter((x) => !x.accepted)) {
    const key = `accept:${t.id}`;
    if (ok(key))
      return out(key, t.id, `"${t.title}"을 맡아 줄 수 있을까요? 약 ${fmtH(eff(t))}시간 예상이에요. 배정 이유는 협업 탭에서 볼 수 있어요.`, [A.object, A.accept]);
  }

  // 2) 팀원이 막혀 있음
  for (const t of tasks.filter((x) => x.status === 'blocked' && x.assignee && x.assignee !== myUid)) {
    const key = `help:${t.id}`;
    if (ok(key, STUCK_REPEAT_MS))
      return out(key, t.id, `"${t.title}"에서 도움이 필요하대요. 같이 볼 수 있다면 한마디 건네 볼까요?`, [A.later, { ...A.open, primary: true }]);
  }

  // 3) 마감 여유 부족
  const late = planWarnings.find((w) => w.kind === 'late' || w.kind === 'tight' || w.kind === 'shortfall');
  if (late && mine.length && ok('plan-tight', 24 * HOUR)) {
    return out('plan-tight', null, '지금 계획대로라면 마감 전에 여유가 부족해요. 범위를 줄이거나 일정과 담당자를 다시 맞춰 볼까요?', [A.later, { ...A.open, primary: true }]);
  }

  // 4) 진행 중인데 예상보다 오래 걸림 → 이유부터 묻기
  for (const t of mine.filter((x) => x.status === 'doing' && x.startedAt)) {
    const expected = Math.max(DAY, (eff(t) / daily) * DAY * 1.5);
    const key = `stuck:${t.id}`;
    if (now - t.startedAt > expected && ok(key, STUCK_REPEAT_MS))
      return out(key, t.id, `"${t.title}" 진행은 어때요? 예상보다 어려운 부분이 생겼다면 도움을 요청하거나 범위를 나눌 수 있어요.`, [A.blocked, A.ok, A.done]);
  }

  // 5) 앞 작업 기다리는 중
  for (const t of mine.filter((x) => x.accepted && x.status === 'todo')) {
    const left = depsLeft(t);
    const key = `waiting:${t.id}`;
    if (left.length && ok(key, 24 * HOUR))
      return out(key, t.id, `"${t.title}"은 앞 작업 "${left[0].title}"이 아직 끝나지 않았어요. 그동안 준비물을 챙기거나 다른 작업부터 할 수 있어요.`, [A.later, A.open]);
  }

  // 6) 시작 전 준비물
  for (const t of mine.filter((x) => x.accepted && x.status === 'todo' && !depsLeft(x).length)) {
    const left = (t.prep || []).filter((p) => !p.done);
    const key = `prep:${t.id}`;
    if (left.length && ok(key))
      return out(key, t.id, `"${t.title}" 시작 전에 준비할 것이 ${left.length}개 남았어요. 먼저 "${left[0].text}"부터 챙겨 볼까요?`, [A.later, { ...A.open, primary: true }]);
  }

  // 7) 시작할 수 있는 일
  for (const t of mine.filter((x) => x.accepted && x.status === 'todo' && !depsLeft(x).length)) {
    const key = `start:${t.id}`;
    const weekly = Number(profile && profile.weeklyHours) || 0;
    const rest = weekly - eff(t);
    const text =
      weekly && rest > 0
        ? `이번 주에는 ${fmtH(weekly)}시간을 쓸 수 있다고 했죠. "${t.title}"은 ${fmtH(eff(t))}시간 정도로 예상해요. 나머지는 검토와 수정에 남겨 둘까요?`
        : `"${t.title}"을 시작할 수 있어요. 약 ${fmtH(eff(t))}시간 예상이에요.`;
    if (ok(key, 24 * HOUR)) return out(key, t.id, text, [A.later, A.start]);
  }

  // 8) 마감 임박
  const dl = parseDate(project.deadline);
  if (dl && mine.length && dl - now < 2 * DAY && dl > now && ok('deadline', 12 * HOUR)) {
    const days = Math.max(0, Math.ceil((dl - now) / DAY));
    return out('deadline', null, `마감까지 ${days}일 남았어요. 남은 내 일: ${mine.map((t) => `"${t.title}"`).join(', ')}. 도움이 필요하면 지금 말해 두는 게 좋아요.`, [A.later, A.open]);
  }

  return null;
}

module.exports = { nextNudge, ACTIONS: A };
