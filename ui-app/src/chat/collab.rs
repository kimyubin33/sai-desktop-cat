//! 협업 탭: 자기소개 · 프로젝트 · 작업 · 배분 제안 · 진행 상태 · 내 작업 시간표
use super::{arr, b, n, num_text, s, schedule};
use crate::api::call;
use crate::dom::*;
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::future::Future;
use std::rc::Rc;
use wasm_bindgen::JsCast;
use web_sys::{Element, HtmlElement};

#[derive(Default)]
struct C {
    view: Option<Value>,
    plan: Option<Value>,
    focus: Option<String>,
    editing: Option<String>,
    sched: Option<Value>,
}

thread_local! {
    static S: RefCell<C> = RefCell::new(C::default());
    static BUSY: Cell<bool> = Cell::new(false);
    static MSG_SEQ: Cell<u32> = Cell::new(0);
}

fn cs<R>(f: impl FnOnce(&mut C) -> R) -> R {
    S.with(|s| f(&mut s.borrow_mut()))
}

const LEVELS: [(i64, &str); 3] = [(1, "배우는 중"), (2, "할 수 있음"), (3, "능숙")];
const STATUS: [(&str, &str); 4] = [("todo", "할 일"), ("doing", "하는 중"), ("blocked", "막힘"), ("done", "완료")];
const DIFF: [(i64, &str); 3] = [(1, "쉬움"), (2, "보통"), (3, "어려움")];

fn flash(text: &str, bad: bool) {
    let Some(m) = by_id("collabMsg") else { return };
    m.set_text_content(Some(text));
    toggle(&m, "bad", bad);
    let seq = MSG_SEQ.with(|c| {
        c.set(c.get() + 1);
        c.get()
    });
    after(4000, move || {
        if MSG_SEQ.with(|c| c.get()) == seq {
            if let Some(m) = by_id("collabMsg") {
                m.set_text_content(Some(""));
            }
        }
    });
}

fn act<F: Future<Output = Result<Value, String>> + 'static>(fut: F, ok: Option<String>) {
    if BUSY.with(|b| b.get()) {
        return;
    }
    BUSY.with(|b| b.set(true));
    spawn(async move {
        match fut.await {
            Ok(v) => {
                if v.get("connected").is_some() {
                    cs(|c| c.view = Some(v));
                } else if v["view"].is_object() {
                    cs(|c| c.view = Some(v["view"].clone()));
                }
                draw();
                if let Some(t) = ok {
                    flash(&t, false);
                }
            }
            Err(e) => flash(&e, true),
        }
        BUSY.with(|b| b.set(false));
    });
}

fn c(ch: &'static str, args: Value) -> impl Future<Output = Result<Value, String>> {
    async move { call(ch, args).await }
}

fn ok(t: &str) -> Option<String> {
    Some(t.to_string())
}

pub fn focus(id: &str) {
    if !id.is_empty() {
        cs(|c| c.focus = Some(id.to_string()));
    }
}

/// 저장된 내용을 먼저 보여 주고, 필요하면 서버와 동기화한 뒤 다시 그린다
pub fn render(refresh: bool) {
    spawn(async move {
        match call("collab:view", json!({ "refresh": false })).await {
            Ok(v) => {
                let connected = b(&v, "connected");
                cs(|c| c.view = Some(v));
                draw();
                if refresh && connected {
                    if let Ok(v) = call("collab:view", json!({ "refresh": true })).await {
                        cs(|c| c.view = Some(v));
                        draw();
                    }
                }
            }
            Err(e) => {
                cs(|c| c.view = Some(json!({ "connected": false, "error": e })));
                draw();
            }
        }
    });
}

fn view() -> Value {
    cs(|c| c.view.clone()).unwrap_or(Value::Null)
}

fn draw() {
    let Some(root) = by_id("collabRoot") else { return };
    clear(&root);
    let v = view();
    let add = |e: HtmlElement| {
        let _ = root.append_child(&e);
    };
    if !b(&v, "connected") {
        add(el("div")
            .cls("empty-state")
            .kid(el("p").text("협업 기능은 팀에 연결한 뒤 쓸 수 있어요."))
            .kid(el("p").cls("hint").text("설정 → 팀 연결에서 팀을 만들거나 참여 코드로 들어와 주세요."))
            .kid(el("button").cls("primary").text("설정 열기").click(|_| super::show_tab("settings")))
            .build());
        return;
    }
    add(el("div").id("collabMsg").cls("status collab-msg").build());
    if arr(&v, "teams").len() > 1 {
        add(el("div").cls("team-banner").text(&format!("{} 팀의 협업이에요. 다른 팀은 위쪽 이름 아래에서 바꿀 수 있어요.", s(&v, "teamName"))).build());
    }
    add(schedule_section());
    add(profile_section(&v));
    add(project_section(&v));
    if v["project"].is_object() {
        add(plan_section(&v));
        add(task_section(&v));
        add(files_section(&v));
        add(task_form(&v));
    }
    if let Some(id) = cs(|c| c.focus.take()) {
        if let Some(card) = q_in(&root, &format!("[data-task=\"{id}\"]")) {
            toggle(&card, "focus", true);
            let o = web_sys::ScrollIntoViewOptions::new();
            o.set_block(web_sys::ScrollLogicalPosition::Center);
            card.scroll_into_view_with_scroll_into_view_options(&o);
        }
    }
}

fn level_name(v: i64) -> &'static str {
    LEVELS.iter().find(|l| l.0 == v).map(|l| l.1).unwrap_or("")
}

// ---------- 자기소개 ----------
fn skill_row(rows: &HtmlElement, name: &str, level: i64) {
    let sel = el("select").cls("skill-level").build();
    for (v, l) in LEVELS {
        let _ = sel.append_child(&el("option").attr("value", &v.to_string()).text(l).flag("selected", v == level).0);
    }
    let row = el("div").cls("inline skill-row").kid(el("input").attr("type", "text").cls("skill-name").attr("value", name).attr("placeholder", "역량 (예: 영상편집)").attr("maxlength", "30")).child(sel).build();
    let r2 = row.clone();
    let _ = row.append_child(&el("button").attr("type", "button").cls("link").text("빼기").click(move |_| r2.remove()).0);
    let _ = rows.append_child(&row);
}

fn profile_section(v: &Value) -> HtmlElement {
    let p = &v["myProfile"];
    let rows = el("div").cls("skill-rows").build();
    let skills: Vec<(String, i64)> = p["skills"].as_object().map(|m| m.iter().map(|(k, x)| (k.clone(), x.as_i64().unwrap_or(2))).collect()).unwrap_or_default();
    if skills.is_empty() {
        skill_row(&rows, "", 2);
    }
    for (k, l) in &skills {
        skill_row(&rows, k, *l);
    }
    let learn: Vec<String> = p["learn"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
    let weekly = if p["weeklyHours"].is_null() { String::new() } else { num_text(n(p, "weeklyHours")) };
    let r2 = rows.clone();
    let r3 = rows.clone();
    let pr = el("textarea").attr("name", "pr").attr("rows", "3").attr("maxlength", "500").text(&s(p, "pr"));
    let form = el("form")
        .cls("settings")
        .kid(el("label").txt("자기소개 (할 수 있는 일, 해 본 경험)").kid(pr))
        .kid(el("div").cls("label").text("역량 — 본인이 적고 언제든 고칠 수 있어요"))
        .child(rows)
        .kid(el("button").attr("type", "button").cls("link").text("+ 역량 추가").click(move |_| skill_row(&r2, "", 2)))
        .kid(el("label").txt("배우고 싶은 일 (쉼표로 구분)").kid(el("input").attr("type", "text").attr("name", "learn").attr("value", &learn.join(", "))))
        .kid(el("label").txt("이번 주에 이 프로젝트에 쓸 수 있는 시간").kid(el("div").cls("inline").kid(el("input").attr("type", "number").attr("name", "weeklyHours").attr("min", "0").attr("max", "80").attr("step", "0.5").attr("value", &weekly)).txt("시간 / 주")))
        .kid(calendar_helper(v))
        .kid(el("div").cls("inline").kid(el("button").attr("type", "submit").cls("primary").text("자기소개 저장")))
        .build();
    let f2 = form.clone();
    on(&form, "submit", move |e| {
        e.prevent_default();
        let sk: Vec<Value> = q_all_in(&r3, ".skill-row")
            .iter()
            .map(|r| json!({ "name": val(&q_in(r, ".skill-name").unwrap()), "level": val(&q_in(r, ".skill-level").unwrap()).parse::<i64>().unwrap_or(2) }))
            .collect();
        let args = json!({ "pr": val(&field(&f2, "pr")), "skills": sk, "learn": val(&field(&f2, "learn")), "weeklyHours": val(&field(&f2, "weeklyHours")) });
        act(c("collab:save-profile", args), ok("자기소개를 저장했어요. 팀원 모두 볼 수 있어요."));
    });

    let my_uid = s(v, "myUid");
    let team = el("div").cls("list").kids(arr(v, "members").iter().map(|m| {
        let title = format!("{}{} · {}", s(m, "name"), if s(m, "uid") == my_uid { " (나)" } else { "" }, if b(m, "hasProfile") { format!("주 {}시간", s(m, "weeklyHours")) } else { "자기소개 아직 없음".into() });
        let chips = el("div")
            .cls("chips")
            .kids(m["skills"].as_object().map(|o| o.iter().map(|(k, x)| el("span").cls("chip").text(&format!("{k} · {}", level_name(x.as_i64().unwrap_or(0))))).collect::<Vec<_>>()).unwrap_or_default())
            .kids(arr(m, "learn").iter().map(|k| el("span").cls("chip learn").text(&format!("배우고 싶음: {}", k.as_str().unwrap_or("")))));
        let pr = s(m, "pr");
        el("div").cls("item").kid(el("div").cls("main").kid(el("div").cls("title").text(&title)).maybe(if pr.is_empty() { None } else { Some(el("div").cls("sub").text(&pr).attr("title", &pr)) }).kid(chips))
    }));
    let has = p.is_object();
    el("details")
        .cls("block")
        .flag("open", !has)
        .kid(el("summary").kid(el("strong").text("자기소개 · 역량 · 가능 시간")).maybe(if has { None } else { Some(el("span").cls("status-pill review").text("입력 필요")) }))
        .child(form)
        .kid(el("div").cls("label").text("팀원"))
        .kid(team)
        .build()
}

/// Google 캘린더의 바쁜 시간으로 빈 시간을 계산해 참고로 보여 준다 (자동 저장하지 않음 — 본인이 정한다)
fn calendar_helper(v: &Value) -> El {
    let out = el("div").cls("hint cal-help").build();
    let o2 = out.clone();
    let pid = v["project"]["id"].clone();
    let btn = el("button").attr("type", "button").cls("link").text("Google 캘린더로 빈 시간 보기").click(move |_| {
        let out = o2.clone();
        let pid = pid.clone();
        out.set_text_content(Some("캘린더를 확인하는 중…"));
        spawn(async move {
            match call("g:availability", json!({ "projectId": pid })).await {
                Ok(r) => {
                    let week = n(&r, "week");
                    let suggest = ((week * 0.4 * 2.0).round() / 2.0).max(0.0);
                    clear(&out);
                    let to_dl = if r["toDeadline"].is_null() { String::new() } else { format!(", 마감까지는 {}시간", num_text(n(&r, "toDeadline"))) };
                    let _ = out.append_child(&document().create_text_node(&format!("앞으로 7일 동안 {} 중 빈 시간은 {}시간{to_dl}이에요. 빈 시간을 전부 쓰긴 어려우니 그중 일부만 적는 걸 권해요. ", s(&r, "window"), num_text(week))));
                    let st = num_text(suggest);
                    let _ = out.append_child(
                        &el("button")
                            .attr("type", "button")
                            .cls("link")
                            .text(&format!("{st}시간으로 적기 (빈 시간의 40%)"))
                            .click(move |_| {
                                if let Some(i) = q("input[name=\"weeklyHours\"]") {
                                    set_val(&i, &st);
                                    let _ = i.focus();
                                }
                            })
                            .0,
                    );
                }
                Err(e) => out.set_text_content(Some(&format!("{e} (설정 → 외부 연결에서 Google 로그인)"))),
            }
        });
    });
    el("div").kid(btn).child(out)
}

// ---------- 프로젝트 ----------
fn project_section(v: &Value) -> HtmlElement {
    let p = &v["project"];
    let projects = arr(v, "projects");
    let new_form = el("form")
        .cls("settings")
        .kid(el("label").txt("프로젝트 이름").kid(el("input").attr("type", "text").attr("name", "title").flag("required", true).attr("maxlength", "80")))
        .kid(el("label").txt("목표").kid(el("input").attr("type", "text").attr("name", "goal").attr("maxlength", "300")))
        .kid(el("label").txt("결과물").kid(el("input").attr("type", "text").attr("name", "deliverable").attr("maxlength", "200")))
        .kid(el("label").txt("마감일").kid(el("input").attr("type", "date").attr("name", "deadline")))
        .kid(el("button").attr("type", "submit").cls("primary").text("만들기"))
        .build();
    let f2 = new_form.clone();
    on(&new_form, "submit", move |e| {
        e.prevent_default();
        let args = json!({ "title": val(&field(&f2, "title")), "goal": val(&field(&f2, "goal")), "deliverable": val(&field(&f2, "deliverable")), "deadline": val(&field(&f2, "deadline")) });
        act(c("collab:create-project", args), ok("프로젝트를 만들었어요."));
    });
    let bx = el("div").cls("block");
    if projects.is_empty() {
        return bx.kid(el("strong").text("프로젝트")).kid(el("p").cls("hint").text("아직 프로젝트가 없어요. 목표와 마감일부터 적어 주세요.")).child(new_form).build();
    }
    let sel = el("select").build();
    let pid = s(p, "id");
    for x in &projects {
        let _ = sel.append_child(&el("option").attr("value", &s(x, "id")).text(&s(x, "title")).flag("selected", s(x, "id") == pid).0);
    }
    let s2 = sel.clone();
    on(&sel, "change", move |_| act(c("collab:set-active", json!({ "id": val(&s2) })), None));
    let meta = if p.is_object() {
        let deadline = s(p, "deadline");
        let dl_text = if deadline.is_empty() {
            "마감일 없음".to_string()
        } else {
            let end = js_sys::Date::new(&format!("{deadline}T23:59").into()).get_time();
            let dl = ((end - now()) / 86_400_000.0).ceil() as i64;
            format!("마감: {deadline} ({})", if dl >= 0 { format!("{dl}일 남음") } else { format!("{}일 지남", -dl) })
        };
        let goal = s(p, "goal");
        let deliv = s(p, "deliverable");
        Some(el("div").cls("project-meta").maybe(if goal.is_empty() { None } else { Some(el("div").text(&format!("목표: {goal}"))) }).maybe(if deliv.is_empty() { None } else { Some(el("div").text(&format!("결과물: {deliv}"))) }).kid(el("div").text(&dl_text)))
    } else {
        None
    };
    bx.kid(el("div").cls("inline").kid(el("strong").text("프로젝트")).child(sel)).maybe(meta).kid(el("details").kid(el("summary").text("새 프로젝트 만들기")).child(new_form)).build()
}

// ---------- 배분 제안 ----------
fn plan_section(v: &Value) -> HtmlElement {
    let plan = cs(|c| c.plan.clone());
    let pid = s(&v["project"], "id");
    let p2 = pid.clone();
    let bx = el("div")
        .cls("block plan")
        .kid(el("div").cls("inline").kid(el("strong").text("배분 제안")).kid(el("button").cls("primary").text(if plan.is_some() { "다시 계산" } else { "배분 제안 받기" }).click(move |_| {
            let pid = p2.clone();
            spawn(async move {
                match call("collab:plan", json!({ "projectId": pid })).await {
                    Ok(r) => {
                        cs(|c| c.plan = Some(r));
                        draw();
                    }
                    Err(e) => flash(&e, true),
                }
            });
        })))
        .kid(el("p").cls("hint").text("같은 개수가 아니라 가능 시간·역량·어려운 일 쏠림·선행 작업·검토 여유를 함께 봐요. 적용해도 각자 수락하거나 이의를 제기할 수 있어요."))
        .build();
    let Some(r) = plan else { return bx };
    let add = |e: El| {
        let _ = bx.append_child(&e.0);
    };
    let missing: Vec<String> = arr(&r, "missingProfiles").iter().filter_map(|x| x.as_str().map(String::from)).collect();
    if !missing.is_empty() {
        add(el("div").cls("warn").text(&format!("자기소개가 없는 팀원: {} — 가능 시간이 0으로 계산돼요.", missing.join(", "))));
    }
    for w in arr(&r, "warnings") {
        add(el("div").cls("warn").text(&s(&w, "text")));
    }
    add(el("div").cls("loads").kids(arr(&r, "members").iter().map(|m| {
        let ratio = m["ratio"].as_f64();
        let pct = ratio.map(|x| (x * 100.0).round().min(150.0)).unwrap_or(0.0);
        let usable = n(m, "usable");
        el("div")
            .cls("load-row")
            .kid(el("span").cls("load-name").text(&s(m, "name")))
            .kid(el("div").cls("bar").kid(el("div").cls(if ratio.unwrap_or(0.0) > 1.0 { "fill over" } else { "fill" }).attr("style", &format!("width:{}%", pct.min(100.0)))))
            .kid(el("span").cls("load-num").text(&if usable > 0.0 { format!("{}/{}h", num_text(n(m, "load")), num_text(usable)) } else { "시간 없음".into() }))
    })));
    add(el("div").cls("hint").text(&format!("마감까지 약 {}주 · 막대는 검토·수정 몫 20%를 뺀 시간 대비 부담", s(&r, "weeks"))));
    add(el("div").cls("list").kids(arr(&r, "assignments").iter().map(|a| {
        let name = s(a, "name");
        el("details")
            .cls("item plan-row")
            .kid(el("summary").kid(el("span").cls("title").text(&s(a, "title"))).txt(" → ").kid(el("b").text(if name.is_empty() { "배정 못 함" } else { &name })).maybe(if b(a, "fixed") { Some(el("span").cls("status-pill sent").text("수락됨")) } else { None }))
            .kid(el("ul").kids(arr(a, "reasons").iter().map(|x| el("li").text(x.as_str().unwrap_or("")))))
    })));
    add(el("p").cls("hint").text(&s(&r, "note")));
    let assignments = r["assignments"].clone();
    add(el("button").cls("primary").text("이대로 배정 제안하기").click(move |_| {
        let args = json!({ "projectId": pid, "assignments": assignments });
        act(
            async move {
                let res = call("collab:apply-plan", args).await?;
                cs(|c| c.plan = None);
                let applied = s(&res, "applied");
                after(0, move || flash(&format!("{applied}개 작업을 제안했어요. 각자 수락하거나 이의를 제기할 수 있어요."), false));
                Ok(res["view"].clone())
            },
            None,
        );
    }));
    bx
}

// ---------- 내 작업 시간표 ----------
fn sched_opts() -> Rc<schedule::Opts> {
    Rc::new(schedule::Opts {
        compact: false,
        say: Rc::new(flash),
        on_view: Some(Rc::new(|v: &Value| cs(|c| c.sched = Some(v.clone())))),
        on_chat: Some(Rc::new(|| spawn(super::talk::start_schedule()))),
    })
}

fn schedule_section() -> HtmlElement {
    let bx = el("div").cls("block").id("schedBox").build();
    match cs(|c| c.sched.clone()) {
        Some(v) => schedule::render(&bx, &v, sched_opts()),
        None => {
            let _ = bx.append_child(&el("p").cls("hint").text("내 작업 시간표를 짜는 중…").0);
        }
    }
    let b2 = bx.clone();
    spawn(async move {
        match call("sched:view", Value::Null).await {
            Ok(v) => {
                cs(|c| c.sched = Some(v.clone()));
                schedule::render(&b2, &v, sched_opts());
            }
            Err(e) => {
                clear(&b2);
                let _ = b2.append_child(&el("p").cls("hint warn").text(&format!("시간표를 짜지 못했어요: {e}")).0);
            }
        }
    });
    bx
}

fn reason_select(v: &Value) -> HtmlElement {
    let sel = el("select").build();
    for r in arr(v, "objectionReasons") {
        let r = r.as_str().unwrap_or("");
        let _ = sel.append_child(&el("option").attr("value", r).text(r).0);
    }
    sel
}

// ---------- 작업 ----------
fn task_section(v: &Value) -> HtmlElement {
    let bx = el("div").cls("block").kid(el("strong").text("작업")).build();
    let mut tasks = arr(v, "tasks");
    if tasks.is_empty() {
        let _ = bx.append_child(&el("p").cls("hint").text("작업을 잘게 나눠 적어 주세요. 예상 시간, 필요한 역량, 먼저 끝나야 하는 작업, 완료 기준이 있으면 배분이 정확해져요.").0);
    }
    let title_of: HashMap<String, String> = tasks.iter().map(|t| (s(t, "id"), s(t, "title"))).collect();
    let order = |st: &str| match st {
        "blocked" => 0,
        "doing" => 1,
        "todo" => 2,
        _ => 3,
    };
    tasks.sort_by_key(|t| order(&s(t, "status")));
    let my_uid = s(v, "myUid");
    let project_id = s(&v["project"], "id");
    for t in &tasks {
        let id = s(t, "id");
        let status = s(t, "status");
        let mine = s(t, "assignee") == my_uid && !my_uid.is_empty();
        let accepted = b(t, "accepted");
        let sel = el("select").cls(&format!("status-select {status}")).build();
        for (k, l) in STATUS {
            let _ = sel.append_child(&el("option").attr("value", k).text(l).flag("selected", status == k).0);
        }
        {
            let (s2, id) = (sel.clone(), id.clone());
            on(&sel, "change", move |_| act(c("collab:status", json!({ "id": id, "status": val(&s2) })), None));
        }
        let diff = DIFF.iter().find(|d| d.0 == n(t, "difficulty") as i64).map(|d| d.1).unwrap_or("보통");
        let an = s(t, "assigneeName");
        let who = if an.is_empty() { " · 미배정".to_string() } else { format!(" · 담당 {an}{}{}", if mine { "(나)" } else { "" }, if accepted { "" } else { " — 확인 기다림" }) };
        let skills: Vec<String> = arr(t, "skills").iter().filter_map(|x| x.as_str().map(String::from)).collect();
        let deps: Vec<String> = arr(t, "dependsOn").iter().map(|d| title_of.get(d.as_str().unwrap_or("")).cloned().unwrap_or_else(|| "삭제된 작업".into())).collect();
        let dc = s(t, "doneCriteria");
        let prep = arr(t, "prep");
        let objections: Vec<String> = arr(t, "objectionNames").iter().filter_map(|x| x.as_str().map(String::from)).collect();
        let card = el("div")
            .cls(&format!("task {status}"))
            .attr("data-task", &id)
            .kid(el("div").cls("task-head").kid(el("b").text(&s(t, "title"))).child(sel))
            .kid(el("div").cls("task-meta").text(&format!("{}시간 · {diff}{who}", num_text(n(t, "estimateHours")))))
            .maybe(if skills.is_empty() { None } else { Some(el("div").cls("chips").kids(skills.iter().map(|x| el("span").cls("chip").text(x)))) })
            .maybe(if deps.is_empty() { None } else { Some(el("div").cls("hint").text(&format!("먼저 끝나야 할 작업: {}", deps.join(", ")))) })
            .maybe(if dc.is_empty() { None } else { Some(el("div").cls("hint").text(&format!("완료 기준: {dc}"))) })
            .maybe(if prep.is_empty() {
                None
            } else {
                Some(el("div").cls("prep").kids(prep.iter().enumerate().map(|(i, p)| {
                    let id = id.clone();
                    el("label").cls("check").kid(el("input").attr("type", "checkbox").flag("checked", b(p, "done")).on("change", move |_| act(c("collab:toggle-prep", json!({ "id": id, "index": i })), None))).txt(&format!(" {}", s(p, "text")))
                })))
            })
            .maybe(if objections.is_empty() { None } else { Some(el("div").cls("hint").text(&format!("이의 제기: {} — 다음 배분 제안에서 빠져요", objections.join(", ")))) })
            .build();
        let actions = el("div").cls("inline task-actions").build();
        let push = |e: El| {
            let _ = actions.append_child(&e.0);
        };
        if mine && !accepted && status != "done" {
            let reason = reason_select(v);
            let (r2, id1, id2) = (reason.clone(), id.clone(), id.clone());
            push(el("button").cls("primary").text("맡을게요").click(move |_| act(c("collab:accept", json!({ "id": id1 })), ok("수락했어요."))));
            let _ = actions.append_child(&reason);
            push(el("button").text("이의 있어요").click(move |_| act(c("collab:object", json!({ "id": id2, "reason": val(&r2) })), ok("이의를 남겼어요. 다시 배분할 때 반영돼요."))));
        } else if mine && accepted && status != "done" {
            let reason = reason_select(v);
            let (r2, id1) = (reason.clone(), id.clone());
            push(el("details").cls("inline-details").kid(el("summary").text("못 하게 됐어요")).child(reason).kid(el("button").text("배정 풀기").click(move |_| act(c("collab:object", json!({ "id": id1, "reason": val(&r2) })), ok("배정을 풀었어요. 팀이 다시 나눌 수 있어요.")))));
        }
        if mine && accepted && status != "done" {
            push(el("button").cls("link").text("내 시간표에서 보기").click(|_| {
                if let Some(bx) = by_id("schedBox") {
                    let el: &Element = bx.unchecked_ref();
                    el.scroll_into_view();
                }
            }));
        }
        let id_e = id.clone();
        push(el("button").cls("link").text("고치기").click(move |_| {
            cs(|c| c.editing = Some(id_e.clone()));
            draw();
            if let Some(f) = by_id("taskForm") {
                f.scroll_into_view();
            }
        }));
        let (title, pid, id_d) = (s(t, "title"), project_id.clone(), id.clone());
        push(el("button").cls("link").text("삭제").click(move |_| {
            if confirm(&format!("\"{title}\" 작업을 지울까요?")) {
                act(c("collab:delete-task", json!({ "projectId": pid, "id": id_d })), ok("지웠어요."));
            }
        }));
        let _ = card.append_child(&actions);
        let _ = bx.append_child(&card);
    }
    bx
}

fn task_form(v: &Value) -> HtmlElement {
    let tasks = arr(v, "tasks");
    let editing = cs(|c| c.editing.clone());
    let t = editing.as_ref().and_then(|id| tasks.iter().find(|x| s(x, "id") == *id).cloned());
    let others: Vec<&Value> = tasks.iter().filter(|x| t.as_ref().map(|t| s(t, "id") != s(x, "id")).unwrap_or(true)).collect();
    let tv = t.clone().unwrap_or(Value::Null);
    let deps_of: Vec<String> = arr(&tv, "dependsOn").iter().filter_map(|x| x.as_str().map(String::from)).collect();
    let diff_now = if t.is_some() { n(&tv, "difficulty") as i64 } else { 2 };
    let diff_sel = el("select").attr("name", "difficulty").build();
    for (k, l) in DIFF {
        let _ = diff_sel.append_child(&el("option").attr("value", &k.to_string()).text(l).flag("selected", k == diff_now).0);
    }
    let skills: Vec<String> = arr(&tv, "skills").iter().filter_map(|x| x.as_str().map(String::from)).collect();
    let prep_text: Vec<String> = arr(&tv, "prep").iter().map(|p| s(p, "text")).collect();
    let form = el("form")
        .id("taskForm")
        .cls("settings")
        .kid(el("label").txt("작업 이름").kid(el("input").attr("type", "text").attr("name", "title").flag("required", true).attr("maxlength", "80").attr("value", &s(&tv, "title"))))
        .kid(
            el("div")
                .cls("inline")
                .kid(el("label").txt("예상 시간").kid(el("input").attr("type", "number").attr("name", "estimateHours").attr("min", "0.5").attr("max", "200").attr("step", "0.5").attr("value", &if t.is_some() { num_text(n(&tv, "estimateHours")) } else { "2".into() })))
                .kid(el("label").txt("난이도").child(diff_sel)),
        )
        .kid(el("label").txt("필요한 역량 (쉼표로 구분)").kid(el("input").attr("type", "text").attr("name", "skills").attr("value", &skills.join(", ")).attr("placeholder", "예: 디자인, 영상편집")))
        .maybe(if others.is_empty() {
            None
        } else {
            Some(el("div").kid(el("div").cls("label").text("먼저 끝나야 하는 작업")).kid(el("div").cls("deps").kids(others.iter().map(|o| el("label").cls("check").kid(el("input").attr("type", "checkbox").attr("name", "dep").attr("value", &s(o, "id")).flag("checked", deps_of.contains(&s(o, "id")))).txt(&format!(" {}", s(o, "title")))))))
        })
        .kid(el("label").txt("완료 기준 (어디까지 하면 끝인지)").kid(el("input").attr("type", "text").attr("name", "doneCriteria").attr("maxlength", "200").attr("value", &s(&tv, "doneCriteria"))))
        .kid(el("label").txt("준비물·참고자료 (쉼표로 구분)").kid(el("input").attr("type", "text").attr("name", "prep").attr("value", &prep_text.join(", "))))
        .kid(el("div").cls("inline").kid(el("button").attr("type", "submit").cls("primary").text(if t.is_some() { "고친 내용 저장" } else { "작업 추가" })).maybe(if t.is_some() {
            Some(el("button").attr("type", "button").cls("link").text("취소").click(|_| {
                cs(|c| c.editing = None);
                draw();
            }))
        } else {
            None
        }))
        .build();
    let f2 = form.clone();
    let project_id = s(&v["project"], "id");
    let t2 = t.clone();
    on(&form, "submit", move |e| {
        e.prevent_default();
        let old_prep = t2.as_ref().map(|t| arr(t, "prep")).unwrap_or_default();
        let prep: Vec<Value> = val(&field(&f2, "prep"))
            .split(',')
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty())
            .map(|text| {
                let done = old_prep.iter().any(|p| s(p, "text") == text && b(p, "done"));
                json!({ "text": text, "done": done })
            })
            .collect();
        let deps: Vec<String> = q_all_in(&f2, "input[name=\"dep\"]").iter().filter(|x| checked(x)).map(|x| val(x)).collect();
        let mut data = json!({
            "projectId": project_id,
            "title": val(&field(&f2, "title")),
            "estimateHours": val(&field(&f2, "estimateHours")),
            "difficulty": val(&field(&f2, "difficulty")),
            "skills": val(&field(&f2, "skills")),
            "dependsOn": deps,
            "doneCriteria": val(&field(&f2, "doneCriteria")),
            "prep": prep,
        });
        if let Some(t) = &t2 {
            data["id"] = json!(s(t, "id"));
            act(
                async move {
                    let v = call("collab:edit-task", data).await?;
                    cs(|c| c.editing = None);
                    Ok(v)
                },
                ok("고쳤어요."),
            );
        } else {
            act(c("collab:add-task", data), ok("작업을 추가했어요."));
        }
    });
    let title = match &t {
        Some(t) => format!("작업 고치기: {}", s(t, "title")),
        None => "작업 추가".into(),
    };
    el("details").cls("block").flag("open", t.is_some() || tasks.is_empty()).kid(el("summary").kid(el("strong").text(&title))).child(form).build()
}

// ---------- 팀 자료 (Google Drive) ----------
const MAX_MB: f64 = 25.0;

fn size_text(n: f64) -> String {
    if n <= 0.0 {
        String::new()
    } else if n < 1024.0 * 1024.0 {
        format!(" · {}KB", (n / 1024.0).ceil() as i64)
    } else {
        format!(" · {:.1}MB", n / 1024.0 / 1024.0)
    }
}

fn open_url(url: String) {
    spawn(async move {
        if let Err(e) = call("app:open-url", json!(url)).await {
            flash(&e, true);
        }
    });
}

/// 고른(또는 끌어다 놓은) 파일을 읽어 앱으로 보낸다. 앱이 내 Drive 에 올리고 팀 자료에 붙인다
fn upload_files(files: web_sys::FileList, project_id: String, task_id: String) {
    let list: Vec<web_sys::File> = (0..files.length()).filter_map(|i| files.get(i)).collect();
    if list.is_empty() {
        return;
    }
    spawn(async move {
        for f in list {
            let name = f.name();
            if f.size() > MAX_MB * 1024.0 * 1024.0 {
                flash(&format!("{name}: {MAX_MB}MB 보다 커요. Drive 에 직접 올린 뒤 공유 링크를 붙여 넣어 주세요."), true);
                continue;
            }
            flash(&format!("{name} 을(를) Drive 에 올리는 중…"), false);
            let buf = match wasm_bindgen_futures::JsFuture::from(f.array_buffer()).await {
                Ok(b) => js_sys::Uint8Array::new(&b).to_vec(),
                Err(_) => {
                    flash(&format!("{name} 을(를) 읽지 못했어요."), true);
                    continue;
                }
            };
            use base64::Engine;
            let data = base64::engine::general_purpose::STANDARD.encode(&buf);
            let args = json!({ "projectId": project_id, "taskId": task_id, "name": name, "mime": f.type_(), "data": data });
            match call("collab:upload-file", args).await {
                Ok(v) => {
                    cs(|c| c.view = Some(v));
                    draw();
                    flash(&format!("{name} 을(를) 올렸어요. 팀원 모두 볼 수 있어요."), false);
                }
                Err(e) => {
                    flash(&e, true);
                    break;
                }
            }
        }
    });
}

fn files_section(v: &Value) -> HtmlElement {
    let pid = s(&v["project"], "id");
    let tasks = arr(v, "tasks");
    let title_of: HashMap<String, String> = tasks.iter().map(|t| (s(t, "id"), s(t, "title"))).collect();
    let signed = b(&v["google"], "signedIn");
    let drive = b(&v["google"], "drive");

    let bx = el("div")
        .cls("block files")
        .id("filesBox")
        .kid(el("strong").text("팀 자료 (Google Drive)"))
        .kid(el("p").cls("hint").text("파일을 여기에 끌어다 놓거나 골라 올리면 내 Google Drive 의 \"사이 - 팀 이름\" 폴더에 들어가고, 링크가 있는 사람은 볼 수 있게 바뀌어 팀원 모두의 사이에 보여요. 이미 Drive 에 있는 파일은 공유 링크를 붙여 넣으면 돼요."))
        .build();
    let add = |e: El| {
        let _ = bx.append_child(&e.0);
    };
    if !signed {
        add(el("div").cls("warn").txt("파일을 올리려면 Google 로그인이 필요해요. ").kid(el("button").cls("link").text("설정 → 외부 연결").click(|_| super::show_tab("settings"))));
    } else if !drive {
        add(el("div").cls("warn").txt("Drive 권한이 새로 필요해요. 설정 → 외부 연결에서 로그아웃 후 다시 로그인해 주세요. (링크 붙여 넣기는 지금도 돼요) ").kid(el("button").cls("link").text("설정 열기").click(|_| super::show_tab("settings"))));
    }

    // 목록
    let files = arr(v, "files");
    if files.is_empty() {
        add(el("div").cls("empty").text("아직 올린 자료가 없어요."));
    }
    let list = el("div").cls("list").build();
    for f in &files {
        let url = s(f, "url");
        let task = title_of.get(&s(f, "taskId")).map(|t| format!(" · {t}")).unwrap_or_default();
        let when = n(f, "at");
        let sub = format!("{}{task}{}{}", s(f, "byName"), size_text(n(f, "size")), if when > 0.0 { format!(" · {}", super::fmt_date(when)) } else { String::new() });
        let (u2, fid, p2) = (url.clone(), s(f, "id"), pid.clone());
        let name = s(f, "name");
        let item = el("div")
            .cls("item")
            .kid(el("div").cls("main").kid(el("div").cls("title").text(&name).attr("title", &url)).kid(el("div").cls("sub").text(&sub)))
            .kid(el("button").text("열기").click(move |_| open_url(u2.clone())))
            .maybe(if b(f, "mine") {
                Some(el("button").cls("link").text("빼기").attr("title", "팀 자료에서만 빼요. Drive 의 파일은 그대로 있어요.").click(move |_| {
                    if confirm(&format!("\"{name}\"을(를) 팀 자료에서 뺄까요? Drive 의 파일은 지워지지 않아요.")) {
                        act(c("collab:delete-file", json!({ "projectId": p2, "id": fid })), ok("팀 자료에서 뺐어요."));
                    }
                }))
            } else {
                None
            });
        let _ = list.append_child(&item.0);
    }
    let _ = bx.append_child(&list);

    // 어느 작업의 자료인지
    let task_sel = el("select").cls("file-task").kid(el("option").attr("value", "").text("프로젝트 전체")).build();
    for t in &tasks {
        let _ = task_sel.append_child(&el("option").attr("value", &s(t, "id")).text(&s(t, "title")).0);
    }

    // 올리기: 고르기 + 끌어다 놓기
    let input = el("input").attr("type", "file").flag("multiple", true).cls("hidden").build();
    {
        let (i2, p2, ts) = (input.clone(), pid.clone(), task_sel.clone());
        on(&input, "change", move |_| {
            let inp: &web_sys::HtmlInputElement = i2.unchecked_ref();
            if let Some(fl) = inp.files() {
                upload_files(fl, p2.clone(), val(&ts));
            }
            inp.set_value("");
        });
    }
    let i3 = input.clone();
    let drop = el("div")
        .cls("drop-zone")
        .flag("disabled", !drive)
        .txt("여기에 파일을 끌어다 놓거나 ")
        .kid(el("button").cls("link").text("파일 고르기").flag("disabled", !drive).click(move |_| i3.click()))
        .txt(&format!(" (한 번에 {MAX_MB}MB 까지)"))
        .build();
    {
        let d2 = drop.clone();
        on(&drop, "dragover", move |e| {
            e.prevent_default();
            toggle(&d2, "over", true);
        });
        let d3 = drop.clone();
        on(&drop, "dragleave", move |_| toggle(&d3, "over", false));
        let (d4, p2, ts) = (drop.clone(), pid.clone(), task_sel.clone());
        on(&drop, "drop", move |e| {
            e.prevent_default();
            toggle(&d4, "over", false);
            if !drive {
                flash("Drive 권한이 필요해요. 설정 → 외부 연결에서 다시 로그인해 주세요.", true);
                return;
            }
            let de: &web_sys::DragEvent = e.unchecked_ref();
            if let Some(fl) = de.data_transfer().and_then(|d| d.files()) {
                upload_files(fl, p2.clone(), val(&ts));
            }
        });
    }
    let _ = bx.append_child(&el("div").cls("inline").kid(el("span").text("어느 작업의 자료인가요?")).child(task_sel.clone()).0);
    let _ = bx.append_child(&drop);
    let _ = bx.append_child(&input);

    // 링크 붙여 넣기
    let url_in = el("input").attr("type", "url").attr("placeholder", "Drive 공유 링크 (https://drive.google.com/…)").attr("maxlength", "500").build();
    let name_in = el("input").attr("type", "text").attr("placeholder", "이름 (비우면 자동)").attr("maxlength", "200").build();
    let (u2, n2, ts, p2) = (url_in.clone(), name_in.clone(), task_sel.clone(), pid.clone());
    let _ = bx.append_child(
        &el("div")
            .cls("inline wrap")
            .child(url_in)
            .child(name_in)
            .kid(el("button").text("링크 추가").click(move |_| {
                let url = val(&u2);
                if url.trim().is_empty() {
                    flash("Drive 공유 링크를 붙여 넣어 주세요.", true);
                    return;
                }
                act(c("collab:link-file", json!({ "projectId": p2, "taskId": val(&ts), "url": url, "name": val(&n2) })), ok("자료를 붙였어요."));
            }))
            .0,
    );
    bx
}
