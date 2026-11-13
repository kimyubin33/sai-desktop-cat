// 개인 상담 대화의 흐름 (Electron·네트워크 의존성 없음).
// 원칙 (개발 방향 문서 4·6장):
//  - 화난 말도 그대로 받아 주되, 원문은 어디에도 전달하지 않는다.
//  - 감정·사실·추측을 나누고, 상대의 잘못을 확정하지 않는다.
//  - 사람에 대한 평가를 관찰 가능한 행동과 필요한 변화로 바꾼다.
//  - 위험 신호가 보이면 흐름을 멈추고 안전 확인을 먼저 한다.
const { TOPICS, ACTION_REQUESTS, TO_CONFIRM, detectTopics, detectSensitive } = require('./topics');
const { analyzeLanguage, detectEmotions, reflectEmotion } = require('./language');
const { checkSafety, safetyMessage } = require('./safety');

const MODES = {
  complaint: { label: '팀원 일로 속상해요', steps: ['open', 'event', 'agreement', 'impact', 'need', 'done'] },
  explain: { label: '내 사정을 이야기하고 싶어요', steps: ['open', 'reason', 'next', 'done'] },
  free: { label: '그냥 이야기할래요', steps: ['open', 'free'] },
};

// 각 단계에서 고양이가 할 일 (AI 문장 다듬기에도 같은 의도를 넘긴다)
const INTENTS = {
  complaint: {
    open: '무슨 일이 있었는지 편하게 말해 달라고 하고, 여기서 한 말은 원문 그대로 다른 사람에게 가지 않는다고 안심시킨다.',
    event: '감정을 짧게 알아주고, 어떤 약속이나 기대가 있었고 실제로는 어떻게 달라졌는지 묻는다.',
    agreement: '그 약속을 서로 함께 확인한 상태였는지, 상대에게 다른 사정이 있었는지 들어 봤는지 묻는다. 상대를 단정하지 않는다.',
    impact: '그 일로 실제로 어떤 영향(일정 지연, 추가 부담 등)이 있었는지 묻는다.',
    need: '앞으로 상대가 어떻게 해 주면 괜찮을지, "이것만 해 줬어도" 싶은 것이 있는지 묻는다.',
    done: '이야기해 줘서 고맙다고 하고, 들은 내용을 정리했으며 확인되지 않은 부분은 추측으로 남겼다고 말한다.',
  },
  explain: {
    open: '어떤 상황이었는지 편하게 말해 달라고 한다. 변명이라고 생각하지 않는다고 안심시킨다.',
    reason: '마음을 알아주고, 그때 어떤 사정이 있었는지 묻는다.',
    next: '다음에 비슷한 일이 생기면 어떻게 하면 좋을지, 팀과 어떤 방식으로 맞추면 좋을지 묻는다.',
    done: '이야기해 줘서 고맙다고 하고, 내용을 정리했다고 말한다.',
  },
  free: {
    open: '오늘 어땠는지, 무슨 이야기든 편하게 해도 된다고 말한다.',
    free: '감정을 알아주고 조금 더 이야기해 달라고 한다. 조언을 서두르지 않는다.',
  },
};

const FALLBACK_TEXT = {
  complaint: {
    open: '어떤 일이 있었는지 편하게 말해 주세요. 거친 말이 나와도 괜찮아요. 여기서 한 말은 그대로 다른 사람에게 가지 않아요.',
    event: (ctx) =>
      `${ctx.reflect}. 어떤 약속이나 기대가 있었고, 실제로는 어떻게 달라졌는지 알려 줄 수 있을까요?`,
    agreement: () => '그 약속은 서로 함께 확인한 상태였나요? 상대에게 다른 사정이 있었는지는 들어 봤나요?',
    impact: () => '그 일 때문에 실제로 어떤 영향이 있었나요? 일정이 밀렸거나, 혼자 더 맡게 되었다거나요.',
    need: () => '앞으로 상대가 어떻게 해 주면 괜찮을 것 같아요? "이것만 해 줬어도" 싶은 게 있다면요.',
    done: () => '이야기해 줘서 고마워요. 들은 내용을 정리해 봤어요. 아직 확인되지 않은 부분은 추측으로 남겨 두었어요.',
  },
  explain: {
    open: '어떤 상황이었는지 편하게 이야기해 주세요. 변명이라고 생각하지 않아요. 사정을 아는 게 조율의 시작이니까요.',
    reason: (ctx) => `${ctx.reflect}. 그때 어떤 사정이 있었나요?`,
    next: () => '다음에 비슷한 일이 생기면 어떻게 하면 좋을 것 같아요? 팀과 맞추고 싶은 방식이 있다면요.',
    done: () => '이야기해 줘서 고마워요. 내용을 정리해 두었어요.',
  },
  free: {
    open: '오늘은 어땠어요? 무슨 이야기든 괜찮아요.',
    free: (ctx) => `${ctx.reflect}. 조금 더 이야기해 줄래요?`,
  },
};

const SUMMARIZE_WORDS = /(정리해\s*줘|정리해줘|정리\s*부탁|그만\s*정리|요약해)/;

let seq = 0;
function newId(now) {
  seq = (seq + 1) % 1000;
  return `s${now.toString(36)}${seq.toString(36)}`;
}

function createSession(mode = 'complaint', { now = Date.now(), topic = null } = {}) {
  if (!MODES[mode]) mode = 'complaint';
  const related = mode === 'explain' && topic && TOPICS[topic] ? topic : null;
  const session = {
    id: newId(now),
    mode,
    startedAt: now,
    step: 'open',
    turns: [],
    answers: {},
    emotions: [],
    topics: [],
    judgments: [],
    profanity: false,
    sensitive: [],
    safetyHold: null,
    summary: null,
    relatedTopic: related,
  };
  if (related) session.topics = [related];
  const openText = related
    ? `"${TOPICS[related].label}"에 관한 조언에 당신 사정도 있었군요. 어떤 상황이었는지 편하게 이야기해 주세요. 변명이라고 생각하지 않아요.`
    : FALLBACK_TEXT[mode].open;
  const reply = { role: 'cat', text: openText, intent: INTENTS[mode].open };
  session.turns.push({ role: 'cat', text: reply.text, at: now });
  return { session, replies: [reply] };
}

function nextStep(session) {
  const steps = MODES[session.mode].steps;
  const i = steps.indexOf(session.step);
  if (session.mode === 'free') return 'free';
  return steps[Math.min(i + 1, steps.length - 1)];
}

function mergeUnique(a, b) {
  return [...new Set([...a, ...b])];
}

/**
 * 사용자의 말 한 번을 처리한다.
 * opts.skip: 지금 질문을 건너뛴다
 */
function handleUser(session, text, { now = Date.now(), skip = false } = {}) {
  const t = (text || '').trim();
  if (!skip && !t) return { session, replies: [] };
  if (t) session.turns.push({ role: 'user', text: t, at: now });

  // 1) 안전 확인이 가장 먼저
  const risk = t ? checkSafety(t) : null;
  if (risk) {
    session.safetyHold = risk;
    const msg = safetyMessage(risk);
    session.turns.push({ role: 'cat', text: msg, at: now, safety: true });
    return { session, replies: [{ role: 'cat', text: msg, safety: risk, fixed: true }], safety: risk };
  }

  // 2) 분석: 감정, 주제, 평가 표현, 민감 정보 (원문은 세션 안에만 남는다)
  if (t) {
    const lang = analyzeLanguage(t);
    session.profanity = session.profanity || lang.profanity;
    session.judgments = mergeByLabel(session.judgments, lang.judgments);
    session.emotions = mergeUnique(session.emotions, detectEmotions(t));
    session.topics = mergeUnique(session.topics, [...detectTopics(t), ...lang.judgments.map((j) => j.topic)]);
    session.sensitive = mergeUnique(session.sensitive, detectSensitive(t));
    if (session.step !== 'open' || session.mode === 'free') session.answers[session.step] = t;
    else session.answers.first = t;
  }

  if (session.step === 'done') {
    const text2 = '정리 카드에서 내용을 고칠 수 있어요. 다른 이야기를 하고 싶으면 "새 대화"를 눌러 주세요.';
    session.turns.push({ role: 'cat', text: text2, at: now });
    return { session, replies: [{ role: 'cat', text: text2, fixed: true }] };
  }

  const wantsSummary = SUMMARIZE_WORDS.test(t) && session.mode !== 'free';
  let step = wantsSummary ? 'done' : nextStep(session);
  // 첫 말에 이미 사건이 자세히 담겨 있으면 같은 질문을 반복하지 않는다
  if (session.mode === 'complaint' && session.step === 'open' && step === 'event' && looksDetailed(t)) {
    session.answers.event = t;
    step = 'agreement';
  }
  session.step = step;

  const ctx = { reflect: reflectEmotion(session.emotions) };
  const tmpl = FALLBACK_TEXT[session.mode][step];
  let replyText = typeof tmpl === 'function' ? tmpl(ctx) : tmpl;
  if (step === 'agreement' && session.answers.first && !session.answers.event) {
    replyText = `${ctx.reflect}. ${replyText}`;
  }
  const reply = { role: 'cat', text: replyText, intent: INTENTS[session.mode][step] };
  session.turns.push({ role: 'cat', text: replyText, at: now });

  const out = { session, replies: [reply] };
  if (step === 'done') {
    session.summary = buildSummary(session);
    out.summary = session.summary;
  }
  return out;
}

function looksDetailed(t) {
  return t.length >= 40 && /(했는데|기로|하기로|약속|했었|안 했|안했|없었)/.test(t);
}

function mergeByLabel(a, b) {
  const seen = new Set(a.map((x) => x.label));
  return [...a, ...b.filter((x) => !seen.has(x.label))];
}

function classifyAgreement(ans) {
  if (!ans) return { confirmed: null, otherSideHeard: null };
  const no = /(몰라|모르|아니|글쎄|확인\s*안|확실하지|애매|안\s*물어|못\s*들)/.test(ans);
  const yes = /^\s*(네|넵|응|맞아|그럼|당연)|확인했|같이\s*정했|정했었|정했어|합의|단톡|공지|캘린더|일정에\s*넣/.test(ans);
  const otherHeard = /(사정.*(들었|말했|얘기했)|이유.*(들었|말했))/.test(ans);
  return {
    confirmed: no ? false : yes ? true : null,
    otherSideHeard: otherHeard ? true : no ? false : null,
  };
}

/** 정리 카드 (개인 기록 — 이 기기에만 저장) */
function buildSummary(session) {
  const a = session.answers;
  const topics = session.topics.filter((id) => TOPICS[id]);
  if (session.mode === 'explain') {
    return {
      mode: 'explain',
      relatedTopic: session.relatedTopic || null,
      situation: a.first || '',
      reason: a.reason || '',
      nextTime: a.next || '',
      emotions: session.emotions,
      topics,
      sensitive: session.sensitive.length > 0,
      note: '내 사정 기록은 이 기기에만 남아요. 아래 버튼으로 이 주제의 조언을 잠시 멈추거나, 팀 약속으로 함께 맞춰 보자고 제안할 수 있어요. 어느 쪽도 내 이야기 원문은 보내지 않아요.',
    };
  }
  const agree = classifyAgreement(a.agreement);
  const toConfirm = [];
  if (agree.confirmed !== true) toConfirm.push('그 약속을 서로 확정된 것으로 알고 있었는지');
  if (agree.otherSideHeard !== true) toConfirm.push('상대에게 다른 사정이 있었는지');
  // 약속·연락 관련 확인 사항은 위의 두 질문과 겹치므로 따로 넣지 않는다
  const covered = new Set(['time_promise', 'advance_notice']);
  for (const id of topics) if (TO_CONFIRM[id] && !covered.has(id)) toConfirm.push(TO_CONFIRM[id]);

  return {
    mode: 'complaint',
    reportedEvent: a.event || a.first || '',
    sharedRecord: '아직 연결된 공동 일정·업무 기록이 없어 확인하지 않았어요.',
    agreement: a.agreement || '',
    agreementNote:
      agree.confirmed === true
        ? '함께 확인했다고 말함 (기록으로는 확인 전)'
        : agree.confirmed === false
          ? '함께 확인했는지는 확실하지 않음'
          : '확인 여부를 아직 모름',
    emotions: session.emotions,
    impact: a.impact || '',
    needs: a.need || '',
    actionRequests: topics.map((id) => ACTION_REQUESTS[id]).filter(Boolean),
    reframes: session.judgments.map((j) => ({ from: j.label, to: j.reframe })),
    toConfirm: toConfirm.slice(0, 4),
    topics,
    sensitive: session.sensitive.length > 0,
    profanity: session.profanity,
  };
}

module.exports = { MODES, INTENTS, createSession, handleUser, buildSummary, classifyAgreement };
