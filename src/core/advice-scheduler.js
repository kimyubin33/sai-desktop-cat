// 언제, 무슨 말을 건넬지 정한다 (Electron 의존성 없음).
// - 빈도 설정, 조용한 모드, 집중 모드, 방해 금지 시간을 지킨다.
// - 최근에 한 말은 다시 하지 않는다.
// - 사용자가 "이런 말 줄이기"를 누른 주제는 비중을 낮춘다.
// - boosts: 나중에 팀 고민에서 허용된 주제를 조금 더 자주 다루기 위한 자리 (3단계)

const INTERVAL_MIN = {
  low: [25, 45],
  normal: [10, 20],
  high: [4, 8],
};

const RECENT_LIMIT = 25;
const DAMPEN_FACTOR = 0.35;
const BOOST_FACTOR = 2.5;

function parseHM(s) {
  const m = /^(\d{1,2}):(\d{2})$/.exec(s || '');
  if (!m) return null;
  return Number(m[1]) * 60 + Number(m[2]);
}

function inDnd(dnd, date) {
  if (!dnd) return false;
  const start = parseHM(dnd.start);
  const end = parseHM(dnd.end);
  if (start === null || end === null || start === end) return false;
  const cur = date.getHours() * 60 + date.getMinutes();
  return start < end ? cur >= start && cur < end : cur >= start || cur < end;
}

// 사이는 업무 도우미라 아예 꺼 두면 쓸모가 없어진다. 그래서 "영구히 끄기"는 없고,
// 쉬는 방법은 모두 정해진 시간이 지나면 저절로 끝난다.
const MAX_QUIET_MS = 60 * 60 * 1000; // 잠시 조용히: 최대 1시간
const MAX_FOCUS_MS = 2 * 60 * 60 * 1000; // 집중 모드: 최대 2시간
const MAX_DND_MINUTES = 10 * 60; // 방해 금지(잠·휴식 시간): 하루 최대 10시간

function dndSpanMinutes(dnd) {
  const start = parseHM(dnd && dnd.start);
  const end = parseHM(dnd && dnd.end);
  if (start === null || end === null) return 0;
  return (end - start + 24 * 60) % (24 * 60);
}

/** 지금 먼저 말을 걸면 안 되는 이유 (없으면 null) */
function silenceReason(settings, now = new Date()) {
  const t = now.getTime();
  // 예전 버전의 저장값(꺼 두기·계속 조용히)은 더 이상 말을 막지 않는다
  if (settings.quietUntil && settings.quietUntil > t && settings.quietUntil - t <= MAX_QUIET_MS) return 'quiet';
  if (settings.focusUntil && settings.focusUntil > t && settings.focusUntil - t <= MAX_FOCUS_MS) return 'focus';
  if (dndSpanMinutes(settings.dnd) <= MAX_DND_MINUTES && inDnd(settings.dnd, now)) return 'dnd';
  return null;
}

function nextDelayMs(settings, rand = Math.random) {
  const [a, b] = INTERVAL_MIN[settings.frequency] || INTERVAL_MIN.low;
  return Math.round((a + rand() * (b - a)) * 60 * 1000);
}

function weightOf(entry, { dampened = {}, boosts = {}, hour = 12 }) {
  let w = 1;
  if (entry.kind === 'quote') w = 0.6;
  if (entry.kind === 'teamwork') w = 0.9;
  if (entry.id === 'daily-wrap-up') w = hour >= 17 ? 1.5 : 0.1;
  if (entry.id === 'daily-start-small') w = hour < 12 ? 1.5 : 0.6;
  if (entry.topic) {
    const n = dampened[entry.topic] || 0;
    w *= Math.pow(DAMPEN_FACTOR, n);
    if (boosts[entry.topic]) w *= BOOST_FACTOR;
  }
  return w;
}

/**
 * 다음 조언 하나를 고른다.
 * recent: 최근에 보여 준 id 목록 (최신이 뒤)
 */
function pickAdvice(library, { recent = [], dampened = {}, boosts = {}, rand = Math.random, now = new Date() } = {}) {
  const recentSet = new Set(recent.slice(-RECENT_LIMIT));
  let pool = library.filter((e) => !recentSet.has(e.id));
  if (pool.length === 0) {
    // 모두 최근에 했다면 가장 오래전에 한 말부터 다시 허용한다
    const order = new Map(recent.map((id, i) => [id, i]));
    pool = [...library].sort((a, b) => (order.get(a.id) ?? -1) - (order.get(b.id) ?? -1)).slice(0, 5);
  }
  const hour = now.getHours();
  const weights = pool.map((e) => weightOf(e, { dampened, boosts, hour }));
  const total = weights.reduce((a, b) => a + b, 0);
  let r = rand() * total;
  for (let i = 0; i < pool.length; i++) {
    r -= weights[i];
    if (r <= 0) return pool[i];
  }
  return pool[pool.length - 1];
}

function rememberShown(recent, id) {
  const next = recent.filter((x) => x !== id);
  next.push(id);
  return next.slice(-RECENT_LIMIT * 2);
}

module.exports = {
  MAX_QUIET_MS,
  MAX_FOCUS_MS,
  MAX_DND_MINUTES,
  dndSpanMinutes,
  INTERVAL_MIN,
  RECENT_LIMIT,
  inDnd,
  silenceReason,
  nextDelayMs,
  pickAdvice,
  rememberShown,
  weightOf,
};
