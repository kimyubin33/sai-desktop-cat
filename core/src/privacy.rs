//! 개인 대화 → 공유 가능한 "일반 주제"로 바꾸는 단계.
//! 공유 대기함에 들어가는 것은 주제 id, 미리 써 둔 일반 문안, 공유 범위, 대략적인 주(週)뿐이다.
//! 원문·이름·날짜·사건 세부는 구조적으로 들어갈 자리가 없다.
use crate::time::iso_week;
use crate::topics::topic;
use crate::util::{to_base36, J};
use rand::Rng;
use serde_json::{json, Value};

pub const SMALL_TEAM_WARNING: &str =
    "팀이 두세 명뿐이면 일반화된 조언이라도 누가 말했는지 짐작될 수 있어요. 출처 노출을 줄이는 것이지 완전한 익명은 아니에요.";

/// 같은 대화·같은 주제에는 항상 같은 예시 문안을 고른다 (미리보기와 저장 내용이 일치하도록)
pub fn pick_stable<'a>(arr: &'a [&'a str], key: &str) -> &'a str {
    let mut h: u32 = 0;
    for ch in key.encode_utf16() {
        h = h.wrapping_mul(31).wrapping_add(ch as u32);
    }
    arr[(h as usize) % arr.len()]
}

pub fn build_preview(summary: &Value, key: &str) -> Value {
    let sensitive = summary.b("sensitive");
    let items: Vec<Value> = summary
        .strs("topics")
        .iter()
        .filter_map(|id| topic(id))
        .map(|t| {
            json!({
                "topic": t.id,
                "label": t.label,
                "advice": pick_stable(t.advice, &format!("{key}:{}", t.id)),
                "teamProposal": t.team_proposal,
                "sensitive": sensitive,
                "defaultScope": if sensitive { "review" } else { "auto" },
            })
        })
        .collect();
    json!({ "items": items, "smallTeamWarning": SMALL_TEAM_WARNING })
}

/// 선택을 공유 대기함 항목으로 바꾼다.
/// - 'private' 은 저장하지 않는다
/// - 민감한 내용이 있으면 'auto' 를 'review' 로 낮춘다
pub fn create_outbox_items(summary: &Value, choices: &[Value], now: i64, key: &str) -> Vec<Value> {
    let allowed = summary.strs("topics");
    let sensitive = summary.b("sensitive");
    let mut out = vec![];
    let mut rng = rand::thread_rng();
    for c in choices {
        let tid = c.s("topic");
        let Some(t) = topic(&tid) else { continue };
        if !allowed.contains(&tid) {
            continue;
        }
        let mut scope = c.s("scope");
        if !["private", "review", "auto"].contains(&scope.as_str()) {
            scope = "private".into();
        }
        if scope == "private" {
            continue;
        }
        if scope == "auto" && sensitive {
            scope = "review".into();
        }
        let target = if c.s("target") == "team" { "team" } else { "person" };
        out.push(json!({
            "id": format!("o{}{}", to_base36(now.max(0) as u64), to_base36(rng.gen_range(0..1_000_000))),
            "topic": t.id,
            "label": t.label,
            "scope": scope,
            "target": target,
            "advice": pick_stable(t.advice, &format!("{key}:{}", t.id)),
            "teamProposal": t.team_proposal,
            "week": iso_week(now),
        }));
    }
    out
}

fn same_key(x: &Value, it: &Value) -> bool {
    x.os("teamId") == it.os("teamId") && x.s("topic") == it.s("topic") && x.s("target") == it.s("target") && x.os("toMember") == it.os("toMember")
}

/// 같은 사람이 같은 주제를 여러 번 말해도 한 건으로 유지한다
/// (여러 팀원이 같은 의견을 낸 것처럼 부풀리지 않기 위해).
pub fn merge_into_outbox(existing: &[Value], items: &[Value]) -> Vec<Value> {
    let mut next: Vec<Value> = existing.to_vec();
    let done = |x: &Value| ["sent", "held"].contains(&x.s("status").as_str());
    for it in items {
        if let Some(i) = next.iter().position(|x| same_key(x, it) && !done(x)) {
            let id = next[i]["id"].clone();
            let mut merged = next[i].clone();
            crate::util::deep_merge(&mut merged, it);
            merged["id"] = id;
            next[i] = merged;
            continue;
        }
        // 이번 주에 이미 보낸 것과 같으면 다시 보내지 않는다
        if next.iter().any(|x| same_key(x, it) && done(x) && x.s("week") == it.s("week")) {
            continue;
        }
        next.push(it.clone());
    }
    next
}

/// 원문의 일부(min_len 자 이상 연속)가 공유 항목에 섞여 들어갔는지 검사한다
pub fn leaks_raw_text(item: &Value, raw_texts: &[String], min_len: usize) -> bool {
    let norm = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let hay = norm(&[item.s("advice"), item.s("teamProposal"), item.s("label")].join("\n"));
    for raw in raw_texts {
        let t: Vec<char> = norm(raw).chars().collect();
        if t.len() < min_len {
            continue;
        }
        for i in 0..=(t.len() - min_len) {
            let piece: String = t[i..i + min_len].iter().collect();
            if hay.contains(&piece) {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outbox_items_carry_no_raw_text() {
        let summary = json!({ "topics": ["time_promise", "workload"], "sensitive": false });
        let choices = vec![
            json!({ "topic": "time_promise", "scope": "auto", "target": "person" }),
            json!({ "topic": "workload", "scope": "private" }),
            json!({ "topic": "help_request", "scope": "auto" }),
        ];
        let items = create_outbox_items(&summary, &choices, 1_700_000_000_000, "k");
        assert_eq!(items.len(), 1);
        let keys: Vec<&String> = items[0].as_object().unwrap().keys().collect();
        for k in keys {
            assert!(["id", "topic", "label", "scope", "target", "advice", "teamProposal", "week"].contains(&k.as_str()));
        }
        assert!(!leaks_raw_text(&items[0], &["어제 민수가 회의에 안 와서 너무 화났어요".into()], 8));
        // 민감하면 auto → review
        let s2 = json!({ "topics": ["time_promise"], "sensitive": true });
        let it = create_outbox_items(&s2, &choices[..1], 0, "k");
        assert_eq!(it[0]["scope"], "review");
    }

    #[test]
    fn merge_keeps_one_per_topic() {
        let a = json!({ "id": "1", "topic": "workload", "target": "team", "status": "queued", "week": "2026-W46" });
        let b = json!({ "id": "2", "topic": "workload", "target": "team", "week": "2026-W46" });
        let m = merge_into_outbox(&[a], &[b]);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0]["id"], "1");
    }
}
