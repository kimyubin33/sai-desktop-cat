//! 시각 도우미. 시각은 모두 1970년부터의 밀리초(i64)로 다루고, 날짜 계산은 이 컴퓨터의 현지 시각으로 한다.
use chrono::{DateTime, Datelike, Local, LocalResult, NaiveDate, TimeZone, Timelike};

pub const MIN: i64 = 60_000;
pub const HOUR: i64 = 60 * MIN;
pub const DAY: i64 = 24 * HOUR;

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

pub fn local(ms: i64) -> DateTime<Local> {
    match Local.timestamp_millis_opt(ms) {
        LocalResult::Single(d) => d,
        LocalResult::Ambiguous(a, _) => a,
        LocalResult::None => Local.timestamp_millis_opt(ms + HOUR).earliest().unwrap(),
    }
}

/// 현지 시각 y-m-d h:mi → ms (없는 날짜면 None). month 는 1부터
pub fn ymd_hm(y: i32, m: u32, d: u32, h: u32, mi: u32) -> Option<i64> {
    let date = NaiveDate::from_ymd_opt(y, m, d)?;
    let dt = date.and_hms_opt(h.min(23), mi.min(59), 0)?;
    let extra = if h >= 24 { (h - 23) as i64 * HOUR } else { 0 };
    match Local.from_local_datetime(&dt) {
        LocalResult::Single(t) => Some(t.timestamp_millis() + extra),
        LocalResult::Ambiguous(a, _) => Some(a.timestamp_millis() + extra),
        LocalResult::None => Some(Local.from_local_datetime(&(dt + chrono::Duration::hours(1))).earliest()?.timestamp_millis() + extra),
    }
}

/// JS 의 new Date(y, m0, d) 처럼 월·일이 넘쳐도 다음 달로 넘긴다 (m0 는 0부터)
pub fn ymd_overflow(y: i32, m0: i32, d: i32) -> i64 {
    let total_m = y * 12 + m0;
    let (yy, mm) = (total_m.div_euclid(12), total_m.rem_euclid(12) as u32 + 1);
    let first = ymd_hm(yy, mm, 1, 0, 0).unwrap_or(0);
    add_days(first, (d - 1) as i64)
}

pub fn start_of_day(ms: i64) -> i64 {
    let d = local(ms);
    ymd_hm(d.year(), d.month(), d.day(), 0, 0).unwrap_or(ms)
}

/// 날짜를 n 일 옮긴다 (일광 절약 시간에도 같은 시각 유지)
pub fn add_days(day_ms: i64, n: i64) -> i64 {
    let d = local(day_ms);
    let nd = d.date_naive() + chrono::Duration::days(n);
    ymd_hm(nd.year(), nd.month(), nd.day(), d.hour(), d.minute()).unwrap_or(day_ms + n * DAY)
}

pub fn weekday(ms: i64) -> u32 {
    local(ms).weekday().num_days_from_sunday()
}

pub fn hour(ms: i64) -> u32 {
    local(ms).hour()
}

pub fn minute_of_day(ms: i64) -> i64 {
    let d = local(ms);
    (d.hour() * 60 + d.minute()) as i64
}

pub fn same_day(a: i64, b: i64) -> bool {
    local(a).date_naive() == local(b).date_naive()
}

pub const WEEKDAYS: [&str; 7] = ["일", "월", "화", "수", "목", "금", "토"];

/// "YYYY-MM-DD" → 그날 23:59 (마감일은 하루 끝까지로 본다)
pub fn parse_deadline(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.len() != 10 {
        return None;
    }
    let y: i32 = s.get(0..4)?.parse().ok()?;
    if s.as_bytes()[4] != b'-' || s.as_bytes()[7] != b'-' {
        return None;
    }
    let m: u32 = s.get(5..7)?.parse().ok()?;
    let d: u32 = s.get(8..10)?.parse().ok()?;
    ymd_hm(y, m, d, 23, 59)
}

/// ISO 주차 (예: 2026-W46) — UTC 기준 (예전 형식과 같게)
pub fn iso_week(ms: i64) -> String {
    let d = chrono::DateTime::from_timestamp_millis(ms).unwrap_or_default();
    let w = d.iso_week();
    format!("{}-W{:02}", w.year(), w.week())
}

pub fn fmt_time(ms: i64) -> String {
    let d = local(ms);
    format!("{:02}:{:02}", d.hour(), d.minute())
}

pub fn fmt_day(ms: i64) -> String {
    let d = local(ms);
    format!("{}/{}({})", d.month(), d.day(), WEEKDAYS[d.weekday().num_days_from_sunday() as usize])
}

pub fn fmt_range(start: i64, end: i64) -> String {
    format!("{} {}–{}", fmt_day(start), fmt_time(start), fmt_time(end))
}

pub fn fmt_month_day(ms: i64) -> String {
    let d = local(ms);
    format!("{}월 {}일", d.month(), d.day())
}

/// "2026-11-15T14:00" 같은 현지 시각 문자열 → ms
pub fn parse_local_iso(s: &str) -> Option<i64> {
    let re = regex::Regex::new(r"^(\d{4})-(\d{2})-(\d{2})[T ](\d{1,2}):(\d{2})").ok()?;
    let c = re.captures(s.trim())?;
    ymd_hm(c[1].parse().ok()?, c[2].parse().ok()?, c[3].parse().ok()?, c[4].parse().ok()?, c[5].parse().ok()?)
}

pub fn to_local_iso(ms: i64) -> String {
    let d = local(ms);
    format!("{:04}-{:02}-{:02}T{:02}:{:02}", d.year(), d.month(), d.day(), d.hour(), d.minute())
}

pub fn to_ymd(ms: i64) -> String {
    let d = local(ms);
    format!("{:04}-{:02}-{:02}", d.year(), d.month(), d.day())
}

/// RFC3339 (Google API 용)
pub fn to_rfc3339(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms).unwrap_or_default().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

pub fn parse_rfc3339(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s).ok().map(|d| d.timestamp_millis())
}
