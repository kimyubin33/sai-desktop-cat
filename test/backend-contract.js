// 팀 백엔드가 지켜야 할 동작과 권한. 메모리 백엔드와 Firebase 에뮬레이터 양쪽에서 같은 검사를 돌린다.
const assert = require('node:assert/strict');

/**
 * makeUsers(n) → n 개의 백엔드 인스턴스 (각각 다른 사용자, 같은 서버)
 */
function defineContract(test, makeUsers) {
  test('팀 만들기 → 참여 코드로 참여 → 팀원 목록', async () => {
    const [a, b] = await makeUsers(2);
    const t = await a.createTeam({ name: '안동소주팀', displayName: '유빈' });
    assert.match(t.joinCode, /^[A-Z2-9]{6}$/);
    const j = await b.joinTeam({ joinCode: t.joinCode.toLowerCase(), displayName: '민지' });
    assert.equal(j.teamId, t.teamId);
    assert.equal(j.name, '안동소주팀');
    const members = await a.listMembers(t.teamId);
    assert.deepEqual(members.map((m) => m.displayName).sort(), ['민지', '유빈']);
  });

  test('없는 참여 코드는 거절된다', async () => {
    const [a] = await makeUsers(1);
    await assert.rejects(() => a.joinTeam({ joinCode: 'ZZZZZZ', displayName: 'x' }));
  });

  test('조언 신호: 받는 사람만 목록으로 보고, 보낸 사람은 id 로 상태만 확인한다', async () => {
    const [a, b, c] = await makeUsers(3);
    const t = await a.createTeam({ name: '팀', displayName: 'A' });
    await b.joinTeam({ joinCode: t.joinCode, displayName: 'B' });
    await c.joinTeam({ joinCode: t.joinCode, displayName: 'C' });
    const bUid = await b.signIn();

    const id = await a.createSignal(t.teamId, { topic: 'time_promise', toMember: bUid, week: '2026-W46' });
    const forB = await b.listSignalsForMe(t.teamId);
    assert.equal(forB.length, 1);
    assert.equal(forB[0].topic, 'time_promise');
    assert.ok(!('from' in forB[0]) && !('sender' in forB[0]), '보낸 사람 정보가 없어야 함');
    assert.equal((await c.listSignalsForMe(t.teamId)).length, 0);

    // 받는 사람이 보류하면 보낸 사람이 id 로 확인할 수 있다
    await b.holdSignal(t.teamId, forB[0].id);
    assert.equal((await a.getSignal(t.teamId, id)).held, true);
    // 받는 사람이 아니면 보류할 수 없다
    await assert.rejects(() => c.holdSignal(t.teamId, id));
  });

  test('자기 자신이나 팀원이 아닌 사람에게는 신호를 보낼 수 없다', async () => {
    const [a, b] = await makeUsers(2);
    const t = await a.createTeam({ name: '팀', displayName: 'A' });
    const aUid = await a.signIn();
    const bUid = await b.signIn();
    await assert.rejects(() => a.createSignal(t.teamId, { topic: 'workload', toMember: aUid, week: '2026-W46' }));
    await assert.rejects(() => a.createSignal(t.teamId, { topic: 'workload', toMember: bUid, week: '2026-W46' }));
  });

  test('팀원이 아니면 팀 데이터를 볼 수 없다', async () => {
    const [a, outsider] = await makeUsers(2);
    const t = await a.createTeam({ name: '팀', displayName: 'A' });
    await assert.rejects(() => outsider.listMembers(t.teamId));
    await assert.rejects(() => outsider.listProposals(t.teamId));
  });

  test('팀 약속 제안과 각자의 응답', async () => {
    const [a, b] = await makeUsers(2);
    const t = await a.createTeam({ name: '팀', displayName: 'A' });
    await b.joinTeam({ joinCode: t.joinCode, displayName: 'B' });
    const pid = await a.createProposal(t.teamId, { topic: 'role_clarity', week: '2026-W46' });
    await b.respondProposal(t.teamId, pid, 'agree');
    await a.respondProposal(t.teamId, pid, 'later');
    const [p] = await b.listProposals(t.teamId);
    assert.equal(p.topic, 'role_clarity');
    assert.equal(p.responses[await b.signIn()], 'agree');
    assert.equal(p.responses[await a.signIn()], 'later');
  });

  test('팀을 나가면 더 이상 팀 데이터를 볼 수 없다', async () => {
    const [a, b] = await makeUsers(2);
    const t = await a.createTeam({ name: '팀', displayName: 'A' });
    await b.joinTeam({ joinCode: t.joinCode, displayName: 'B' });
    await b.leaveTeam(t.teamId);
    await assert.rejects(() => b.listMembers(t.teamId));
    assert.equal((await a.listMembers(t.teamId)).length, 1);
  });
}

module.exports = { defineContract };
