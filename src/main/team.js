// 팀 연결을 앱에 붙인다: 백엔드 선택, 주기적 동기화, 설정 저장.
const { TeamHub } = require('../core/team/team-hub');
const { createGoogleService } = require('./google');
const { FirebaseBackend, parseFirebaseConfig } = require('../core/team/firebase-backend');
const { MemoryBackend } = require('../core/team/memory-backend');
const { randomCode } = require('../core/team/firebase-backend');
const { encryptSecret, decryptJson } = require('./secret');
const { BUILTIN_FIREBASE } = require('./firebase-config');

const SYNC_MS = 5 * 60 * 1000;

function createTeamService({ stores, onChange = () => {} }) {
  const settings = stores.settings;

  function makeBackend() {
    // 로컬 시험 모드: 한 컴퓨터에서 앱 두 개를 띄워 팀처럼 시험 (SAI_TEAM_FILE=공유 파일 경로)
    if (process.env.SAI_TEAM_FILE) {
      if (!settings.get().localUid) settings.update((s) => (s.localUid = `local-${randomCode(10).toLowerCase()}`));
      return new MemoryBackend({ file: process.env.SAI_TEAM_FILE, uid: settings.get().localUid });
    }
    const cfg = settings.get().firebase || BUILTIN_FIREBASE;
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

  const hub = new TeamHub({
    settings,
    outbox: stores.sharedOutbox,
    teamCache: stores.teamCache,
    makeBackend,
    delayMs: process.env.SAI_SEND_DELAY_MS != null ? Number(process.env.SAI_SEND_DELAY_MS) : null,
  });
  const google = createGoogleService({
    stores,
    collabs: () => hub.all().map((b) => b.collab),
    activeCollab: () => hub.active().collab,
  });

  let timer = null;
  async function syncNow() {
    const results = await hub.syncAll();
    if (results.length) await google.syncTodo().catch(() => {});
    onChange();
    return results.find((r) => !r.ok) || results[0] || { ok: false, reason: 'not-connected' };
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
        s.teams = [];
        s.activeTeamId = null;
      }
    });
    hub.resetBackend();
    return cfg;
  }

  function status() {
    const s = hub.active().sync.status();
    const cfg = settings.get().firebase || BUILTIN_FIREBASE;
    return {
      ...s,
      configured: !!process.env.SAI_TEAM_FILE || !!(cfg && cfg.apiKey),
      projectId: process.env.SAI_TEAM_FILE ? '로컬 시험 모드' : cfg && cfg.projectId,
      localMode: !!process.env.SAI_TEAM_FILE,
    };
  }

  function teams() {
    const activeId = hub.active().teamId;
    return hub.teams().map((t) => {
      const b = hub.bundle(t.teamId);
      const c = b.sync.cache.get();
      return {
        teamId: t.teamId,
        name: t.name,
        joinCode: t.joinCode,
        displayName: t.displayName,
        active: t.teamId === activeId,
        memberCount: (c.members || []).length,
        lastSync: c.lastSync || 0,
        lastError: c.lastError || null,
      };
    });
  }

  return {
    hub,
    // 화면(협업·정리함·설정)은 "지금 보고 있는 팀"을 쓴다
    get sync() {
      return hub.active().sync;
    },
    get collab() {
      return hub.active().collab;
    },
    get notifier() {
      return hub.active().notifier;
    },
    google,
    teams,
    setActive: (id) => hub.setActive(id),
    createTeam: (name, displayName) => hub.createTeam(name, displayName),
    joinTeam: (code, displayName) => hub.joinTeam(code, displayName),
    leaveTeam: (id) => hub.leaveTeam(id || hub.active().teamId),
    syncNow,
    start,
    stop,
    configure,
    status,
  };
}

module.exports = { createTeamService };
