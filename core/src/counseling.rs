//! 개인 상담 대화의 흐름.
//! 원칙:
//!  - 화난 말도 그대로 받아 주되, 원문은 어디에도 전달하지 않는다.
//!  - 감정·사실·추측을 나누고, 상대의 잘못을 확정하지 않는다.
//!  - 사람에 대한 평가를 관찰 가능한 행동과 필요한 변화로 바꾼다.
//!  - 위험 신호가 보이면 흐름을 멈추고 안전 확인을 먼저 한다.
use crate::language::{analyze_language, detect_emotions, reflect_emotion, Judgment};
use crate::safety::{check_safety, safety_message};
use crate::topics::{detect_sensitive, detect_topics, topic};
use crate::util::to_base36;
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

pub fn mode_label(mode: &str) -> &'static str {
    match mode {
        "complaint" => "팀원 일로 속상해요",
        "explain" => "내 사정을 이야기하고 싶어요",
        "free" => "그냥 이야기할래요",
        _ => "",
    }
}

fn steps(mode: &str) -> &'static [&'static str] {
    match mode {
        "complaint" => &["open", "event", "agreement", "impact", "need", "done"],
        "explain" => &["open", "reason", "next", "done"],
        _ => &["open", "free"],
    }
}

/// 각 단계에서 고양이가 할 일 (AI 문장 다듬기에도 같은 의도를 넘긴다)
pub fn intent(mode: &str, step: &str) -> Option<&'static str> {
    Some(match (mode, step) {
        ("complaint", "open") => "무슨 일이 있었는지 편하게 말해 달라고 하고, 여기서 한 말은 원문 그대로 다른 사람에게 가지 않는다고 안심시킨다.",
        ("complaint", "event") => "감정을 짧게 알아주고, 어떤 약속이나 기대가 있었고 실제로는 어떻게 달라졌는지 묻는다.",
        ("complaint", "agreement") => "그 약속을 서로 함께 확인한 상태였는지, 상대에게 다른 사정이 있었는지 들어 봤는지 묻는다. 상대를 단정하지 않는다.",
        ("complaint", "impact") => "그 일로 실제로 어떤 영향(일정 지연, 추가 부담 등)이 있었는지 묻는다.",
        ("complaint", "need") => "앞으로 상대가 어떻게 해 주면 괜찮을지, \"이것만 해 줬어도\" 싶은 것이 있는지 묻는다.",
        ("complaint", "done") => "이야기해 줘서 고맙다고 하고, 들은 내용을 정리했으며 확인되지 않은 부분은 추측으로 남겼다고 말한다.",
        ("explain", "open") => "어떤 상황이었는지 편하게 말해 달라고 한다. 변명이라고 생각하지 않는다고 안심시킨다.",
        ("explain", "reason") => "마음을 알아주고, 그때 어떤 사정이 있었는지 묻는다.",
        ("explain", "next") => "다음에 비슷한 일이 생기면 어떻게 하면 좋을지, 팀과 어떤 방식으로 맞추면 좋을지 묻는다.",
        ("explain", "done") => "이야기해 줘서 고맙다고 하고, 내용을 정리했다고 말한다.",
        ("free", "open") => "오늘 어땠는지, 무슨 이야기든 편하게 해도 된다고 말한다.",
        ("free", "free") => "감정을 알아주고 조금 더 이야기해 달라고 한다. 조언을 서두르지 않는다.",
        _ => return None,
    })
}

fn fallback_text(mode: &str, step: &str, reflect: &str) -> String {
    match (mode, step) {
        ("complaint", "open") => "어떤 일이 있었는지 편하게 말해 주세요. 거친 말이 나와도 괜찮아요. 여기서 한 말은 그대로 다른 사람에게 가지 않아요.".into(),
        ("complaint", "event") => format!("{reflect}. 어떤 약속이나 기대가 있었고, 실제로는 어떻게 달라졌는지 알려 줄 수 있을까요?"),
        ("complaint", "agreement") => "그 약속은 서로 함께 확인한 상태였나요? 상대에게 다른 사정이 있었는지는 들어 봤나요?".into(),
        ("complaint", "impact") => "그 일 때문에 실제로 어떤 영향이 있었나요? 일정이 밀렸거나, 혼자 더 맡게 되었다거나요.".into(),
        ("complaint", "need") => "앞으로 상대가 어떻게 해 주면 괜찮을 것 같아요? \"이것만 해 줬어도\" 싶은 게 있다면요.".into(),
        ("complaint", "done") => "이야기해 줘서 고마워요. 들은 내용을 정리해 봤어요. 아직 확인되지 않은 부분은 추측으로 남겨 두었어요.".into(),
        ("explain", "open") => "어떤 상황이었는지 편하게 이야기해 주세요. 변명이라고 생각하지 않아요. 사정을 아는 게 조율의 시작이니까요.".into(),
        ("explain", "reason") => format!("{reflect}. 그때 어떤 사정이 있었나요?"),
        ("explain", "next") => "다음에 비슷한 일이 생기면 어떻게 하면 좋을 것 같아요? 팀과 맞추고 싶은 방식이 있다면요.".into(),
        ("explain", "done") => "이야기해 줘서 고마워요. 내용을 정리해 두었어요.".into(),
        ("free", "open") => "오늘은 어땠어요? 무슨 이야기든 괜찮아요.".into(),
        ("free", _) => format!("{reflect}. 조금 더 이야기해 줄래요?"),
        _ => String::new(),
    }
}

static SUMMARIZE_WORDS: Lazy<Regex> = Lazy::new(|| Regex::new(r"(정리해\s*줘|정리해줘|정리\s*부탁|그만\s*정리|요약해)").unwrap());

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Turn {
    pub role: String,
    pub text: String,
    pub at: i64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub safety: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ai: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub id: String,
    pub mode: String,
    pub started_at: i64,
    pub step: String,
    #[serde(default)]
    pub turns: Vec<Turn>,
    #[serde(default)]
    pub answers: Map<String, Value>,
    #[serde(default)]
    pub emotions: Vec<String>,
    #[serde(default)]
    pub topics: Vec<String>,
    #[serde(default)]
    pub judgments: Vec<Judgment>,
    #[serde(default)]
    pub profanity: bool,
    #[serde(default)]
    pub sensitive: Vec<String>,
    #[serde(default)]
    pub safety_hold: Option<String>,
    #[serde(default)]
    pub summary: Option<Value>,
    #[serde(default)]
    pub related_topic: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Reply {
    pub role: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub safety: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub fixed: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub ai: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub streamed: bool,
}

impl Reply {
    pub fn cat(text: impl Into<String>) -> Self {
        Reply { role: "cat".into(), text: text.into(), intent: None, safety: None, fixed: false, ai: false, streamed: false }
    }
}

pub struct Handled {
    pub replies: Vec<Reply>,
    pub safety: Option<String>,
    pub summary: Option<Value>,
}

static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn new_id(now: i64) -> String {
    let s = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % 1000;
    format!("s{}{}", to_base36(now.max(0) as u64), to_base36(s))
}

pub fn create_session(mode: &str, topic_id: Option<&str>, now: i64) -> (Session, Vec<Reply>) {
    let mode = if ["complaint", "explain", "free"].contains(&mode) { mode } else { "complaint" };
    let related = if mode == "explain" { topic_id.filter(|t| topic(t).is_some()).map(String::from) } else { None };
    let mut s = Session { id: new_id(now), mode: mode.into(), started_at: now, step: "open".into(), related_topic: related.clone(), ..Default::default() };
    if let Some(r) = &related {
        s.topics = vec![r.clone()];
    }
    let text = match &related {
        Some(r) => format!("\"{}\"에 관한 조언에 당신 사정도 있었군요. 어떤 상황이었는지 편하게 이야기해 주세요. 변명이라고 생각하지 않아요.", topic(r).unwrap().label),
        None => fallback_text(mode, "open", ""),
    };
    s.turns.push(Turn { role: "cat".into(), text: text.clone(), at: now, ..Default::default() });
    let mut r = Reply::cat(text);
    r.intent = intent(mode, "open").map(String::from);
    (s, vec![r])
}

fn next_step(s: &Session) -> String {
    if s.mode == "free" {
        return "free".into();
    }
    let st = steps(&s.mode);
    let i = st.iter().position(|x| *x == s.step).unwrap_or(0);
    st[(i + 1).min(st.len() - 1)].into()
}

fn merge_unique(a: &mut Vec<String>, b: Vec<String>) {
    for x in b {
        if !a.contains(&x) {
            a.push(x);
        }
    }
}

static DETAILED: Lazy<Regex> = Lazy::new(|| Regex::new(r"(했는데|기로|하기로|약속|했었|안 했|안했|없었)").unwrap());

fn looks_detailed(t: &str) -> bool {
    t.chars().count() >= 40 && DETAILED.is_match(t)
}

/// 사용자의 말 한 번을 처리한다. skip: 지금 질문을 건너뛴다
pub fn handle_user(s: &mut Session, text: &str, now: i64, skip: bool) -> Handled {
    let t = text.trim().to_string();
    if !skip && t.is_empty() {
        return Handled { replies: vec![], safety: None, summary: None };
    }
    if !t.is_empty() {
        s.turns.push(Turn { role: "user".into(), text: t.clone(), at: now, ..Default::default() });
    }

    // 1) 안전 확인이 가장 먼저
    if let Some(risk) = (!t.is_empty()).then(|| check_safety(&t)).flatten() {
        s.safety_hold = Some(risk.into());
        let msg = safety_message(risk);
        s.turns.push(Turn { role: "cat".into(), text: msg.clone(), at: now, safety: true, ..Default::default() });
        let mut r = Reply::cat(msg);
        r.safety = Some(risk.into());
        r.fixed = true;
        return Handled { replies: vec![r], safety: Some(risk.into()), summary: None };
    }

    // 2) 분석: 감정, 주제, 평가 표현, 민감 정보 (원문은 세션 안에만 남는다)
    if !t.is_empty() {
        let lang = analyze_language(&t);
        s.profanity = s.profanity || lang.profanity;
        for j in &lang.judgments {
            if !s.judgments.iter().any(|x| x.label == j.label) {
                s.judgments.push(j.clone());
            }
        }
        merge_unique(&mut s.emotions, detect_emotions(&t));
        let mut tp = detect_topics(&t);
        tp.extend(lang.judgments.iter().map(|j| j.topic.clone()));
        merge_unique(&mut s.topics, tp);
        merge_unique(&mut s.sensitive, detect_sensitive(&t));
        let key = if s.step != "open" || s.mode == "free" { s.step.clone() } else { "first".into() };
        s.answers.insert(key, Value::String(t.clone()));
    }

    if s.step == "done" {
        let t2 = "정리 카드에서 내용을 고칠 수 있어요. 다른 이야기를 하고 싶으면 \"새 대화\"를 눌러 주세요.";
        s.turns.push(Turn { role: "cat".into(), text: t2.into(), at: now, ..Default::default() });
        let mut r = Reply::cat(t2);
        r.fixed = true;
        return Handled { replies: vec![r], safety: None, summary: None };
    }

    let wants_summary = SUMMARIZE_WORDS.is_match(&t) && s.mode != "free";
    let mut step = if wants_summary { "done".to_string() } else { next_step(s) };
    // 첫 말에 이미 사건이 자세히 담겨 있으면 같은 질문을 반복하지 않는다
    if s.mode == "complaint" && s.step == "open" && step == "event" && looks_detailed(&t) {
        s.answers.insert("event".into(), Value::String(t.clone()));
        step = "agreement".into();
    }
    s.step = step.clone();

    let reflect = reflect_emotion(&s.emotions);
    let mut reply_text = fallback_text(&s.mode, &step, &reflect);
    if step == "agreement" && s.answers.contains_key("first") && !s.answers.contains_key("event") {
        reply_text = format!("{reflect}. {reply_text}");
    }
    s.turns.push(Turn { role: "cat".into(), text: reply_text.clone(), at: now, ..Default::default() });
    let mut r = Reply::cat(reply_text);
    r.intent = intent(&s.mode, &step).map(String::from);

    let mut summary = None;
    if step == "done" {
        let sm = build_summary(s);
        s.summary = Some(sm.clone());
        summary = Some(sm);
    }
    Handled { replies: vec![r], safety: None, summary }
}

static AG_NO: Lazy<Regex> = Lazy::new(|| Regex::new(r"(몰라|모르|아니|글쎄|확인\s*안|확실하지|애매|안\s*물어|못\s*들)").unwrap());
static AG_YES: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\s*(네|넵|응|맞아|그럼|당연)|확인했|같이\s*정했|정했었|정했어|합의|단톡|공지|캘린더|일정에\s*넣").unwrap());
static AG_HEARD: Lazy<Regex> = Lazy::new(|| Regex::new(r"(사정.*(들었|말했|얘기했)|이유.*(들었|말했))").unwrap());

/// (확인했는지, 상대 사정을 들었는지) — 모르면 None
pub fn classify_agreement(ans: Option<&str>) -> (Option<bool>, Option<bool>) {
    let Some(a) = ans else { return (None, None) };
    let no = AG_NO.is_match(a);
    let yes = AG_YES.is_match(a);
    let heard = AG_HEARD.is_match(a);
    (if no { Some(false) } else if yes { Some(true) } else { None }, if heard { Some(true) } else if no { Some(false) } else { None })
}

/// 정리 카드 (개인 기록 — 이 기기에만 저장)
pub fn build_summary(s: &Session) -> Value {
    let a = |k: &str| s.answers.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let topics: Vec<String> = s.topics.iter().filter(|id| topic(id).is_some()).cloned().collect();
    if s.mode == "explain" {
        return json!({
            "mode": "explain",
            "relatedTopic": s.related_topic,
            "situation": a("first"),
            "reason": a("reason"),
            "nextTime": a("next"),
            "emotions": s.emotions,
            "topics": topics,
            "sensitive": !s.sensitive.is_empty(),
            "note": "내 사정 기록은 이 기기에만 남아요. 아래 버튼으로 이 주제의 조언을 잠시 멈추거나, 팀 약속으로 함께 맞춰 보자고 제안할 수 있어요. 어느 쪽도 내 이야기 원문은 보내지 않아요."
        });
    }
    let agreement = s.answers.get("agreement").and_then(|v| v.as_str());
    let (confirmed, heard) = classify_agreement(agreement);
    let mut to_confirm = vec![];
    if confirmed != Some(true) {
        to_confirm.push("그 약속을 서로 확정된 것으로 알고 있었는지".to_string());
    }
    if heard != Some(true) {
        to_confirm.push("상대에게 다른 사정이 있었는지".to_string());
    }
    for id in &topics {
        if id != "time_promise" && id != "advance_notice" {
            to_confirm.push(topic(id).unwrap().to_confirm.into());
        }
    }
    to_confirm.truncate(4);
    let event = if s.answers.contains_key("event") { a("event") } else { a("first") };
    json!({
        "mode": "complaint",
        "reportedEvent": event,
        "sharedRecord": "아직 연결된 공동 일정·업무 기록이 없어 확인하지 않았어요.",
        "agreement": a("agreement"),
        "agreementNote": match confirmed {
            Some(true) => "함께 확인했다고 말함 (기록으로는 확인 전)",
            Some(false) => "함께 확인했는지는 확실하지 않음",
            None => "확인 여부를 아직 모름",
        },
        "emotions": s.emotions,
        "impact": a("impact"),
        "needs": a("need"),
        "actionRequests": topics.iter().map(|id| topic(id).unwrap().action_request).collect::<Vec<_>>(),
        "reframes": s.judgments.iter().map(|j| json!({ "from": j.label, "to": j.reframe })).collect::<Vec<_>>(),
        "toConfirm": to_confirm,
        "topics": topics,
        "sensitive": !s.sensitive.is_empty(),
        "profanity": s.profanity
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complaint_flow_reframes_and_summarizes() {
        let (mut s, r) = create_session("complaint", None, 1000);
        assert_eq!(r.len(), 1);
        let h = handle_user(&mut s, "시간도 안 지키고 맡은 일도 제대로 안 해서 너무 화나요. 진짜 무책임해요", 1001, false);
        assert!(h.replies[0].text.contains("화가"));
        handle_user(&mut s, "어제 같이 검토하기로 했는데 연락이 없었어요", 1002, false);
        handle_user(&mut s, "사정은 모르겠어요. 물어보진 않았어요", 1003, false);
        handle_user(&mut s, "제 다음 작업이 하루 밀렸어요", 1004, false);
        let h = handle_user(&mut s, "미리 알려주기만 했어도 괜찮았을 것 같아요", 1005, false);
        let sm = h.summary.expect("summary");
        assert_eq!(sm["mode"], "complaint");
        assert!(sm["reframes"].as_array().unwrap().iter().any(|r| r["from"] == "책임감에 대한 평가"));
        assert!(sm["topics"].as_array().unwrap().iter().any(|t| t == "time_promise"));
        assert_eq!(sm["agreementNote"], "함께 확인했는지는 확실하지 않음");
    }

    #[test]
    fn safety_stops_flow() {
        let (mut s, _) = create_session("free", None, 0);
        let h = handle_user(&mut s, "그냥 죽고 싶어요", 1, false);
        assert_eq!(h.safety.as_deref(), Some("self"));
        assert!(h.replies[0].text.contains("109"));
        assert_eq!(s.safety_hold.as_deref(), Some("self"));
        // 일상 과장은 위험 신호가 아니다
        let (mut s2, _) = create_session("free", None, 0);
        assert!(handle_user(&mut s2, "과제 때문에 힘들어 죽겠다", 1, false).safety.is_none());
    }
}
