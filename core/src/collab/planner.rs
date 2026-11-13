//! 업무 배분 제안.
//!
//! "공정"을 같은 개수로 보지 않는다. 다음을 함께 본다.
//!  - 가용 시간 대비 부담 (검토·수정 몫 20%를 먼저 남겨 둔다)
//!  - 필요한 역량과 경험, 배우고 싶은 일 (배우는 사람은 시간을 더 잡는다)
//!  - 어려운 일·반복적인 일이 한 사람에게 쏠리지 않는지
//!  - 선행 작업 순서와 마감까지의 여유
//!  - 당사자가 이의를 제기한 일은 그 사람에게 다시 배정하지 않는다
//! 결과에는 배정 이유와 추정의 불확실성을 함께 담는다. 실제 배정은 사람이 확인한 뒤에만 바뀐다.
use crate::time::{fmt_month_day, parse_deadline, DAY};
use crate::util::{fmt_h, round1, round2, J};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

pub const REVIEW_RESERVE: f64 = 0.2;

pub fn level_label(l: i64) -> &'static str {
    match l {
        1 => "배우는 중",
        2 => "할 수 있음",
        3 => "능숙",
        _ => "경험 없음",
    }
}

fn norm(s: &str) -> String {
    s.trim().to_lowercase()
}

/// 선행 작업 기준 위상 정렬. 순환이 있으면 cycle 에 해당 작업 id 를 담는다
pub fn topo_order(tasks: &[Value]) -> (Vec<Value>, Vec<String>) {
    let by_id: HashMap<String, &Value> = tasks.iter().map(|t| (t.s("id"), t)).collect();
    let mut state: HashMap<String, u8> = HashMap::new();
    let mut order = vec![];
    let mut cycle: Vec<String> = vec![];
    fn visit(t: &Value, stack: Vec<String>, by_id: &HashMap<String, &Value>, state: &mut HashMap<String, u8>, order: &mut Vec<Value>, cycle: &mut Vec<String>) {
        let id = t.s("id");
        match state.get(&id) {
            Some(2) => return,
            Some(1) => {
                if let Some(pos) = stack.iter().position(|x| *x == id) {
                    for x in &stack[pos..] {
                        if !cycle.contains(x) {
                            cycle.push(x.clone());
                        }
                    }
                }
                return;
            }
            _ => {}
        }
        state.insert(id.clone(), 1);
        for dep in t.strs("dependsOn") {
            if let Some(d) = by_id.get(&dep) {
                let mut st = stack.clone();
                st.push(id.clone());
                visit(d, st, by_id, state, order, cycle);
            }
        }
        state.insert(id, 2);
        order.push(t.clone());
    }
    for t in tasks {
        visit(t, vec![t.s("id")], &by_id, &mut state, &mut order, &mut cycle);
    }
    (order, cycle)
}

struct M {
    uid: String,
    name: String,
    weekly: f64,
    daily: f64,
    capacity: f64,
    usable: f64,
    skills: HashMap<String, i64>,
    learn: HashSet<String>,
    load: f64,
    hard: i64,
    easy: i64,
    count: i64,
}

struct Fit {
    fit: f64,
    levels: Vec<(String, i64)>,
    learn_match: bool,
    req: Vec<String>,
}

fn fit_of(m: &M, task: &Value) -> Fit {
    let req: Vec<String> = task.strs("skills").iter().map(|s| norm(s)).filter(|s| !s.is_empty()).collect();
    if req.is_empty() {
        return Fit { fit: 0.5, levels: vec![], learn_match: false, req };
    }
    let levels: Vec<(String, i64)> = req.iter().map(|s| (s.clone(), *m.skills.get(s).unwrap_or(&0))).collect();
    let fit = levels.iter().map(|l| l.1 as f64).sum::<f64>() / (3.0 * req.len() as f64);
    let learn_match = levels.iter().any(|l| l.1 < 2 && m.learn.contains(&l.0));
    Fit { fit, levels, learn_match, req }
}

/// 역량이 낮을수록, 배우는 일일수록 시간을 더 잡는다
pub fn effective_hours(est: f64, fit: f64, learn_match: bool, has_req: bool) -> f64 {
    if !has_req {
        return est;
    }
    if fit < 0.34 {
        return est * if learn_match { 1.3 } else { 1.5 };
    }
    if fit < 0.67 {
        return est * 1.15;
    }
    est
}

fn est_of(t: &Value) -> f64 {
    let e = t.n("estimateHours");
    if e > 0.0 {
        e
    } else {
        1.0
    }
}

fn diff_of(t: &Value) -> i64 {
    let d = t.i("difficulty");
    if d == 0 {
        2
    } else {
        d
    }
}

/// members: [{ uid, name, weeklyHours, skills: {이름: 0~3}, learn: [이름] }]
/// tasks:   [{ id, title, estimateHours, difficulty(1~3), skills: [], dependsOn: [], assignee, accepted, status, objections: {uid: 사유} }]
pub fn plan(members: &[Value], tasks: &[Value], deadline: &str, now: i64) -> Value {
    let mut warnings: Vec<Value> = vec![];
    let deadline_at = parse_deadline(deadline);
    let days = deadline_at.map(|d| ((d - now) as f64 / DAY as f64).max(1.0)).unwrap_or(14.0);
    if deadline_at.is_none() {
        warnings.push(json!({ "kind": "no-deadline", "text": "마감일이 없어 2주를 기준으로 계산했어요." }));
    }
    let weeks = days / 7.0;

    let mut ms: Vec<M> = members
        .iter()
        .map(|m| {
            let weekly = m.n("weeklyHours").max(0.0);
            let capacity = weekly * weeks;
            M {
                uid: m.s("uid"),
                name: if m.s("name").is_empty() { "팀원".into() } else { m.s("name") },
                weekly,
                daily: weekly / 7.0,
                capacity,
                usable: capacity * (1.0 - REVIEW_RESERVE),
                skills: m.obj("skills").iter().map(|(k, v)| (norm(k), v.as_f64().unwrap_or(0.0) as i64)).collect(),
                learn: m.strs("learn").iter().map(|s| norm(s)).collect(),
                load: 0.0,
                hard: 0,
                easy: 0,
                count: 0,
            }
        })
        .collect();
    if ms.is_empty() {
        return json!({ "assignments": [], "members": [], "warnings": [{ "kind": "no-members", "text": "팀원 정보가 없어요." }] });
    }
    for m in &ms {
        if m.weekly == 0.0 {
            warnings.push(json!({ "kind": "no-hours", "text": format!("{}님의 주간 가능 시간이 비어 있어 배정에서 빠졌어요.", m.name) }));
        }
    }
    let idx_of = |ms: &Vec<M>, uid: &str| ms.iter().position(|m| m.uid == uid);

    let open: Vec<Value> = tasks.iter().filter(|t| t.s("status") != "done").cloned().collect();
    let (order, cycle) = topo_order(&open);
    if !cycle.is_empty() {
        let names: Vec<String> = cycle.iter().filter_map(|id| open.iter().find(|t| &t.s("id") == id).map(|t| t.s("title"))).collect();
        warnings.push(json!({ "kind": "cycle", "text": format!("선행 작업이 서로 꼬여 있어요: {}. 순서를 다시 정해 주세요.", names.join(" ↔ ")) }));
    }
    for t in &open {
        if t.n("estimateHours") <= 0.0 {
            warnings.push(json!({ "kind": "no-estimate", "text": format!("\"{}\"의 예상 시간이 없어 1시간으로 계산했어요.", t.s("title")) }));
        }
    }

    // 이미 수락된 배정은 그대로 두고 부담에 넣는다
    let mut assignments: Vec<Value> = vec![];
    for t in &open {
        if !t.b("accepted") {
            continue;
        }
        let Some(i) = idx_of(&ms, &t.s("assignee")) else { continue };
        let f = fit_of(&ms[i], t);
        let eff = effective_hours(est_of(t), f.fit, f.learn_match, !f.req.is_empty());
        let m = &mut ms[i];
        m.load += eff;
        m.count += 1;
        if diff_of(t) == 3 {
            m.hard += 1;
        }
        if diff_of(t) == 1 {
            m.easy += 1;
        }
        assignments.push(json!({ "taskId": t.s("id"), "uid": m.uid, "effectiveHours": eff, "fixed": true, "reasons": ["이미 수락한 배정이에요."] }));
    }

    // 어려운 일, 큰 일부터 나눈다 (작은 일로 마지막 균형을 맞추기 쉽도록)
    let mut todo: Vec<&Value> = open.iter().filter(|t| !(t.b("accepted") && idx_of(&ms, &t.s("assignee")).is_some())).collect();
    todo.sort_by(|a, b| diff_of(b).cmp(&diff_of(a)).then(est_of(b).partial_cmp(&est_of(a)).unwrap_or(std::cmp::Ordering::Equal)));

    for t in todo {
        let est = est_of(t);
        let diff = diff_of(t);
        let objected: HashSet<String> = t.obj("objections").keys().cloned().collect();
        let mut best: Option<(usize, f64, f64, f64)> = None; // (idx, eff, ratio, score)
        for (i, m) in ms.iter().enumerate() {
            if m.weekly == 0.0 || objected.contains(&m.uid) {
                continue;
            }
            let f = fit_of(m, t);
            let eff = effective_hours(est, f.fit, f.learn_match, !f.req.is_empty());
            let ratio = if m.usable > 0.0 { (m.load + eff) / m.usable } else { f64::INFINITY };
            let score = 3.0 * f.fit + if f.learn_match { 0.8 } else { 0.0 } - 4.0 * ratio
                - if diff == 3 { 1.2 * m.hard as f64 } else { 0.0 }
                - if diff == 1 { 0.6 * m.easy as f64 } else { 0.0 }
                - if ratio > 1.0 { 10.0 } else { 0.0 };
            let better = match best {
                None => true,
                Some((_, _, br, bs)) => score > bs + 1e-9 || ((score - bs).abs() < 1e-9 && ratio < br),
            };
            if better {
                best = Some((i, eff, ratio, score));
            }
        }
        let Some((i, eff, ratio, _)) = best else {
            assignments.push(json!({ "taskId": t.s("id"), "uid": null, "effectiveHours": est, "reasons": ["배정할 수 있는 팀원이 없어요 (가능 시간이 없거나 모두 이의를 제기함)."] }));
            warnings.push(json!({ "kind": "unassigned", "text": format!("\"{}\"을 맡을 수 있는 사람이 없어요.", t.s("title")) }));
            continue;
        };
        let f = fit_of(&ms[i], t);
        {
            let m = &mut ms[i];
            m.load += eff;
            m.count += 1;
            if diff == 3 {
                m.hard += 1;
            }
            if diff == 1 {
                m.easy += 1;
            }
        }
        let reasons = reasons_for(&ms[i], t, &f, eff, ratio, &ms);
        assignments.push(json!({ "taskId": t.s("id"), "uid": ms[i].uid, "effectiveHours": eff, "reasons": reasons }));
    }

    // 역량 공백·부담 초과·전체 시간 부족
    for t in &open {
        for s in t.strs("skills").iter().map(|s| norm(s)).filter(|s| !s.is_empty()) {
            if !ms.iter().any(|m| *m.skills.get(&s).unwrap_or(&0) >= 2) {
                warnings.push(json!({ "kind": "missing-skill", "text": format!("\"{}\"에 필요한 \"{}\"를 할 수 있다고 적은 사람이 없어요. 배울 시간과 도움 받을 곳을 함께 정해 주세요.", t.s("title"), s) }));
            }
        }
    }
    for m in &ms {
        if m.usable > 0.0 && m.load > m.usable {
            warnings.push(json!({ "kind": "overload", "uid": m.uid, "text": format!("{}님은 마감까지 쓸 수 있는 {}시간보다 {}시간 많이 맡게 돼요.", m.name, fmt_h(m.usable), fmt_h(m.load - m.usable)) }));
        }
    }
    let need: f64 = assignments.iter().map(|a| a.n("effectiveHours")).sum();
    let have: f64 = ms.iter().map(|m| m.usable).sum();
    if need > have {
        warnings.push(json!({ "kind": "shortfall", "text": format!("필요한 시간이 검토 몫을 뺀 가능 시간보다 약 {}시간 많아요. 범위를 줄이거나, 마감을 늦추거나, 함께할 사람을 늘려야 해요.", fmt_h(need - have)) }));
    }

    // 일정 추정: 선행 작업과 한 사람이 동시에 두 일을 못 한다는 점을 반영
    let (by_task, finish_at) = schedule(&order, &assignments, &ms, now);
    let mut slack_days = Value::Null;
    if let (Some(dl), Some(fin)) = (deadline_at, finish_at) {
        let slack = (dl - fin) as f64 / DAY as f64;
        slack_days = json!(round1(slack));
        let review_days = (days * 0.1).max(1.0);
        if slack < 0.0 {
            warnings.push(json!({ "kind": "late", "text": format!("지금 계획대로면 {}쯤 끝나 마감보다 {}일 늦어요. 범위·담당·일정을 다시 맞춰 볼까요?", fmt_month_day(fin), (-slack).ceil() as i64) }));
        } else if slack < review_days {
            warnings.push(json!({ "kind": "tight", "text": format!("{}쯤 끝나 마감 전 여유가 {}일뿐이에요. 검토·수정 시간이 부족할 수 있어요.", fmt_month_day(fin), slack.floor() as i64) }));
        }
    }

    let assignments: Vec<Value> = assignments
        .into_iter()
        .map(|mut a| {
            a["effectiveHours"] = json!(round1(a.n("effectiveHours")));
            if let Some((s, e)) = by_task.get(&a.s("taskId")) {
                a["startAt"] = json!(s);
                a["finishAt"] = json!(e);
            }
            a
        })
        .collect();
    json!({
        "weeks": round1(weeks),
        "assignments": assignments,
        "members": ms.iter().map(|m| json!({
            "uid": m.uid, "name": m.name, "weeklyHours": m.weekly, "capacity": round1(m.capacity), "usable": round1(m.usable),
            "load": round1(m.load), "ratio": if m.usable > 0.0 { json!(round2(m.load / m.usable)) } else { Value::Null },
            "count": m.count, "hard": m.hard,
        })).collect::<Vec<_>>(),
        "warnings": warnings,
        "finishAt": finish_at,
        "slackDays": slack_days,
        "note": "자기소개와 예상 시간을 바탕으로 한 추정이에요. 실제로 해 보면서 고쳐 주세요.",
    })
}

fn reasons_for(m: &M, t: &Value, f: &Fit, eff: f64, ratio: f64, all: &[M]) -> Vec<String> {
    let mut r = vec![];
    let est = est_of(t);
    if !f.req.is_empty() {
        let lv = f.levels.iter().map(|(s, l)| format!("{}: {}", s, level_label(*l))).collect::<Vec<_>>().join(", ");
        r.push(format!("필요한 역량({lv})을 자기소개 기준으로 봤어요."));
    }
    if f.learn_match {
        r.push("배우고 싶다고 적은 일이라, 배우는 시간을 더해 계산했어요.".into());
    }
    if eff > est + 0.01 {
        r.push(format!("익숙하지 않은 부분을 고려해 {}시간 → {}시간으로 잡았어요.", fmt_h(est), fmt_h(eff)));
    }
    r.push(format!("마감까지 쓸 수 있는 {}시간 중 검토 몫을 남기고 {}시간({}%)을 맡게 돼요.", fmt_h(m.capacity), fmt_h(m.load), (ratio * 100.0).round() as i64));
    if diff_of(t) == 3 && all.iter().any(|x| x.uid != m.uid && x.usable > 0.0 && x.hard > m.hard - 1) {
        r.push("어려운 일이 한 사람에게 몰리지 않게 나눴어요.".into());
    }
    if ratio > 1.0 {
        r.push("그래도 가능 시간을 넘어요. 범위를 나누거나 도와줄 사람을 붙여 주세요.".into());
    }
    r
}

fn schedule(order: &[Value], assignments: &[Value], ms: &[M], now: i64) -> (HashMap<String, (i64, i64)>, Option<i64>) {
    let asg: HashMap<String, &Value> = assignments.iter().map(|a| (a.s("taskId"), a)).collect();
    let mut by_task = HashMap::new();
    let mut finish: HashMap<String, f64> = HashMap::new();
    let mut free: HashMap<String, f64> = HashMap::new();
    let mut finish_at: Option<f64> = None;
    for t in order {
        let Some(a) = asg.get(&t.s("id")) else { continue };
        let uid = a.s("uid");
        let Some(m) = ms.iter().find(|m| m.uid == uid) else { continue };
        if m.daily == 0.0 {
            continue;
        }
        let dep_end = t.strs("dependsOn").iter().map(|d| *finish.get(d).unwrap_or(&(now as f64))).fold(now as f64, f64::max);
        let start = dep_end.max(*free.get(&uid).unwrap_or(&(now as f64)));
        let end = start + (a.n("effectiveHours") / m.daily) * DAY as f64;
        finish.insert(t.s("id"), end);
        free.insert(uid, end);
        by_task.insert(t.s("id"), (start.round() as i64, end.round() as i64));
        finish_at = Some(finish_at.unwrap_or(0.0).max(end));
    }
    (by_task, finish_at.map(|f| f.round() as i64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assigns_by_skill_and_load() {
        let now = crate::time::ymd_hm(2026, 11, 14, 10, 0).unwrap();
        let members = vec![
            json!({ "uid": "a", "name": "유빈", "weeklyHours": 10, "skills": { "영상편집": 3, "촬영": 2 }, "learn": [] }),
            json!({ "uid": "b", "name": "민수", "weeklyHours": 6, "skills": { "디자인": 3, "글쓰기": 2 }, "learn": ["영상편집"] }),
        ];
        let tasks = vec![
            json!({ "id": "t1", "title": "촬영", "estimateHours": 3, "difficulty": 2, "skills": ["촬영"], "status": "todo" }),
            json!({ "id": "t2", "title": "컷 편집", "estimateHours": 5, "difficulty": 3, "skills": ["영상편집"], "dependsOn": ["t1"], "status": "todo" }),
            json!({ "id": "t3", "title": "포스터 디자인", "estimateHours": 4, "difficulty": 2, "skills": ["디자인"], "status": "todo" }),
        ];
        let p = plan(&members, &tasks, "2026-11-28", now);
        let who = |id: &str| p["assignments"].as_array().unwrap().iter().find(|a| a["taskId"] == id).unwrap()["uid"].clone();
        assert_eq!(who("t2"), "a");
        assert_eq!(who("t3"), "b");
        let t2 = p["assignments"].as_array().unwrap().iter().find(|a| a["taskId"] == "t2").unwrap();
        let t1 = p["assignments"].as_array().unwrap().iter().find(|a| a["taskId"] == "t1").unwrap();
        assert!(t2.i("startAt") >= t1.i("finishAt"));
    }

    #[test]
    fn objection_excludes_member() {
        let members = vec![json!({ "uid": "a", "weeklyHours": 10 }), json!({ "uid": "b", "weeklyHours": 10 })];
        let tasks = vec![json!({ "id": "t", "title": "x", "estimateHours": 2, "status": "todo", "objections": { "a": "기타", "b": "기타" } })];
        let p = plan(&members, &tasks, "", 0);
        assert!(p["assignments"][0]["uid"].is_null());
    }
}
