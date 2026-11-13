const test = require('node:test');
const assert = require('node:assert/strict');

const { CatBehavior } = require('../src/core/behavior');
const { LIBRARY } = require('../src/core/advice-library');
const { silenceReason, pickAdvice, inDnd, rememberShown } = require('../src/core/advice-scheduler');
const { createSession, handleUser } = require('../src/core/counseling');
const { checkSafety } = require('../src/core/safety');
const { analyzeLanguage } = require('../src/core/language');
const { detectTopics, TOPICS } = require('../src/core/topics');
const { buildPreview, createOutboxItems, mergeIntoOutbox, leaksRawText, OUTBOX_KEYS } = require('../src/core/privacy');

function seeded(seed = 1) {
  return () => {
    seed = (seed * 16807) % 2147483647;
    return (seed - 1) / 2147483646;
  };
}

// ---------- 고양이 행동 ----------
test('고양이는 화면 범위를 벗어나지 않고 여러 상태를 오간다', () => {
  const b = new CatBehavior({ rand: seeded(7), minX: 0, maxX: 600, x: 300 });
  const seen = new Set();
  for (let i = 0; i < 20000; i++) {
    b.tick(33);
    seen.add(b.state);
    assert.ok(b.x >= 0 && b.x <= 600, `x=${b.x}`);
  }
  for (const s of ['sit', 'walk', 'loaf', 'sleep']) assert.ok(seen.has(s), `${s} 상태가 나와야 함`);
});

test('말하는 중과 들려 있는 중에는 움직이지 않는다', () => {
  const b = new CatBehavior({ rand: seeded(3), minX: 0, maxX: 600, x: 300 });
  b.enter('walk');
  b.startTalking();
  const x = b.x;
  for (let i = 0; i < 100; i++) b.tick(33);
  assert.equal(b.x, x);
  b.stopTalking();
  assert.equal(b.state, 'sit');
  b.startDrag();
  b.endDrag(9999);
  assert.equal(b.x, 600);
});

// ---------- 말풍선 조언 ----------
test('조용한 모드·집중 모드·방해 금지·끄기에서는 먼저 말 걸지 않는다', () => {
  const now = new Date(2026, 10, 13, 14, 0);
  const base = { frequency: 'normal', quiet: false, focusUntil: 0, dnd: null };
  assert.equal(silenceReason(base, now), null);
  assert.equal(silenceReason({ ...base, quiet: true }, now), 'quiet');
  assert.equal(silenceReason({ ...base, frequency: 'off' }, now), 'off');
  assert.equal(silenceReason({ ...base, focusUntil: now.getTime() + 1000 }, now), 'focus');
  assert.equal(silenceReason({ ...base, dnd: { start: '13:00', end: '15:00' } }, now), 'dnd');
});

test('방해 금지 시간은 자정을 넘겨도 동작한다', () => {
  const dnd = { start: '23:00', end: '08:00' };
  assert.ok(inDnd(dnd, new Date(2026, 0, 1, 23, 30)));
  assert.ok(inDnd(dnd, new Date(2026, 0, 1, 7, 59)));
  assert.ok(!inDnd(dnd, new Date(2026, 0, 1, 8, 0)));
  assert.ok(!inDnd(dnd, new Date(2026, 0, 1, 15, 0)));
});

test('최근에 한 말은 반복하지 않는다', () => {
  const rand = seeded(11);
  let recent = [];
  for (let i = 0; i < 25; i++) {
    const e = pickAdvice(LIBRARY, { recent, rand });
    assert.ok(!recent.includes(e.id), `반복됨: ${e.id}`);
    recent = rememberShown(recent, e.id);
  }
});

test('"이런 말 줄이기"를 누른 주제는 덜 나온다', () => {
  const count = (dampened) => {
    const rand = seeded(5);
    let n = 0;
    for (let i = 0; i < 3000; i++) if (pickAdvice(LIBRARY, { rand, dampened }).topic === 'time_promise') n++;
    return n;
  };
  assert.ok(count({ time_promise: 2 }) < count({}) / 3);
});

test('사람 이름을 단 명언은 없고, 고전·속담에는 출처가 붙어 있다', () => {
  for (const e of LIBRARY) {
    assert.ok(e.source && e.source.label, `${e.id} 출처 표시 없음`);
    if (e.kind === 'quote') assert.ok(['classic', 'proverb'].includes(e.source.type));
  }
});

// ---------- 안전·언어 ----------
test('위험 신호는 감지하고, 일상 과장 표현은 넘긴다', () => {
  assert.equal(checkSafety('요즘 그냥 죽고 싶어요'), 'self');
  assert.equal(checkSafety('진짜 죽여버리고 싶다'), 'others');
  assert.equal(checkSafety('과제 때문에 힘들어 죽겠어요'), null);
  assert.equal(checkSafety('배고파 죽겠다'), null);
});

test('사람에 대한 평가는 행동 표현으로 바뀐다', () => {
  const r = analyzeLanguage('그 사람 진짜 능력이 없어요. 씨발');
  assert.ok(r.profanity);
  assert.equal(r.judgments[0].topic, 'help_request');
  assert.match(r.judgments[0].reframe, /작은 단위로 진행 상황을 공유/);
});

test('주제 감지', () => {
  assert.deepEqual(detectTopics('약속 시간에 늦고 연락도 없었어요').slice(0, 2).sort(), ['advance_notice', 'time_promise']);
  assert.ok(!detectTopics('사정은 모르겠어요').includes('help_request'));
});

// ---------- 상담 흐름 ----------
function runComplaint(lines) {
  let { session } = createSession('complaint', { now: 1 });
  let last;
  for (const l of lines) {
    last = handleUser(session, l, { now: 2 });
    session = last.session;
  }
  return last;
}

test('불만 대화는 질문을 거쳐 정리 카드로 끝난다', () => {
  const r = runComplaint([
    '시간도 안 지키고 맡은 일도 제대로 안 해서 너무 화나요',
    '어제 같이 검토하기로 했는데 연락이 없었어요',
    '사정은 모르겠어요',
    '다음 작업이 하루 밀렸어요',
    '미리 알려주기만 했어도 괜찮았을 것 같아요',
  ]);
  assert.ok(r.summary);
  const s = r.summary;
  assert.equal(s.mode, 'complaint');
  assert.ok(s.emotions.includes('화남'));
  assert.ok(s.topics.includes('time_promise'));
  assert.ok(s.topics.includes('advance_notice'));
  assert.ok(s.toConfirm.some((x) => x.includes('사정')), '상대 사정 확인이 남아야 함');
  assert.match(s.agreementNote, /확실하지 않음|모름/);
  assert.match(s.sharedRecord, /확인하지 않았/);
});

test('"정리해 줘"라고 하면 바로 정리한다', () => {
  const r = runComplaint(['팀원이 회의 때 대답이 없어요', '정리해 줘']);
  assert.ok(r.summary);
  assert.ok(r.summary.topics.includes('meeting_participation'));
});

test('위험 신호가 나오면 흐름을 멈추고 공유를 막는다', () => {
  const r = runComplaint(['팀플 때문에 너무 힘들어서 죽고 싶어요']);
  assert.equal(r.safety, 'self');
  assert.equal(r.session.safetyHold, 'self');
  assert.match(r.replies[0].text, /109/);
  assert.ok(!r.summary);
});

test('내 사정 대화도 정리된다', () => {
  let { session } = createSession('explain', { now: 1 });
  for (const l of ['검토 약속에 못 갔어요', '일정이 확정된 줄 몰랐어요', '다음엔 참석 확인을 남길게요'])
    session = handleUser(session, l).session;
  assert.equal(session.summary.mode, 'explain');
  assert.match(session.summary.reason, /확정된 줄/);
});

// ---------- 공유 범위와 개인정보 ----------
test('공유 대기함에는 원문이 들어가지 않고 정해진 항목만 저장된다', () => {
  const raw = ['김철수가 11월 12일에 검토 약속을 펑크냈어요 진짜 무책임해', '연락도 없었어요'];
  const summary = { topics: ['time_promise', 'advance_notice', 'role_clarity'], sensitive: false };
  const items = createOutboxItems(
    summary,
    [
      { topic: 'time_promise', scope: 'auto', target: 'person' },
      { topic: 'advance_notice', scope: 'review', target: 'team' },
      { topic: 'role_clarity', scope: 'private' },
      { topic: 'not_a_topic', scope: 'auto' },
      { topic: 'workload', scope: 'auto' }, // 정리에 없던 주제
    ],
    { now: Date.UTC(2026, 10, 13), key: 's1' },
  );
  assert.equal(items.length, 2);
  for (const it of items) {
    assert.deepEqual(Object.keys(it).sort(), [...OUTBOX_KEYS].sort());
    assert.ok(!leaksRawText(it, raw, 6));
    const all = JSON.stringify(it);
    assert.ok(!all.includes('김철수') && !all.includes('11월'));
  }
  assert.equal(items[0].week, '2026-W46');
});

test('민감한 내용이 있으면 자동 반영은 검토 후 공유로 낮아진다', () => {
  const items = createOutboxItems({ topics: ['workload'], sensitive: true }, [{ topic: 'workload', scope: 'auto' }]);
  assert.equal(items[0].scope, 'review');
  assert.equal(buildPreview({ topics: ['workload'], sensitive: true }).items[0].defaultScope, 'review');
});

test('미리보기 문안과 저장 문안이 같다', () => {
  const summary = { topics: ['time_promise'], sensitive: false };
  const p = buildPreview(summary, { key: 'abc' }).items[0];
  const [it] = createOutboxItems(summary, [{ topic: 'time_promise', scope: 'auto' }], { key: 'abc' });
  assert.equal(p.advice, it.advice);
  assert.ok(TOPICS.time_promise.advice.includes(it.advice));
});

test('같은 주제를 여러 번 공유해도 한 건으로 유지된다', () => {
  const a = createOutboxItems({ topics: ['time_promise'] }, [{ topic: 'time_promise', scope: 'review' }], { now: 1 });
  const b = createOutboxItems({ topics: ['time_promise'] }, [{ topic: 'time_promise', scope: 'auto' }], { now: 2 });
  const merged = mergeIntoOutbox(mergeIntoOutbox([], a), b);
  assert.equal(merged.length, 1);
  assert.equal(merged[0].scope, 'auto');
});
