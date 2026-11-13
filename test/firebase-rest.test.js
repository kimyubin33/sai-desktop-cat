// FirebaseBackend 가 Firestore/Auth REST 형식대로 요청하는지 가짜 fetch 로 확인한다.
// (실제 서버·보안 규칙 검사는 test/backend-firebase.emulator.js 에서)
const test = require('node:test');
const assert = require('node:assert/strict');
const { FirebaseBackend, parseFirebaseConfig } = require('../src/core/team/firebase-backend');
const { encodeFields, decodeDoc } = require('../src/core/team/codec');

function fakeFetch(handler) {
  const calls = [];
  const fn = async (url, opts = {}) => {
    calls.push({ url, ...opts, json: opts.body && opts.headers['content-type'] === 'application/json' ? JSON.parse(opts.body) : null });
    const { status = 200, body = {} } = (await handler(url, opts)) || {};
    return { ok: status < 400, status, json: async () => body, text: async () => JSON.stringify(body) };
  };
  fn.calls = calls;
  return fn;
}

const DOCS = 'https://firestore.googleapis.com/v1/projects/p1/databases/(default)/documents';

test('익명 로그인 후 Bearer 토큰으로 문서를 만든다', async () => {
  const f = fakeFetch((url) => {
    if (url.includes('accounts:signUp')) return { body: { idToken: 'T1', refreshToken: 'R1', localId: 'U1', expiresIn: '3600' } };
    return { body: { name: `${DOCS}/teams/abc`, fields: encodeFields({ name: 'x' }) } };
  });
  let saved;
  const b = new FirebaseBackend({ config: { apiKey: 'K', projectId: 'p1' }, fetchImpl: f, saveSession: (s) => (saved = s) });
  const doc = await b.createDoc('teams/abc/members', 'U1', { displayName: '유빈', joinCode: 'ABC234' });
  assert.equal(f.calls[0].url, 'https://identitytoolkit.googleapis.com/v1/accounts:signUp?key=K');
  assert.equal(f.calls[1].url, `${DOCS}/teams/abc/members?documentId=U1`);
  assert.equal(f.calls[1].method, 'POST');
  assert.equal(f.calls[1].headers.authorization, 'Bearer T1');
  assert.deepEqual(f.calls[1].json.fields.displayName, { stringValue: '유빈' });
  assert.deepEqual(saved, { uid: 'U1', refreshToken: 'R1' });
  assert.equal(doc.id, 'abc');
});

test('저장된 refresh token 이 있으면 securetoken 으로 갱신한다', async () => {
  const f = fakeFetch((url) =>
    url.includes('securetoken')
      ? { body: { id_token: 'T2', refresh_token: 'R2', user_id: 'U9', expires_in: '3600' } }
      : { status: 404 },
  );
  const b = new FirebaseBackend({ config: { apiKey: 'K', projectId: 'p1' }, fetchImpl: f, loadSession: () => ({ uid: 'U9', refreshToken: 'R1' }) });
  assert.equal(await b.signIn(), 'U9');
  assert.equal(f.calls[0].url, 'https://securetoken.googleapis.com/v1/token?key=K');
  assert.match(f.calls[0].body, /grant_type=refresh_token&refresh_token=R1/);
});

test('응답은 자기 uid 필드만 updateMask 로 바꾼다', async () => {
  const f = fakeFetch((url) =>
    url.includes('signUp') ? { body: { idToken: 'T', refreshToken: 'R', localId: 'U1', expiresIn: '3600' } } : { body: {} },
  );
  const b = new FirebaseBackend({ config: { apiKey: 'K', projectId: 'p1' }, fetchImpl: f });
  await b.respondProposal('t1', 'p1', 'agree');
  const call = f.calls[1];
  assert.equal(call.method, 'PATCH');
  assert.ok(call.url.startsWith(`${DOCS}/teams/t1/proposals/p1?updateMask.fieldPaths=${encodeURIComponent('responses.`U1`')}`));
  assert.ok(call.url.endsWith('&currentDocument.exists=true'));
  assert.deepEqual(call.json.fields.responses, { mapValue: { fields: { U1: { stringValue: 'agree' } } } });
});

test('받은 신호는 toMember == 내 uid 쿼리로 가져온다', async () => {
  const f = fakeFetch((url) => {
    if (url.includes('signUp')) return { body: { idToken: 'T', refreshToken: 'R', localId: 'U2', expiresIn: '3600' } };
    return {
      body: [
        { document: { name: `${DOCS}/teams/t1/signals/s1`, fields: encodeFields({ topic: 'workload', toMember: 'U2', week: '2026-W46', held: false }) } },
        { readTime: 'x' },
      ],
    };
  });
  const b = new FirebaseBackend({ config: { apiKey: 'K', projectId: 'p1' }, fetchImpl: f });
  const rows = await b.listSignalsForMe('t1');
  const q = f.calls[1];
  assert.equal(q.url, `${DOCS}/teams/t1:runQuery`);
  assert.deepEqual(q.json.structuredQuery.where.fieldFilter, {
    field: { fieldPath: 'toMember' },
    op: 'EQUAL',
    value: { stringValue: 'U2' },
  });
  assert.deepEqual(rows, [{ id: 's1', topic: 'workload', toMember: 'U2', week: '2026-W46', held: false }]);
});

test('Firestore 값 인코딩 왕복', () => {
  const data = { a: 'x', n: 3, f: 1.5, b: true, z: null, m: { k: ['p', 2] } };
  assert.deepEqual(decodeDoc({ name: 'x/y/id1', fields: encodeFields(data) }), { id: 'id1', ...data });
});

test('Firebase 콘솔에서 복사한 설정을 읽는다', () => {
  const snippet = `const firebaseConfig = {
    apiKey: "AIzaSyA-123",
    authDomain: "sai-team.firebaseapp.com",
    projectId: "sai-team",
  };`;
  assert.deepEqual(parseFirebaseConfig(snippet), { apiKey: 'AIzaSyA-123', projectId: 'sai-team' });
  assert.deepEqual(parseFirebaseConfig('{"apiKey":"k","projectId":"p"}'), { apiKey: 'k', projectId: 'p' });
  assert.equal(parseFirebaseConfig('nothing'), null);
});
