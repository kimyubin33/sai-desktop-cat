// 간단한 JSON 파일 저장소.
// 개인 데이터(private/)와 공유 가능 데이터(shared/)를 서로 다른 폴더·파일에 분리해 저장한다.
const fs = require('fs');
const path = require('path');

class JsonFile {
  constructor(file, defaults) {
    this.file = file;
    this.defaults = defaults;
    this.data = this.load();
    this.saveTimer = null;
  }

  load() {
    try {
      const raw = fs.readFileSync(this.file, 'utf8');
      return deepMerge(structuredClone(this.defaults), JSON.parse(raw));
    } catch {
      return structuredClone(this.defaults);
    }
  }

  get() {
    return this.data;
  }

  update(fn) {
    fn(this.data);
    this.scheduleSave();
    return this.data;
  }

  replace(data) {
    this.data = data;
    this.scheduleSave();
  }

  scheduleSave() {
    clearTimeout(this.saveTimer);
    this.saveTimer = setTimeout(() => this.saveNow(), 300);
  }

  saveNow() {
    clearTimeout(this.saveTimer);
    fs.mkdirSync(path.dirname(this.file), { recursive: true });
    const tmp = this.file + '.tmp';
    fs.writeFileSync(tmp, JSON.stringify(this.data, null, 2), 'utf8');
    fs.renameSync(tmp, this.file);
  }
}

function deepMerge(base, extra) {
  if (!extra || typeof extra !== 'object' || Array.isArray(extra)) return extra ?? base;
  for (const [k, v] of Object.entries(extra)) {
    if (v && typeof v === 'object' && !Array.isArray(v) && base[k] && typeof base[k] === 'object' && !Array.isArray(base[k])) {
      base[k] = deepMerge(base[k], v);
    } else {
      base[k] = v;
    }
  }
  return base;
}

const DEFAULT_SETTINGS = {
  catName: '사이',
  onboarded: false,
  // 말 거는 빈도: off / low / normal / high
  frequency: 'normal', // low / normal / high — 끄는 선택지는 없다
  quietUntil: 0, // "잠시 조용히" 가 끝나는 시각 (최대 1시간)
  focusUntil: 0,
  // 방해 금지 시간 (예: { start: '23:00', end: '08:00' }) — null 이면 사용 안 함
  dnd: null,
  // 사용자가 "이런 말 줄이기"를 누른 주제별 가중치 감소 횟수
  dampenedTopics: {},
  ai: { enabled: false, model: 'claude-sonnet-5-5' },
  apiKeyEnc: null,
  retentionDays: 30,
  position: null,
  // 팀 연결 (3단계)
  firebase: null, // { apiKey, projectId } — Firebase 콘솔의 웹 앱 설정 (공개돼도 되는 값)
  firebaseSessionEnc: null, // 익명 로그인 refresh token (암호화)
  team: null, // { teamId, name, joinCode, displayName }
  localUid: null, // 로컬 시험 모드용
  // 외부 연결 (5단계) — 모두 이 기기에만 저장
  googleClient: null, // 설정에서 넣은 OAuth 클라이언트 (없으면 내장값)
  googleTokensEnc: null, // Google refresh token (암호화)
  googleTaskLinks: {}, // { projectId: { saiTaskId: googleTaskId } }
  telegramAway: true, // 자리 비웠을 때 진행 확인을 Telegram 으로
};

function createStores(userDataDir) {
  return {
    settings: new JsonFile(path.join(userDataDir, 'settings.json'), DEFAULT_SETTINGS),
    // 개인 대화 원문: 이 기기에만 저장하고 어떤 공유 경로에도 넣지 않는다
    privateConv: new JsonFile(path.join(userDataDir, 'private', 'conversations.json'), { sessions: [] }),
    // 공유 대기함: 일반화된 주제와 문안만 저장 (원문·이름·날짜 세부 없음)
    sharedOutbox: new JsonFile(path.join(userDataDir, 'shared', 'outbox.json'), { items: [] }),
    // 팀 캐시: 팀원 이름, 받은 주제, 팀 약속 제안 (원문 없음)
    teamCache: new JsonFile(path.join(userDataDir, 'shared', 'team-cache.json'), {
      members: [],
      inbox: [],
      proposals: [],
      lastSync: 0,
      lastError: null,
      myUid: null,
      shownProposals: {},
    }),
    // 내 작업 시간표와 일정 조정 대화: 이 기기에만 저장
    schedule: new JsonFile(path.join(userDataDir, 'private', 'schedule.json'), {}),
    adviceHistory: new JsonFile(path.join(userDataDir, 'advice-history.json'), { recent: [] }),
  };
}

module.exports = { JsonFile, createStores, DEFAULT_SETTINGS, deepMerge };
