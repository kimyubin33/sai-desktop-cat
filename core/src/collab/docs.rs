//! Google 문서로 남길 내용: 작업 노트(팀이 함께 쓰는 결과물·메모), 프로젝트 현황 기록.
use crate::team::Team;
use crate::util::J;
use serde_json::Value;
use std::collections::HashMap;

pub fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn status_label(s: &str) -> &'static str {
    match s {
        "doing" => "하는 중",
        "blocked" => "막힘",
        "done" => "완료",
        _ => "할 일",
    }
}

fn date(ms: i64) -> String {
    let d = chrono::DateTime::from_timestamp_millis(ms).map(|d| d.with_timezone(&chrono::Local));
    d.map(|d| d.format("%Y-%m-%d %H:%M").to_string()).unwrap_or_default()
}

const STYLE: &str = "<style>body{font-family:sans-serif} table{border-collapse:collapse} td,th{border:1px solid #ccc;padding:4px 8px;text-align:left} th{background:#f7efe6}</style>";

impl Team {
    fn names(&self) -> HashMap<String, String> {
        self.collab_members().iter().map(|m| (m.s("uid"), m.s("name"))).collect()
    }

    /// 작업 노트의 첫 내용: 작업 정보 + 팀원이 채워 갈 칸
    pub fn task_note_html(&self, project_id: &str, task_id: &str) -> Option<(String, String)> {
        let p = self.project(project_id)?;
        let t = self.tasks_of(project_id).into_iter().find(|x| x.s("id") == task_id)?;
        let names = self.names();
        let who = names.get(&t.s("assignee")).cloned().unwrap_or_else(|| "미배정".into());
        let prep: Vec<String> = t.arr("prep").iter().map(|x| esc(&x.s("text"))).collect();
        let title = format!("[{}] {} - 작업 노트", p.s("title"), t.s("title"));
        let html = format!(
            "<html><head><meta charset=\"utf-8\">{STYLE}</head><body>\
<h1>{}</h1>\
<p>프로젝트: {} · 팀: {} · 담당: {} · 예상 {}시간</p>\
<p>완료 기준: {}</p>{}\
<p><i>사이에서 만든 팀 공용 노트예요. 링크가 있는 팀원은 누구나 고칠 수 있어요.</i></p>\
<h2>진행 메모</h2><p>(날짜와 함께 한 일, 알게 된 것, 막힌 점을 적어 주세요)</p>\
<h2>결과물</h2><p>(완성본 링크나 내용을 붙여 주세요)</p>\
<h2>검토 의견</h2><p></p>\
</body></html>",
            esc(&t.s("title")),
            esc(&p.s("title")),
            esc(&self.name()),
            esc(&who),
            t.n("estimateHours"),
            if t.s("doneCriteria").is_empty() { "(아직 없음)".into() } else { esc(&t.s("doneCriteria")) },
            if prep.is_empty() { String::new() } else { format!("<p>준비물·참고자료: {}</p>", prep.join(", ")) },
        );
        Some((title, html))
    }

    /// 프로젝트 현황 기록 (회의·보고용)
    pub fn project_report_html(&self, project_id: &str) -> Option<(String, String)> {
        let p = self.project(project_id)?;
        let now = self.now();
        let names = self.names();
        let tasks = self.tasks_of(project_id);
        let title_of: HashMap<String, String> = tasks.iter().map(|t| (t.s("id"), t.s("title"))).collect();
        let total: f64 = tasks.iter().map(|t| t.n("estimateHours")).sum();
        let done: f64 = tasks.iter().filter(|t| t.s("status") == "done").map(|t| t.n("estimateHours")).sum();
        let pct = if total > 0.0 { (done / total * 100.0).round() as i64 } else { 0 };
        let count = |s: &str| tasks.iter().filter(|t| t.s("status") == s).count();

        let mut rows = String::new();
        let mut sorted = tasks.clone();
        let order = |s: &str| match s {
            "blocked" => 0,
            "doing" => 1,
            "todo" => 2,
            _ => 3,
        };
        sorted.sort_by_key(|t| order(&t.s("status")));
        for t in &sorted {
            let who = names.get(&t.s("assignee")).cloned().unwrap_or_else(|| "미배정".into());
            let deps: Vec<String> = t.arr("dependsOn").iter().filter_map(|d| title_of.get(d.as_str().unwrap_or(""))).cloned().collect();
            rows += &format!(
                "<tr><td>{}</td><td>{}</td><td>{}{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                esc(&t.s("title")),
                status_label(&t.s("status")),
                esc(&who),
                if t.b("accepted") || t.s("assignee").is_empty() { "" } else { " (확인 전)" },
                t.n("estimateHours"),
                esc(&deps.join(", ")),
                esc(&t.s("doneCriteria")),
            );
        }
        let mut people = String::new();
        for m in self.collab_members() {
            let mine: Vec<&Value> = tasks.iter().filter(|t| t.s("assignee") == m.s("uid")).collect();
            let left: f64 = mine.iter().filter(|t| t.s("status") != "done").map(|t| t.n("estimateHours")).sum();
            let fin = mine.iter().filter(|t| t.s("status") == "done").count();
            people += &format!("<tr><td>{}</td><td>{}</td><td>{}개 / 완료 {}개</td><td>{}시간</td></tr>", esc(&m.s("name")), m.n("weeklyHours"), mine.len(), fin, left);
        }
        let files = self.files_of(project_id);
        let file_list: String = files
            .iter()
            .take(30)
            .map(|f| {
                let task = title_of.get(&f.s("taskId")).map(|t| format!(" ({})", esc(t))).unwrap_or_default();
                format!("<li><a href=\"{}\">{}</a>{task} · {}</li>", esc(&f.s("url")), esc(&f.s("name")), esc(&names.get(&f.s("by")).cloned().unwrap_or_default()))
            })
            .collect();
        let deadline = p.s("deadline");
        let left_days = crate::time::parse_deadline(&deadline).map(|d| ((d - now) as f64 / 86_400_000.0).ceil() as i64);
        let title = format!("[{}] 현황 {}", p.s("title"), date(now).get(..10).unwrap_or(""));
        let html = format!(
            "<html><head><meta charset=\"utf-8\">{STYLE}</head><body>\
<h1>{} 현황</h1>\
<p>{} 기준 · 팀: {}</p>\
<p>목표: {}<br>결과물: {}<br>마감: {}</p>\
<h2>진행</h2><p>작업 {}개 중 완료 {} · 하는 중 {} · 막힘 {} · 할 일 {} — 예상 시간 기준 {}% 진행</p>\
<h2>작업</h2><table><tr><th>작업</th><th>상태</th><th>담당</th><th>예상(시간)</th><th>먼저 끝나야 할 작업</th><th>완료 기준</th></tr>{rows}</table>\
<h2>팀원별</h2><table><tr><th>팀원</th><th>주간 가능 시간</th><th>맡은 작업</th><th>남은 예상 시간</th></tr>{people}</table>\
<h2>팀 자료</h2>{}\
<h2>회의 메모</h2><p></p>\
<p><i>사이가 만든 기록이에요. 이 문서는 그 시점의 기록이라 이후 바뀐 내용은 다시 기록해 주세요.</i></p>\
</body></html>",
            esc(&p.s("title")),
            date(now),
            esc(&self.name()),
            esc(&p.s("goal")),
            esc(&p.s("deliverable")),
            if deadline.is_empty() { "없음".to_string() } else { format!("{} ({})", esc(&deadline), left_days.map(|d| if d >= 0 { format!("{d}일 남음") } else { format!("{}일 지남", -d) }).unwrap_or_default()) },
            tasks.len(),
            count("done"),
            count("doing"),
            count("blocked"),
            count("todo"),
            pct,
            if file_list.is_empty() { "<p>없음</p>".to_string() } else { format!("<ul>{file_list}</ul>") },
        );
        Some((title, html))
    }
}
