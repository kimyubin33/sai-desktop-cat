// 팀 연결을 앱에 붙인다: 백엔드 선택, 주기적 동기화, 설정 저장.
const { TeamSync } = require('../core/team/team-sync');
const { FirebaseBackend, parseFirebaseConfig } = require('../core/team/firebase-backend');
const { MemoryBackend } = require('../core/team/memory-backend');
const { randomCode } = require('../core/team/firebase-backend');
const { encryptSecret, decryptJson } = require('./secret');

const SYNC_MS = 5 * 60 * 1000;

function createTeamService({ stores, onChange = () => {} }) {
  const settings = stores.settings;

  function makeBackend() {
    // 로컬 시험 모드: 한 컴퓨터에서 앱 두 개를 띄워 팀처럼 시험 (SAI_TEAM_FILE=공유 파일 경로)
    if (process.env.SAI_TEAM_FILE) {
      if (!settings.get().localUid) settings.update((s) => (s.localUid = `local-${randomCode(10).toLowerCase()}`));
      return new MemoryBackend({ file: process.env.SAI_TEAM_FILE, uid: settings.get().localUid });
    }
    const cfg = settings.get().firebase;
    if (!cfg || !cfg.apiKey || !cfg.projectId) return null;
    const emulator = process.env.SAI_FIREBASE_EMULATOR
      ? { auth: 'http://127.0.0.1:9099', firestore: 'http://127.0.0.1:8080' }
      : undefined;
    return new FirebaseBackend({
      config: { ...cfg, emulator },
      loadSession: () => decryptJson(settings.get().firebaseSessionEnc),
      saveSession: (sess) => settings.update((s) => (s.firebaseSessionEnc = encryptSecret(sess))),
    });
  }

  const sync = new TeamSync({
    makeBackend,
    state: {
      get: () => settings.get().team,
      set: (v) => settings.update((s) => (s.team = v)),
    },
    outbox: stores.sharedOutbox,
    cache: stores.teamCache,
    delayMs: process.env.SAI_SEND_DELAY_MS != null ? Number(process.env.SAI_SEND_DELAY_MS) : null,
  });

  let timer = null;
  async function syncNow() {
    if (!sync.isConnected()) return { ok: false, reason: 'not-connected' };
    const r = await sync.sync();
    onChange();
    return r;
  }

  function start() {
    clearInterval(timer);
    timer = setInterval(syncNow, SYNC_MS);
    setTimeout(syncNow, 3000);
  }

  function stop() {
    clearInterval(timer);
  }

  function configure(text) {
    const cfg = parseFirebaseConfig(text);
    if (!cfg) throw new Error('apiKey 와 projectId 를 찾지 못했어요. Firebase 콘솔의 firebaseConfig 를 그대로 붙여 넣어 주세요.');
    const prev = settings.get().firebase;
    settings.update((s) => {
      s.firebase = cfg;
      if (!prev || prev.projectId !== cfg.projectId) {
        // 다른 프로젝트로 바꾸면 이전 로그인과 팀 연결은 쓸 수 없다
        s.firebaseSessionEnc = null;
        s.team = null;
      }
    });
    sync.resetBackend();
    return cfg;
  }

  function status() {
    const s = sync.status();
    const cfg = settings.get().firebase;
    return {
      ...s,
      configured: !!process.env.SAI_TEAM_FILE || !!(cfg && cfg.apiKey),
      projectId: process.env.SAI_TEAM_FILE ? '로컬 시험 모드' : cfg && cfg.projectId,
      localMode: !!process.env.SAI_TEAM_FILE,
    };
  }

  return { sync, syncNow, start, stop, configure, status };
}

module.exports = { createTeamService };
