//! 내 작업 시간표: 배치 계산, 사용자의 수정(옮기기·빼기·추가·시간 변경·안 되는 시간), 캘린더 반영 차이 계산.
//! 시간표는 이 기기에만 저장된다. 팀에 보이는 것은 작업 상태뿐이다.
use super::time_plan::{plan_blocks, Interval, Item, Prefs};
use crate::store::Store;
use crate::time::{fmt_range, start_of_day, HOUR};
use crate::util::{arr_mut, round1, to_base36, J};
use crate::{err, Result};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};

const KEEP_PAST_MS: i64 = 14 * 24 * HOUR;
pub const HORIZON_DAYS: i64 = 42;

static SEQ: AtomicU64 = AtomicU64::new(0);
fn new_id(now: i64) -> String {
    format!("b{}{}", to_base36(now.max(0) as u64), to_base36(SEQ.fetch_add(1, Ordering::Relaxed)))
}

/// 다시 배치할 때 필요한 바깥 정보
pub struct Ctx {
    pub now: i64,
    pub items: Vec<Item>,
    /// 캘린더의 바쁜 시간 (사이가 만든 일정 포함), 읽기 성공 여부, 오류
    pub busy: (Vec<Interval>, bool, Option<String>),
}

#[derive(Clone)]
pub struct Book {
    pub store: Store,
}

fn ov(v: &Value, k: &str) -> Interval {
    Interval { start: v.i(&format!("{k}start")), end: v.i(&format!("{k}end")) }
}

fn need_hours(st: &Value, it: &Item) -> f64 {
    match st["hours"].get(&it.task_id).and_then(|v| v.as_f64()) {
        Some(h) => h,
        None => it.hours,
    }
}

impl Book {
    pub fn new(store: Store) -> Book {
        store.update(|s| {
            for (k, d) in [("prefs", Prefs::default().to_value()), ("unavailable", json!([])), ("hours", json!({})), ("blocks", json!([])), ("applied", json!({})), ("warnings", json!([])), ("chat", json!([]))] {
                if s.get(k).map(|v| v.is_null()).unwrap_or(true) {
                    s[k] = d;
                }
            }
        });
        Book { store }
    }

    pub fn state(&self) -> Value {
        self.store.get()
    }

    pub fn update<R>(&self, f: impl FnOnce(&mut Value) -> R) -> R {
        self.store.update(f)
    }

    /// 사이가 캘린더에 넣은 일정의 시간 (바쁜 시간에서 뺄 것)
    pub fn own_events(&self) -> Vec<Interval> {
        self.state().obj("applied").values().map(|a| ov(a, "")).collect()
    }

    /// 맡은 작업 기준으로 다시 배치한다. 지난 블록과 직접 정한 블록은 그대로 둔다.
    pub fn replan(&self, ctx: &Ctx) {
        let now = ctx.now;
        let st = self.state();
        let prefs = Prefs::from_value(&st["prefs"]);
        let ids: HashSet<&str> = ctx.items.iter().map(|i| i.task_id.as_str()).collect();
        let blocks = st.arr("blocks");

        // 지난 블록(기록)과, 아직 맡고 있는 작업의 고정 블록·진행 중인 블록은 남긴다
        let past: Vec<Value> = blocks.iter().filter(|b| b.i("end") <= now && b.i("end") > now - KEEP_PAST_MS).cloned().collect();
        let keep: Vec<Value> = blocks
            .iter()
            .filter(|b| b.i("end") > now && ids.contains(b.s("taskId").as_str()) && (b.b("pinned") || b.i("start") <= now))
            .cloned()
            .collect();
        let keep_ids: HashSet<String> = keep.iter().map(|b| b.s("id")).collect();

        let mut spent: HashMap<String, f64> = HashMap::new();
        for b in past.iter().chain(keep.iter()) {
            if ids.contains(b.s("taskId").as_str()) {
                *spent.entry(b.s("taskId")).or_default() += (b.i("end") - b.i("start")) as f64 / HOUR as f64;
            }
        }

        let own = self.own_events();
        let busy = super::subtract(&ctx.busy.0, &own);
        let unavailable: Vec<Interval> = st.arr("unavailable").iter().filter(|u| u.i("end") > now).map(|u| ov(u, "")).collect();
        let plan_items: Vec<Item> = ctx.items.iter().map(|i| Item { hours: (need_hours(&st, i) - spent.get(&i.task_id).copied().unwrap_or(0.0)).max(0.0), ..i.clone() }).collect();
        let fixed: Vec<(String, i64, i64)> = keep.iter().map(|b| (b.s("taskId"), b.i("start"), b.i("end"))).collect();
        let all_busy: Vec<Interval> = busy.into_iter().chain(unavailable).collect();
        let (planned, warnings) = plan_blocks(&plan_items, &all_busy, &fixed, now, HORIZON_DAYS, &prefs);

        // 같은 작업의 예전 블록 id 를 이어 받는다 → 캘린더에서는 새로 만들지 않고 시간만 고친다
        let mut reuse: HashMap<String, Vec<String>> = HashMap::new();
        let mut old: Vec<&Value> = blocks.iter().filter(|b| b.i("end") > now && !keep_ids.contains(&b.s("id"))).collect();
        old.sort_by_key(|b| b.i("start"));
        for b in old {
            reuse.entry(b.s("taskId")).or_default().push(b.s("id"));
        }
        let meta: HashMap<&str, &Item> = ctx.items.iter().map(|i| (i.task_id.as_str(), i)).collect();
        let mut fresh: Vec<Value> = vec![];
        for b in planned {
            let ids = reuse.entry(b.task_id.clone()).or_default();
            let id = if ids.is_empty() { new_id(now) } else { ids.remove(0) };
            let m = meta.get(b.task_id.as_str());
            fresh.push(json!({
                "id": id, "taskId": b.task_id, "projectId": m.map(|m| m.project_id.clone()), "teamId": m.map(|m| m.team_id.clone()),
                "title": b.title, "start": b.start, "end": b.end, "pinned": false, "late": b.late,
            }));
        }
        let keep: Vec<Value> = keep
            .into_iter()
            .map(|mut b| {
                if let Some(m) = meta.get(b.s("taskId").as_str()) {
                    b["title"] = json!(m.title);
                }
                b
            })
            .collect();

        let mut all: Vec<Value> = past.into_iter().chain(keep).chain(fresh).collect();
        all.sort_by_key(|b| b.i("start"));
        let mut w: Vec<Value> = vec![];
        if !ctx.busy.1 {
            w.push(json!({ "kind": "calendar", "text": format!("캘린더를 읽지 못해 다른 일정은 피하지 못했어요 ({}). 설정 → 외부 연결에서 Google 로그인을 확인해 주세요.", ctx.busy.2.clone().unwrap_or_else(|| "연결 안 됨".into())) }));
        }
        w.extend(warnings);
        let cal_ok = ctx.busy.1;
        self.update(|s| {
            s["blocks"] = json!(all);
            s["warnings"] = json!(w);
            s["calendarNote"] = if cal_ok { Value::Null } else { json!("no-calendar") };
            s["lastPlanAt"] = json!(now);
        });
    }

    // ---------- 사용자의 수정 ----------
    pub fn upcoming(&self, now: i64) -> Vec<Value> {
        let mut v: Vec<Value> = self.state().arr("blocks").into_iter().filter(|b| b.i("end") > now).collect();
        v.sort_by_key(|b| b.i("start"));
        v
    }

    /// "2번" 같은 번호(오늘 이후 블록 순서, 1부터) 또는 id 로 블록을 찾는다
    pub fn find_block(&self, r: &Value, now: i64) -> Option<Value> {
        let list = self.upcoming(now);
        if let Some(n) = r.as_u64().or_else(|| r.as_str().and_then(|s| s.parse().ok())) {
            return list.get((n as usize).wrapping_sub(1)).cloned();
        }
        let id = r.as_str()?;
        self.state().arr("blocks").into_iter().find(|b| b.s("id") == id)
    }

    pub fn move_block(&self, ctx: &Ctx, r: &Value, start: i64, end: i64) -> Result<Value> {
        let Some(b) = self.find_block(r, ctx.now) else { return err("그 번호의 일정을 찾지 못했어요.") };
        let end = if end > start { end } else { start + (b.i("end") - b.i("start")) };
        if start < ctx.now - 5 * 60_000 {
            return err("지난 시간으로는 옮길 수 없어요.");
        }
        let id = b.s("id");
        self.update(|s| {
            for x in arr_mut(s, "blocks") {
                if x.s("id") == id {
                    x["start"] = json!(start);
                    x["end"] = json!(end);
                    x["pinned"] = json!(true);
                    x["late"] = json!(false);
                }
            }
        });
        self.replan(ctx);
        Ok(self.state().arr("blocks").into_iter().find(|x| x.s("id") == id).unwrap_or(b))
    }

    /// 블록을 빼면 그 시간은 비워 두고, 작업 시간은 다른 빈 시간으로 옮긴다
    pub fn remove_block(&self, ctx: &Ctx, r: &Value) -> Result<Value> {
        let Some(b) = self.find_block(r, ctx.now) else { return err("그 번호의 일정을 찾지 못했어요.") };
        let id = b.s("id");
        let un = json!({ "id": new_id(ctx.now), "start": b.i("start"), "end": b.i("end"), "note": format!("{} 뺀 시간", b.s("title")) });
        self.update(|s| {
            arr_mut(s, "blocks").retain(|x| x.s("id") != id);
            arr_mut(s, "unavailable").push(un);
        });
        self.replan(ctx);
        Ok(b)
    }

    pub fn add_block(&self, ctx: &Ctx, task_id: &str, start: i64, end: i64) -> Result<()> {
        let Some(it) = ctx.items.iter().find(|i| i.task_id == task_id) else { return err("내가 맡은 작업 중에서 찾지 못했어요.") };
        if end <= start {
            return err("시작과 끝 시간을 알려 주세요.");
        }
        let b = json!({ "id": new_id(ctx.now), "taskId": task_id, "projectId": it.project_id, "teamId": it.team_id, "title": it.title, "start": start, "end": end, "pinned": true, "late": false });
        // 전체 필요 시간은 그대로라 다른 블록이 그만큼 줄어든다
        self.update(|s| arr_mut(s, "blocks").push(b));
        self.replan(ctx);
        Ok(())
    }

    pub fn set_hours(&self, ctx: &Ctx, task_id: &str, hours: f64) -> Result<()> {
        if !hours.is_finite() {
            return err("시간을 숫자로 알려 주세요.");
        }
        let h = hours.clamp(0.0, 200.0);
        self.update(|s| {
            if !s["hours"].is_object() {
                s["hours"] = json!({});
            }
            s["hours"][task_id] = json!(h);
        });
        self.replan(ctx);
        Ok(())
    }

    pub fn add_unavailable(&self, ctx: &Ctx, start: i64, end: i64, note: &str) -> Result<()> {
        if end <= start {
            return err("언제부터 언제까지인지 알려 주세요.");
        }
        let now = ctx.now;
        let u = json!({ "id": new_id(now), "start": start, "end": end, "note": note.chars().take(40).collect::<String>() });
        self.update(|s| {
            arr_mut(s, "unavailable").push(u);
            // 그 시간에 잡혀 있던 (앞으로의) 블록은 다시 배치한다
            arr_mut(s, "blocks").retain(|b| !(b.i("start") < end && b.i("end") > start && b.i("start") > now));
        });
        self.replan(ctx);
        Ok(())
    }

    pub fn remove_unavailable(&self, ctx: &Ctx, id: &str) {
        self.update(|s| arr_mut(s, "unavailable").retain(|u| u.s("id") != id));
        self.replan(ctx);
    }

    pub fn set_prefs(&self, ctx: &Ctx, patch: &Value) {
        self.update(|s| {
            let mut p = s["prefs"].clone();
            if let Some(o) = patch.as_object() {
                for (k, v) in o {
                    p[k] = v.clone();
                }
            }
            s["prefs"] = Prefs::from_value(&p).to_value();
        });
        self.replan(ctx);
    }

    /// 직접 정한 블록을 모두 풀고 처음부터 다시
    pub fn reset(&self, ctx: &Ctx) {
        self.update(|s| {
            for b in arr_mut(s, "blocks") {
                b["pinned"] = json!(false);
            }
            arr_mut(s, "unavailable").retain(|u| !u.s("note").ends_with("뺀 시간"));
        });
        self.replan(ctx);
    }

    // ---------- 캘린더 반영 ----------
    /// 캘린더와 다른 점: (만들 것, 고칠 것, 지울 것 [(blockId, eventId)])
    pub fn diff(&self, now: i64) -> (Vec<Value>, Vec<Value>, Vec<(String, String)>) {
        let st = self.state();
        let applied = st.obj("applied");
        let future: Vec<Value> = st.arr("blocks").into_iter().filter(|b| b.i("end") > now).collect();
        let ids: HashSet<String> = future.iter().map(|b| b.s("id")).collect();
        let create: Vec<Value> = future.iter().filter(|b| !applied.contains_key(&b.s("id"))).cloned().collect();
        let patch: Vec<Value> = future
            .iter()
            .filter(|b| applied.get(&b.s("id")).map(|a| a.i("start") != b.i("start") || a.i("end") != b.i("end") || a.s("title") != b.s("title")).unwrap_or(false))
            .cloned()
            .collect();
        let remove: Vec<(String, String)> = applied.iter().filter(|(id, a)| !ids.contains(*id) && a.i("end") > now).map(|(id, a)| (id.clone(), a.s("eventId"))).collect();
        (create, patch, remove)
    }

    pub fn record_applied(&self, block: &Value, event_id: &str) {
        let id = block.s("id");
        let rec = json!({ "eventId": event_id, "start": block.i("start"), "end": block.i("end"), "title": block.s("title") });
        self.update(|s| {
            if !s["applied"].is_object() {
                s["applied"] = json!({});
            }
            s["applied"][&id] = rec;
        });
    }

    pub fn forget_applied(&self, block_id: &str) {
        self.update(|s| {
            if let Some(o) = s["applied"].as_object_mut() {
                o.remove(block_id);
            }
        });
    }

    pub fn applied_event(&self, block_id: &str) -> Option<String> {
        self.state()["applied"].get(block_id).map(|a| a.s("eventId"))
    }

    /// 오래된 기록 정리
    pub fn prune(&self, now: i64) {
        self.update(|s| {
            if let Some(o) = s["applied"].as_object_mut() {
                o.retain(|_, a| a.i("end") > now - KEEP_PAST_MS);
            }
            arr_mut(s, "unavailable").retain(|u| u.i("end") > now);
        });
    }

    // ---------- 보기 ----------
    pub fn view(&self, now: i64, items: &[Item]) -> Value {
        let st = self.state();
        let applied = st.obj("applied");
        let mut placed: HashMap<String, f64> = HashMap::new();
        for b in st.arr("blocks") {
            *placed.entry(b.s("taskId")).or_default() += (b.i("end") - b.i("start")) as f64 / HOUR as f64;
        }
        let (c, p, r) = self.diff(now);
        let blocks: Vec<Value> = self
            .upcoming(now)
            .into_iter()
            .enumerate()
            .map(|(i, mut b)| {
                let a = applied.get(&b.s("id"));
                b["no"] = json!(i + 1);
                b["label"] = json!(fmt_range(b.i("start"), b.i("end")));
                b["synced"] = json!(a.map(|a| a.i("start") == b.i("start") && a.i("end") == b.i("end")).unwrap_or(false));
                b["running"] = json!(b.i("start") <= now);
                b
            })
            .collect();
        json!({
            "prefs": Prefs::from_value(&st["prefs"]).to_value(),
            "blocks": blocks,
            "tasks": items.iter().map(|i| json!({
                "taskId": i.task_id, "title": i.title, "project": i.project_title,
                "need": round1(need_hours(&st, i)), "placed": round1(placed.get(&i.task_id).copied().unwrap_or(0.0)),
                "deadline": i.deadline, "custom": st["hours"].get(&i.task_id).is_some(),
            })).collect::<Vec<_>>(),
            "unavailable": st.arr("unavailable").into_iter().filter(|u| u.i("end") > now).map(|mut u| {
                u["label"] = json!(fmt_range(u.i("start"), u.i("end")));
                u
            }).collect::<Vec<_>>(),
            "warnings": st.arr("warnings"),
            "pending": c.len() + p.len() + r.len(),
            "pendingDetail": { "create": c.len(), "patch": p.len(), "remove": r.len() },
            "lastPlanAt": st.i("lastPlanAt"),
            "calendarNote": st["calendarNote"],
        })
    }
}

#[allow(dead_code)]
fn _day(t: i64) -> i64 {
    start_of_day(t)
}
