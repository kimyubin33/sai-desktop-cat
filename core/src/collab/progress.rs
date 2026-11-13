//! 고양이가 업무 진행을 챙기는 말.
//! 원칙: 지연을 꾸짖기 전에 진행을 막는 이유를 묻고, 불가능한 일정은 무엇을 바꾸면 되는지 말한다.
use crate::time::{parse_deadline, DAY, HOUR};
use crate::util::{fmt_h, J};
use serde_json::{json, Value};
use std::collections::HashMap;

const REPEAT_MS: i64 = 4 * HOUR;
const STUCK_REPEAT_MS: i64 = 12 * HOUR;

fn a(id: &str, label: &str, primary: bool) -> Value {
    if primary {
        json!({ "id": id, "label": label, "primary": true })
    } else {
        json!({ "id": id, "label": label })
    }
}

/// 지금 건넬 진행 확인 한마디를 고른다 (없으면 None).
/// recent: { key: 마지막으로 말한 시각 }, plan_warnings: 배분 제안의 warnings
#[allow(clippy::too_many_arguments)]
pub fn next_nudge(my_uid: &str, project: &Value, tasks: &[Value], profile: Option<&Value>, plan_warnings: &[Value], effective: &HashMap<String, f64>, now: i64, recent: &Value) -> Option<Value> {
    if my_uid.is_empty() {
        return None;
    }
    let by_id: HashMap<String, &Value> = tasks.iter().map(|t| (t.s("id"), t)).collect();
    let mine: Vec<&Value> = tasks.iter().filter(|t| t.s("assignee") == my_uid && t.s("status") != "done").collect();
    let weekly = profile.map(|p| p.n("weeklyHours")).unwrap_or(0.0);
    let daily = (if weekly > 0.0 { weekly } else { 7.0 } / 7.0).max(0.5);
    let eff = |t: &Value| effective.get(&t.s("id")).copied().unwrap_or_else(|| if t.n("estimateHours") > 0.0 { t.n("estimateHours") } else { 1.0 });
    let deps_left = |t: &Value| -> Vec<&Value> { t.strs("dependsOn").iter().filter_map(|d| by_id.get(d).copied()).filter(|d| d.s("status") != "done").collect() };
    let ok = |key: &str, wait: i64| recent.get(key).and_then(|v| v.as_f64()).map(|t| now - t as i64 > wait).unwrap_or(true);
    let source = format!("협업 · {}", project.s("title"));
    let out = |key: String, task_id: Option<String>, text: String, actions: Vec<Value>| Some(json!({ "key": key, "taskId": task_id, "text": text, "actions": actions, "source": source }));

    // 1) 배정 확인
    for t in mine.iter().filter(|x| !x.b("accepted")) {
        let key = format!("accept:{}", t.s("id"));
        if ok(&key, REPEAT_MS) {
            return out(key, Some(t.s("id")), format!("\"{}\"을 맡아 줄 수 있을까요? 약 {}시간 예상이에요. 배정 이유는 협업 탭에서 볼 수 있어요.", t.s("title"), fmt_h(eff(t))), vec![a("task-object", "이의 있어요", false), a("task-accept", "맡을게요", true)]);
        }
    }
    // 2) 팀원이 막혀 있음
    for t in tasks.iter().filter(|x| x.s("status") == "blocked" && !x.s("assignee").is_empty() && x.s("assignee") != my_uid) {
        let key = format!("help:{}", t.s("id"));
        if ok(&key, STUCK_REPEAT_MS) {
            return out(key, Some(t.s("id")), format!("\"{}\"에서 도움이 필요하대요. 같이 볼 수 있다면 한마디 건네 볼까요?", t.s("title")), vec![a("later", "나중에", false), a("open-collab", "협업 탭 열기", true)]);
        }
    }
    // 3) 마감 여유 부족
    let late = plan_warnings.iter().any(|w| ["late", "tight", "shortfall"].contains(&w.s("kind").as_str()));
    if late && !mine.is_empty() && ok("plan-tight", 24 * HOUR) {
        return out("plan-tight".into(), None, "지금 계획대로라면 마감 전에 여유가 부족해요. 범위를 줄이거나 일정과 담당자를 다시 맞춰 볼까요?".into(), vec![a("later", "나중에", false), a("open-collab", "협업 탭 열기", true)]);
    }
    // 4) 진행 중인데 예상보다 오래 걸림 → 이유부터 묻기
    for t in mine.iter().filter(|x| x.s("status") == "doing" && x.i("startedAt") > 0) {
        let expected = (DAY as f64).max(eff(t) / daily * DAY as f64 * 1.5) as i64;
        let key = format!("stuck:{}", t.s("id"));
        if now - t.i("startedAt") > expected && ok(&key, STUCK_REPEAT_MS) {
            return out(key, Some(t.s("id")), format!("\"{}\" 진행은 어때요? 예상보다 어려운 부분이 생겼다면 도움을 요청하거나 범위를 나눌 수 있어요.", t.s("title")), vec![a("task-blocked", "막혔어요", false), a("task-ok", "잘 되고 있어요", false), a("task-done", "끝났어요", true)]);
        }
    }
    // 5) 앞 작업 기다리는 중
    for t in mine.iter().filter(|x| x.b("accepted") && x.s("status") == "todo") {
        let left = deps_left(t);
        let key = format!("waiting:{}", t.s("id"));
        if !left.is_empty() && ok(&key, 24 * HOUR) {
            return out(key, Some(t.s("id")), format!("\"{}\"은 앞 작업 \"{}\"이 아직 끝나지 않았어요. 그동안 준비물을 챙기거나 다른 작업부터 할 수 있어요.", t.s("title"), left[0].s("title")), vec![a("later", "나중에", false), a("open-collab", "협업 탭 열기", false)]);
        }
    }
    // 6) 시작 전 준비물
    for t in mine.iter().filter(|x| x.b("accepted") && x.s("status") == "todo" && deps_left(x).is_empty()) {
        let left: Vec<Value> = t.arr("prep").into_iter().filter(|p| !p.b("done")).collect();
        let key = format!("prep:{}", t.s("id"));
        if !left.is_empty() && ok(&key, REPEAT_MS) {
            return out(key, Some(t.s("id")), format!("\"{}\" 시작 전에 준비할 것이 {}개 남았어요. 먼저 \"{}\"부터 챙겨 볼까요?", t.s("title"), left.len(), left[0].s("text")), vec![a("later", "나중에", false), a("open-collab", "협업 탭 열기", true)]);
        }
    }
    // 7) 시작할 수 있는 일
    for t in mine.iter().filter(|x| x.b("accepted") && x.s("status") == "todo" && deps_left(x).is_empty()) {
        let key = format!("start:{}", t.s("id"));
        let rest = weekly - eff(t);
        let text = if weekly > 0.0 && rest > 0.0 {
            format!("이번 주에는 {}시간을 쓸 수 있다고 했죠. \"{}\"은 {}시간 정도로 예상해요. 나머지는 검토와 수정에 남겨 둘까요?", fmt_h(weekly), t.s("title"), fmt_h(eff(t)))
        } else {
            format!("\"{}\"을 시작할 수 있어요. 약 {}시간 예상이에요.", t.s("title"), fmt_h(eff(t)))
        };
        if ok(&key, 24 * HOUR) {
            return out(key, Some(t.s("id")), text, vec![a("later", "나중에", false), a("task-start", "시작할게요", true)]);
        }
    }
    // 8) 마감 임박
    if let Some(dl) = parse_deadline(&project.s("deadline")) {
        if !mine.is_empty() && dl - now < 2 * DAY && dl > now && ok("deadline", 12 * HOUR) {
            let days = ((dl - now) as f64 / DAY as f64).ceil().max(0.0) as i64;
            let names = mine.iter().map(|t| format!("\"{}\"", t.s("title"))).collect::<Vec<_>>().join(", ");
            return out("deadline".into(), None, format!("마감까지 {days}일 남았어요. 남은 내 일: {names}. 도움이 필요하면 지금 말해 두는 게 좋아요."), vec![a("later", "나중에", false), a("open-collab", "협업 탭 열기", false)]);
        }
    }
    None
}
