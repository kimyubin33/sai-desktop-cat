//! 한국어 날짜·시간 표현을 읽는다. 예: "내일 오후 3시", "목요일 2시 반부터 4시까지", "11월 20일", "15:30~17:00", "2시간 반"
//! 모든 계산은 이 컴퓨터의 현지 시각 기준이다.
use crate::time::{add_days, local, start_of_day, weekday, ymd_hm, ymd_overflow};
use chrono::Datelike;
use once_cell::sync::Lazy;
use regex::Regex;

const WD: &str = "일월화수목금토";

static MD: Lazy<Regex> = Lazy::new(|| Regex::new(r"(\d{1,2})\s*월\s*(\d{1,2})\s*일").unwrap());
static SLASH: Lazy<Regex> = Lazy::new(|| Regex::new(r"(\d{1,2})/(\d{1,2})").unwrap());
static ISO: Lazy<Regex> = Lazy::new(|| Regex::new(r"(\d{4})-(\d{2})-(\d{2})").unwrap());
static WEEK: Lazy<Regex> = Lazy::new(|| Regex::new(r"(이번\s*주|다다음\s*주|다음\s*주|담주)?\s*([일월화수목금토])\s*요일").unwrap());
static DAYN: Lazy<Regex> = Lazy::new(|| Regex::new(r"(\d{1,2})\s*일").unwrap());
static TIME: Lazy<Regex> = Lazy::new(|| Regex::new(r"(오전|오후|아침|낮|저녁|밤|새벽)?\s*(\d{1,2})\s*(?:시\s*(반|(\d{1,2})\s*분)?|:(\d{2}))").unwrap());
static DUR_H: Lazy<Regex> = Lazy::new(|| Regex::new(r"(\d+(?:\.\d+)?)\s*시간\s*(반)?(?:\s*(\d{1,2})\s*분)?").unwrap());
static DUR_M: Lazy<Regex> = Lazy::new(|| Regex::new(r"(\d{1,3})\s*분").unwrap());
static HM_MIN: Lazy<Regex> = Lazy::new(|| Regex::new(r"시\s*\d{1,2}\s*분").unwrap());

fn prev_char(s: &str, i: usize) -> Option<char> {
    s[..i].chars().next_back()
}

fn next_char(s: &str, i: usize) -> Option<char> {
    s[i..].chars().next()
}

/// 날짜 하나를 찾는다 (그날 0시)
pub fn parse_day(s: &str, now: i64) -> Option<i64> {
    let today = start_of_day(now);
    if s.contains("오늘") {
        return Some(today);
    }
    if s.contains("내일") {
        return Some(add_days(today, 1));
    }
    if s.contains("모레") {
        return Some(add_days(today, 2));
    }
    if s.contains("글피") {
        return Some(add_days(today, 3));
    }
    let year = local(now).year();
    let md = MD.captures(s).map(|c| (c[1].parse::<i32>().unwrap(), c[2].parse::<i32>().unwrap())).or_else(|| {
        SLASH.captures_iter(s).find_map(|c| {
            let m = c.get(0).unwrap();
            let bad = |ch: Option<char>| ch.map(|c| c.is_ascii_digit() || c == ':').unwrap_or(false);
            (!bad(prev_char(s, m.start())) && !bad(next_char(s, m.end()))).then(|| (c[1].parse().unwrap(), c[2].parse().unwrap()))
        })
    });
    if let Some((mo, d)) = md {
        let mut t = ymd_overflow(year, mo - 1, d);
        if t < add_days(today, -30) {
            t = ymd_overflow(year + 1, mo - 1, d);
        }
        return Some(t);
    }
    if let Some(c) = ISO.captures(s) {
        return ymd_hm(c[1].parse().ok()?, c[2].parse().ok()?, c[3].parse().ok()?, 0, 0);
    }
    if let Some(c) = WEEK.captures(s) {
        let want = WD.chars().position(|x| c[2].starts_with(x)).unwrap() as i64;
        let cur = weekday(today) as i64;
        let which: String = c.get(1).map(|m| m.as_str().chars().filter(|c| !c.is_whitespace()).collect()).unwrap_or_default();
        return Some(match which.as_str() {
            // 이번 주(월요일 시작) 안의 그 요일
            "이번주" => add_days(add_days(today, -((cur + 6) % 7)), (want + 6) % 7),
            "다음주" | "담주" => add_days(add_days(today, 7 - ((cur + 6) % 7)), (want + 6) % 7),
            "다다음주" => add_days(add_days(today, 14 - ((cur + 6) % 7)), (want + 6) % 7),
            _ => add_days(today, (want - cur + 7) % 7),
        });
    }
    for c in DAYN.captures_iter(s) {
        let m = c.get(0).unwrap();
        if prev_char(s, m.start()).map(|ch| ch.is_ascii_digit() || ch == '월').unwrap_or(false) {
            continue;
        }
        let after = s[m.end()..].trim_start();
        if after.starts_with("동안") || after.starts_with('간') {
            continue;
        }
        // "1일차" 같은 표현도 날짜로 보되, "시간·분"이 바로 붙으면 아니다
        if s[m.start()..].chars().take(m.as_str().chars().count() + 1).collect::<String>().contains("시간") {
            continue;
        }
        let n: i32 = c[1].parse().ok()?;
        if (1..=31).contains(&n) {
            let d0 = local(today);
            let mut t = ymd_overflow(d0.year(), d0.month0() as i32, n);
            if t < today {
                t = ymd_overflow(d0.year(), d0.month0() as i32 + 1, n);
            }
            return Some(t);
        }
    }
    None
}

#[derive(Debug, Clone)]
pub struct FoundTime {
    pub minutes: i64,
    pub has_period: bool,
    pub colon: bool,
}

fn to_minutes(period: Option<&str>, h: i64, half: bool, min: Option<i64>, colon_min: Option<i64>) -> i64 {
    let mut hour = h;
    let minute = if half { 30 } else { min.or(colon_min).unwrap_or(0) };
    match period {
        Some("오후") | Some("저녁") | Some("밤") => {
            if hour < 12 {
                hour += 12
            }
        }
        Some("오전") | Some("아침") | Some("새벽") => {
            if hour == 12 {
                hour = 0
            }
        }
        Some("낮") => {
            if hour < 6 {
                hour += 12
            }
        }
        // "3시" 처럼 오전·오후가 없으면 낮 시간으로 본다 (새벽 3시에 작업을 잡는 일은 드물다)
        _ => {
            if colon_min.is_none() && (1..=7).contains(&hour) {
                hour += 12
            }
        }
    }
    hour * 60 + minute
}

/// 시각들을 찾는다
pub fn find_times(s: &str) -> Vec<FoundTime> {
    let mut out = vec![];
    for c in TIME.captures_iter(s) {
        let m = c.get(0).unwrap();
        // "2시간" 은 시각이 아니다
        if s[m.end()..].starts_with('간') {
            continue;
        }
        let h: i64 = c[2].parse().unwrap_or(99);
        if h > 24 {
            continue;
        }
        let colon = c.get(5).map(|x| x.as_str().parse::<i64>().unwrap_or(0));
        if colon.is_some() && h > 23 {
            continue;
        }
        let period = c.get(1).map(|x| x.as_str());
        out.push(FoundTime {
            minutes: to_minutes(period, h, c.get(3).map(|x| x.as_str() == "반").unwrap_or(false), c.get(4).and_then(|x| x.as_str().parse().ok()), colon),
            has_period: period.is_some(),
            colon: colon.is_some(),
        });
    }
    out
}

/// "N시간 (반)", "N분", "N.5시간"
pub fn parse_duration(s: &str) -> Option<f64> {
    if let Some(c) = DUR_H.captures(s) {
        let mut h: f64 = c[1].parse().ok()?;
        if c.get(2).is_some() {
            h += 0.5;
        }
        if let Some(mm) = c.get(3) {
            h += mm.as_str().parse::<f64>().ok()? / 60.0;
        }
        return Some(h);
    }
    if let Some(c) = DUR_M.captures(s) {
        if !HM_MIN.is_match(s) {
            return Some(c[1].parse::<f64>().ok()? / 60.0);
        }
    }
    None
}

#[derive(Debug, Clone, PartialEq)]
pub struct When {
    pub day: Option<i64>,
    pub start: Option<i64>,
    pub end: Option<i64>,
    pub duration: Option<f64>,
}

/// 한 문장에서 날짜·시작·끝·길이를 읽는다. start/end 는 그날 0시부터의 분
pub fn parse_when(s: &str, now: i64) -> When {
    let day = parse_day(s, now);
    let times = find_times(s);
    let mut start = None;
    let mut end = None;
    if let Some(t0) = times.first() {
        start = Some(t0.minutes);
        if let Some(t) = times.get(1) {
            let mut e = t.minutes;
            // "오후 2시부터 4시까지": 뒤 시각에 오전/오후가 없으면 앞 시각을 따른다
            if !t.has_period && !t.colon && e <= t0.minutes && e + 12 * 60 > t0.minutes && e < 12 * 60 {
                e += 12 * 60;
            }
            end = Some(e);
        }
    }
    let duration = parse_duration(s);
    if let (Some(st), None, Some(d)) = (start, end, duration) {
        end = Some(st + (d * 60.0).round() as i64);
    }
    When { day, start, end, duration }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(d: u32, h: u32) -> i64 {
        ymd_hm(2026, 11, d, h, 0).unwrap()
    }

    #[test]
    fn reads_korean_dates_and_times() {
        std::env::set_var("TZ", "Asia/Seoul");
        let now = ymd_hm(2026, 11, 14, 10, 0).unwrap(); // 토요일
        assert_eq!(parse_day("내일", now), Some(day(15, 0)));
        assert_eq!(parse_day("목요일", now), Some(day(19, 0)));
        assert_eq!(parse_day("다음 주 월요일", now), Some(day(16, 0)));
        assert_eq!(parse_day("11월 20일", now), Some(day(20, 0)));
        let w = parse_when("목요일 오후 2시부터 4시까지", now);
        assert_eq!((w.day, w.start, w.end), (Some(day(19, 0)), Some(14 * 60), Some(16 * 60)));
        let w = parse_when("내일 3시 반", now);
        assert_eq!((w.day, w.start), (Some(day(15, 0)), Some(15 * 60 + 30)));
        let w = parse_when("15:00~17:30", now);
        assert_eq!((w.start, w.end), (Some(15 * 60), Some(17 * 60 + 30)));
        let w = parse_when("오전 10시에 2시간", now);
        assert_eq!((w.start, w.end), (Some(600), Some(720)));
        assert_eq!(parse_when("3시간 걸려", now).start, None);
        assert_eq!(parse_duration("2시간 반"), Some(2.5));
    }
}
