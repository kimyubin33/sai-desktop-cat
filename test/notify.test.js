const test = require('node:test');
const assert = require('node:assert/strict');
const { TeamSync } = require('../src/core/team/team-sync');
const { Notifier } = require('../src/core/notify/notifier');
const { TelegramClient, findStartCode, notifyText, NOTIFY_KINDS } = require('../src/core/notify/telegram');
const { MemoryBackend, emptyHub } = require('../src/core/team/memory-backend');

function box(initial) {
  let v = structuredClone(initial);
  return { get: () => v, set: (x) => (v = x), update: (fn) => (fn(v), v) };
}

// 가짜 Telegram: 보낸 메시지와 사용자가 봇에 보낸 /start 를 흉내 낸다
function fakeTelegram() {
  const tg = { sent: [], inbox: [] };
  tg.make = (token) => ({
    token,
    getMe: async () => ({ username: 'sai_team_bot' }),
    getUpdates: async () => tg.inbox,
    sendMessage: async (chatId, text) => tg.sent.push({ chatId, text }),
  });
  return tg;
}

async function setup() {
  const hub = emptyHub();
  const tg = fakeTelegram();
  const mk = () => {
    const backend = new MemoryBackend({ hub });
    const cache = box({});
    const team = new TeamSync({ makeBackend: () => backend, state: box(null), outbox: box({ items: [] }), cache });
    return { team, n: new Notifier({ team, cache, makeClient: tg.make }), uid: () => backend.uid };
  };
  const a = mk();
  const b = mk();
  const t = await a.team.createTeam('팀', '유빈');
  await b.team.joinTeam(t.joinCode, '민지');
  for (const u of [a, b]) {
    await u.team.sync();
  }
  return { a, b, tg };
}

async function link(u, tg, chatId) {
  const { code, url } = u.n.startLink();
  assert.match(url, /^https:\/\/t\.me\/sai_team_bot\?start=sai[a-z0-9]{10}$/);
  assert.deepEqual(await u.n.checkLink(), { linked: false, waiting: true });
  tg.inbox.push({ update_id: chatId, message: { text: `/start ${code}`, chat: { id: chatId, first_name: 'x' } } });
  assert.deepEqual(await u.n.checkLink(), { linked: true });
}

test('봇 등록 → 각자 /start 코드로 연결 → 업무 알림만 팀원에게', async () => {
  const { a, b, tg } = await setup();
  assert.equal(await a.n.setBot('123456:ABCDEFGHIJKLMNOPQRSTUVWXYZ'), 'sai_team_bot');
  await a.n.sync();
  await b.n.sync();
  await link(a, tg, 111);
  await link(b, tg, 222);
  assert.equal(tg.sent.length, 2, '연결 확인 메시지');
  assert.match(tg.sent[0].text, /개인 대화 내용은 보내지 않아요/);
  await a.n.sync();
  await b.n.sync();

  tg.sent.length = 0;
  await a.n.assigned([{ uid: b.uid(), title: '포스터', hours: 4 }, { uid: a.uid(), title: '촬영', hours: 3 }], '홍보 영상');
  assert.deepEqual(tg.sent.map((m) => m.chatId), [222], '나에게 배정한 건 보내지 않음');
  assert.match(tg.sent[0].text, /"포스터" 배정 제안/);

  tg.sent.length = 0;
  await b.n.blocked('포스터', '홍보 영상', '민지');
  assert.deepEqual(tg.sent.map((m) => m.chatId), [111]);

  // 팀 알림을 끄면 팀 알림은 안 받지만, 내 진행 확인은 받는다
  await b.n.setTeamAlerts(false);
  await a.n.sync();
  tg.sent.length = 0;
  await a.n.proposal('완료 기준을 한 줄씩 맞춰 볼까요?');
  assert.equal(tg.sent.length, 0);
  await b.n.nudgeMe('"포스터" 진행은 어때요?', '홍보 영상');
  assert.deepEqual(tg.sent.map((m) => m.chatId), [222]);
});

test('연결을 끊으면 더 이상 보내지 않는다', async () => {
  const { a, b, tg } = await setup();
  await a.n.setBot('123456:ABCDEFGHIJKLMNOPQRSTUVWXYZ');
  await b.n.sync();
  await link(b, tg, 222);
  await b.n.unlink();
  await a.n.sync();
  tg.sent.length = 0;
  await a.n.blocked('촬영', '홍보 영상', '유빈');
  assert.equal(tg.sent.length, 0);
});

test('상담 관련 종류는 보낼 수 없다', () => {
  assert.deepEqual(NOTIFY_KINDS.sort(), ['assigned', 'blocked', 'linked', 'nudge', 'proposal', 'test']);
  assert.throws(() => notifyText('advice', { text: 'x' }));
  assert.throws(() => notifyText('summary', {}));
});

test('봇 토큰 형식과 /start 코드 찾기', () => {
  assert.throws(() => new TelegramClient({ token: 'abc' }), /토큰 형식/);
  const ups = [{ message: { text: '/start other', chat: { id: 1 } } }, { message: { text: '/start sai123', chat: { id: 2, first_name: '민지' } } }];
  assert.deepEqual(findStartCode(ups, 'sai123'), { chatId: 2, name: '민지' });
  assert.equal(findStartCode(ups, 'nope'), null);
});

test('Telegram API 요청 형식', async () => {
  const calls = [];
  const c = new TelegramClient({
    token: '123456:ABCDEFGHIJKLMNOPQRSTUVWXYZ',
    fetchImpl: async (url, o) => (calls.push({ url, body: JSON.parse(o.body) }), { status: 200, json: async () => ({ ok: true, result: {} }) }),
  });
  await c.sendMessage(42, '안녕');
  await c.getUpdates();
  assert.equal(calls[0].url, 'https://api.telegram.org/bot123456:ABCDEFGHIJKLMNOPQRSTUVWXYZ/sendMessage');
  assert.deepEqual(calls[0].body, { chat_id: 42, text: '안녕', disable_web_page_preview: true });
  assert.ok(!('offset' in calls[1].body), '다른 팀원의 연결 코드를 지우지 않도록 offset 을 넘기지 않음');
  const bad = new TelegramClient({ token: '123456:ABCDEFGHIJKLMNOPQRSTUVWXYZ', fetchImpl: async () => ({ status: 401, json: async () => ({ ok: false, description: 'Unauthorized' }) }) });
  await assert.rejects(() => bad.getMe(), /Unauthorized/);
});
