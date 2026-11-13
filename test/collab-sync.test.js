const test = require('node:test');
const assert = require('node:assert/strict');
const { TeamSync } = require('../src/core/team/team-sync');
const { CollabSync } = require('../src/core/collab/collab-sync');
const { MemoryBackend, emptyHub } = require('../src/core/team/memory-backend');

function box(initial) {
  let v = structuredClone(initial);
  return { get: () => v, set: (x) => (v = x), update: (fn) => (fn(v), v) };
}

function user(hub, clock) {
  const backend = new MemoryBackend({ hub });
  const cache = box({});
  const team = new TeamSync({ makeBackend: () => backend, state: box(null), outbox: box({ items: [] }), cache, now: () => clock.t });
  const collab = new CollabSync({ team, cache, now: () => clock.t });
  return { team, collab, sync: async () => (await team.sync(), await collab.sync()) };
}

test('자기소개 → 작업 입력 → 배분 제안 → 적용 → 수락·이의 → 진행 확인', async () => {
  const hub = emptyHub();
  const clock = { t: new Date(2026, 10, 14, 10).getTime() };
  const a = user(hub, clock);
  const b = user(hub, clock);
  const t = await a.team.createTeam('BION 홍보팀', '유빈');
  await b.team.joinTeam(t.joinCode, '민지');
  await a.sync();
  await b.sync();

  await a.collab.saveProfile({ pr: '영상 편집 해 봤어요', skills: [{ name: '영상편집', level: 3 }], learn: '', weeklyHours: 10 });
  await b.collab.saveProfile({ pr: '포스터 만들기 좋아해요', skills: [{ name: '디자인', level: 3 }], learn: '영상편집', weeklyHours: 6 });

  const pid = await a.collab.createProject({ title: '학생회 홍보 영상', goal: '1분 영상과 포스터', deadline: '2026-11-28' });
  const shoot = await a.collab.addTask(pid, { title: '촬영', estimateHours: 3, difficulty: 2, skills: '영상편집', prep: '카메라 대여, 장소 섭외' });
  await a.collab.addTask(pid, { title: '컷 편집', estimateHours: 5, difficulty: 3, skills: '영상편집', dependsOn: [shoot] });
  const poster = await a.collab.addTask(pid, { title: '포스터', estimateHours: 4, difficulty: 2, skills: '디자인' });

  await a.sync();
  const p = a.collab.plan(pid);
  const who = Object.fromEntries(p.assignments.map((x) => [x.taskId, x.uid]));
  const bUid = await b.team.getBackend().signIn();
  assert.equal(who[poster], bUid);
  assert.equal(await a.collab.applyPlan(pid, p.assignments), 3);

  await b.sync();
  // 민지: 배정 확인부터
  let n = b.collab.nudge();
  assert.equal(n.key, `accept:${poster}`);
  // 유빈이 대신 수락할 수는 없다
  await assert.rejects(() => a.collab.accept(poster));
  await b.collab.accept(poster);
  n = b.collab.nudge();
  assert.equal(n.key, `start:${poster}`);

  // 유빈: 촬영 수락 후 준비물 안내, 컷 편집은 이의 제기 → 다시 제안하면 유빈에게 가지 않음
  await a.sync();
  await a.collab.accept(shoot);
  assert.equal(a.collab.nudge().key.split(':')[0], 'accept');
  const cut = a.collab.tasksOf(pid).find((x) => x.title === '컷 편집').id;
  await a.collab.object(cut, '가능 시간이 부족해요');
  // 컷 편집이 배우는 중인 민지에게 가면 전체 시간이 모자라므로 먼저 계획 조정을 권한다
  assert.equal(a.collab.nudge().key, 'plan-tight');
  a.collab.markNudged('plan-tight');
  assert.equal(a.collab.nudge().key, `prep:${shoot}`);
  const p2 = a.collab.plan(pid);
  assert.equal(p2.assignments.find((x) => x.taskId === cut).uid, bUid);

  // 막힘 → 다른 팀원 고양이가 도움을 권함
  await a.collab.setStatus(shoot, 'doing');
  await a.collab.setStatus(shoot, 'blocked');
  await b.sync();
  assert.equal(b.collab.nudge().key, `help:${shoot}`);
});
