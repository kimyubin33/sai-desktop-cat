const test = require('node:test');
const assert = require('node:assert/strict');
const { TeamSync } = require('../src/core/team/team-sync');
const { MemoryBackend, emptyHub } = require('../src/core/team/memory-backend');
const { createOutboxItems, mergeIntoOutbox } = require('../src/core/privacy');

function box(initial) {
  let v = structuredClone(initial);
  return { get: () => v, set: (x) => (v = x), update: (fn) => (fn(v), v) };
}

function makeUser(hub, clock) {
  const backend = new MemoryBackend({ hub });
  const state = box(null);
  return new TeamSync({
    makeBackend: () => backend,
    state,
    outbox: box({ items: [] }),
    cache: box({}),
    now: () => clock.t,
    delayMs: 10 * 60 * 1000,
  });
}

function share(user, topic, scope, target, toMember) {
  const summary = { topics: [topic], sensitive: false };
  let items = createOutboxItems(summary, [{ topic, scope, target }], { now: user.now(), key: 'k' });
  items = items.map((x) => ({ ...x, toMember }));
  user.outbox.update((d) => (d.items = mergeIntoOutbox(d.items, user.prepareItems(items))));
  return user.outbox.get().items.at(-1);
}

async function team3() {
  const hub = emptyHub();
  const clock = { t: Date.UTC(2026, 10, 13, 12) };
  const [a, b, c] = [makeUser(hub, clock), makeUser(hub, clock), makeUser(hub, clock)];
  const t = await a.createTeam('팀', '유빈');
  await b.joinTeam(t.joinCode, '민지');
  await c.joinTeam(t.joinCode, '준호');
  await a.sync();
  return { a, b, c, clock };
}

test('자동 반영 주제는 늦춰 보낸 뒤 받는 사람의 조언 가중치가 된다', async () => {
  const { a, b, c, clock } = await team3();
  const bUid = b.members({ includeMe: true }).find((m) => m.displayName === '민지')?.uid || (await b.getBackend().signIn());
  const item = share(a, 'time_promise', 'auto', 'person', bUid);
  assert.equal(item.status, 'queued');

  await a.sync();
  assert.equal(a.outbox.get().items[0].status, 'queued', '지연 시간 전에는 보내지 않음');

  clock.t += 11 * 60 * 1000;
  await a.sync();
  assert.equal(a.outbox.get().items[0].status, 'sent');

  await b.sync();
  await c.sync();
  assert.deepEqual(b.boosts(), { time_promise: true });
  assert.deepEqual(c.boosts(), {});
});

test('검토 후 공유는 확인하기 전까지 보내지 않는다', async () => {
  const { a, b, clock } = await team3();
  const bUid = await b.getBackend().signIn();
  const item = share(a, 'workload', 'review', 'person', bUid);
  assert.equal(item.status, 'review');
  clock.t += 5 * 60 * 60 * 1000;
  await a.sync();
  await b.sync();
  assert.deepEqual(b.boosts(), {});
  a.confirmReview(item.id);
  clock.t += 11 * 60 * 1000;
  await a.sync();
  await b.sync();
  assert.deepEqual(b.boosts(), { workload: true });
});

test('받을 팀원을 고르지 않으면 보내지 않는다', async () => {
  const { a, b } = await team3();
  const item = share(a, 'workload', 'auto', 'person', null);
  assert.equal(item.status, 'needs-recipient');
  a.setRecipient(item.id, await b.getBackend().signIn());
  assert.equal(a.outbox.get().items[0].status, 'queued');
});

test('"내 사정도 있어요"를 누르면 보류되고, 보낸 사람에게도 보류로 보인다', async () => {
  const { a, b, clock } = await team3();
  share(a, 'time_promise', 'auto', 'person', await b.getBackend().signIn());
  clock.t += 11 * 60 * 1000;
  await a.sync();
  await b.sync();
  assert.equal(await b.holdTopic('time_promise'), 1);
  assert.deepEqual(b.boosts(), {});
  await a.sync();
  assert.equal(a.outbox.get().items[0].status, 'held');
});

test('팀 약속 제안: 같은 주의 같은 주제는 하나로 모이고, 모두 동의하면 팀 약속이 된다', async () => {
  const { a, b, c, clock } = await team3();
  a.proposeTeam('role_clarity');
  b.proposeTeam('role_clarity');
  clock.t += 11 * 60 * 1000;
  await a.sync();
  await b.sync();
  await c.sync();
  const ps = c.proposals();
  assert.equal(ps.length, 1);
  assert.equal(ps[0].agree, 2);
  assert.equal(ps[0].agreedByAll, false);
  assert.equal(c.pendingProposalsForMe().length, 1);
  await c.respond(ps[0].id, 'agree');
  await a.sync();
  assert.equal(a.proposals()[0].agreedByAll, true);
});

test('팀을 나가면 보낸 기록은 지우고 연결을 끊는다', async () => {
  const { a, b, clock } = await team3();
  share(a, 'time_promise', 'auto', 'person', await b.getBackend().signIn());
  clock.t += 11 * 60 * 1000;
  await a.sync();
  await a.leaveTeam();
  assert.equal(a.isConnected(), false);
  assert.equal(a.outbox.get().items.length, 0);
  await b.sync();
  assert.equal(b.members().length, 1);
});
