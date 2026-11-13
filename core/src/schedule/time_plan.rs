//! 맡은 작업을 실제 시간 블록(몇 시부터 몇 시까지)으로 배치한다.
//!
//! 기준
//!  - 하루 중 작업할 수 있는 시간대(기본 9~22시) 안에서, 캘린더의 바쁜 시간과 "안 되는 시간"을 피한다
//!  - 마감이 빠른 작업부터, 먼저 끝나야 하는 작업 뒤에 배치한다
//!  - 한 번에 최대 2시간, 블록 사이와 다른 일정 앞뒤에 15분 쉬는 시간, 하루 최대 6시간
//!  - 사용자가 직접 정한(고정한) 블록은 그대로 두고 남은 시간만 채운다
//!  - 마감 안에 다 못 넣으면 마감 뒤에 넣고 알려 준다
use crate::time::{add_days, start_of_day, DAY, HOUR, MIN};
use crate::util::{fmt_h, J};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

const STEP: i64 = 30 * MIN;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Interval {
    pub start: i64,
    pub end: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Prefs {
    pub day_start: f64,
    pub day_end: f64,
    pub max_block_hours: f64,
    pub min_block_minutes: f64,
    pub gap_minutes: f64,
    pub daily_max_hours: f64,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs { day_start: 9.0, day_end: 22.0, max_block_hours: 2.0, min_block_minutes: 30.0, gap_minutes: 15.0, daily_max_hours: 6.0 }
    }
}

impl Prefs {
    pub fn from_value(v: &Value) -> Prefs {
        let d = Prefs::default();
        let g = |k: &str, def: f64| v.on(k).filter(|x| x.is_finite()).unwrap_or(def);
        let mut p = Prefs {
            day_start: g("dayStart", d.day_start),
            day_end: g("dayEnd", d.day_end),
            max_block_hours: g("maxBlockHours", d.max_block_hours),
            min_block_minutes: g("minBlockMinutes", d.min_block_minutes),
            gap_minutes: g("gapMinutes", d.gap_minutes),
            daily_max_hours: g("dailyMaxHours", d.daily_max_hours),
        };
        p.day_start = p.day_start.clamp(0.0, 23.0).floor();
        p.day_end = p.day_end.clamp(p.day_start + 1.0, 24.0).floor();
        p.max_block_hours = p.max_block_hours.clamp(0.5, 8.0);
        p.min_block_minutes = p.min_block_minutes.clamp(15.0, 120.0);
        p.gap_minutes = p.gap_minutes.clamp(0.0, 60.0);
        p.daily_max_hours = p.daily_max_hours.clamp(0.5, 16.0);
        p
    }

    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).unwrap()
    }
}

#[derive(Clone, Debug, Default)]
pub struct Item {
    pub task_id: String,
    pub project_id: String,
    pub project_title: String,
    pub team_id: String,
    pub title: String,
    /// 남은 시간
    pub hours: f64,
    pub deadline: Option<i64>,
    pub after: Option<i64>,
    pub deps: Vec<String>,
    pub order: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub task_id: String,
    pub title: String,
    pub start: i64,
    pub end: i64,
    pub late: bool,
}

pub fn merge(mut v: Vec<Interval>) -> Vec<Interval> {
    v.retain(|i| i.end > i.start);
    v.sort_by_key(|i| i.start);
    let mut out: Vec<Interval> = vec![];
    for i in v {
        match out.last_mut() {
            Some(last) if i.start <= last.end => last.end = last.end.max(i.end),
            _ => out.push(i),
        }
    }
    out
}

/// [from,to) 에서 occupied(정렬·병합됨) 를 뺀 빈 구간
pub fn gaps(from: i64, to: i64, occupied: &[Interval]) -> Vec<Interval> {
    let mut out = vec![];
    let mut cur = from;
    for o in occupied {
        if o.end <= cur {
            continue;
        }
        if o.start >= to {
            break;
        }
        if o.start > cur {
            out.push(Interval { start: cur, end: o.start.min(to) });
        }
        cur = cur.max(o.end);
        if cur >= to {
            break;
        }
    }
    if cur < to {
        out.push(Interval { start: cur, end: to });
    }
    out
}

/// 선행 작업을 먼저, 그다음 마감이 빠른 순
pub fn order_items(items: &[Item]) -> Vec<Item> {
    let by_id: HashMap<&str, &Item> = items.iter().map(|i| (i.task_id.as_str(), i)).collect();
    let mut sorted: Vec<&Item> = items.iter().collect();
    sorted.sort_by(|a, b| a.deadline.unwrap_or(i64::MAX).cmp(&b.deadline.unwrap_or(i64::MAX)).then(a.order.cmp(&b.order)));
    let mut out = vec![];
    let mut seen = HashSet::new();
    fn visit<'a>(it: &'a Item, by_id: &HashMap<&str, &'a Item>, seen: &mut HashSet<String>, stack: &mut HashSet<String>, out: &mut Vec<Item>) {
        if seen.contains(&it.task_id) || stack.contains(&it.task_id) {
            return;
        }
        stack.insert(it.task_id.clone());
        for d in &it.deps {
            if let Some(x) = by_id.get(d.as_str()) {
                visit(x, by_id, seen, stack, out);
            }
        }
        seen.insert(it.task_id.clone());
        out.push(it.clone());
    }
    for it in sorted {
        let mut stack = HashSet::new();
        visit(it, &by_id, &mut seen, &mut stack, &mut out);
    }
    out
}

fn day_key(t: i64) -> i64 {
    start_of_day(t)
}

/// busy: 캘린더의 다른 일정 + 안 되는 시간, fixed: 이미 정해진 블록 (task_id, start, end)
pub fn plan_blocks(items: &[Item], busy: &[Interval], fixed: &[(String, i64, i64)], from: i64, horizon_days: i64, p: &Prefs) -> (Vec<Block>, Vec<Value>) {
    let gap = (p.gap_minutes * MIN as f64) as i64;
    let max_block = (p.max_block_hours * HOUR as f64) as i64;
    let min_block = (p.min_block_minutes * MIN as f64) as i64;
    let daily_max = (p.daily_max_hours * HOUR as f64) as i64;
    let start0 = (from + STEP - 1).div_euclid(STEP) * STEP;
    let until = add_days(start_of_day(start0), horizon_days);

    // 다른 일정 앞뒤로 쉬는 시간을 둔다
    let mut occupied = merge(busy.iter().copied().chain(fixed.iter().map(|f| Interval { start: f.1, end: f.2 })).map(|i| Interval { start: i.start - gap, end: i.end + gap }).collect());
    let mut used: HashMap<i64, i64> = HashMap::new();
    let mut end_of: HashMap<String, i64> = HashMap::new();
    for f in fixed {
        *used.entry(day_key(f.1)).or_default() += f.2 - f.1;
        let e = end_of.entry(f.0.clone()).or_default();
        *e = (*e).max(f.2);
    }

    let ceil_step = |t: i64| (t + STEP - 1).div_euclid(STEP) * STEP;
    let mut blocks: Vec<Block> = vec![];
    let mut warnings: Vec<Value> = vec![];
    for it in order_items(items) {
        let mut remaining = ((it.hours.max(0.0) * HOUR as f64) / STEP as f64).ceil() as i64 * STEP;
        if remaining <= 0 {
            continue;
        }
        let dep_end = it.deps.iter().filter_map(|d| end_of.get(d)).copied().max().unwrap_or(0);
        let earliest = ceil_step(start0.max(it.after.unwrap_or(0)).max(if dep_end > 0 { dep_end + gap } else { 0 }));
        let mut day = start_of_day(earliest);
        while day < until && remaining > 0 {
            let ws = earliest.max(day + (p.day_start * HOUR as f64) as i64);
            let we = day + (p.day_end * HOUR as f64) as i64;
            if we > ws {
                for g in gaps(ws, we, &occupied) {
                    let mut s = ceil_step(g.start);
                    while remaining > 0 {
                        let cap_left = daily_max - used.get(&day_key(day)).copied().unwrap_or(0);
                        let mut len = (g.end - s).min(max_block).min(remaining).min(cap_left);
                        len = len.div_euclid(STEP) * STEP;
                        if len <= 0 || len < min_block.min(remaining) {
                            break;
                        }
                        let b = Block { task_id: it.task_id.clone(), title: it.title.clone(), start: s, end: s + len, late: it.deadline.map(|d| s + len > d).unwrap_or(false) };
                        remaining -= len;
                        *used.entry(day_key(day)).or_default() += len;
                        let e = end_of.entry(it.task_id.clone()).or_default();
                        *e = (*e).max(b.end);
                        occupied.push(Interval { start: b.start - gap, end: b.end + gap });
                        occupied = merge(occupied);
                        s = ceil_step(b.end + gap);
                        blocks.push(b);
                    }
                    if remaining <= 0 {
                        break;
                    }
                }
            }
            day = add_days(day, 1);
        }
        if remaining > 0 {
            warnings.push(json!({ "taskId": it.task_id, "kind": "unplaced", "hours": remaining as f64 / HOUR as f64,
                "text": format!("\"{}\"의 {}시간은 앞으로 {}일 안에 넣을 빈 시간이 없어요.", it.title, fmt_h(remaining as f64 / HOUR as f64), horizon_days) }));
        } else if let Some(dl) = it.deadline {
            let late: i64 = blocks.iter().filter(|b| b.task_id == it.task_id && b.late).map(|b| (b.end - b.start).min(b.end - dl)).sum();
            if late > 0 {
                let h = late as f64 / HOUR as f64;
                warnings.push(json!({ "taskId": it.task_id, "kind": "late", "hours": h,
                    "text": format!("\"{}\"은 마감 전에 {}시간이 모자라 마감 뒤에도 잡혔어요. 마감·시간·담당을 조정해 주세요.", it.title, fmt_h(h)) }));
            }
        }
    }
    let _ = DAY;
    (blocks, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::ymd_hm;

    fn day(d: u32, h: u32) -> i64 {
        ymd_hm(2026, 11, d, h, 0).unwrap()
    }

    #[test]
    fn avoids_busy_and_respects_order_and_caps() {
        std::env::set_var("TZ", "Asia/Seoul");
        let now = day(14, 10);
        let items = vec![
            Item { task_id: "b".into(), title: "발표 자료".into(), hours: 3.0, deadline: Some(day(20, 23)), deps: vec!["a".into()], ..Default::default() },
            Item { task_id: "a".into(), title: "자료조사".into(), hours: 4.0, deadline: Some(day(18, 23)), ..Default::default() },
        ];
        let busy = [Interval { start: day(14, 13), end: day(14, 15) }];
        let p = Prefs { daily_max_hours: 4.0, ..Default::default() };
        let (blocks, warnings) = plan_blocks(&items, &busy, &[], now, 42, &p);
        assert!(warnings.is_empty());
        let sum = |id: &str| blocks.iter().filter(|b| b.task_id == id).map(|b| b.end - b.start).sum::<i64>();
        assert_eq!(sum("a"), 4 * HOUR);
        assert_eq!(sum("b"), 3 * HOUR);
        let a_end = blocks.iter().filter(|b| b.task_id == "a").map(|b| b.end).max().unwrap();
        assert!(blocks.iter().filter(|b| b.task_id == "b").all(|b| b.start >= a_end));
        for b in &blocks {
            assert!(b.end - b.start <= 2 * HOUR);
            assert!(!(b.start < busy[0].end + 15 * MIN && b.end > busy[0].start - 15 * MIN));
        }
        let mut per_day: HashMap<i64, i64> = HashMap::new();
        for b in &blocks {
            *per_day.entry(start_of_day(b.start)).or_default() += b.end - b.start;
        }
        assert!(per_day.values().all(|v| *v <= 4 * HOUR));
    }

    #[test]
    fn warns_when_late() {
        std::env::set_var("TZ", "Asia/Seoul");
        let items = vec![Item { task_id: "a".into(), title: "큰 일".into(), hours: 10.0, deadline: Some(day(14, 22)), ..Default::default() }];
        let (blocks, w) = plan_blocks(&items, &[], &[], day(14, 10), 42, &Prefs::default());
        assert!(blocks.iter().any(|b| b.late));
        assert_eq!(w[0]["kind"], "late");
    }
}
