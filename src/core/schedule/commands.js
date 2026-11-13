// AI 없이도 시간표를 말로 고칠 수 있게, 자주 쓰는 표현을 규칙으로 읽는다.
// 예) "2번 목요일 3시로 옮겨줘", "자료조사 금요일 2시부터 4시까지", "수요일은 안 돼", "발표 준비 3시간이면 돼",
//     "1번 빼줘", "하루에 4시간만", "밤 10시 이후는 안 돼", "캘린더에 넣어줘", "다시 짜줘"
const { parseWhen, findTimes, parseDuration } = require('./korean-time');

const MIN = 60 * 1000;

const has = (re, s) => re.test(s);
const norm = (s) => String(s || '').replace(/\s+/g, '').toLowerCase();

/** 문장에서 가장 잘 맞는 작업 이름 */
function matchTask(text, tasks) {
  const t = norm(text);
  let best = null;
  for (const task of tasks) {
    const name = norm(task.title);
    if (!name) continue;
    let score = 0;
    if (t.includes(name)) score = name.length * 2;
    else {
      // 이름의 앞부분(2글자 이상)이나 단어가 들어 있으면
      const words = String(task.title).split(/[\s·,/()]+/).filter((w) => w.length >= 2);
      for (const w of words) if (t.includes(norm(w))) score = Math.max(score, norm(w).length);
      if (!score && name.length >= 3 && t.includes(name.slice(0, 3))) score = 2;
    }
    if (score && (!best || score > best.score)) best = { task, score };
  }
  return best ? best.task : null;
}

function at(day, minutes) {
  const d = new Date(day);
  d.setHours(0, 0, 0, 0);
  return d.getTime() + minutes * MIN;
}

/**
 * ctx: { blocks: [{ no, id, taskId, title, start, end }], tasks: [{ taskId, title, need }], prefs, now }
 * 돌려주는 값: [{ type, ... }]
 */
function parseCommand(text, ctx) {
  const s = String(text || '').trim();
  const now = ctx.now || Date.now();
  if (!s) return [{ type: 'unknown' }];

  if (has(/(캘린더|구글|달력|할\s*일).*(넣|반영|올려|추가|적용|등록)|^(적용|반영|확정|등록)|그대로\s*(넣|진행|해)|좋아요?\s*(넣|반영)/, s)) return [{ type: 'apply' }];
  if (has(/처음부터|초기화|리셋/, s)) return [{ type: 'reset' }];
  if (has(/다시\s*(짜|계획|배치|잡)|재배치|새로\s*(짜|잡)/, s)) return [{ type: 'replan' }];
  if (has(/(보여|알려|뭐야|어때).*$/, s) && has(/일정|시간표|계획|스케줄/, s) && !findTimes(s).length) return [{ type: 'show' }];

  // 하루 최대 시간, 시작·끝 시각
  const prefs = {};
  let m = /하루(?:에|에는)?\s*(?:최대\s*)?(\d+(?:\.\d+)?)\s*시간/.exec(s);
  if (m && has(/만|까지|넘|최대|이하|이상은/, s)) prefs.dailyMaxHours = Number(m[1]);
  m = /한\s*번에\s*(?:최대\s*)?(\d+(?:\.\d+)?)\s*시간/.exec(s);
  if (m) prefs.maxBlockHours = Number(m[1]);
  const times = findTimes(s);
  if (times.length === 1 && has(/이후|넘어서|지나서|부터는\s*안|뒤로는/, s) && has(/안|싫|못|말고/, s)) prefs.dayEnd = Math.round(times[0].minutes / 60);
  if (times.length === 1 && has(/이전|전에는|전엔|보다\s*일찍|아침/, s) && has(/안|싫|못|말고/, s)) prefs.dayStart = Math.round(times[0].minutes / 60);
  if (times.length === 1 && has(/부터\s*(시작|하자|해)/, s) && !ctx.blocks.length) prefs.dayStart = Math.round(times[0].minutes / 60);
  if (Object.keys(prefs).length) return [{ type: 'prefs', patch: prefs }];

  const refM = /(\d{1,2})\s*번(?!\s*째\s*주)/.exec(s);
  const block = refM ? ctx.blocks.find((b) => b.no === Number(refM[1])) : null;
  if (refM && !block) return [{ type: 'unknown', hint: `${refM[1]}번 일정이 없어요. 목록의 번호를 확인해 주세요.` }];
  const task = block ? null : matchTask(s, ctx.tasks || []);
  const when = parseWhen(s.replace(/(\d{1,2})\s*번/, ' '), now);
  const timeGiven = when.start != null;
  const dayGiven = when.day != null;

  // 빼기
  if (has(/(빼|취소|지워|삭제|없애|안\s*할래)/, s) && (block || task)) {
    if (block) return [{ type: 'remove', ref: block.no }];
    const first = ctx.blocks.find((b) => b.taskId === task.taskId && (!dayGiven || sameDay(b.start, when.day)));
    if (first) return [{ type: 'remove', ref: first.no }];
    return [{ type: 'unknown', hint: `"${task.title}" 일정이 그날엔 없어요.` }];
  }

  // 작업 시간 바꾸기: "자료조사 3시간이면 돼", "발표 준비 2시간 더 필요해"
  const dur = parseDuration(s.replace(/(\d{1,2})\s*시\s*(반|\d{1,2}\s*분)?(?!간)/g, ' '));
  if (task && dur != null && !timeGiven && has(/(걸려|필요|더|추가|줄|덜|이면|면\s*돼|로\s*(잡|해|바꿔)|짜리|정도)/, s)) {
    const cur = Number(task.need) || 0;
    let hours = dur;
    if (has(/더|추가|늘려/, s)) hours = cur + dur;
    else if (has(/줄여|덜|빼/, s)) hours = Math.max(0, cur - dur);
    return [{ type: 'hours', taskId: task.taskId, hours }];
  }

  // 옮기기 / 추가
  if ((block || task) && (timeGiven || dayGiven)) {
    const base = block || ctx.blocks.find((b) => b.taskId === task.taskId);
    if (task && has(/추가|더\s*잡|하나\s*더|넣어/, s)) {
      if (!timeGiven) return [{ type: 'unknown', hint: '몇 시부터 할지 알려 주세요. 예: "금요일 2시부터 4시까지"' }];
      const day = dayGiven ? when.day : now;
      const start = at(day, when.start);
      const end = when.end != null ? at(day, when.end) : start + Math.round((dur || 1) * 60) * MIN;
      return [{ type: 'add', taskId: task.taskId, start, end }];
    }
    if (!base) {
      if (!timeGiven) return [{ type: 'unknown', hint: `"${task.title}"을 몇 시에 할지 알려 주세요.` }];
      const day = dayGiven ? when.day : now;
      const start = at(day, when.start);
      const end = when.end != null ? at(day, when.end) : start + Math.round((dur || Math.min(2, task.need || 1)) * 60) * MIN;
      return [{ type: 'add', taskId: task.taskId, start, end }];
    }
    const len = base.end - base.start;
    const day = dayGiven ? when.day : base.start;
    const startMin = timeGiven ? when.start : new Date(base.start).getHours() * 60 + new Date(base.start).getMinutes();
    const start = at(day, startMin);
    const end = when.end != null ? at(day, when.end) : dur ? start + Math.round(dur * 60) * MIN : start + len;
    return [{ type: 'move', ref: base.no, start, end }];
  }

  // 안 되는 시간: "수요일은 안 돼", "목요일 2시부터 5시까지 수업", "내일 오후는 약속"
  if ((dayGiven || timeGiven) && has(/안\s*돼|안돼|안\s*되|못\s*해|못해|바빠|약속|일정\s*(이\s*)?있|수업|알바|시험|비워|쉴|쉬|불가|어려워/, s)) {
    const day = dayGiven ? when.day : now;
    const p = ctx.prefs || { dayStart: 9, dayEnd: 22 };
    let startMin = p.dayStart * 60;
    let endMin = p.dayEnd * 60;
    if (timeGiven) {
      startMin = when.start;
      endMin = when.end != null ? when.end : has(/부터|이후|넘어/, s) ? p.dayEnd * 60 : when.start + 60 * (dur || 2);
    } else if (has(/오전/, s)) endMin = 12 * 60;
    else if (has(/오후|낮/, s)) {
      startMin = 12 * 60;
      endMin = 18 * 60;
    } else if (has(/저녁|밤/, s)) startMin = 18 * 60;
    const note = (/(약속|수업|알바|시험|일정)/.exec(s) || [])[1] || '안 되는 시간';
    return [{ type: 'unavailable', start: at(day, startMin), end: at(day, endMin), note }];
  }

  if (block || task) return [{ type: 'unknown', hint: '어떻게 바꿀지 알려 주세요. 예: "2번 목요일 3시로 옮겨줘", "1번 빼줘", "자료조사 3시간이면 돼"' }];
  return [{ type: 'unknown' }];
}

function sameDay(a, b) {
  const x = new Date(a);
  const y = new Date(b);
  return x.getFullYear() === y.getFullYear() && x.getMonth() === y.getMonth() && x.getDate() === y.getDate();
}

module.exports = { parseCommand, matchTask };
