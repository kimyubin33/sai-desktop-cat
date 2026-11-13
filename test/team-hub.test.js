const test = require('node:test');
const assert = require('node:assert/strict');
const { TeamHub } = require('../src/core/team/team-hub');
const { MemoryBackend, emptyHub } = require('../src/core/team/memory-backend');

function box(initial) {
  let v = structuredClone(initial);
  return { get: () => v, set: (x) => (v = x), update: (fn) => (fn(v), v) };
}

function user(server, settingsInit = {}) {
  const backend = new MemoryBackend({ hub: server });
  const stores = { settings: box({ teams: [], ...settingsInit }), outbox: box({ items: [] }), teamCache: box({}) };
  const hub = new TeamHub({ ...stores, makeBackend: () => backend, delayMs: 0 });
  return { hub, backend, ...stores };
}

test('팀 두 개를 만들고 오가며, 데이터는 팀별로 나뉜다', async () => {
  const server = emptyHub();
  const a = user(server);
  const b = user(server);
  const t1 = await a.hub.createTeam('BION 홍보팀', '유빈');
  const t2 = await a.hub.createTeam('안동소주 팀플', '유빈');
  assert.deepEqual(a.hub.teams().map((t) => t.name), ['BION 홍보팀', '안동소주 팀플']);
  assert.equal(a.hub.active().name(), '안동소주 팀플', '새로 만든 팀이 보고 있는 팀이 됨');

  await b.hub.joinTeam(t1.joinCode, '민지');
  await assert.rejects(() => b.hub.joinTeam(t1.joinCode.toLowerCase(), '민지'), /이미 참여한 팀/);

  // 팀1에만 프로젝트를 만든다
  a.hub.setActive(t1.teamId);
  await a.hub.syncAll();
  const pid = await a.hub.active().collab.createProject({ title: '홍보 영상', deadline: '2026-11-28' });
  assert.ok(pid);
  assert.equal(a.hub.bundle(t2.teamId).collab.data().projects.length, 0);
  await a.hub.syncAll();
  assert.deepEqual(a.hub.bundle(t1.teamId).sync.members().map((m) => m.displayName), ['민지']);
  assert.deepEqual(a.hub.bundle(t2.teamId).sync.members(), []);
});

test('공유 대기함과 받은 조언 주제는 팀별로 가고, 고양이는 모든 팀 것을 함께 본다', async () => {
  const server = emptyHub();
  const a = user(server);
  const b = user(server);
  const t1 = await a.hub.createTeam('팀1', 'A');
  const t2 = await a.hub.createTeam('팀2', 'A');
  await b.hub.joinTeam(t1.joinCode, 'B');
  await b.hub.joinTeam(t2.joinCode, 'B');
  await a.hub.syncAll();
  const bUid = b.backend.uid;

  // A 가 팀1 로 time_promise, 팀2 로 workload 를 B 에게
  for (const [teamId, topic] of [[t1.teamId, 'time_promise'], [t2.teamId, 'workload']]) {
    const s = a.hub.bundle(teamId).sync;
    const items = s.prepareItems([{ id: `o-${topic}`, topic, target: 'person', scope: 'auto', toMember: bUid, label: topic, advice: '', teamProposal: '', week: '2026-W46' }]);
    s.outbox.update((d) => d.items.push(...items));
  }
  assert.deepEqual(a.outbox.get().items.map((x) => [x.topic, x.teamId === t1.teamId ? 1 : 2]), [['time_promise', 1], ['workload', 2]]);
  await a.hub.syncAll();
  await b.hub.syncAll();
  assert.deepEqual(b.hub.bundle(t1.teamId).sync.boosts(), { time_promise: true });
  assert.deepEqual(b.hub.bundle(t2.teamId).sync.boosts(), { workload: true });
  assert.deepEqual(b.hub.teamFacade().boosts(), { time_promise: true, workload: true });

  // "내 사정도 있어요" 보류는 그 주제를 받은 팀 모두에
  assert.equal(await b.hub.teamFacade().holdTopic('workload'), 1);
  assert.deepEqual(b.hub.teamFacade().boosts(), { time_promise: true });
});

test('진행 확인은 모든 팀에서 고르고, 여러 팀이면 팀 이름을 붙인다', async () => {
  const server = emptyHub();
  const a = user(server);
  const b = user(server);
  const t1 = await a.hub.createTeam('팀1', 'A');
  const t2 = await a.hub.createTeam('팀2', 'A');
  await b.hub.joinTeam(t2.joinCode, 'B');
  await a.hub.syncAll();
  a.hub.setActive(t1.teamId);
  // 팀2 에서 B 에게 배정
  const c2 = a.hub.bundle(t2.teamId).collab;
  await c2.saveProfile({ weeklyHours: 5 });
  const pid = await c2.createProject({ title: '소주 브랜드', deadline: '2026-11-28' });
  const tid = await c2.addTask(pid, { title: '라벨 디자인', estimateHours: 3 });
  await c2.patchTask(pid, tid, { assignee: b.backend.uid, accepted: false });

  await b.hub.syncAll();
  const n = b.hub.collabFacade().nudge();
  assert.equal(n.key, `accept:${tid}`);
  assert.equal(n.teamId, t2.teamId);
  await b.hub.collabFacade().accept(tid);
  b.hub.collabFacade().markNudged(n.key);
  assert.equal(b.hub.bundle(t2.teamId).collab.tasksOf(pid)[0].accepted, true);
});

test('팀을 나가면 그 팀 데이터만 지우고 다른 팀은 그대로', async () => {
  const server = emptyHub();
  const a = user(server);
  const t1 = await a.hub.createTeam('팀1', 'A');
  const t2 = await a.hub.createTeam('팀2', 'A');
  a.outbox.update((d) => d.items.push({ id: 'x', topic: 'workload', teamId: t1.teamId, status: 'review' }, { id: 'y', topic: 'workload', teamId: t2.teamId, status: 'review' }));
  await a.hub.leaveTeam(t1.teamId);
  assert.deepEqual(a.hub.teams().map((t) => t.name), ['팀2']);
  assert.deepEqual(a.outbox.get().items.map((x) => x.id), ['y']);
  assert.ok(!a.teamCache.get().teams[t1.teamId]);
  assert.equal(a.hub.active().teamId, t2.teamId);
});

test('예전(팀 하나) 저장 형식을 옮긴다', () => {
  const server = emptyHub();
  const backend = new MemoryBackend({ hub: server });
  const settings = box({ team: { teamId: 'T1', name: '옛 팀', joinCode: 'ABC234', displayName: 'A' } });
  const teamCache = box({ members: [{ uid: 'u' }], inbox: [{ id: 's', topic: 'workload', week: '2026-W46', held: false }], myUid: 'u' });
  const outbox = box({ items: [{ id: 'o1', topic: 'workload', status: 'sent' }] });
  const hub = new TeamHub({ settings, outbox, teamCache, makeBackend: () => backend });
  assert.equal(settings.get().team, null);
  assert.deepEqual(settings.get().teams.map((t) => t.teamId), ['T1']);
  assert.equal(settings.get().activeTeamId, 'T1');
  assert.deepEqual(teamCache.get().teams.T1.members, [{ uid: 'u' }]);
  assert.equal(outbox.get().items[0].teamId, 'T1');
  assert.equal(hub.active().name(), '옛 팀');
});

test('팀이 없을 때 저장해 둔 공유 항목은 처음 연결한 팀으로 간다', async () => {
  const server = emptyHub();
  const a = user(server);
  a.outbox.update((d) => d.items.push({ id: 'early', topic: 'workload', status: 'needs-recipient', target: 'person' }));
  const t = await a.hub.createTeam('팀', 'A');
  assert.equal(a.outbox.get().items[0].teamId, t.teamId);
  await a.hub.createTeam('두번째', 'A');
  assert.equal(a.outbox.get().items[0].teamId, t.teamId, '두 번째 팀으로 옮겨가지 않음');
});
