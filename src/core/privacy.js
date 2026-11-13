// 개인 대화 → 공유 가능한 "일반 주제"로 바꾸는 단계.
// 공유 대기함에 들어가는 것은 주제 id, 미리 써 둔 일반 문안, 공유 범위, 대략적인 주(週)뿐이다.
// 원문·이름·날짜·사건 세부는 구조적으로 들어갈 자리가 없다.
const { TOPICS } = require('./topics');

const SCOPES = ['private', 'review', 'auto'];
const TARGETS = ['person', 'team'];

const SMALL_TEAM_WARNING =
  '팀이 두세 명뿐이면 일반화된 조언이라도 누가 말했는지 짐작될 수 있어요. 출처 노출을 줄이는 것이지 완전한 익명은 아니에요.';

// 같은 대화·같은 주제에는 항상 같은 예시 문안을 고른다 (미리보기와 저장 내용이 일치하도록)
function pickStable(arr, key) {
  let h = 0;
  for (const ch of key) h = (h * 31 + ch.charCodeAt(0)) >>> 0;
  return arr[h % arr.length];
}

function buildPreview(summary, { key = '' } = {}) {
  const items = (summary.topics || [])
    .filter((id) => TOPICS[id])
    .map((id) => ({
      topic: id,
      label: TOPICS[id].label,
      advice: pickStable(TOPICS[id].advice, `${key}:${id}`),
      teamProposal: TOPICS[id].teamProposal,
      sensitive: !!summary.sensitive,
      defaultScope: summary.sensitive ? 'review' : 'auto',
    }));
  return { items, smallTeamWarning: SMALL_TEAM_WARNING };
}

/** ISO 주차 (예: 2026-W46). 정확한 날짜 대신 대략적인 시기만 남긴다 */
function isoWeek(ts) {
  const d = new Date(ts);
  const day = (d.getUTCDay() + 6) % 7;
  d.setUTCDate(d.getUTCDate() - day + 3);
  const firstThursday = new Date(Date.UTC(d.getUTCFullYear(), 0, 4));
  const week = 1 + Math.round(((d - firstThursday) / 86400000 - 3 + ((firstThursday.getUTCDay() + 6) % 7)) / 7);
  return `${d.getUTCFullYear()}-W${String(week).padStart(2, '0')}`;
}

/**
 * 선택을 공유 대기함 항목으로 바꾼다.
 * - 'private' 은 저장하지 않는다
 * - 민감한 내용이 있으면 'auto' 를 'review' 로 낮춘다
 */
function createOutboxItems(summary, choices, { now = Date.now(), rand = Math.random, key = '' } = {}) {
  const allowed = new Set(summary.topics || []);
  const out = [];
  for (const c of choices || []) {
    if (!TOPICS[c.topic] || !allowed.has(c.topic)) continue;
    let scope = SCOPES.includes(c.scope) ? c.scope : 'private';
    if (scope === 'private') continue;
    if (scope === 'auto' && summary.sensitive) scope = 'review';
    const target = TARGETS.includes(c.target) ? c.target : 'person';
    out.push({
      id: `o${now.toString(36)}${Math.floor(rand() * 1e6).toString(36)}`,
      topic: c.topic,
      label: TOPICS[c.topic].label,
      scope,
      target,
      advice: pickStable(TOPICS[c.topic].advice, `${key}:${c.topic}`),
      teamProposal: TOPICS[c.topic].teamProposal,
      week: isoWeek(now),
    });
  }
  return out;
}

/**
 * 같은 사람이 같은 주제를 여러 번 말해도 한 건으로 유지한다
 * (여러 팀원이 같은 의견을 낸 것처럼 부풀리지 않기 위해).
 */
function mergeIntoOutbox(existing, items) {
  const next = [...existing];
  for (const it of items) {
    const i = next.findIndex((x) => x.topic === it.topic && x.target === it.target);
    if (i >= 0) next[i] = { ...next[i], scope: it.scope, week: it.week };
    else next.push(it);
  }
  return next;
}

/** 원문의 일부(8자 이상 연속)가 공유 항목에 섞여 들어갔는지 검사한다 */
function leaksRawText(item, rawTexts, minLen = 8) {
  const hay = [item.advice, item.teamProposal, item.label].join('\n').replace(/\s+/g, ' ');
  for (const raw of rawTexts) {
    const t = (raw || '').replace(/\s+/g, ' ');
    for (let i = 0; i + minLen <= t.length; i++) {
      if (hay.includes(t.slice(i, i + minLen))) return true;
    }
  }
  return false;
}

const OUTBOX_KEYS = ['id', 'topic', 'label', 'scope', 'target', 'advice', 'teamProposal', 'week'];

module.exports = {
  SCOPES,
  TARGETS,
  SMALL_TEAM_WARNING,
  buildPreview,
  pickStable,
  createOutboxItems,
  mergeIntoOutbox,
  leaksRawText,
  isoWeek,
  OUTBOX_KEYS,
};
