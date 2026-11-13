const test = require('node:test');
const assert = require('node:assert/strict');
const crypto = require('crypto');
const { GoogleAuth, pkce, authUrl, emailFromIdToken, parseGoogleClient } = require('../src/core/google/oauth');
const { freeIntervals, freeHours, findSlot, queryBusy } = require('../src/core/google/calendar');
const { ensureList, syncTasks } = require('../src/core/google/tasks');

const at = (d, h, m = 0) => new Date(2026, 10, d, h, m).getTime();
const HOUR = 3600 * 1000;

// ---------- 로그인 ----------
test('PKCE 와 로그인 주소', () => {
  const { verifier, challenge } = pkce();
  assert.match(verifier, /^[A-Za-z0-9_-]{43}$/);
  const expect = crypto.createHash('sha256').update(verifier).digest('base64').replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
  assert.equal(challenge, expect);
  const u = new URL(authUrl({ clientId: 'cid', redirectUri: 'http://127.0.0.1:5555', challenge, state: 'st' }));
  assert.equal(u.origin + u.pathname, 'https://accounts.google.com/o/oauth2/v2/auth');
  assert.equal(u.searchParams.get('code_challenge_method'), 'S256');
  assert.equal(u.searchParams.get('access_type'), 'offline');
  assert.match(u.searchParams.get('scope'), /calendar\.freebusy/);
  assert.ok(!/calendar\.readonly|auth\/calendar(\s|$)/.test(u.searchParams.get('scope')), '일정 내용을 읽는 넓은 권한은 요청하지 않음');
});

test('코드 교환·토큰 갱신·만료 처리', async () => {
  let saved = null;
  const calls = [];
  const idToken = ['x', Buffer.from(JSON.stringify({ email: 'yubin@gmail.com' })).toString('base64url'), 'sig'].join('.');
  let mode = 'ok';
  const fetchImpl = async (url, o) => {
    const p = Object.fromEntries(new URLSearchParams(o.body));
    calls.push({ url, p });
    if (p.grant_type === 'authorization_code') return { ok: true, status: 200, json: async () => ({ access_token: 'A1', refresh_token: 'R1', expires_in: 3600, id_token: idToken }) };
    if (mode === 'expired') return { ok: false, status: 400, json: async () => ({ error: 'invalid_grant' }) };
    return { ok: true, status: 200, json: async () => ({ access_token: 'A2', expires_in: 3600 }) };
  };
  let now = 0;
  const g = new GoogleAuth({ client: { clientId: 'cid', clientSecret: 'sec' }, loadTokens: () => saved, saveTokens: (t) => (saved = t), fetchImpl, now: () => now });
  assert.deepEqual(await g.exchangeCode({ code: 'C', verifier: 'V', redirectUri: 'http://127.0.0.1:1' }), { email: 'yubin@gmail.com' });
  assert.equal(calls[0].p.code_verifier, 'V');
  assert.equal(saved.refreshToken, 'R1');
  assert.equal(await g.accessToken(), 'A1');
  now = 2 * HOUR;
  assert.equal(await g.accessToken(), 'A2');
  assert.equal(calls[1].p.grant_type, 'refresh_token');
  now = 4 * HOUR;
  mode = 'expired';
  await assert.rejects(() => g.accessToken(), /만료/);
  assert.equal(saved, null);
  assert.equal(emailFromIdToken('bad'), null);
});

test('콘솔에서 받은 클라이언트 정보를 읽는다', () => {
  const json = '{"installed":{"client_id":"123-abc.apps.googleusercontent.com","client_secret":"GOCSPX-xyz_1"}}';
  assert.deepEqual(parseGoogleClient(json), { clientId: '123-abc.apps.googleusercontent.com', clientSecret: 'GOCSPX-xyz_1' });
  assert.deepEqual(parseGoogleClient('클라이언트 ID 123-abc.apps.googleusercontent.com\n보안 비밀번호 GOCSPX-xyz_1'), {
    clientId: '123-abc.apps.googleusercontent.com',
    clientSecret: 'GOCSPX-xyz_1',
  });
  assert.equal(parseGoogleClient('없음'), null);
});

// ---------- 캘린더 ----------
test('바쁜 시간을 빼고 일할 수 있는 시간대의 빈 시간을 센다', () => {
  const busy = [
    { start: at(16, 10), end: at(16, 12) }, // 월 수업
    { start: at(16, 11), end: at(16, 13) }, // 겹침
    { start: at(16, 20), end: at(17, 1) }, // 밤 약속 (다음날까지)
  ];
  const free = freeIntervals(busy, { from: at(16, 0), to: at(17, 23, 59), dayStart: 9, dayEnd: 22 });
  assert.deepEqual(
    free.map((f) => [new Date(f.start).getHours(), new Date(f.end).getHours()]),
    [[9, 10], [13, 20], [9, 22]],
  );
  assert.equal(freeHours(busy, { from: at(16, 0), to: at(17, 23, 59) }), 21);
});

test('작업 시간은 첫 빈 틈에 최대 2시간 블록으로 잡는다', () => {
  const busy = [{ start: at(16, 9), end: at(16, 18, 30) }];
  const s = findSlot(busy, { from: at(16, 8, 10), hours: 4 });
  assert.equal(new Date(s.start).getHours() * 60 + new Date(s.start).getMinutes(), 18 * 60 + 30);
  assert.equal((s.end - s.start) / HOUR, 2);
  // 1시간보다 짧은 틈은 건너뛴다
  const s2 = findSlot([{ start: at(16, 9), end: at(16, 21, 30) }], { from: at(16, 9), hours: 3 });
  assert.equal(new Date(s2.start).getDate(), 17);
});

test('free/busy 요청은 primary 캘린더의 바쁜 시간만 묻는다', async () => {
  let body;
  const api = async (m, url, b) => ((body = { m, url, b }), { calendars: { primary: { busy: [{ start: '2026-11-16T01:00:00Z', end: '2026-11-16T03:00:00Z' }] } } });
  const r = await queryBusy(api, { from: at(16, 0), to: at(17, 0) });
  assert.equal(body.url, 'https://www.googleapis.com/calendar/v3/freeBusy');
  assert.deepEqual(body.b.items, [{ id: 'primary' }]);
  assert.equal(r[0].end - r[0].start, 2 * HOUR);
});

// ---------- 할 일 ----------
function fakeTasksApi() {
  const db = { lists: [], tasks: {} };
  let n = 0;
  const api = async (method, url, body) => {
    const u = new URL(url);
    const p = u.pathname.replace('/tasks/v1', '');
    if (p === '/users/@me/lists' && method === 'GET') return { items: db.lists };
    if (p === '/users/@me/lists' && method === 'POST') {
      const l = { id: `L${++n}`, title: body.title };
      db.lists.push(l);
      db.tasks[l.id] = [];
      return l;
    }
    let m = /^\/lists\/([^/]+)\/tasks$/.exec(p);
    if (m && method === 'GET') return { items: db.tasks[m[1]] };
    if (m && method === 'POST') {
      const t = { id: `T${++n}`, status: 'needsAction', ...body };
      db.tasks[m[1]].push(t);
      return t;
    }
    m = /^\/lists\/([^/]+)\/tasks\/([^/]+)$/.exec(p);
    if (m && method === 'PATCH') return Object.assign(db.tasks[m[1]].find((t) => t.id === m[2]), body);
    throw new Error(`unexpected ${method} ${url}`);
  };
  return { api, db };
}

test('수락한 작업을 올리고 완료 상태를 양쪽으로 맞춘다', async () => {
  const { api, db } = fakeTasksApi();
  const listId = await ensureList(api, '사이 · 홍보 영상');
  assert.equal(await ensureList(api, '사이 · 홍보 영상'), listId, '같은 목록을 두 번 만들지 않음');

  const mine = [
    { id: 's1', title: '촬영', doneCriteria: '인터뷰 3명', status: 'todo' },
    { id: 's2', title: '컷 편집', status: 'doing' },
    { id: 's3', title: '이미 끝난 일', status: 'done' },
  ];
  let r = await syncTasks(api, { listId, mine, deadline: '2026-11-28', projectTitle: '홍보 영상' });
  assert.equal(r.created, 2, '끝난 일은 새로 올리지 않음');
  assert.equal(db.tasks[listId][0].due, '2026-11-28T00:00:00.000Z');
  assert.match(db.tasks[listId][0].notes, /완료 기준: 인터뷰 3명/);

  // 다시 동기화해도 중복으로 만들지 않음
  r = await syncTasks(api, { listId, mine, links: r.links, deadline: '2026-11-28', projectTitle: '홍보 영상' });
  assert.equal(r.created, 0);
  assert.equal(db.tasks[listId].length, 2);

  // Google 에서 완료 → 사이에 완료 표시 / 사이에서 완료 → Google 완료 / 제목은 사이 기준
  db.tasks[listId].find((t) => t.title === '촬영').status = 'completed';
  db.tasks[listId].find((t) => t.title === '컷 편집').title = '컷편집(수정)';
  mine[1].status = 'done';
  r = await syncTasks(api, { listId, mine, links: r.links, deadline: '2026-11-28', projectTitle: '홍보 영상' });
  assert.deepEqual(r.markDone, ['s1']);
  assert.equal(r.completedInGoogle, 1);
  assert.equal(r.renamed, 1);
  assert.equal(db.tasks[listId].find((t) => t.id === r.links.s2).title, '컷 편집');

  // Google 에서 지운 할 일은 다시 만들지 않음
  db.tasks[listId] = db.tasks[listId].filter((t) => t.id !== r.links.s1);
  mine[0].status = 'doing';
  r = await syncTasks(api, { listId, mine, links: r.links, deadline: '2026-11-28', projectTitle: '홍보 영상' });
  assert.equal(r.links.s1, 'deleted');
  assert.equal(r.created, 0);
});
