//! 내 작업 시간표 카드: 협업 탭과 "일정 조정하기" 대화에서 함께 쓴다.
use super::{arr, b, n, num_text, s};
use crate::api::call;
use crate::dom::*;
use serde_json::{json, Value};
use std::cell::Cell;
use std::future::Future;
use std::rc::Rc;
use web_sys::HtmlElement;

const DAYS: [&str; 7] = ["일", "월", "화", "수", "목", "금", "토"];

pub struct Opts {
    pub compact: bool,
    pub say: Rc<dyn Fn(&str, bool)>,
    pub on_view: Option<Rc<dyn Fn(&Value)>>,
    pub on_chat: Option<Rc<dyn Fn()>>,
}

struct Ctx {
    container: HtmlElement,
    opts: Rc<Opts>,
    busy: Cell<bool>,
}

fn date(ms: f64) -> js_sys::Date {
    js_sys::Date::new(&ms.into())
}

fn p2(n: u32) -> String {
    format!("{n:02}")
}

fn hm(t: f64) -> String {
    let d = date(t);
    format!("{}:{}", p2(d.get_hours()), p2(d.get_minutes()))
}

fn midnight(t: f64) -> f64 {
    let d = date(t);
    d.set_hours(0);
    d.set_minutes(0);
    d.set_seconds(0);
    d.set_milliseconds(0);
    d.get_time()
}

fn day_label(t: f64) -> String {
    let d = date(t);
    let diff = ((midnight(t) - midnight(now())) / 86_400_000.0).round() as i64;
    let rel = match diff {
        0 => " · 오늘",
        1 => " · 내일",
        2 => " · 모레",
        _ => "",
    };
    format!("{}월 {}일 ({}){}", d.get_month() + 1, d.get_date(), DAYS[d.get_day() as usize], rel)
}

fn local_input(t: f64) -> String {
    let d = date(t);
    format!("{}-{}-{}T{}:{}", d.get_full_year(), p2(d.get_month() + 1), p2(d.get_date()), p2(d.get_hours()), p2(d.get_minutes()))
}

fn from_input(v: &str) -> Value {
    if v.is_empty() {
        return Value::Null;
    }
    let t = js_sys::Date::new(&v.into()).get_time();
    if t.is_nan() {
        Value::Null
    } else {
        json!(t)
    }
}

fn num(v: &str) -> f64 {
    v.trim().parse().unwrap_or(0.0)
}

/// 성공 문구: 고정 문구, 또는 결과의 text
#[derive(Clone, Copy)]
enum Done {
    Text(&'static str),
    FromResult,
}

fn run<F: Future<Output = Result<Value, String>> + 'static>(cx: &Rc<Ctx>, fut: F, ok: Done) {
    if cx.busy.get() {
        return;
    }
    cx.busy.set(true);
    toggle(&cx.container, "busy", true);
    let cx = cx.clone();
    spawn(async move {
        match fut.await {
            Ok(r) => {
                let v = if r.get("blocks").is_some() { Some(r.clone()) } else if r.get("view").map(|v| v.is_object()).unwrap_or(false) { Some(r["view"].clone()) } else { None };
                if let Some(v) = v {
                    render(&cx.container, &v, cx.opts.clone());
                    if let Some(f) = &cx.opts.on_view {
                        f(&v);
                    }
                }
                match ok {
                    Done::Text(t) => (cx.opts.say)(t, false),
                    Done::FromResult => (cx.opts.say)(&s(&r, "text"), false),
                }
            }
            Err(e) => (cx.opts.say)(&e, true),
        }
        cx.busy.set(false);
        toggle(&cx.container, "busy", false);
    });
}

fn edit(x: Value) -> impl Future<Output = Result<Value, String>> {
    async move { call("sched:edit", x).await }
}

fn input(kind: &str, value: &str) -> HtmlElement {
    el("input").attr("type", kind).build().tap(|i| set_val(i, value))
}

trait Tap: Sized {
    fn tap(self, f: impl FnOnce(&Self)) -> Self {
        f(&self);
        self
    }
}
impl Tap for HtmlElement {}

/// container 안에 시간표를 그린다
pub fn render(container: &HtmlElement, view: &Value, opts: Rc<Opts>) {
    clear(container);
    let cx = Rc::new(Ctx { container: container.clone(), opts: opts.clone(), busy: Cell::new(false) });
    let blocks = arr(view, "blocks");
    let tasks = arr(view, "tasks");
    let pending = n(view, "pending") as i64;

    let card = el("div").cls(if opts.compact { "sched compact" } else { "sched" }).build();
    let _ = container.append_child(&card);

    let synced = !blocks.is_empty() && pending == 0;
    let state_text = if blocks.is_empty() {
        String::new()
    } else if pending > 0 {
        format!("캘린더 반영 전 {pending}건")
    } else {
        "캘린더와 같아요".into()
    };
    let _ = card.append_child(&el("div").cls("sched-head").kid(el("strong").text("내 작업 시간표")).kid(el("span").cls(&format!("sched-state {}", if pending > 0 { "pending" } else if synced { "ok" } else { "" })).text(&state_text)).0);

    if tasks.is_empty() {
        let _ = card.append_child(&el("p").cls("hint").text("맡은 작업이 생기면 캘린더의 빈 시간을 찾아 \"몇 시부터 몇 시까지\" 할지 짜 드려요. 협업 탭에서 배정된 작업에 \"맡을게요\"를 눌러 주세요.").0);
        return;
    }
    if !b(&view["google"], "ready") {
        let _ = card.append_child(
            &el("p")
                .cls("hint warn")
                .txt("Google 캘린더에 로그인하지 않아 다른 일정은 피하지 못했어요. ")
                .kid(el("button").cls("link").text("설정 → 외부 연결").click(|_| super::show_tab("settings")))
                .0,
        );
    }
    for w in arr(view, "warnings") {
        if s(&w, "kind") != "calendar" {
            let _ = card.append_child(&el("p").cls("hint warn").text(&s(&w, "text")).0);
        }
    }

    // 블록 (날짜별)
    let list = el("div").cls("sched-list").build();
    let mut last_day = String::new();
    for bl in &blocks {
        let start = n(bl, "start");
        let end = n(bl, "end");
        let dl = day_label(start);
        if dl != last_day {
            let _ = list.append_child(&el("div").cls("sched-day").text(&dl).0);
            last_day = dl;
        }
        let mut cls = "sched-row".to_string();
        if b(bl, "late") {
            cls += " late";
        }
        if b(bl, "running") {
            cls += " running";
        }
        let mut tags = vec![];
        if b(bl, "pinned") {
            tags.push("직접 정함");
        }
        if b(bl, "late") {
            tags.push("마감 뒤");
        }
        if b(bl, "running") {
            tags.push("지금");
        }
        if !b(bl, "synced") {
            tags.push("반영 전");
        }
        let row = el("div")
            .cls(&cls)
            .kid(el("span").cls("sched-no").text(&s(bl, "no")))
            .kid(el("span").cls("sched-time").text(&format!("{}–{}", hm(start), hm(end))))
            .kid(el("span").cls("sched-title").text(&s(bl, "title")))
            .kid(el("span").cls("sched-tags").kids(tags.iter().map(|t| el("span").cls(if *t == "반영 전" { "tag soft" } else { "tag" }).text(t))))
            .build();
        if !opts.compact {
            let id = s(bl, "id");
            let si = input("datetime-local", &local_input(start));
            let _ = si.set_attribute("step", "1800");
            let ei = input("datetime-local", &local_input(end));
            let _ = ei.set_attribute("step", "1800");
            let edit_box = el("div").cls("sched-edit hidden").build();
            {
                let (cx, si, ei, id) = (cx.clone(), si.clone(), ei.clone(), id.clone());
                let _ = edit_box.append_child(&si);
                let _ = edit_box.append_child(&el("span").text("~").0);
                let _ = edit_box.append_child(&ei);
                let _ = edit_box.append_child(
                    &el("button")
                        .cls("primary")
                        .text("저장")
                        .click(move |_| run(&cx, edit(json!({ "type": "move", "ref": id, "start": from_input(&val(&si)), "end": from_input(&val(&ei)) })), Done::Text("옮기고 고정했어요. 나머지는 다시 짰어요.")))
                        .0,
                );
            }
            let eb = edit_box.clone();
            let cx2 = cx.clone();
            let _ = row.append_child(
                &el("span")
                    .cls("sched-actions")
                    .kid(el("button").cls("link").text("시간 바꾸기").click(move |_| {
                        let h = has_class(&eb, "hidden");
                        toggle(&eb, "hidden", !h);
                    }))
                    .kid(el("button").cls("link").text("빼기").attr("title", "이 시간은 비우고 다른 빈 시간에 다시 잡아요").click(move |_| run(&cx2, edit(json!({ "type": "remove", "ref": id })), Done::Text("그 시간은 비우고 다른 시간에 다시 잡았어요."))))
                    .0,
            );
            let _ = row.append_child(&edit_box);
        }
        let _ = list.append_child(&row);
    }
    if blocks.is_empty() {
        let _ = list.append_child(&el("p").cls("hint").text("잡힌 시간이 없어요. 위의 알림을 확인해 주세요.").0);
    }
    let _ = card.append_child(&list);

    // 버튼
    let cxa = cx.clone();
    let cxr = cx.clone();
    let mut actions = el("div")
        .cls("inline sched-buttons")
        .kid(
            el("button")
                .cls("primary")
                .text(&if pending > 0 { format!("캘린더·할 일에 넣기 ({pending})") } else { "캘린더·할 일 다시 맞추기".into() })
                .flag("disabled", blocks.is_empty() && pending == 0)
                .click(move |_| run(&cxa, async { call("sched:apply", Value::Null).await }, Done::FromResult)),
        )
        .kid(el("button").text("다시 짜기").click(move |_| run(&cxr, async { call("sched:replan", Value::Null).await }, Done::Text("캘린더를 다시 읽고 짰어요."))));
    if let Some(f) = opts.on_chat.clone() {
        actions = actions.kid(el("button").text("대화로 고치기").click(move |_| f()));
    }
    let _ = card.append_child(&actions.0);
    if opts.compact {
        return;
    }

    // 자세히
    let more = el("details").cls("sched-more").kid(el("summary").text("작업 시간 · 안 되는 시간 · 작업 시간대")).build();
    let tbox = el("div").cls("sched-sub").kid(el("b").text("작업별 필요 시간")).build();
    for t in &tasks {
        let inp = input("number", &num_text(n(t, "need")));
        for (k, v) in [("min", "0"), ("max", "200"), ("step", "0.5"), ("class", "num")] {
            let _ = inp.set_attribute(k, v);
        }
        let mut label = s(t, "title");
        if !s(t, "project").is_empty() {
            label += &format!(" · {}", s(t, "project"));
        }
        if n(t, "deadline") > 0.0 {
            let d = date(n(t, "deadline"));
            label += &format!(" · 마감 {}/{}", d.get_month() + 1, d.get_date());
        }
        let (cx, i2, task_id) = (cx.clone(), inp.clone(), s(t, "taskId"));
        let _ = tbox.append_child(
            &el("div")
                .cls("inline")
                .kid(el("span").cls("grow").text(&label))
                .child(inp)
                .kid(el("span").text(&format!("시간 (잡힘 {})", num_text(n(t, "placed")))))
                .kid(el("button").text("바꾸기").click(move |_| run(&cx, edit(json!({ "type": "hours", "taskId": task_id, "hours": num(&val(&i2)) })), Done::Text("필요 시간을 바꾸고 다시 짰어요."))))
                .0,
        );
    }
    let un = el("div").cls("sched-sub").kid(el("b").text("안 되는 시간")).build();
    for u in arr(view, "unavailable") {
        let mut label = s(&u, "label");
        if !s(&u, "note").is_empty() {
            label += &format!(" · {}", s(&u, "note"));
        }
        let (cx, id) = (cx.clone(), s(&u, "id"));
        let _ = un.append_child(
            &el("div")
                .cls("inline")
                .kid(el("span").cls("grow").text(&label))
                .kid(el("button").cls("link").text("지우기").click(move |_| run(&cx, edit(json!({ "type": "remove-unavailable", "id": id })), Done::Text("지우고 다시 짰어요."))))
                .0,
        );
    }
    let hour = (now() / 3_600_000.0).ceil() * 3_600_000.0;
    let us = input("datetime-local", &local_input(hour));
    let ue = input("datetime-local", &local_input(hour + 2.0 * 3_600_000.0));
    let _ = us.set_attribute("step", "1800");
    let _ = ue.set_attribute("step", "1800");
    let un2 = el("input").attr("type", "text").attr("placeholder", "메모 (예: 수업)").attr("maxlength", "40").build();
    {
        let (cx, us2, ue2, un3) = (cx.clone(), us.clone(), ue.clone(), un2.clone());
        let _ = un.append_child(
            &el("div")
                .cls("inline")
                .child(us)
                .kid(el("span").text("~"))
                .child(ue)
                .child(un2)
                .kid(el("button").text("추가").click(move |_| run(&cx, edit(json!({ "type": "unavailable", "start": from_input(&val(&us2)), "end": from_input(&val(&ue2)), "note": val(&un3) })), Done::Text("그 시간은 비우고 다시 짰어요."))))
                .0,
        );
    }
    let p = &view["prefs"];
    let mk = |k: &str, min: &str, max: &str, step: &str| {
        let i = input("number", &num_text(n(p, k)));
        for (a, v) in [("min", min), ("max", max), ("step", step), ("class", "num")] {
            let _ = i.set_attribute(a, v);
        }
        i
    };
    let f = [("dayStart", mk("dayStart", "0", "23", "1")), ("dayEnd", mk("dayEnd", "1", "24", "1")), ("dailyMaxHours", mk("dailyMaxHours", "0.5", "16", "0.5")), ("maxBlockHours", mk("maxBlockHours", "0.5", "8", "0.5"))];
    let fc = f.clone();
    let cxp = cx.clone();
    let cxz = cx.clone();
    let pr = el("div")
        .cls("sched-sub")
        .kid(el("b").text("작업 시간대"))
        .kid(
            el("div")
                .cls("inline wrap")
                .child(f[0].1.clone())
                .kid(el("span").text("시 ~"))
                .child(f[1].1.clone())
                .kid(el("span").text("시, 하루 최대"))
                .child(f[2].1.clone())
                .kid(el("span").text("시간, 한 번에 최대"))
                .child(f[3].1.clone())
                .kid(el("span").text("시간"))
                .kid(el("button").text("저장").click(move |_| {
                    let mut patch = serde_json::Map::new();
                    for (k, i) in &fc {
                        patch.insert((*k).into(), json!(num(&val(i))));
                    }
                    run(&cxp, edit(json!({ "type": "prefs", "patch": patch })), Done::Text("작업 시간대를 바꾸고 다시 짰어요."));
                })),
        )
        .kid(el("button").cls("link").text("직접 정한 시간·뺀 시간 모두 풀고 처음부터").click(move |_| {
            if confirm("직접 옮기거나 뺀 시간을 모두 풀고 다시 짤까요?") {
                run(&cxz, edit(json!({ "type": "reset" })), Done::Text("처음부터 다시 짰어요."));
            }
        }))
        .build();
    let _ = more.append_child(&tbox);
    let _ = more.append_child(&un);
    let _ = more.append_child(&pr);
    let _ = card.append_child(&more);
}
