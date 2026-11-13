//! 내 작업 시간표: 맡은 작업 모으기, Google 캘린더 바쁜 시간 읽기, 캘린더·할 일 반영,
//! 그리고 대화로 고치기 (Claude API 키가 있으면 AI가 도구로 고치고, 없으면 규칙으로 읽는다).
pub mod book;
pub mod commands;
pub mod korean_time;
pub mod time_plan;

use crate::ai::{call_messages, Ai};
use crate::google::Google;
use crate::team::Hub;
use crate::time::{fmt_range, now_ms, parse_deadline, parse_local_iso, to_local_iso, DAY, WEEKDAYS};
use crate::util::{arr_mut, round1, J};
use crate::{err, Result};
use book::{Book, Ctx, HORIZON_DAYS};
use commands::{parse_command, Cmd};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use time_plan::{Interval, Item};

const MAX_TURNS: usize = 30;
const BUSY_CACHE_MS: i64 = 2 * 60 * 1000;

pub fn subtract(intervals: &[Interval], cuts: &[Interval]) -> Vec<Interval> {
    let mut out: Vec<Interval> = intervals.to_vec();
    for c in cuts {
        let mut next = vec![];
        for i in out {
            if c.end <= i.start || c.start >= i.end {
                next.push(i);
            } else {
                if c.start > i.start {
                    next.push(Interval { start: i.start, end: c.start });
                }
                if c.end < i.end {
                    next.push(Interval { start: c.end, end: i.end });
                }
            }
        }
        out = next;
    }
    out
}

#[derive(Clone)]
pub struct Schedule(Arc<Inner>);

struct Inner {
    book: Book,
    hub: Hub,
    google: Google,
    ai: Arc<Ai>,
    busy_cache: Mutex<Option<(i64, i64, i64, (Vec<Interval>, bool, Option<String>))>>,
    /// 한 번에 하나씩 고친다 (대화와 화면 버튼이 동시에 와도 꼬이지 않게)
    lock: tokio::sync::Mutex<()>,
}

impl Schedule {
    pub fn new(book: Book, hub: Hub, google: Google, ai: Arc<Ai>) -> Schedule {
        Schedule(Arc::new(Inner { book, hub, google, ai, busy_cache: Mutex::new(None), lock: tokio::sync::Mutex::new(()) }))
    }

    pub fn book(&self) -> &Book {
        &self.0.book
    }

    /// 내가 맡은(수락한) 끝나지 않은 작업
    pub fn collect_items(&self) -> Vec<Item> {
        let mut out = vec![];
        let mut order = 0;
        for t in self.0.hub.all().into_iter().filter(|t| t.is_connected()) {
            let me = t.my_uid();
            let team_id = t.id.clone().unwrap_or_default();
            for p in t.collab_data().arr("projects") {
                let pid = p.s("id");
                let tasks = t.tasks_of(&pid);
                let mine: Vec<&Value> = tasks.iter().filter(|x| x.s("assignee") == me && x.b("accepted") && x.s("status") != "done").collect();
                if mine.is_empty() {
                    continue;
                }
                let plan = t.plan(&pid);
                let asg: HashMap<String, Value> = plan.map(|p| p.arr("assignments").into_iter().map(|a| (a.s("taskId"), a)).collect()).unwrap_or_default();
                let by_id: HashMap<String, &Value> = tasks.iter().map(|x| (x.s("id"), x)).collect();
                for x in mine {
                    let a = asg.get(&x.s("id"));
                    let open_deps: Vec<String> = x.strs("dependsOn").into_iter().filter(|d| by_id.get(d).map(|d| d.s("status") != "done").unwrap_or(false)).collect();
                    // 다른 사람이 맡은 선행 작업이 남아 있으면, 팀 일정 추정상 시작 가능한 때부터
                    let others = open_deps.iter().any(|d| by_id.get(d).map(|d| d.s("assignee") != me || !d.b("accepted")).unwrap_or(false));
                    let eff = a.map(|a| a.n("effectiveHours")).filter(|h| *h > 0.0);
                    out.push(Item {
                        task_id: x.s("id"),
                        project_id: pid.clone(),
                        project_title: p.s("title"),
                        team_id: team_id.clone(),
                        title: x.s("title"),
                        hours: eff.unwrap_or(if x.n("estimateHours") > 0.0 { x.n("estimateHours") } else { 1.0 }),
                        deadline: parse_deadline(&p.s("deadline")),
                        after: if others { a.and_then(|a| a.on("startAt")).map(|v| v as i64) } else { None },
                        deps: open_deps,
                        order,
                    });
                    order += 1;
                }
            }
        }
        out
    }

    async fn busy(&self, now: i64, fresh: bool) -> (Vec<Interval>, bool, Option<String>) {
        let to = crate::time::start_of_day(now) + HORIZON_DAYS * DAY;
        if !fresh {
            if let Some((from, until, at, res)) = self.0.busy_cache.lock().unwrap().clone() {
                if from <= now && until >= to && now - at < BUSY_CACHE_MS {
                    return res;
                }
            }
        }
        let res = self.0.google.busy_between(now, to).await;
        *self.0.busy_cache.lock().unwrap() = Some((now, to, now, res.clone()));
        res
    }

    async fn ctx(&self, fresh: bool) -> Ctx {
        let now = now_ms();
        Ctx { now, items: self.collect_items(), busy: self.busy(now, fresh).await }
    }

    fn sig(items: &[Item]) -> String {
        items.iter().map(|i| format!("{}|{}|{}|{:?}|{:?}", i.task_id, i.title, i.hours, i.deadline, i.deps)).collect::<Vec<_>>().join(";")
    }

    fn decorate(&self, mut v: Value) -> Value {
        let g = self.0.google.status();
        v["google"] = json!({ "ready": self.0.google.is_ready(), "configured": g["configured"], "email": g["email"] });
        v["ai"] = json!(self.0.ai.has_key());
        let chat = self.0.book.state().arr("chat");
        v["chat"] = json!(chat[chat.len().saturating_sub(MAX_TURNS)..].to_vec());
        v
    }

    fn view_now(&self) -> Value {
        let items = self.collect_items();
        self.decorate(self.0.book.view(now_ms(), &items))
    }

    /// 맡은 작업이 바뀌었으면 다시 짠다
    pub async fn view(&self, refresh: bool) -> Value {
        let _g = self.0.lock.lock().await;
        let items = self.collect_items();
        let s = Self::sig(&items);
        let st = self.0.book.state();
        if refresh || st.s("sig") != s || st.i("lastPlanAt") == 0 {
            let ctx = self.ctx(refresh).await;
            self.0.book.replan(&ctx);
            self.0.book.update(|x| x["sig"] = json!(s));
        }
        self.view_now()
    }

    pub async fn replan(&self) -> Value {
        let _g = self.0.lock.lock().await;
        self.replan_locked().await
    }

    async fn replan_locked(&self) -> Value {
        let ctx = self.ctx(true).await;
        self.0.book.replan(&ctx);
        let s = Self::sig(&ctx.items);
        self.0.book.update(|x| x["sig"] = json!(s));
        self.view_now()
    }

    pub async fn apply(&self) -> Result<Value> {
        let _g = self.0.lock.lock().await;
        self.apply_locked().await
    }

    async fn apply_locked(&self) -> Result<Value> {
        if !self.0.google.is_ready() {
            return err("먼저 설정 → 외부 연결에서 Google에 로그인해 주세요.");
        }
        let now = now_ms();
        let (create, patch, remove) = self.0.book.diff(now);
        let (mut created, mut patched, mut removed, mut failed) = (0, 0, 0, 0);
        let mut errors: Vec<String> = vec![];
        for b in &create {
            match self.0.google.create_block_event(b).await {
                Ok(id) => {
                    self.0.book.record_applied(b, &id);
                    created += 1;
                }
                Err(e) => {
                    failed += 1;
                    errors.push(e);
                }
            }
        }
        for b in &patch {
            let ev = self.0.book.applied_event(&b.s("id")).unwrap_or_default();
            match self.0.google.patch_block_event(&ev, b).await {
                Ok(()) => {
                    self.0.book.record_applied(b, &ev);
                    patched += 1;
                }
                // 캘린더에서 지운 일정이면 새로 만든다
                Err(_) => match self.0.google.create_block_event(b).await {
                    Ok(id) => {
                        self.0.book.record_applied(b, &id);
                        created += 1;
                    }
                    Err(e) => {
                        failed += 1;
                        errors.push(e);
                    }
                },
            }
        }
        for (bid, ev) in &remove {
            let _ = self.0.google.remove_block_event(ev).await;
            self.0.book.forget_applied(bid);
            removed += 1;
        }
        self.0.book.prune(now);
        *self.0.busy_cache.lock().unwrap() = None;
        let todo = match self.0.google.sync_todo().await {
            Ok(t) => t,
            Err(e) => Some(format!("할 일 동기화 실패: {e}")),
        };
        let text = if created + patched + removed + failed == 0 {
            "캘린더는 이미 지금 시간표와 같아요.".to_string()
        } else {
            let mut parts = vec![];
            if created > 0 {
                parts.push(format!("{created}개 추가"));
            }
            if patched > 0 {
                parts.push(format!("{patched}개 시간 변경"));
            }
            if removed > 0 {
                parts.push(format!("{removed}개 삭제"));
            }
            let mut t = format!("캘린더에 반영했어요 ({}).", parts.join(", "));
            if failed > 0 {
                t += &format!(" {failed}개는 실패했어요: {}", errors[0]);
            }
            if let Some(td) = &todo {
                t += &format!(" Google 할 일: {td}.");
            }
            t
        };
        Ok(json!({ "result": { "created": created, "patched": patched, "removed": removed, "failed": failed, "errors": errors }, "todo": todo, "text": text, "view": self.view_now() }))
    }

    // ---------- 직접 고치기 (화면 버튼) ----------
    pub async fn edit(&self, op: &Value) -> Result<Value> {
        let _g = self.0.lock.lock().await;
        let ctx = self.ctx(false).await;
        let b = &self.0.book;
        let r = |k: &str| op.get(k).cloned().unwrap_or(Value::Null);
        match op.s("type").as_str() {
            "move" => {
                b.move_block(&ctx, &r("ref"), op.i("start"), op.i("end"))?;
            }
            "remove" => {
                b.remove_block(&ctx, &r("ref"))?;
            }
            "hours" => b.set_hours(&ctx, &op.s("taskId"), op.n("hours"))?,
            "unavailable" => b.add_unavailable(&ctx, op.i("start"), op.i("end"), &op.s("note"))?,
            "remove-unavailable" => b.remove_unavailable(&ctx, &op.s("id")),
            "prefs" => b.set_prefs(&ctx, &r("patch")),
            "reset" => b.reset(&ctx),
            _ => return err("알 수 없는 수정이에요."),
        }
        Ok(self.view_now())
    }

    // ---------- 대화로 고치기 ----------
    fn remember(&self, role: &str, text: &str) {
        let at = now_ms();
        self.0.book.update(|s| {
            let c = arr_mut(s, "chat");
            c.push(json!({ "role": role, "text": text, "at": at }));
            let n = c.len();
            if n > MAX_TURNS {
                c.drain(..n - MAX_TURNS);
            }
        });
    }

    pub fn clear_chat(&self) {
        self.0.book.update(|s| s["chat"] = json!([]));
    }

    pub async fn chat(&self, text: &str) -> Result<Value> {
        let text: String = text.trim().chars().take(20000).collect();
        if text.is_empty() {
            return err("하실 말을 적어 주세요.");
        }
        self.view(false).await;
        let _g = self.0.lock.lock().await;
        self.remember("user", &text);
        let cfg = self.0.ai.config();
        let mut reply = None;
        let mut via = "rules";
        if let Some(k) = &cfg.api_key {
            match self.ai_chat(&text, k, &cfg.model, &cfg.name).await {
                Ok(r) => {
                    reply = Some(r);
                    via = "ai";
                }
                Err(e) => *self.0.ai.last_error.lock().unwrap() = Some(e),
            }
        }
        let reply = match reply {
            Some(r) => r,
            None => {
                let before: Vec<String> = self.0.book.state().arr("warnings").iter().map(|w| w.s("text")).collect();
                let mut r = self.rule_chat(&text, cfg.api_key.is_some()).await;
                let fresh: Vec<String> = self.0.book.state().arr("warnings").iter().filter(|w| w.s("kind") != "calendar" && !before.contains(&w.s("text"))).map(|w| w.s("text")).collect();
                if !fresh.is_empty() {
                    r += &format!(" 다만 {}", fresh.join(" "));
                }
                r
            }
        };
        self.remember("cat", &reply);
        Ok(json!({ "reply": reply, "via": via, "view": self.view_now() }))
    }

    async fn rule_chat(&self, text: &str, ai_failed: bool) -> String {
        let now = now_ms();
        let items = self.collect_items();
        let v = self.0.book.view(now, &items);
        let cmd = parse_command(text, &v.arr("blocks"), &v.arr("tasks"), &v["prefs"], now);
        let b = &self.0.book;
        let r: Result<String> = async {
            match cmd {
                Cmd::Apply => Ok(self.apply_locked().await?.s("text")),
                Cmd::Replan => {
                    let v = self.replan_locked().await;
                    Ok(format!("다시 짰어요. {}개 시간으로 나눴어요.", v.arr("blocks").len()))
                }
                Cmd::Reset => {
                    b.reset(&self.ctx(true).await);
                    Ok("직접 정한 시간과 빼 둔 시간을 풀고 처음부터 다시 짰어요.".into())
                }
                Cmd::Show => Ok(if v.arr("blocks").is_empty() { "아직 잡힌 시간이 없어요. 협업 탭에서 작업을 맡으면 시간표를 짜 드려요.".into() } else { "지금 시간표는 아래와 같아요.".into() }),
                Cmd::Move { no, start, end } => {
                    let x = b.move_block(&self.ctx(false).await, &json!(no), start, end)?;
                    Ok(format!("{}을 {}로 옮기고 고정했어요. 겹치는 일정은 다른 시간으로 옮겼어요.", x.s("title"), fmt_range(x.i("start"), x.i("end"))))
                }
                Cmd::Remove { no } => {
                    let x = b.remove_block(&self.ctx(false).await, &json!(no))?;
                    Ok(format!("{} {}은 비워 두고, 그만큼 다른 빈 시간에 다시 잡았어요. 작업 시간 자체를 줄이려면 \"{} N시간이면 돼\"라고 말해 주세요.", fmt_range(x.i("start"), x.i("end")), x.s("title"), x.s("title")))
                }
                Cmd::Add { task_id, start, end } => {
                    b.add_block(&self.ctx(false).await, &task_id, start, end)?;
                    Ok(format!("{}에 넣었어요. 전체 시간은 그대로라 다른 날 시간이 그만큼 줄었어요.", fmt_range(start, end)))
                }
                Cmd::Hours { task_id, hours } => {
                    b.set_hours(&self.ctx(false).await, &task_id, hours)?;
                    let title = items.iter().find(|i| i.task_id == task_id).map(|i| i.title.clone()).unwrap_or_else(|| "그 작업".into());
                    Ok(format!("{title}을 {}시간으로 바꾸고 다시 짰어요.", round1(hours)))
                }
                Cmd::Unavailable { start, end, note } => {
                    b.add_unavailable(&self.ctx(false).await, start, end, &note)?;
                    Ok(format!("{}은 비워 두고 다시 짰어요.", fmt_range(start, end)))
                }
                Cmd::Prefs(patch) => {
                    b.set_prefs(&self.ctx(false).await, &patch);
                    let p = b.state()["prefs"].clone();
                    Ok(format!("작업 시간대를 {}시~{}시, 하루 최대 {}시간, 한 번에 최대 {}시간으로 맞춰 다시 짰어요.", p.n("dayStart"), p.n("dayEnd"), p.n("dailyMaxHours"), p.n("maxBlockHours")))
                }
                Cmd::Unknown(hint) => {
                    let help = "이렇게 말해 주시면 고칠게요: \"2번 목요일 3시로 옮겨줘\", \"1번 빼줘\", \"수요일은 안 돼\", \"내일 2시부터 5시까지 수업 있어\", \"자료조사 3시간이면 돼\", \"하루에 4시간만\", \"캘린더에 넣어줘\".";
                    let tip = if ai_failed { " (AI 연결이 안 돼서 정해진 표현만 알아들을 수 있어요.)" } else if self.0.ai.has_key() { "" } else { " 설정에서 Claude API 키를 넣으면 더 자유롭게 말해도 알아들어요." };
                    Ok(match hint {
                        Some(h) => format!("{h}{tip}"),
                        None => format!("무엇을 바꿀지 잘 모르겠어요. {help}{tip}"),
                    })
                }
            }
        }
        .await;
        r.unwrap_or_else(|e| format!("고치지 못했어요: {e}"))
    }

    // ---------- AI (도구 사용) ----------
    fn tools() -> Value {
        let iso = "YYYY-MM-DDTHH:MM (현지 시각)";
        json!([
            { "name": "move_block", "description": "시간표의 블록 하나를 다른 시간으로 옮긴다. 옮긴 블록은 고정되고 나머지는 자동으로 다시 배치된다. end 를 생략하면 길이를 유지한다.",
              "input_schema": { "type": "object", "properties": { "no": { "type": "integer", "description": "블록 번호" }, "start": { "type": "string", "description": iso }, "end": { "type": "string" } }, "required": ["no", "start"] } },
            { "name": "remove_block", "description": "블록 하나를 뺀다. 그 시간은 비워 두고, 작업 시간은 다른 빈 시간에 다시 잡힌다. 작업 시간 자체를 줄이려면 set_task_hours 를 쓴다.",
              "input_schema": { "type": "object", "properties": { "no": { "type": "integer" } }, "required": ["no"] } },
            { "name": "add_block", "description": "특정 작업을 특정 시간에 하도록 고정 블록을 추가한다. 전체 필요 시간은 그대로라 다른 블록이 줄어든다.",
              "input_schema": { "type": "object", "properties": { "task_id": { "type": "string" }, "start": { "type": "string" }, "end": { "type": "string" } }, "required": ["task_id", "start", "end"] } },
            { "name": "set_task_hours", "description": "작업에 필요한 전체 시간(시간 단위)을 바꾸고 다시 배치한다.",
              "input_schema": { "type": "object", "properties": { "task_id": { "type": "string" }, "hours": { "type": "number" } }, "required": ["task_id", "hours"] } },
            { "name": "add_unavailable", "description": "사용자가 작업할 수 없는 시간(약속, 수업 등)을 추가하고 다시 배치한다.",
              "input_schema": { "type": "object", "properties": { "start": { "type": "string" }, "end": { "type": "string" }, "note": { "type": "string" } }, "required": ["start", "end"] } },
            { "name": "remove_unavailable", "description": "안 되는 시간 하나를 지운다.",
              "input_schema": { "type": "object", "properties": { "id": { "type": "string" } }, "required": ["id"] } },
            { "name": "set_prefs", "description": "작업 시간대(시작·끝 시각, 0~24), 하루 최대 작업 시간, 한 번에 최대 작업 시간을 바꾸고 다시 배치한다.",
              "input_schema": { "type": "object", "properties": { "day_start": { "type": "number" }, "day_end": { "type": "number" }, "daily_max_hours": { "type": "number" }, "max_block_hours": { "type": "number" } } } },
            { "name": "replan", "description": "고정한 블록은 두고 나머지를 처음부터 다시 배치한다.", "input_schema": { "type": "object", "properties": {} } },
            { "name": "add_task", "description": "새 작업을 팀 프로젝트에 추가하고 사용자가 맡은 것으로 한다 (팀원 모두에게 보인다). 사용자가 새 할 일을 추가해 달라고 할 때만 쓴다.",
              "input_schema": { "type": "object", "properties": { "project_id": { "type": "string" }, "title": { "type": "string" }, "hours": { "type": "number" }, "done_criteria": { "type": "string" } }, "required": ["project_id", "title", "hours"] } },
            { "name": "rename_task", "description": "작업 이름을 바꾼다 (팀원 모두에게 보인다).",
              "input_schema": { "type": "object", "properties": { "task_id": { "type": "string" }, "title": { "type": "string" } }, "required": ["task_id", "title"] } },
            { "name": "complete_task", "description": "작업을 완료로 표시한다. 남은 블록은 시간표에서 빠진다.",
              "input_schema": { "type": "object", "properties": { "task_id": { "type": "string" } }, "required": ["task_id"] } },
            { "name": "apply_to_calendar", "description": "지금 시간표를 Google 캘린더 일정과 Google 할 일에 반영한다. 사용자가 반영·넣기·확정을 요청했을 때만 쓴다.",
              "input_schema": { "type": "object", "properties": {} } }
        ])
    }

    fn context_text(&self) -> String {
        let now = now_ms();
        let items = self.collect_items();
        let v = self.0.book.view(now, &items);
        let p = &v["prefs"];
        let mut l = vec![
            format!("지금: {} ({}요일)", to_local_iso(now), WEEKDAYS[crate::time::weekday(now) as usize]),
            format!("작업 시간대: {}~{}시, 하루 최대 {}시간, 한 번에 최대 {}시간", p.n("dayStart"), p.n("dayEnd"), p.n("dailyMaxHours"), p.n("maxBlockHours")),
            format!("Google 캘린더: {}", if self.0.google.is_ready() { "연결됨(바쁜 시간은 피해서 배치됨)" } else { "연결 안 됨" }),
            "\n내가 맡은 작업:".into(),
        ];
        for t in v.arr("tasks") {
            let dl = t.on("deadline").map(|d| format!(", 마감 {}", &to_local_iso(d as i64)[..10])).unwrap_or_default();
            l.push(format!("- task_id={} \"{}\" [{}] 필요 {}시간, 배치 {}시간{}", t.s("taskId"), t.s("title"), t.s("project"), t.n("need"), t.n("placed"), dl));
        }
        if v.arr("tasks").is_empty() {
            l.push("(없음)".into());
        }
        l.push("\n시간표 (번호. 시작 ~ 끝 작업):".into());
        for b in v.arr("blocks") {
            l.push(format!(
                "{}. {} ~ {} {}{}{}{}",
                b.i("no"),
                to_local_iso(b.i("start")),
                &to_local_iso(b.i("end"))[11..],
                b.s("title"),
                if b.b("pinned") { " (고정)" } else { "" },
                if b.b("late") { " (마감 뒤)" } else { "" },
                if b.b("synced") { "" } else { " (캘린더 미반영)" }
            ));
        }
        if v.arr("blocks").is_empty() {
            l.push("(없음)".into());
        }
        let un = v.arr("unavailable");
        if !un.is_empty() {
            l.push("\n안 되는 시간:".into());
            for u in un {
                l.push(format!("- id={} {} ~ {} {}", u.s("id"), to_local_iso(u.i("start")), to_local_iso(u.i("end")), u.s("note")));
            }
        }
        let w = v.arr("warnings");
        if !w.is_empty() {
            l.push(format!("\n알림: {}", w.iter().map(|x| x.s("text")).collect::<Vec<_>>().join(" / ")));
        }
        let mut ps = vec![];
        for t in self.0.hub.all().into_iter().filter(|t| t.is_connected()) {
            for p in t.collab_data().arr("projects") {
                ps.push(format!("project_id={} \"{}\"({})", p.s("id"), p.s("title"), t.name()));
            }
        }
        if !ps.is_empty() {
            l.push(format!("\n프로젝트: {}", ps.join(", ")));
        }
        l.join("\n")
    }

    async fn run_tool(&self, name: &str, input: &Value) -> Result<String> {
        let t = |k: &str| parse_local_iso(&input.s(k)).ok_or_else(|| format!("시각 형식이 맞지 않아요: {}", input.s(k)));
        let b = &self.0.book;
        match name {
            "move_block" => {
                let end = if input.s("end").is_empty() { 0 } else { t("end")? };
                let x = b.move_block(&self.ctx(false).await, &json!(input.i("no")), t("start")?, end)?;
                Ok(format!("옮김: {} {}", fmt_range(x.i("start"), x.i("end")), x.s("title")))
            }
            "remove_block" => {
                let x = b.remove_block(&self.ctx(false).await, &json!(input.i("no")))?;
                Ok(format!("뺌: {} {} (그 시간은 비워 둠)", fmt_range(x.i("start"), x.i("end")), x.s("title")))
            }
            "add_block" => {
                b.add_block(&self.ctx(false).await, &input.s("task_id"), t("start")?, t("end")?)?;
                Ok("추가함".into())
            }
            "set_task_hours" => {
                b.set_hours(&self.ctx(false).await, &input.s("task_id"), input.n("hours"))?;
                Ok(format!("필요 시간 {}시간으로 바꿈", input.n("hours")))
            }
            "add_unavailable" => {
                b.add_unavailable(&self.ctx(false).await, t("start")?, t("end")?, &input.s("note"))?;
                Ok("안 되는 시간 추가함".into())
            }
            "remove_unavailable" => {
                b.remove_unavailable(&self.ctx(false).await, &input.s("id"));
                Ok("안 되는 시간 지움".into())
            }
            "set_prefs" => {
                let mut p = serde_json::Map::new();
                for (src, dst) in [("day_start", "dayStart"), ("day_end", "dayEnd"), ("daily_max_hours", "dailyMaxHours"), ("max_block_hours", "maxBlockHours")] {
                    if let Some(v) = input.on(src) {
                        p.insert(dst.into(), json!(v));
                    }
                }
                b.set_prefs(&self.ctx(false).await, &Value::Object(p));
                Ok("설정 바꿈".into())
            }
            "replan" => {
                self.replan_locked().await;
                Ok("다시 배치함".into())
            }
            "add_task" => {
                let pid = input.s("project_id");
                let Some(team) = self.0.hub.all().into_iter().find(|t| t.project(&pid).is_some()) else { return err("그 프로젝트를 찾지 못했어요.") };
                let id = team.add_task(&pid, &json!({ "title": input.s("title"), "estimateHours": input.n("hours"), "doneCriteria": input.s("done_criteria") })).await?;
                team.patch_task(&pid, &id, json!({ "assignee": team.my_uid(), "accepted": true })).await?;
                self.replan_locked().await;
                Ok(format!("작업 추가함 (task_id={id})"))
            }
            "rename_task" => {
                let tid = input.s("task_id");
                let Some(team) = self.0.hub.find_task_team(&tid) else { return err("그 작업을 찾지 못했어요.") };
                let (pid, _) = team.find_task(&tid).unwrap();
                team.patch_task(&pid, &tid, json!({ "title": input.s("title").chars().take(80).collect::<String>() })).await?;
                self.replan_locked().await;
                Ok("이름 바꿈".into())
            }
            "complete_task" => {
                let tid = input.s("task_id");
                let Some(team) = self.0.hub.find_task_team(&tid) else { return err("그 작업을 찾지 못했어요.") };
                team.set_status(&tid, "done").await?;
                self.replan_locked().await;
                Ok("완료로 표시함".into())
            }
            "apply_to_calendar" => Ok(self.apply_locked().await?.s("text")),
            _ => err(format!("알 수 없는 도구: {name}")),
        }
    }

    async fn ai_chat(&self, text: &str, key: &str, model: &str, name: &str) -> Result<String> {
        let system = format!(
            "당신은 사용자의 바탕화면에 사는 고양이 \"{name}\"이고, 사용자의 작업 시간표를 함께 고칩니다.\n\
- 사용자가 시간이나 할 일을 바꿔 달라고 하면 도구로 직접 고칩니다. 확인을 묻기보다 합리적으로 해석해 바로 고치고, 무엇을 했는지 짧게 말합니다.\n\
- 시각은 현지 시각 YYYY-MM-DDTHH:MM 형식으로 넘깁니다. \"목요일\", \"내일 오후 3시\" 같은 말은 아래의 \"지금\"을 기준으로 계산합니다. 오전/오후가 없으면 낮 시간으로 봅니다.\n\
- 블록 번호는 고칠 때마다 바뀔 수 있으니, 도구 결과에 붙은 최신 시간표를 기준으로 합니다.\n\
- 캘린더 반영(apply_to_calendar)은 사용자가 넣어 달라·반영해 달라·확정한다고 할 때만 합니다. 고친 뒤에는 반영하려면 말해 달라고 안내합니다.\n\
- 할 수 없는 일(다른 사람의 캘린더 수정 등)은 솔직히 말합니다.\n\
- 부드러운 존댓말로 2~4문장, 이모지·마크다운·목록 기호 없이 답합니다. 시간표 전체를 다시 나열하지 않습니다 (화면에 따로 보입니다)."
        );
        let chat = self.0.book.state().arr("chat");
        let hist = &chat[chat.len().saturating_sub(11)..chat.len().saturating_sub(1)];
        let mut msgs: Vec<Value> = vec![];
        for m in hist {
            let role = if m.s("role") == "user" { "user" } else { "assistant" };
            match msgs.last_mut() {
                Some(last) if last["role"] == role => {
                    let j = format!("{}\n{}", last.s("content"), m.s("text"));
                    last["content"] = json!(j);
                }
                _ => msgs.push(json!({ "role": role, "content": m.s("text") })),
            }
        }
        while msgs.first().map(|m| m["role"] != "user").unwrap_or(false) {
            msgs.remove(0);
        }
        if msgs.last().map(|m| m["role"] == "user").unwrap_or(false) {
            msgs.pop();
        }
        msgs.push(json!({ "role": "user", "content": format!("[현재 상태]\n{}\n\n[사용자 말]\n{}", self.context_text(), text) }));
        let tools = Self::tools();
        for _ in 0..8 {
            let d = call_messages(key, model, &system, &json!(msgs), &tools).await?;
            let content = d.arr("content");
            msgs.push(json!({ "role": "assistant", "content": content }));
            let uses: Vec<&Value> = content.iter().filter(|b| b.s("type") == "tool_use").collect();
            if uses.is_empty() || d.s("stop_reason") != "tool_use" {
                let out: String = content.iter().filter(|b| b.s("type") == "text").map(|b| b.s("text")).collect::<String>().trim().to_string();
                return Ok(if out.is_empty() { "고쳤어요.".into() } else { out });
            }
            let mut results = vec![];
            for u in uses {
                let (res, is_err) = match self.run_tool(&u.s("name"), &u["input"]).await {
                    Ok(r) => (r, false),
                    Err(e) => (format!("실패: {e}"), true),
                };
                results.push(json!({ "type": "tool_result", "tool_use_id": u.s("id"), "content": format!("{res}\n\n[최신 상태]\n{}", self.context_text()), "is_error": is_err }));
            }
            msgs.push(json!({ "role": "user", "content": results }));
        }
        Ok("여러 번 고쳐 봤는데 마무리하지 못했어요. 아래 시간표를 확인해 주세요.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::book::*;
    use super::time_plan::*;
    use crate::store::Store;
    use crate::time::{ymd_hm, HOUR};
    use serde_json::json;

    fn day(d: u32, h: u32) -> i64 {
        ymd_hm(2026, 11, d, h, 0).unwrap()
    }

    fn items() -> Vec<Item> {
        vec![Item { task_id: "t1".into(), project_id: "p".into(), team_id: "x".into(), title: "자료조사".into(), hours: 4.0, deadline: Some(day(20, 23)), ..Default::default() }]
    }

    fn total(b: &Book, now: i64) -> i64 {
        b.upcoming(now).iter().map(|x| x["end"].as_i64().unwrap() - x["start"].as_i64().unwrap()).sum()
    }

    #[test]
    fn edits_and_calendar_diff() {
        std::env::set_var("TZ", "Asia/Seoul");
        let now = day(14, 10);
        let b = Book::new(Store::memory(json!({})));
        let ctx = Ctx { now, items: items(), busy: (vec![], true, None) };
        b.replan(&ctx);
        assert_eq!(total(&b, now), 4 * HOUR);
        // 모두 반영한 것으로 기록
        let (c, _, _) = b.diff(now);
        for (i, x) in c.iter().enumerate() {
            b.record_applied(x, &format!("e{i}"));
        }
        assert_eq!(b.diff(now).0.len() + b.diff(now).1.len(), 0);
        // 2번을 목요일 15시로 → 고정, 캘린더에서는 시간만 바뀜
        let ctx = Ctx { now, items: items(), busy: (b.own_events(), true, None) };
        b.move_block(&ctx, &json!(2), day(19, 15), day(19, 17)).unwrap();
        assert!(b.upcoming(now).iter().any(|x| x["start"] == day(19, 15) && x["pinned"] == true));
        assert_eq!(total(&b, now), 4 * HOUR);
        let (c, p, _) = b.diff(now);
        assert_eq!(c.len(), 0);
        assert!(!p.is_empty());
        // 1번을 빼면 그 시간은 비우고 다른 곳으로
        let first = b.upcoming(now)[0].clone();
        b.remove_block(&ctx, &json!(1)).unwrap();
        assert!(!b.upcoming(now).iter().any(|x| x["start"].as_i64() < first["end"].as_i64() && x["end"].as_i64() > first["start"].as_i64()));
        assert_eq!(total(&b, now), 4 * HOUR);
        // 일요일은 안 됨
        b.add_unavailable(&ctx, day(15, 9), day(15, 22), "약속").unwrap();
        assert!(!b.upcoming(now).iter().any(|x| crate::time::same_day(x["start"].as_i64().unwrap(), day(15, 0))));
        // 2시간이면 된다
        b.set_hours(&ctx, "t1", 2.0).unwrap();
        assert_eq!(total(&b, now), 2 * HOUR);
        assert!(!b.diff(now).2.is_empty());
    }

    #[test]
    fn past_blocks_count_and_dropped_tasks_are_removed() {
        std::env::set_var("TZ", "Asia/Seoul");
        let now = day(14, 10);
        let b = Book::new(Store::memory(json!({})));
        b.replan(&Ctx { now, items: items(), busy: (vec![], true, None) });
        let first = b.upcoming(now)[0].clone();
        let later = first["end"].as_i64().unwrap() + 60_000;
        b.replan(&Ctx { now: later, items: items(), busy: (vec![], true, None) });
        assert_eq!(total(&b, later), 4 * HOUR - (first["end"].as_i64().unwrap() - first["start"].as_i64().unwrap()));
        b.replan(&Ctx { now: later, items: vec![], busy: (vec![], true, None) });
        assert!(b.upcoming(later).is_empty());
    }
}
