//! AI 없이도 시간표를 말로 고칠 수 있게, 자주 쓰는 표현을 규칙으로 읽는다.
//! 예) "2번 목요일 3시로 옮겨줘", "자료조사 금요일 2시부터 4시까지", "수요일은 안 돼", "발표 준비 3시간이면 돼",
//!     "1번 빼줘", "하루에 4시간만", "밤 10시 이후는 안 돼", "캘린더에 넣어줘", "다시 짜줘"
use super::korean_time::{find_times, parse_duration, parse_when};
use crate::time::{minute_of_day, same_day, start_of_day, MIN};
use crate::util::J;
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{json, Value};

#[derive(Debug, Clone, PartialEq)]
pub enum Cmd {
    Apply,
    Reset,
    Replan,
    Show,
    Prefs(Value),
    Move { no: i64, start: i64, end: i64 },
    Remove { no: i64 },
    Add { task_id: String, start: i64, end: i64 },
    Hours { task_id: String, hours: f64 },
    Unavailable { start: i64, end: i64, note: String },
    Unknown(Option<String>),
}

fn re(s: &str) -> Regex {
    Regex::new(s).unwrap()
}

static APPLY: Lazy<Regex> = Lazy::new(|| re(r"(캘린더|구글|달력|할\s*일).*(넣|반영|올려|추가|적용|등록)|^(적용|반영|확정|등록)|그대로\s*(넣|진행|해)|좋아요?\s*(넣|반영)"));
static RESET: Lazy<Regex> = Lazy::new(|| re(r"처음부터|초기화|리셋"));
static REPLAN: Lazy<Regex> = Lazy::new(|| re(r"다시\s*(짜|계획|배치|잡)|재배치|새로\s*(짜|잡)"));
static SHOW_A: Lazy<Regex> = Lazy::new(|| re(r"(보여|알려|뭐야|어때)"));
static SHOW_B: Lazy<Regex> = Lazy::new(|| re(r"일정|시간표|계획|스케줄"));
static DAILY: Lazy<Regex> = Lazy::new(|| re(r"하루(?:에|에는)?\s*(?:최대\s*)?(\d+(?:\.\d+)?)\s*시간"));
static ONCE: Lazy<Regex> = Lazy::new(|| re(r"한\s*번에\s*(?:최대\s*)?(\d+(?:\.\d+)?)\s*시간"));
static REF: Lazy<Regex> = Lazy::new(|| re(r"(\d{1,2})\s*번"));
static REMOVE: Lazy<Regex> = Lazy::new(|| re(r"(빼|취소|지워|삭제|없애|안\s*할래)"));
static HOURS_CTX: Lazy<Regex> = Lazy::new(|| re(r"(걸려|필요|더|추가|줄|덜|이면|면\s*돼|로\s*(잡|해|바꿔)|짜리|정도)"));
static CLOCK: Lazy<Regex> = Lazy::new(|| re(r"(\d{1,2})\s*시(\s*(반|\d{1,2}\s*분))?"));
static BUSY: Lazy<Regex> = Lazy::new(|| re(r"안\s*돼|안돼|안\s*되|못\s*해|못해|바빠|약속|일정\s*(이\s*)?있|수업|알바|시험|비워|쉴|쉬|불가|어려워"));
static NOTE: Lazy<Regex> = Lazy::new(|| re(r"(약속|수업|알바|시험|일정)"));

fn norm(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_lowercase()
}

/// 문장에서 가장 잘 맞는 작업 (taskId, title, need)
pub fn match_task(text: &str, tasks: &[Value]) -> Option<Value> {
    let t = norm(text);
    let mut best: Option<(usize, Value)> = None;
    for task in tasks {
        let name = norm(&task.s("title"));
        if name.is_empty() {
            continue;
        }
        let mut score = 0;
        if t.contains(&name) {
            score = name.chars().count() * 2;
        } else {
            for w in task.s("title").split(|c: char| c.is_whitespace() || "·,/()".contains(c)).filter(|w| w.chars().count() >= 2) {
                let nw = norm(w);
                if t.contains(&nw) {
                    score = score.max(nw.chars().count());
                }
            }
            if score == 0 && name.chars().count() >= 3 && t.contains(&name.chars().take(3).collect::<String>()) {
                score = 2;
            }
        }
        if score > 0 && best.as_ref().map(|b| score > b.0).unwrap_or(true) {
            best = Some((score, task.clone()));
        }
    }
    best.map(|b| b.1)
}

fn at(day: i64, minutes: i64) -> i64 {
    start_of_day(day) + minutes * MIN
}

/// blocks: [{ no, id, taskId, title, start, end }], tasks: [{ taskId, title, need }], prefs: { dayStart, dayEnd }
pub fn parse_command(text: &str, blocks: &[Value], tasks: &[Value], prefs: &Value, now: i64) -> Cmd {
    let s = text.trim();
    if s.is_empty() {
        return Cmd::Unknown(None);
    }
    if APPLY.is_match(s) {
        return Cmd::Apply;
    }
    if RESET.is_match(s) {
        return Cmd::Reset;
    }
    if REPLAN.is_match(s) {
        return Cmd::Replan;
    }
    if SHOW_A.is_match(s) && SHOW_B.is_match(s) && find_times(s).is_empty() {
        return Cmd::Show;
    }

    // 하루 최대 시간, 시작·끝 시각
    let mut p = serde_json::Map::new();
    if let Some(c) = DAILY.captures(s) {
        if re(r"만|까지|넘|최대|이하|이상은").is_match(s) {
            p.insert("dailyMaxHours".into(), json!(c[1].parse::<f64>().unwrap_or(6.0)));
        }
    }
    if let Some(c) = ONCE.captures(s) {
        p.insert("maxBlockHours".into(), json!(c[1].parse::<f64>().unwrap_or(2.0)));
    }
    let times = find_times(s);
    let neg = re(r"안|싫|못|말고").is_match(s);
    if times.len() == 1 && re(r"이후|넘어서|지나서|부터는\s*안|뒤로는").is_match(s) && neg {
        p.insert("dayEnd".into(), json!((times[0].minutes as f64 / 60.0).round()));
    }
    if times.len() == 1 && re(r"이전|전에는|전엔|보다\s*일찍|아침").is_match(s) && neg {
        p.insert("dayStart".into(), json!((times[0].minutes as f64 / 60.0).round()));
    }
    if !p.is_empty() {
        return Cmd::Prefs(Value::Object(p));
    }

    let ref_no = REF.captures(s).filter(|c| !s[c.get(0).unwrap().end()..].trim_start().starts_with("째")).map(|c| c[1].parse::<i64>().unwrap_or(0));
    let block = ref_no.and_then(|n| blocks.iter().find(|b| b.i("no") == n).cloned());
    if let (Some(n), None) = (ref_no, &block) {
        return Cmd::Unknown(Some(format!("{n}번 일정이 없어요. 목록의 번호를 확인해 주세요.")));
    }
    let task = if block.is_some() { None } else { match_task(s, tasks) };
    let stripped = REF.replace(s, " ").to_string();
    let when = parse_when(&stripped, now);
    let time_given = when.start.is_some();
    let day_given = when.day.is_some();

    // 빼기
    if REMOVE.is_match(s) && (block.is_some() || task.is_some()) {
        if let Some(b) = &block {
            return Cmd::Remove { no: b.i("no") };
        }
        let t = task.unwrap();
        if let Some(first) = blocks.iter().find(|b| b.s("taskId") == t.s("taskId") && when.day.map(|d| same_day(b.i("start"), d)).unwrap_or(true)) {
            return Cmd::Remove { no: first.i("no") };
        }
        return Cmd::Unknown(Some(format!("\"{}\" 일정이 그날엔 없어요.", t.s("title"))));
    }

    // 작업 시간 바꾸기: "자료조사 3시간이면 돼", "발표 준비 2시간 더 필요해"
    let no_clock = CLOCK.replace_all(s, |c: &regex::Captures| if s[c.get(0).unwrap().end()..].starts_with('간') { c[0].to_string() } else { " ".into() }).to_string();
    let dur = parse_duration(&no_clock);
    if let (Some(t), Some(d)) = (&task, dur) {
        if !time_given && HOURS_CTX.is_match(s) {
            let cur = t.n("need");
            let hours = if re(r"더|추가|늘려").is_match(s) {
                cur + d
            } else if re(r"줄여|덜|빼").is_match(s) {
                (cur - d).max(0.0)
            } else {
                d
            };
            return Cmd::Hours { task_id: t.s("taskId"), hours };
        }
    }

    // 옮기기 / 추가
    if (block.is_some() || task.is_some()) && (time_given || day_given) {
        let base = block.clone().or_else(|| task.as_ref().and_then(|t| blocks.iter().find(|b| b.s("taskId") == t.s("taskId")).cloned()));
        if let Some(t) = &task {
            if re(r"추가|더\s*잡|하나\s*더|넣어").is_match(s) || base.is_none() {
                let Some(st) = when.start else { return Cmd::Unknown(Some(format!("\"{}\"을 몇 시에 할지 알려 주세요. 예: \"금요일 2시부터 4시까지\"", t.s("title")))) };
                let day = when.day.unwrap_or(now);
                let start = at(day, st);
                let default_h = if base.is_none() { t.n("need").clamp(0.5, 2.0) } else { 1.0 };
                let end = when.end.map(|e| at(day, e)).unwrap_or(start + (dur.unwrap_or(default_h) * 60.0).round() as i64 * MIN);
                return Cmd::Add { task_id: t.s("taskId"), start, end };
            }
        }
        let b = base.unwrap();
        let len = b.i("end") - b.i("start");
        let day = when.day.unwrap_or(b.i("start"));
        let start_min = when.start.unwrap_or_else(|| minute_of_day(b.i("start")));
        let start = at(day, start_min);
        let end = when.end.map(|e| at(day, e)).unwrap_or_else(|| dur.map(|d| start + (d * 60.0).round() as i64 * MIN).unwrap_or(start + len));
        return Cmd::Move { no: b.i("no"), start, end };
    }

    // 안 되는 시간: "수요일은 안 돼", "목요일 2시부터 5시까지 수업", "내일 오후는 약속"
    if (day_given || time_given) && BUSY.is_match(s) {
        let day = when.day.unwrap_or(now);
        let (ds, de) = ((prefs.on("dayStart").unwrap_or(9.0) * 60.0) as i64, (prefs.on("dayEnd").unwrap_or(22.0) * 60.0) as i64);
        let (mut sm, mut em) = (ds, de);
        if let Some(st) = when.start {
            sm = st;
            em = when.end.unwrap_or(if re(r"부터|이후|넘어").is_match(s) { de } else { st + (60.0 * dur.unwrap_or(2.0)) as i64 });
        } else if s.contains("오전") {
            em = 12 * 60;
        } else if s.contains("오후") || s.contains("낮") {
            sm = 12 * 60;
            em = 18 * 60;
        } else if s.contains("저녁") || s.contains("밤") {
            sm = 18 * 60;
        }
        let note = NOTE.captures(s).map(|c| c[1].to_string()).unwrap_or_else(|| "안 되는 시간".into());
        return Cmd::Unavailable { start: at(day, sm), end: at(day, em), note };
    }

    if block.is_some() || task.is_some() {
        return Cmd::Unknown(Some("어떻게 바꿀지 알려 주세요. 예: \"2번 목요일 3시로 옮겨줘\", \"1번 빼줘\", \"자료조사 3시간이면 돼\"".into()));
    }
    Cmd::Unknown(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::ymd_hm;

    fn day(d: u32, h: u32) -> i64 {
        ymd_hm(2026, 11, d, h, 0).unwrap()
    }

    #[test]
    fn understands_common_phrases() {
        std::env::set_var("TZ", "Asia/Seoul");
        let now = day(14, 10);
        let tasks = vec![json!({ "taskId": "t1", "title": "자료조사", "need": 4 }), json!({ "taskId": "t2", "title": "발표 준비", "need": 3 })];
        let blocks = vec![
            json!({ "no": 1, "id": "x1", "taskId": "t1", "title": "자료조사", "start": day(14, 11), "end": day(14, 13) }),
            json!({ "no": 2, "id": "x2", "taskId": "t2", "title": "발표 준비", "start": day(15, 9), "end": day(15, 11) }),
        ];
        let p = json!({ "dayStart": 9, "dayEnd": 22 });
        let c = |t: &str| parse_command(t, &blocks, &tasks, &p, now);
        assert_eq!(c("2번 목요일 3시로 옮겨줘"), Cmd::Move { no: 2, start: day(19, 15), end: day(19, 17) });
        assert_eq!(c("1번 빼줘"), Cmd::Remove { no: 1 });
        assert_eq!(c("발표 준비 2시간 더 필요해"), Cmd::Hours { task_id: "t2".into(), hours: 5.0 });
        assert_eq!(c("자료조사는 3시간이면 돼"), Cmd::Hours { task_id: "t1".into(), hours: 3.0 });
        assert_eq!(c("수요일은 안 돼"), Cmd::Unavailable { start: day(18, 9), end: day(18, 22), note: "안 되는 시간".into() });
        assert_eq!(c("내일 오후 2시부터 5시까지 수업 있어"), Cmd::Unavailable { start: day(15, 14), end: day(15, 17), note: "수업".into() });
        assert_eq!(c("발표 준비 금요일 2시부터 4시까지 추가해줘"), Cmd::Add { task_id: "t2".into(), start: day(20, 14), end: day(20, 16) });
        assert_eq!(c("하루에 4시간만 할래"), Cmd::Prefs(json!({ "dailyMaxHours": 4.0 })));
        assert_eq!(c("밤 10시 이후는 안 돼"), Cmd::Prefs(json!({ "dayEnd": 22.0 })));
        assert_eq!(c("캘린더에 넣어줘"), Cmd::Apply);
        assert_eq!(c("다시 짜줘"), Cmd::Replan);
    }
}
