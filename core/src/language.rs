//! 언어 정리: 사람에 대한 평가·욕설을 관찰 가능한 행동과 필요한 변화로 바꾼다.
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};

static PROFANITY: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(씨발|시발|ㅅㅂ|ㅆㅂ|병신|ㅂㅅ|개새|새끼|좆|존나|졸라|ㅈㄴ|미친놈|미친년|미친 놈|꺼져|닥쳐|지랄|ㅈㄹ|빡대가리)").unwrap());

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Judgment {
    pub label: String,
    pub topic: String,
    pub reframe: String,
}

struct JudgmentRule {
    re: Regex,
    label: &'static str,
    topic: &'static str,
    reframe: &'static str,
}

static JUDGMENTS: Lazy<Vec<JudgmentRule>> = Lazy::new(|| {
    let r = |re: &str, label, topic, reframe| JudgmentRule { re: Regex::new(re).unwrap(), label, topic, reframe };
    vec![
        r(r"(능력이?\s*없|무능|실력이?\s*없|일을?\s*못)", "능력에 대한 평가", "help_request", "처음 맡는 부분은 작은 단위로 진행 상황을 공유하고, 막히면 일찍 도움을 요청해 보자"),
        r(r"(게으르|게을러|성의\s*없|대충|열심히\s*안)", "태도에 대한 평가", "role_clarity", "맡은 일의 완료 기준을 먼저 맞추고, 중간에 진행 상황을 한 번 공유해 보자"),
        r(r"(무책임|책임감\s*없|책임감이\s*없)", "책임감에 대한 평가", "advance_notice", "맡은 일이 어려워졌을 때는 미리 알리고, 새 계획을 함께 정하자"),
        r(r"(싸가지|예의\s*없|예의가\s*없|재수\s*없|싹수)", "말투·태도에 대한 평가", "communication_tone", "의견은 사람보다 결과물에 대해, 부드럽게 전하자"),
        r(r"(이기적|자기만|지만\s*알|자기\s*것만)", "이기적이라는 평가", "workload", "각자 쓸 수 있는 시간과 맡은 일의 무게를 함께 맞춰 보자"),
        r(r"(믿을\s*수가?\s*없|못\s*믿)", "신뢰에 대한 평가", "time_promise", "작은 약속부터 확인하며 지키고, 어려우면 미리 알리자"),
    ]
});

pub struct LangAnalysis {
    pub profanity: bool,
    pub judgments: Vec<Judgment>,
}

pub fn analyze_language(text: &str) -> LangAnalysis {
    LangAnalysis {
        profanity: PROFANITY.is_match(text),
        judgments: JUDGMENTS
            .iter()
            .filter(|j| j.re.is_match(text))
            .map(|j| Judgment { label: j.label.into(), topic: j.topic.into(), reframe: j.reframe.into() })
            .collect(),
    }
}

static EMOTIONS: Lazy<Vec<(&'static str, Regex)>> = Lazy::new(|| {
    [
        ("화남", r"(화나|화가|열받|열 받|빡치|빡쳐|짜증|어이없|어이가 없)"),
        ("답답함", r"(답답|막막|미치겠)"),
        ("서운함", r"(서운|섭섭)"),
        ("무시당한 느낌", r"(무시|투명인간|없는 사람)"),
        ("지침", r"(지쳐|지친|힘들|피곤|버겁)"),
        ("불안", r"(불안|걱정|초조|조마조마)"),
        ("억울함", r"(억울|불공평|불공정)"),
        ("실망", r"(실망|기대했는데|기대 했는데)"),
    ]
    .iter()
    .map(|(l, r)| (*l, Regex::new(r).unwrap()))
    .collect()
});

pub fn detect_emotions(text: &str) -> Vec<String> {
    EMOTIONS.iter().filter(|(_, r)| r.is_match(text)).map(|(l, _)| l.to_string()).collect()
}

pub fn reflect_emotion(emotions: &[String]) -> String {
    let Some(first) = emotions.first() else { return "그런 일이 있었군요".into() };
    match first.as_str() {
        "화남" => "많이 화가 났겠어요",
        "답답함" => "답답했겠어요",
        "서운함" => "서운한 마음이 들었겠어요",
        "무시당한 느낌" => "무시당한 것 같은 느낌이 들었겠어요",
        "지침" => "많이 지쳤겠어요",
        "불안" => "마음이 불안했겠어요",
        "억울함" => "억울한 마음이 들었겠어요",
        "실망" => "기대했던 만큼 실망도 컸겠어요",
        _ => "그런 일이 있었군요",
    }
    .into()
}
