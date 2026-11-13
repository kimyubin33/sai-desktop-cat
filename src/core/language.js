// 언어 정리: 사람에 대한 평가·욕설을 관찰 가능한 행동과 필요한 변화로 바꾼다.

const PROFANITY = /(씨발|시발|ㅅㅂ|ㅆㅂ|병신|ㅂㅅ|개새|새끼|좆|존나|졸라|ㅈㄴ|미친놈|미친년|미친 놈|꺼져|닥쳐|지랄|ㅈㄹ|빡대가리)/;

// 사람에 대한 판단 → (관련 주제, 행동으로 바꾼 표현)
const JUDGMENTS = [
  {
    re: /(능력이?\s*없|무능|실력이?\s*없|일을?\s*못)/,
    label: '능력에 대한 평가',
    topic: 'help_request',
    reframe: '처음 맡는 부분은 작은 단위로 진행 상황을 공유하고, 막히면 일찍 도움을 요청해 보자',
  },
  {
    re: /(게으르|게을러|성의\s*없|대충|열심히\s*안)/,
    label: '태도에 대한 평가',
    topic: 'role_clarity',
    reframe: '맡은 일의 완료 기준을 먼저 맞추고, 중간에 진행 상황을 한 번 공유해 보자',
  },
  {
    re: /(무책임|책임감\s*없|책임감이\s*없)/,
    label: '책임감에 대한 평가',
    topic: 'advance_notice',
    reframe: '맡은 일이 어려워졌을 때는 미리 알리고, 새 계획을 함께 정하자',
  },
  {
    re: /(싸가지|예의\s*없|예의가\s*없|재수\s*없|싹수)/,
    label: '말투·태도에 대한 평가',
    topic: 'communication_tone',
    reframe: '의견은 사람보다 결과물에 대해, 부드럽게 전하자',
  },
  {
    re: /(이기적|자기만|지만\s*알|자기\s*것만)/,
    label: '이기적이라는 평가',
    topic: 'workload',
    reframe: '각자 쓸 수 있는 시간과 맡은 일의 무게를 함께 맞춰 보자',
  },
  {
    re: /(믿을\s*수가?\s*없|못\s*믿)/,
    label: '신뢰에 대한 평가',
    topic: 'time_promise',
    reframe: '작은 약속부터 확인하며 지키고, 어려우면 미리 알리자',
  },
];

function analyzeLanguage(text) {
  const t = text || '';
  return {
    profanity: PROFANITY.test(t),
    judgments: JUDGMENTS.filter((j) => j.re.test(t)).map(({ label, topic, reframe }) => ({ label, topic, reframe })),
  };
}

const EMOTIONS = [
  { label: '화남', re: /(화나|화가|열받|열 받|빡치|빡쳐|짜증|어이없|어이가 없)/ },
  { label: '답답함', re: /(답답|막막|미치겠)/ },
  { label: '서운함', re: /(서운|섭섭)/ },
  { label: '무시당한 느낌', re: /(무시|투명인간|없는 사람)/ },
  { label: '지침', re: /(지쳐|지친|힘들|피곤|버겁)/ },
  { label: '불안', re: /(불안|걱정|초조|조마조마)/ },
  { label: '억울함', re: /(억울|불공평|불공정)/ },
  { label: '실망', re: /(실망|기대했는데|기대 했는데)/ },
];

function detectEmotions(text) {
  const t = text || '';
  return EMOTIONS.filter((e) => e.re.test(t)).map((e) => e.label);
}

const REFLECT = {
  화남: '많이 화가 났겠어요',
  답답함: '답답했겠어요',
  서운함: '서운한 마음이 들었겠어요',
  '무시당한 느낌': '무시당한 것 같은 느낌이 들었겠어요',
  지침: '많이 지쳤겠어요',
  불안: '마음이 불안했겠어요',
  억울함: '억울한 마음이 들었겠어요',
  실망: '기대했던 만큼 실망도 컸겠어요',
};

function reflectEmotion(emotions) {
  if (!emotions.length) return '그런 일이 있었군요';
  return REFLECT[emotions[0]];
}

module.exports = { analyzeLanguage, detectEmotions, reflectEmotion, JUDGMENTS, EMOTIONS };
