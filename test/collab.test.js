const test = require('node:test');
const assert = require('node:assert/strict');
const { plan, topoOrder } = require('../src/core/collab/planner');
const { nextNudge } = require('../src/core/collab/progress');

const NOW = new Date(2026, 10, 14, 10, 0).getTime();
const DAY = 24 * 60 * 60 * 1000;

const members = [
  { uid: 'a', name: '유빈', weeklyHours: 10, skills: { 영상편집: 3, 블렌더: 2 }, learn: [] },
  { uid: 'b', name: '민지', weeklyHours: 6, skills: { 디자인: 3 }, learn: ['영상편집'] },
  { uid: 'c', name: '준호', weeklyHours: 8, skills: { 글쓰기: 3, 디자인: 1 }, learn: [] },
];

const T = (id, title, est, diff, skills = [], dependsOn = [], extra = {}) => ({
  id, title, estimateHours: est, difficulty: diff, skills, dependsOn, status: 'todo', ...extra,
});

test('역량이 맞는 사람에게 배정하고 이유를 남긴다', () => {
  const r = plan({
    members,
    tasks: [T('t1', '포스터 디자인', 4, 2, ['디자인']), T('t2', '홍보 영상 편집', 6, 3, ['영상편집']), T('t3', '소개 글', 3, 1, ['글쓰기'])],
    deadline: '2026-11-28',
    now: NOW,
  });
  const who = Object.fromEntries(r.assignments.map((a) => [a.taskId, a.uid]));
  assert.deepEqual(who, { t1: 'b', t2: 'a', t3: 'c' });
  const a2 = r.assignments.find((a) => a.taskId === 't2');
  assert.ok(a2.reasons.some((x) => x.includes('영상편집: 능숙')));
  assert.ok(a2.reasons.some((x) => x.includes('검토 몫')));
  assert.match(r.note, /추정/);
});

test('같은 역량이면 가용 시간 대비 부담이 적은 쪽으로 나눈다', () => {
  const same = [
    { uid: 'a', name: 'A', weeklyHours: 10, skills: { x: 2 } },
    { uid: 'b', name: 'B', weeklyHours: 10, skills: { x: 2 } },
  ];
  const tasks = [1, 2, 3, 4].map((i) => T(`t${i}`, `일${i}`, 3, 2, ['x']));
  const r = plan({ members: same, tasks, deadline: '2026-11-28', now: NOW });
  const loads = r.members.map((m) => m.load);
  assert.equal(loads[0], loads[1]);
});

test('어려운 일이 한 사람에게 몰리지 않는다', () => {
  const same = [
    { uid: 'a', name: 'A', weeklyHours: 20, skills: {} },
    { uid: 'b', name: 'B', weeklyHours: 20, skills: {} },
  ];
  const tasks = [T('h1', '어려운1', 2, 3), T('h2', '어려운2', 2, 3), T('e1', '쉬운1', 2, 1), T('e2', '쉬운2', 2, 1)];
  const r = plan({ members: same, tasks, deadline: '2026-11-28', now: NOW });
  const hard = r.members.map((m) => m.hard);
  assert.deepEqual(hard, [1, 1]);
});

test('배우고 싶은 일은 배정될 수 있지만 시간을 더 잡는다', () => {
  const ms = [
    { uid: 'a', name: 'A', weeklyHours: 2, skills: { 영상편집: 3 } },
    { uid: 'b', name: 'B', weeklyHours: 20, skills: {}, learn: ['영상편집'] },
  ];
  const r = plan({ members: ms, tasks: [T('t', '컷 편집', 4, 2, ['영상편집'])], deadline: '2026-11-21', now: NOW });
  const a = r.assignments[0];
  assert.equal(a.uid, 'b');
  assert.equal(a.effectiveHours, 5.2);
  assert.ok(a.reasons.some((x) => x.includes('배우고 싶다고')));
});

test('이의를 제기한 일은 그 사람에게 다시 배정하지 않는다', () => {
  const r = plan({
    members,
    tasks: [T('t1', '포스터 디자인', 4, 2, ['디자인'], [], { objections: { b: '시간 부족' } })],
    deadline: '2026-11-28',
    now: NOW,
  });
  assert.notEqual(r.assignments[0].uid, 'b');
});

test('수락한 배정은 바꾸지 않고 부담에 넣는다', () => {
  const r = plan({
    members,
    tasks: [T('t1', '포스터 디자인', 4, 2, ['디자인'], [], { assignee: 'c', accepted: true })],
    deadline: '2026-11-28',
    now: NOW,
  });
  assert.equal(r.assignments[0].uid, 'c');
  assert.ok(r.assignments[0].fixed);
  assert.ok(r.members.find((m) => m.uid === 'c').load > 4);
});

test('시간이 모자라면 무엇을 바꿔야 하는지 알려 준다', () => {
  const r = plan({
    members: [{ uid: 'a', name: 'A', weeklyHours: 5, skills: {} }],
    tasks: [T('t1', '큰 일', 20, 2), T('t2', '또 큰 일', 20, 2, [], ['t1'])],
    deadline: '2026-11-21',
    now: NOW,
  });
  const kinds = r.warnings.map((w) => w.kind);
  assert.ok(kinds.includes('shortfall'));
  assert.ok(kinds.includes('overload'));
  assert.ok(kinds.includes('late'));
  assert.match(r.warnings.find((w) => w.kind === 'shortfall').text, /범위를 줄이거나/);
});

test('선행 작업 순서대로 일정을 잡고, 순환은 경고한다', () => {
  const r = plan({
    members: [
      { uid: 'a', name: 'A', weeklyHours: 14, skills: {} },
      { uid: 'b', name: 'B', weeklyHours: 14, skills: {} },
    ],
    tasks: [T('t1', '기획', 4, 2), T('t2', '제작', 4, 2, [], ['t1'])],
    deadline: '2026-11-28',
    now: NOW,
  });
  const a1 = r.assignments.find((a) => a.taskId === 't1');
  const a2 = r.assignments.find((a) => a.taskId === 't2');
  assert.ok(a2.startAt >= a1.finishAt);

  const { cycle } = topoOrder([T('x', 'X', 1, 1, [], ['y']), T('y', 'Y', 1, 1, [], ['x'])]);
  assert.deepEqual(cycle.sort(), ['x', 'y']);
});

test('필요한 역량을 가진 사람이 없으면 알린다', () => {
  const r = plan({ members, tasks: [T('t', '3D 모델링', 3, 3, ['마야'])], deadline: '2026-11-28', now: NOW });
  assert.ok(r.warnings.some((w) => w.kind === 'missing-skill'));
});

// ---------- 진행 확인 ----------
const project = { id: 'p', title: 'BION 홍보', deadline: '2026-11-28' };
const profile = { weeklyHours: 6 };

test('배정 확인을 먼저 묻는다', () => {
  const n = nextNudge({ myUid: 'a', project, profile, tasks: [T('t1', '컷 편집', 4, 2, [], [], { assignee: 'a' })], now: NOW });
  assert.equal(n.key, 'accept:t1');
  assert.deepEqual(n.actions.map((x) => x.id), ['task-object', 'task-accept']);
});

test('앞 작업을 기다리는 동안 할 수 있는 일을 알려 준다', () => {
  const tasks = [T('t0', '촬영', 3, 2, [], [], { assignee: 'b', accepted: true }), T('t1', '컷 편집', 4, 2, [], ['t0'], { assignee: 'a', accepted: true })];
  const n = nextNudge({ myUid: 'a', project, profile, tasks, now: NOW });
  assert.equal(n.key, 'waiting:t1');
  assert.match(n.text, /앞 작업 "촬영"/);
});

test('시작할 수 있는 일은 가용 시간과 함께 안내한다 (7.3 예시)', () => {
  const n = nextNudge({ myUid: 'a', project, profile, tasks: [T('t1', '컷 편집', 4, 2, [], [], { assignee: 'a', accepted: true })], now: NOW });
  assert.equal(n.key, 'start:t1');
  assert.match(n.text, /6시간을 쓸 수 있다고.*4시간 정도로 예상.*검토와 수정/);
});

test('준비물이 남아 있으면 먼저 챙기게 한다', () => {
  const tasks = [T('t1', '촬영', 3, 2, [], [], { assignee: 'a', accepted: true, prep: [{ text: '카메라 대여', done: false }] })];
  assert.equal(nextNudge({ myUid: 'a', project, profile, tasks, now: NOW }).key, 'prep:t1');
});

test('오래 걸리면 꾸짖지 않고 이유를 묻는다', () => {
  const tasks = [T('t1', '컷 편집', 4, 2, [], [], { assignee: 'a', accepted: true, status: 'doing', startedAt: NOW - 8 * DAY })];
  const n = nextNudge({ myUid: 'a', project, profile, tasks, now: NOW });
  assert.equal(n.key, 'stuck:t1');
  assert.match(n.text, /어려운 부분/);
  assert.ok(!/늦|왜 안/.test(n.text));
});

test('팀원이 막혔다고 하면 도움을 권한다, 같은 말은 반복하지 않는다', () => {
  const tasks = [T('t1', '자막', 2, 1, [], [], { assignee: 'b', accepted: true, status: 'blocked' })];
  const n = nextNudge({ myUid: 'a', project, profile, tasks, now: NOW });
  assert.equal(n.key, 'help:t1');
  assert.equal(nextNudge({ myUid: 'a', project, profile, tasks, now: NOW + 1000, recent: { 'help:t1': NOW } }), null);
});
