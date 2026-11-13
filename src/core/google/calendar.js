// Google 캘린더: 바쁜 시간(free/busy)만 읽어 빈 시간을 계산하고, 작업 시간을 일정으로 잡는다.
// 일정 제목·내용·참석자는 읽지 않는다 (freebusy 범위만 사용). 계산한 빈 시간은 이 기기에서만 쓰고,
// 팀에 보이는 것은 본인이 확인해 저장한 "주간 가능 시간" 숫자뿐이다.

const CAL = 'https://www.googleapis.com/calendar/v3';
const HOUR = 60 * 60 * 1000;
const DAY = 24 * HOUR;

async function queryBusy(api, { from, to, timeZone = 'Asia/Seoul' }) {
  const d = await api('POST', `${CAL}/freeBusy`, {
    timeMin: new Date(from).toISOString(),
    timeMax: new Date(to).toISOString(),
    timeZone,
    items: [{ id: 'primary' }],
  });
  const cal = (d.calendars && d.calendars.primary) || {};
  if (cal.errors && cal.errors.length) throw new Error(`캘린더를 읽지 못했어요: ${cal.errors[0].reason}`);
  return (cal.busy || []).map((b) => ({ start: Date.parse(b.start), end: Date.parse(b.end) }));
}

function merge(intervals) {
  const s = [...intervals].sort((a, b) => a.start - b.start);
  const out = [];
  for (const i of s) {
    const last = out[out.length - 1];
    if (last && i.start <= last.end) last.end = Math.max(last.end, i.end);
    else out.push({ ...i });
  }
  return out;
}

/** 하루 중 일할 수 있는 시간대(예: 9~22시)에서 바쁜 시간을 뺀 빈 구간들 */
function freeIntervals(busy, { from, to, dayStart = 9, dayEnd = 22 }) {
  const merged = merge(busy);
  const out = [];
  const d = new Date(from);
  d.setHours(0, 0, 0, 0);
  for (let day = d.getTime(); day < to; day += DAY) {
    const date = new Date(day);
    const ws = Math.max(from, new Date(date.getFullYear(), date.getMonth(), date.getDate(), dayStart).getTime());
    const we = Math.min(to, new Date(date.getFullYear(), date.getMonth(), date.getDate(), dayEnd).getTime());
    if (we <= ws) continue;
    let cur = ws;
    for (const b of merged) {
      if (b.end <= cur || b.start >= we) continue;
      if (b.start > cur) out.push({ start: cur, end: b.start });
      cur = Math.max(cur, b.end);
    }
    if (cur < we) out.push({ start: cur, end: we });
  }
  return out;
}

function freeHours(busy, opts) {
  return Math.round((freeIntervals(busy, opts).reduce((a, i) => a + (i.end - i.start), 0) / HOUR) * 10) / 10;
}

/**
 * 작업 시간을 잡을 첫 빈 구간. 긴 작업은 한 번에 maxBlockHours 까지만 잡는다.
 * minBlockHours 보다 짧은 틈은 건너뛴다.
 */
function findSlot(busy, { from, hours, dayStart = 9, dayEnd = 22, days = 14, maxBlockHours = 2, minBlockHours = 1 }) {
  const want = Math.min(hours, maxBlockHours) * HOUR;
  const min = Math.min(want, minBlockHours * HOUR);
  // 30분 단위로 시작
  const start = Math.ceil(from / (30 * 60 * 1000)) * 30 * 60 * 1000;
  for (const f of freeIntervals(busy, { from: start, to: start + days * DAY, dayStart, dayEnd })) {
    const len = f.end - f.start;
    if (len >= min) return { start: f.start, end: f.start + Math.min(len, want) };
  }
  return null;
}

async function createEvent(api, { summary, description, start, end, saiTaskId, timeZone = 'Asia/Seoul' }) {
  return api('POST', `${CAL}/calendars/primary/events`, {
    summary,
    description,
    start: { dateTime: new Date(start).toISOString(), timeZone },
    end: { dateTime: new Date(end).toISOString(), timeZone },
    reminders: { useDefault: true },
    extendedProperties: { private: { saiTaskId: String(saiTaskId || '') } },
  });
}

module.exports = { queryBusy, freeIntervals, freeHours, findSlot, createEvent, merge };
