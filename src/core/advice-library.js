// 사이가 평소에 건네는 말 모음.
// source.type:
//   self    — 사이(이 앱)가 직접 쓴 조언. 유명인의 말처럼 꾸미지 않는다.
//   proverb — 한국 속담
//   classic — 출전이 분명한 고전 문장. 출전(책·편)을 함께 표시한다.
// 사람 이름을 붙인 "명언"은 출처를 확인한 것만 추가한다 (잘못 알려진 인용이 많기 때문).
const { TOPICS } = require('./topics');

const SELF = { type: 'self', label: '사이의 한마디' };

const DAILY = [
  ['start-small', '오늘은 가장 작은 한 가지부터 시작해 볼까요?'],
  ['five-minutes', '하기 싫은 일은 딱 5분만 해 보기로 해요. 5분 뒤에 계속할지 정해도 돼요.'],
  ['water', '물 한 잔 마시고 올래요? 저는 여기서 기다릴게요.'],
  ['stretch', '한 시간 넘게 같은 자세였다면, 어깨를 한 번 쭉 펴 봐요.'],
  ['one-tab', '지금 할 일과 관계없는 창을 하나만 닫아 볼까요?'],
  ['done-list', '오늘 한 일을 세 가지만 떠올려 봐요. 생각보다 많이 했을 거예요.'],
  ['next-step', '막막할 땐 "다음에 할 한 동작"만 정하면 돼요. 파일 열기 같은 것도 괜찮아요.'],
  ['break-ok', '쉬는 것도 일의 일부예요. 다시 집중할 힘을 모으는 시간이니까요.'],
  ['eyes', '화면에서 눈을 떼고 먼 곳을 20초쯤 바라봐 주세요.'],
  ['wrap-up', '하루를 마치기 전에, 내일 처음 할 일 하나만 적어 두면 아침이 가벼워요.'],
];

const REFLECTION = [
  ['trust-small', '작은 약속을 지키는 일부터 팀의 신뢰가 쌓여요.'],
  ['ask-early', '모르는 건 일찍 물어볼수록 작게 끝나요.'],
  ['assume-less', '상대가 왜 그랬는지 짐작이 들 때, 한 번은 직접 물어보는 쪽을 골라 봐요.'],
  ['thank', '오늘 도움 받은 일이 있다면, 짧게라도 고맙다고 전해 볼까요?'],
  ['say-limits', '할 수 없는 걸 할 수 없다고 미리 말하는 것도 팀을 돕는 일이에요.'],
  ['same-page', '각자 생각하는 완료 기준을 한 줄씩 맞춰 볼까요?'],
];

const CLASSIC = [
  {
    id: 'classic-thousand-li',
    text: '천 리 길도 발밑의 한 걸음에서 시작된다.',
    source: { type: 'classic', label: '『도덕경』 64장 (千里之行 始於足下)' },
  },
  {
    id: 'classic-three-people',
    text: '세 사람이 길을 가면 그중에 반드시 나의 스승이 있다.',
    source: { type: 'classic', label: '『논어』 술이편 (三人行 必有我師焉)' },
  },
  {
    id: 'classic-not-want',
    text: '내가 원하지 않는 일을 남에게 하지 말라.',
    source: { type: 'classic', label: '『논어』 안연편 (己所不欲 勿施於人)' },
  },
  {
    id: 'classic-fault',
    text: '잘못을 하고도 고치지 않는 것, 그것을 잘못이라 한다.',
    source: { type: 'classic', label: '『논어』 위령공편 (過而不改 是謂過矣)' },
  },
  {
    id: 'classic-know',
    text: '아는 것을 안다 하고, 모르는 것을 모른다 하는 것이 아는 것이다.',
    source: { type: 'classic', label: '『논어』 위정편 (知之爲知之 不知爲不知 是知也)' },
  },
  {
    id: 'classic-know-others',
    text: '남이 나를 알아주지 않음을 걱정하지 말고, 내가 남을 알지 못함을 걱정하라.',
    source: { type: 'classic', label: '『논어』 학이편 (不患人之不己知 患不知人也)' },
  },
  { id: 'proverb-paper', text: '백지장도 맞들면 낫다.', source: { type: 'proverb', label: '한국 속담' } },
  { id: 'proverb-words', text: '가는 말이 고와야 오는 말이 곱다.', source: { type: 'proverb', label: '한국 속담' } },
  { id: 'proverb-dust', text: '티끌 모아 태산.', source: { type: 'proverb', label: '한국 속담' } },
];

function buildLibrary() {
  const items = [];
  for (const [id, text] of DAILY) items.push({ id: `daily-${id}`, kind: 'daily', topic: null, text, source: SELF });
  for (const [id, text] of REFLECTION)
    items.push({ id: `reflect-${id}`, kind: 'reflection', topic: null, text, source: SELF });
  for (const c of CLASSIC) items.push({ ...c, kind: 'quote', topic: null });
  for (const [topicId, topic] of Object.entries(TOPICS)) {
    topic.advice.forEach((text, i) =>
      items.push({ id: `team-${topicId}-${i}`, kind: 'teamwork', topic: topicId, text, source: SELF }),
    );
  }
  return items;
}

const LIBRARY = buildLibrary();

function sourceLabel(entry) {
  return entry.source ? `— ${entry.source.label}` : '';
}

module.exports = { LIBRARY, buildLibrary, sourceLabel };
