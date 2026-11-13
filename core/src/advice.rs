//! 사이가 평소에 건네는 말 모음과, 언제 무슨 말을 건넬지 정하는 규칙.
//! source.type: self(사이가 직접 쓴 조언) / proverb(한국 속담) / classic(출전이 분명한 고전 문장)
//! 사람 이름을 붙인 "명언"은 출처를 확인한 것만 추가한다 (잘못 알려진 인용이 많기 때문).
use crate::topics::TOPICS;
use crate::util::J;
use once_cell::sync::Lazy;
use rand::Rng;
use serde_json::Value;
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct Entry {
    pub id: String,
    pub kind: &'static str, // daily | reflection | quote | teamwork
    pub topic: Option<&'static str>,
    pub text: String,
    pub source_label: &'static str,
}

const SELF_LABEL: &str = "사이의 한마디";

const DAILY: &[(&str, &str)] = &[
    ("start-small", "오늘은 가장 작은 한 가지부터 시작해 볼까요?"),
    ("five-minutes", "하기 싫은 일은 딱 5분만 해 보기로 해요. 5분 뒤에 계속할지 정해도 돼요."),
    ("water", "물 한 잔 마시고 올래요? 저는 여기서 기다릴게요."),
    ("stretch", "한 시간 넘게 같은 자세였다면, 어깨를 한 번 쭉 펴 봐요."),
    ("one-tab", "지금 할 일과 관계없는 창을 하나만 닫아 볼까요?"),
    ("done-list", "오늘 한 일을 세 가지만 떠올려 봐요. 생각보다 많이 했을 거예요."),
    ("next-step", "막막할 땐 \"다음에 할 한 동작\"만 정하면 돼요. 파일 열기 같은 것도 괜찮아요."),
    ("break-ok", "쉬는 것도 일의 일부예요. 다시 집중할 힘을 모으는 시간이니까요."),
    ("eyes", "화면에서 눈을 떼고 먼 곳을 20초쯤 바라봐 주세요."),
    ("wrap-up", "하루를 마치기 전에, 내일 처음 할 일 하나만 적어 두면 아침이 가벼워요."),
];

const REFLECTION: &[(&str, &str)] = &[
    ("trust-small", "작은 약속을 지키는 일부터 팀의 신뢰가 쌓여요."),
    ("ask-early", "모르는 건 일찍 물어볼수록 작게 끝나요."),
    ("assume-less", "상대가 왜 그랬는지 짐작이 들 때, 한 번은 직접 물어보는 쪽을 골라 봐요."),
    ("thank", "오늘 도움 받은 일이 있다면, 짧게라도 고맙다고 전해 볼까요?"),
    ("say-limits", "할 수 없는 걸 할 수 없다고 미리 말하는 것도 팀을 돕는 일이에요."),
    ("same-page", "각자 생각하는 완료 기준을 한 줄씩 맞춰 볼까요?"),
];

const CLASSIC: &[(&str, &str, &str)] = &[
    ("classic-thousand-li", "천 리 길도 발밑의 한 걸음에서 시작된다.", "『도덕경』 64장 (千里之行 始於足下)"),
    ("classic-three-people", "세 사람이 길을 가면 그중에 반드시 나의 스승이 있다.", "『논어』 술이편 (三人行 必有我師焉)"),
    ("classic-not-want", "내가 원하지 않는 일을 남에게 하지 말라.", "『논어』 안연편 (己所不欲 勿施於人)"),
    ("classic-fault", "잘못을 하고도 고치지 않는 것, 그것을 잘못이라 한다.", "『논어』 위령공편 (過而不改 是謂過矣)"),
    ("classic-know", "아는 것을 안다 하고, 모르는 것을 모른다 하는 것이 아는 것이다.", "『논어』 위정편 (知之爲知之 不知爲不知 是知也)"),
    ("classic-know-others", "남이 나를 알아주지 않음을 걱정하지 말고, 내가 남을 알지 못함을 걱정하라.", "『논어』 학이편 (不患人之不己知 患不知人也)"),
    ("proverb-paper", "백지장도 맞들면 낫다.", "한국 속담"),
    ("proverb-words", "가는 말이 고와야 오는 말이 곱다.", "한국 속담"),
    ("proverb-dust", "티끌 모아 태산.", "한국 속담"),
];

pub static LIBRARY: Lazy<Vec<Entry>> = Lazy::new(|| {
    let mut v = vec![];
    for (id, text) in DAILY {
        v.push(Entry { id: format!("daily-{id}"), kind: "daily", topic: None, text: text.to_string(), source_label: SELF_LABEL });
    }
    for (id, text) in REFLECTION {
        v.push(Entry { id: format!("reflect-{id}"), kind: "reflection", topic: None, text: text.to_string(), source_label: SELF_LABEL });
    }
    for (id, text, label) in CLASSIC {
        v.push(Entry { id: id.to_string(), kind: "quote", topic: None, text: text.to_string(), source_label: label });
    }
    for t in TOPICS {
        for (i, a) in t.advice.iter().enumerate() {
            v.push(Entry { id: format!("team-{}-{i}", t.id), kind: "teamwork", topic: Some(t.id), text: a.to_string(), source_label: SELF_LABEL });
        }
    }
    v
});

// ---------- 언제 말할지 ----------
pub const RECENT_LIMIT: usize = 25;
const BOOST_FACTOR: f64 = 2.5;

// 사이는 업무 도우미라 아예 꺼 두면 쓸모가 없어진다. 그래서 "영구히 끄기"는 없고,
// 쉬는 방법은 모두 정해진 시간이 지나면 저절로 끝난다.
pub const MAX_QUIET_MS: i64 = 60 * 60 * 1000;
pub const MAX_FOCUS_MS: i64 = 2 * 60 * 60 * 1000;
pub const MAX_DND_MINUTES: i64 = 10 * 60;

fn parse_hm(s: &str) -> Option<i64> {
    let (h, m) = s.split_once(':')?;
    if m.len() != 2 || h.is_empty() || h.len() > 2 {
        return None;
    }
    Some(h.parse::<i64>().ok()? * 60 + m.parse::<i64>().ok()?)
}

pub fn dnd_span_minutes(dnd: &Value) -> i64 {
    match (parse_hm(&dnd.s("start")), parse_hm(&dnd.s("end"))) {
        (Some(s), Some(e)) => (e - s + 24 * 60).rem_euclid(24 * 60),
        _ => 0,
    }
}

pub fn in_dnd(dnd: &Value, minute_of_day: i64) -> bool {
    let (Some(s), Some(e)) = (parse_hm(&dnd.s("start")), parse_hm(&dnd.s("end"))) else { return false };
    if s == e {
        return false;
    }
    if s < e {
        minute_of_day >= s && minute_of_day < e
    } else {
        minute_of_day >= s || minute_of_day < e
    }
}

/// 지금 먼저 말을 걸면 안 되는 이유 (없으면 None)
pub fn silence_reason(settings: &Value, now: i64) -> Option<&'static str> {
    let q = settings.i("quietUntil");
    if q > now && q - now <= MAX_QUIET_MS {
        return Some("quiet");
    }
    let f = settings.i("focusUntil");
    if f > now && f - now <= MAX_FOCUS_MS {
        return Some("focus");
    }
    let dnd = settings.get("dnd").cloned().unwrap_or(Value::Null);
    if dnd.is_object() && dnd_span_minutes(&dnd) <= MAX_DND_MINUTES && in_dnd(&dnd, crate::time::minute_of_day(now)) {
        return Some("dnd");
    }
    None
}

pub fn next_delay_ms(settings: &Value) -> u64 {
    let (a, b) = match settings.s("frequency").as_str() {
        "normal" => (10.0, 20.0),
        "high" => (4.0, 8.0),
        _ => (25.0, 45.0),
    };
    let r: f64 = rand::thread_rng().gen();
    ((a + r * (b - a)) * 60_000.0).round() as u64
}

fn weight_of(e: &Entry, boosts: &HashMap<String, bool>, hour: u32) -> f64 {
    let mut w = 1.0;
    if e.kind == "quote" {
        w = 0.6;
    }
    if e.kind == "teamwork" {
        w = 0.9;
    }
    if e.id == "daily-wrap-up" {
        w = if hour >= 17 { 1.5 } else { 0.1 };
    }
    if e.id == "daily-start-small" {
        w = if hour < 12 { 1.5 } else { 0.6 };
    }
    if let Some(t) = e.topic {
        if boosts.get(t).copied().unwrap_or(false) {
            w *= BOOST_FACTOR;
        }
    }
    w
}

/// 다음 조언 하나를 고른다. recent: 최근에 보여 준 id (최신이 뒤)
pub fn pick_advice(recent: &[String], boosts: &HashMap<String, bool>, now: i64) -> Entry {
    let start = recent.len().saturating_sub(RECENT_LIMIT);
    let recent_set: std::collections::HashSet<&String> = recent[start..].iter().collect();
    let mut pool: Vec<&Entry> = LIBRARY.iter().filter(|e| !recent_set.contains(&e.id)).collect();
    if pool.is_empty() {
        let order: HashMap<&String, usize> = recent.iter().enumerate().map(|(i, id)| (id, i)).collect();
        let mut all: Vec<&Entry> = LIBRARY.iter().collect();
        all.sort_by_key(|e| order.get(&e.id).map(|x| *x as i64).unwrap_or(-1));
        pool = all.into_iter().take(5).collect();
    }
    let hour = crate::time::hour(now);
    let weights: Vec<f64> = pool.iter().map(|e| weight_of(e, boosts, hour)).collect();
    let total: f64 = weights.iter().sum();
    let mut r = rand::thread_rng().gen::<f64>() * total;
    for (i, e) in pool.iter().enumerate() {
        r -= weights[i];
        if r <= 0.0 {
            return (*e).clone();
        }
    }
    pool.last().map(|e| (*e).clone()).unwrap()
}

pub fn remember_shown(recent: &[String], id: &str) -> Vec<String> {
    let mut next: Vec<String> = recent.iter().filter(|x| *x != id).cloned().collect();
    next.push(id.into());
    let n = next.len();
    next.into_iter().skip(n.saturating_sub(RECENT_LIMIT * 2)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn silence_has_caps() {
        let now = 1_000_000_000_000;
        assert_eq!(silence_reason(&json!({ "quietUntil": now + 30 * 60_000 }), now), Some("quiet"));
        // 예전 버전의 "계속 조용히"(아주 먼 미래)는 더 이상 막지 않는다
        assert_eq!(silence_reason(&json!({ "quietUntil": now + 30 * 86_400_000i64 }), now), None);
        assert_eq!(dnd_span_minutes(&json!({ "start": "23:00", "end": "08:00" })), 540);
        assert!(in_dnd(&json!({ "start": "23:00", "end": "08:00" }), 60));
        assert!(!in_dnd(&json!({ "start": "23:00", "end": "08:00" }), 12 * 60));
    }

    #[test]
    fn picks_not_recent() {
        let recent: Vec<String> = LIBRARY.iter().take(LIBRARY.len() - 1).map(|e| e.id.clone()).collect();
        // RECENT_LIMIT 보다 많으면 오래된 것은 다시 허용된다
        let e = pick_advice(&recent, &HashMap::new(), 0);
        assert!(!recent[recent.len() - RECENT_LIMIT..].contains(&e.id));
    }
}
