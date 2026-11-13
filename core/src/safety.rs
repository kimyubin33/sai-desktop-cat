//! 긴급한 위험 신호를 감지하면 평소 흐름(정리·공유)을 멈추고 안전 확인을 먼저 한다.
//! 일상적인 과장 표현("힘들어 죽겠다")은 위험 신호로 보지 않는다.
use once_cell::sync::Lazy;
use regex::Regex;

static SELF_RISK: Lazy<Vec<Regex>> = Lazy::new(|| {
    [
        r"죽고\s*싶",
        r"자살",
        r"자해",
        r"살기\s*싫",
        r"사라지고\s*싶",
        r"없어지고\s*싶",
        r"목숨을?\s*끊",
        r"끝내고\s*싶[다어].*(삶|인생|모든\s*걸|다)",
    ]
    .iter()
    .map(|r| Regex::new(r).unwrap())
    .collect()
});

static OTHER_RISK: Lazy<Vec<Regex>> = Lazy::new(|| {
    [r"죽여\s*버리", r"죽이고\s*싶", r"때리고\s*싶", r"해치고\s*싶", r"칼로", r"패\s*버리고\s*싶"].iter().map(|r| Regex::new(r).unwrap()).collect()
});

/// "self" | "others"
pub fn check_safety(text: &str) -> Option<&'static str> {
    if SELF_RISK.iter().any(|r| r.is_match(text)) {
        return Some("self");
    }
    if OTHER_RISK.iter().any(|r| r.is_match(text)) {
        return Some("others");
    }
    None
}

pub fn safety_message(kind: &str) -> String {
    if kind == "self" {
        return String::from(
            "지금 많이 힘든 마음이 느껴져요. 팀 이야기는 잠시 내려놓고, 당신이 먼저예요.\n\
혹시 지금 스스로를 해칠 생각이 들거나 위험한 상황이라면, 혼자 견디지 말고 바로 사람의 도움을 받아 주세요. \
109(자살예방상담전화, 24시간)나 1577-0199(정신건강위기상담전화)에서 이야기를 들어 줄 거예요. 급하면 119나 112에 연락해 주세요.\n\
가까운 사람 한 명에게 지금 연락해 보는 것도 좋아요. 저도 여기 있을게요. 지금 안전한 곳에 있나요?",
        );
    }
    String::from(
        "정말 많이 화가 났다는 게 느껴져요. 그 마음 자체는 이상한 게 아니에요.\n\
다만 혹시 실제로 누군가를 다치게 할 것 같은 상황이라면, 지금은 그 자리에서 잠시 떨어져 숨을 고르고 믿을 수 있는 사람과 먼저 이야기해 주세요. 위급하면 112에 연락해 주세요.\n\
이 대화 내용은 정리나 공유에 쓰지 않을게요. 지금은 좀 괜찮아요?",
    )
}
