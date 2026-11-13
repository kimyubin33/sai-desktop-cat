process.env.TZ = 'Asia/Seoul';
const test = require('node:test');
const assert = require('node:assert');
const { parseWhen, parseDay } = require('../src/core/schedule/korean-time');
const { planBlocks } = require('../src/core/schedule/time-plan');
const { ScheduleBook } = require('../src/core/schedule/schedule-book');
const { parseCommand } = require('../src/core/schedule/commands');

const H = 3600000;
// 2026-11-14(토) 10:00
const NOW = new Date(2026, 10, 14, 10, 0).getTime();
const day = (d, h = 0, m = 0) => new Date(2026, 10, d, h, m).getTime();
const memStore = () => {
  let data = {};
  return { get: () => data, update: (fn) => (fn(data), data) };
};

test('한국어 날짜·시간 읽기', () => {
  assert.strictEqual(parseDay('내일', NOW), day(15));
  assert.strictEqual(parseDay('목요일', NOW), day(19));
  assert.strictEqual(parseDay('다음 주 월요일', NOW), day(16));
  assert.strictEqual(parseDay('11월 20일', NOW), day(20));
  let w = parseWhen('목요일 오후 2시부터 4시까지', NOW);
  assert.deepStrictEqual([w.day, w.start, w.end], [day(19), 14 * 60, 16 * 60]);
  w = parseWhen('내일 3시 반', NOW);
  assert.deepStrictEqual([w.day, w.start], [day(15), 15 * 60 + 30]);
  w = parseWhen('15:00~17:30', NOW);
  assert.deepStrictEqual([w.start, w.end], [15 * 60, 17 * 60 + 30]);
  w = parseWhen('오전 10시에 2시간', NOW);
  assert.deepStrictEqual([w.start, w.end], [10 * 60, 12 * 60]);
  assert.strictEqual(parseWhen('3시간 걸려', NOW).start, null);
});

test('바쁜 시간을 피하고, 마감 순서·선행 작업·하루 최대 시간을 지킨다', () => {
  const items = [
    { taskId: 'b', title: '발표 자료', hours: 3, deadline: day(20, 23, 59), deps: ['a'] },
    { taskId: 'a', title: '자료조사', hours: 4, deadline: day(18, 23, 59) },
  ];
  const busy = [{ start: day(14, 13), end: day(14, 15) }];
  const { blocks, warnings } = planBlocks({ items, busy, from: NOW, prefs: { dailyMaxHours: 4 } });
  assert.strictEqual(warnings.length, 0);
  const a = blocks.filter((b) => b.taskId === 'a');
  const b = blocks.filter((x) => x.taskId === 'b');
  assert.strictEqual(a.reduce((s, x) => s + x.end - x.start, 0), 4 * H);
  assert.strictEqual(b.reduce((s, x) => s + x.end - x.start, 0), 3 * H);
  // 선행 작업이 끝난 뒤
  assert.ok(Math.min(...b.map((x) => x.start)) >= Math.max(...a.map((x) => x.end)));
  for (const x of blocks) {
    assert.ok(x.end - x.start <= 2 * H);
    assert.ok(!(x.start < busy[0].end + 15 * 60000 && x.end > busy[0].start - 15 * 60000), '바쁜 시간과 겹침');
    assert.ok(new Date(x.start).getHours() >= 9 && x.end <= new Date(x.start).setHours(22, 0, 0, 0));
  }
  const perDay = {};
  for (const x of blocks) perDay[new Date(x.start).getDate()] = (perDay[new Date(x.start).getDate()] || 0) + (x.end - x.start);
  for (const v of Object.values(perDay)) assert.ok(v <= 4 * H);
});

test('마감 안에 못 넣으면 알려 준다', () => {
  const { warnings, blocks } = planBlocks({ items: [{ taskId: 'a', title: '큰 일', hours: 10, deadline: day(14, 22) }], from: NOW, prefs: {} });
  assert.ok(blocks.some((b) => b.late));
  assert.strictEqual(warnings[0].kind, 'late');
});

function book(items, busy = []) {
  const store = memStore();
  let now = NOW;
  const b = new ScheduleBook({ store, getItems: () => items, getBusy: async () => ({ busy, ok: true }), now: () => now });
  return { b, store, setNow: (t) => (now = t) };
}

const cal = () => {
  const ev = new Map();
  let n = 0;
  return {
    ev,
    create: async (blk) => (ev.set(`e${++n}`, { ...blk }), `e${n}`),
    patch: async (id, blk) => {
      if (!ev.has(id)) throw new Error('gone');
      ev.set(id, { ...blk });
    },
    remove: async (id) => ev.delete(id),
  };
};

test('시간표: 옮기기·빼기·안 되는 시간·시간 변경, 캘린더 반영은 바뀐 것만', async () => {
  const items = [{ taskId: 't1', projectId: 'p', teamId: 'x', title: '자료조사', hours: 4, deadline: day(20, 23, 59) }];
  const { b } = book(items);
  let v = await b.replan();
  assert.strictEqual(v.blocks.reduce((s, x) => s + x.end - x.start, 0), 4 * H);
  const c = cal();
  let r = await b.apply(c);
  assert.strictEqual(r.created, v.blocks.length);
  assert.strictEqual(b.diff().count, 0);

  // 2번을 목요일 15시로 옮기면 고정되고, 캘린더에서는 시간만 바뀐다
  await b.moveBlock(2, day(19, 15), day(19, 17));
  v = b.view();
  const moved = v.blocks.find((x) => x.start === day(19, 15));
  assert.ok(moved && moved.pinned);
  assert.strictEqual(v.blocks.reduce((s, x) => s + x.end - x.start, 0), 4 * H);
  r = await b.apply(c);
  assert.strictEqual(r.created, 0);
  assert.strictEqual(c.ev.size, v.blocks.length);

  // 1번을 빼면 그 시간은 비우고 다른 곳으로
  const first = v.blocks[0];
  await b.removeBlock(1);
  v = b.view();
  assert.ok(!v.blocks.some((x) => x.start < first.end && x.end > first.start));
  assert.strictEqual(v.blocks.reduce((s, x) => s + x.end - x.start, 0), 4 * H);

  // 일요일 하루 안 됨
  await b.addUnavailable(day(15, 9), day(15, 22), '약속');
  v = b.view();
  assert.ok(!v.blocks.some((x) => new Date(x.start).getDate() === 15));

  // 2시간이면 된다
  await b.setHours('t1', 2);
  v = b.view();
  assert.strictEqual(v.blocks.reduce((s, x) => s + x.end - x.start, 0), 2 * H);
  r = await b.apply(c);
  assert.strictEqual(c.ev.size, v.blocks.length);
  assert.ok(r.removed > 0);
});

test('지난 시간은 다시 잡지 않고, 맡지 않게 된 작업의 일정은 지운다', async () => {
  const items = [{ taskId: 't1', title: '자료조사', hours: 4, deadline: day(30) }];
  const { b, setNow } = book(items);
  await b.replan();
  const c = cal();
  await b.apply(c);
  const firstEnd = b.view().blocks[0].end;
  setNow(firstEnd + 60000);
  let v = await b.replan();
  const future = v.blocks.reduce((s, x) => s + x.end - x.start, 0);
  assert.strictEqual(future, 4 * H - (b.state().blocks[0].end - b.state().blocks[0].start));
  items.length = 0;
  v = await b.replan();
  assert.strictEqual(v.blocks.length, 0);
  const r = await b.apply(c);
  assert.ok(r.removed > 0);
});

test('말로 고치기 (AI 없이)', () => {
  const ctx = {
    now: NOW,
    prefs: { dayStart: 9, dayEnd: 22 },
    tasks: [{ taskId: 't1', title: '자료조사', need: 4 }, { taskId: 't2', title: '발표 준비', need: 3 }],
    blocks: [
      { no: 1, id: 'x1', taskId: 't1', title: '자료조사', start: day(14, 11), end: day(14, 13) },
      { no: 2, id: 'x2', taskId: 't2', title: '발표 준비', start: day(15, 9), end: day(15, 11) },
    ],
  };
  assert.deepStrictEqual(parseCommand('2번 목요일 3시로 옮겨줘', ctx), [{ type: 'move', ref: 2, start: day(19, 15), end: day(19, 17) }]);
  assert.deepStrictEqual(parseCommand('1번 빼줘', ctx), [{ type: 'remove', ref: 1 }]);
  assert.deepStrictEqual(parseCommand('발표 준비 2시간 더 필요해', ctx), [{ type: 'hours', taskId: 't2', hours: 5 }]);
  assert.deepStrictEqual(parseCommand('자료조사는 3시간이면 돼', ctx), [{ type: 'hours', taskId: 't1', hours: 3 }]);
  assert.deepStrictEqual(parseCommand('수요일은 안 돼', ctx), [{ type: 'unavailable', start: day(18, 9), end: day(18, 22), note: '안 되는 시간' }]);
  assert.deepStrictEqual(parseCommand('내일 오후 2시부터 5시까지 수업 있어', ctx), [{ type: 'unavailable', start: day(15, 14), end: day(15, 17), note: '수업' }]);
  assert.deepStrictEqual(parseCommand('발표 준비 금요일 2시부터 4시까지 추가해줘', ctx), [{ type: 'add', taskId: 't2', start: day(20, 14), end: day(20, 16) }]);
  assert.deepStrictEqual(parseCommand('하루에 4시간만 할래', ctx), [{ type: 'prefs', patch: { dailyMaxHours: 4 } }]);
  assert.deepStrictEqual(parseCommand('밤 10시 이후는 안 돼', ctx), [{ type: 'prefs', patch: { dayEnd: 22 } }]);
  assert.deepStrictEqual(parseCommand('캘린더에 넣어줘', ctx), [{ type: 'apply' }]);
  assert.deepStrictEqual(parseCommand('다시 짜줘', ctx), [{ type: 'replan' }]);
});
