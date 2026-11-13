// Firebase 에뮬레이터에서 실제 REST 호출과 firestore.rules 를 함께 검사한다.
// 실행: npm run test:emulator  (Java 와 firebase-tools 필요)
const test = require('node:test');
const { defineContract } = require('./backend-contract');
const { FirebaseBackend } = require('../src/core/team/firebase-backend');

const host = (v, d) => (v ? `http://${v}` : d);
const emulator = {
  auth: host(process.env.FIREBASE_AUTH_EMULATOR_HOST, 'http://127.0.0.1:9099'),
  firestore: host(process.env.FIRESTORE_EMULATOR_HOST, 'http://127.0.0.1:8080'),
};
const config = { apiKey: 'demo-key', projectId: process.env.GCLOUD_PROJECT || 'demo-sai', emulator };

defineContract(test, async (n) => Array.from({ length: n }, () => new FirebaseBackend({ config })));

test('저장한 refresh token 으로 다시 켜도 같은 사용자다', async () => {
  const assert = require('node:assert/strict');
  let saved = null;
  const a = new FirebaseBackend({ config, saveSession: (s) => (saved = s) });
  const uid = await a.signIn();
  const again = new FirebaseBackend({ config, loadSession: () => saved });
  assert.equal(await again.signIn(), uid);
});
