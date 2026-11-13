// 한국어 날짜·시간 표현을 읽는다. 예: "내일 오후 3시", "목요일 2시 반부터 4시까지", "11월 20일", "15:30~17:00", "2시간 반"
// 모든 계산은 이 컴퓨터의 현지 시각 기준이다.

const DAY_MS = 24 * 60 * 60 * 1000;
const WEEKDAYS = ['일', '월', '화', '수', '목', '금', '토'];

function startOfDay(t) {
  const d = new Date(t);
  d.setHours(0, 0, 0, 0);
  return d.getTime();
}

function addDays(dayMs, n) {
  const d = new Date(dayMs);
  d.setDate(d.getDate() + n);
  return d.getTime();
}

/** 날짜 하나를 찾는다. 없으면 null */
function parseDay(text, now = Date.now()) {
  const today = startOfDay(now);
  const s = String(text);
  if (/오늘/.test(s)) return today;
  if (/내일/.test(s)) return addDays(today, 1);
  if (/모레/.test(s)) return addDays(today, 2);
  if (/글피/.test(s)) return addDays(today, 3);

  let m = /(\d{1,2})\s*월\s*(\d{1,2})\s*일/.exec(s) || /(?<![\d:])(\d{1,2})\/(\d{1,2})(?![\d:])/.exec(s);
  if (m) {
    const y = new Date(now).getFullYear();
    let d = new Date(y, Number(m[1]) - 1, Number(m[2])).getTime();
    if (d < addDays(today, -30)) d = new Date(y + 1, Number(m[1]) - 1, Number(m[2])).getTime();
    return d;
  }
  m = /(\d{4})-(\d{2})-(\d{2})/.exec(s);
  if (m) return new Date(Number(m[1]), Number(m[2]) - 1, Number(m[3])).getTime();

  m = /(이번\s*주|다음\s*주|담주|다다음\s*주)?\s*([일월화수목금토])\s*요일/.exec(s);
  if (m) {
    const want = WEEKDAYS.indexOf(m[2]);
    const cur = new Date(today).getDay();
    const which = (m[1] || '').replace(/\s/g, '');
    if (which === '이번주') {
      // 이번 주(월요일 시작) 안의 그 요일
      const monday = addDays(today, -((cur + 6) % 7));
      return addDays(monday, (want + 6) % 7);
    }
    if (which === '다음주' || which === '담주' || which === '다다음주') {
      const monday = addDays(today, 7 - ((cur + 6) % 7) + (which === '다다음주' ? 7 : 0));
      return addDays(monday, (want + 6) % 7);
    }
    return addDays(today, (want - cur + 7) % 7);
  }

  m = /(?<![\d월])(\d{1,2})\s*일(?![\s]*(?:동안|간))/.exec(s);
  if (m && !/시간|분/.test(s.slice(m.index, m.index + m[0].length + 1))) {
    const n = Number(m[1]);
    if (n >= 1 && n <= 31) {
      const d0 = new Date(today);
      let d = new Date(d0.getFullYear(), d0.getMonth(), n).getTime();
      if (d < today) d = new Date(d0.getFullYear(), d0.getMonth() + 1, n).getTime();
      return d;
    }
  }
  return null;
}

const PERIOD = '(오전|오후|아침|낮|저녁|밤|새벽)?';
const TIME_RE = new RegExp(`${PERIOD}\\s*(\\d{1,2})\\s*(?:시\\s*(반|(\\d{1,2})\\s*분)?|:(\\d{2}))`, 'g');

function toMinutes(period, h, half, min, colonMin) {
  let hour = Number(h);
  const minute = half === '반' ? 30 : Number(min || colonMin || 0);
  if (period === '오후' || period === '저녁' || period === '밤') {
    if (hour < 12) hour += 12;
  } else if (period === '오전' || period === '아침' || period === '새벽') {
    if (hour === 12) hour = 0;
  } else if (period === '낮') {
    if (hour < 6) hour += 12;
  } else if (colonMin === undefined && hour >= 1 && hour <= 7) {
    // "3시" 처럼 오전·오후가 없으면 낮 시간으로 본다 (새벽 3시에 작업을 잡는 일은 드물다)
    hour += 12;
  }
  return hour * 60 + minute;
}

/** 시각들을 찾는다. [{ minutes, period, index }] */
function findTimes(text) {
  const out = [];
  const s = String(text);
  TIME_RE.lastIndex = 0;
  let m;
  while ((m = TIME_RE.exec(s))) {
    // "2시간" 은 시각이 아니다
    if (s.slice(m.index + m[0].length).startsWith('간')) continue;
    if (m[0].includes(':') && Number(m[2]) > 23) continue;
    if (Number(m[2]) > 24) continue;
    out.push({ minutes: toMinutes(m[1], m[2], m[3], m[4], m[5]), period: m[1] || null, index: m.index, raw: m[0], hasPeriod: !!m[1], colon: m[5] !== undefined });
  }
  return out;
}

/** "N시간 (반)", "N분", "N.5시간" */
function parseDuration(text) {
  const s = String(text);
  let m = /(\d+(?:\.\d+)?)\s*시간\s*(반)?(?:\s*(\d{1,2})\s*분)?/.exec(s);
  if (m) return Number(m[1]) + (m[2] ? 0.5 : 0) + (m[3] ? Number(m[3]) / 60 : 0);
  m = /(\d{1,3})\s*분/.exec(s);
  if (m && !/시\s*\d{1,2}\s*분/.test(s)) return Number(m[1]) / 60;
  return null;
}

/**
 * 한 문장에서 날짜·시작·끝·길이를 읽는다.
 * { day, start, end, duration } — start/end 는 그날 0시부터의 분, day 는 그날 0시(ms)
 */
function parseWhen(text, now = Date.now()) {
  const s = String(text);
  const day = parseDay(s, now);
  const times = findTimes(s);
  let start = null;
  let end = null;
  if (times.length) {
    start = times[0].minutes;
    if (times.length > 1) {
      const t = times[1];
      end = t.minutes;
      // "오후 2시부터 4시까지": 뒤 시각에 오전/오후가 없으면 앞 시각을 따른다
      if (!t.hasPeriod && !t.colon && end <= start && end + 12 * 60 > start && end < 12 * 60) end += 12 * 60;
    }
  }
  const duration = parseDuration(s);
  if (start != null && end == null && duration) end = start + Math.round(duration * 60);
  return { day, start, end, duration };
}

function fmtTime(t) {
  const d = new Date(t);
  return `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`;
}

function fmtDay(t) {
  const d = new Date(t);
  return `${d.getMonth() + 1}/${d.getDate()}(${WEEKDAYS[d.getDay()]})`;
}

function fmtRange(start, end) {
  return `${fmtDay(start)} ${fmtTime(start)}–${fmtTime(end)}`;
}

/** "2026-11-15T14:00" 같은 현지 시각 문자열 → ms */
function parseLocalIso(s) {
  const m = /^(\d{4})-(\d{2})-(\d{2})[T ](\d{1,2}):(\d{2})/.exec(String(s || ''));
  if (!m) return null;
  return new Date(Number(m[1]), Number(m[2]) - 1, Number(m[3]), Number(m[4]), Number(m[5])).getTime();
}

function toLocalIso(t) {
  const d = new Date(t);
  const p = (n) => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}T${p(d.getHours())}:${p(d.getMinutes())}`;
}

module.exports = { parseDay, parseWhen, findTimes, parseDuration, startOfDay, addDays, fmtTime, fmtDay, fmtRange, parseLocalIso, toLocalIso, WEEKDAYS, DAY_MS };
