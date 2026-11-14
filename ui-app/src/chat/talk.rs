//! 대화 탭: 말 주고받기, 답 이어 받기(스트리밍), 정리 카드, 공유 범위, 일정 조정 대화.
pub(super) mod delivery;

use super::{arr, b, s, schedule, st, topic_label};
use crate::api::{call, listen};
use crate::dom::*;
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{HtmlElement, HtmlTemplateElement, KeyboardEvent};

fn messages() -> HtmlElement {
    by_id("messages").unwrap()
}

fn input() -> HtmlElement {
    by_id("input").unwrap()
}

fn scroll_bottom(e: &HtmlElement) {
    e.set_scroll_top(e.scroll_height());
}

pub fn add_msg(role: &str, text: &str, extra: &str) -> HtmlElement {
    let m = el("div").cls(format!("msg {role} {extra}").trim()).text(text).build();
    let box_ = messages();
    let _ = box_.append_child(&m);
    scroll_bottom(&box_);
    if role == "cat" {
        let len = text.chars().count() as f64;
        st(|s| s.talking_until = now() + (400.0 + len * 40.0).min(4000.0));
    }
    m
}

fn set_busy(on: bool) {
    st(|s| s.busy = on);
    // 보내기 버튼은 막지 않는다 — 답을 기다리는 중에 보낸 말은 줄을 세워 두었다가 보낸다
    for id in ["skipBtn", "summarizeBtn", "complaintDelivery", "clearScheduleChat"] {
        if let Some(e) = by_id(id) {
            set_disabled(&e, on);
        }
    }
}

fn focus_input() {
    let _ = input().focus();
}

fn update_helpers() {
    if let Some(button) = by_id("clearScheduleChat") { toggle(&button, "hidden", session_mode() != "schedule"); }
    let can_share = st(|x| x.session.as_ref().is_some_and(|v| v["mode"] == "complaint" && v["step"] != "open" && v["step"] != "done" && v["safetyHold"].is_null()));
    toggle(&by_id("complaintDelivery").unwrap(), "hidden", !can_share);
    let guided = st(|x| x.session.as_ref().map(|s| { let m = s["mode"].as_str().unwrap_or(""); m != "free" && m != "schedule" && s["step"] != "done" }).unwrap_or(false));
    for id in ["skipBtn", "summarizeBtn"] {
        if let Some(e) = by_id(id) {
            toggle(&e, "hidden", !guided);
        }
    }
}

fn open_composer() {
    toggle(&by_id("modePicker").unwrap(), "hidden", true);
    toggle(&by_id("composer").unwrap(), "hidden", false);
}

fn session_mode() -> String {
    st(|x| x.session.as_ref().map(|s| s["mode"].as_str().unwrap_or("").to_string()).unwrap_or_default())
}

// ---------- 일정 조정 ----------
fn show_sched_card(view: &Value) {
    if let Some(old) = st(|s| s.sched_card.take()) {
        old.remove();
    }
    let card = el("div").cls("msg-card").build();
    schedule::render(
        &card,
        view,
        Rc::new(schedule::Opts {
            compact: true,
            say: Rc::new(|t, bad| {
                add_msg("cat", t, if bad { "safety" } else { "" });
            }),
            on_view: None,
            on_chat: None,
        }),
    );
    let box_ = messages();
    let _ = box_.append_child(&card);
    scroll_bottom(&box_);
    st(|s| s.sched_card = Some(card));
}

pub async fn start_schedule() {
    delivery::close(false);
    super::show_tab("chat");
    st(|s| {
        s.queue.clear();
        s.sched_card = None;
        s.session = Some(json!({ "id": "schedule", "mode": "schedule", "step": "talk" }));
    });
    clear(&messages());
    open_composer();
    update_helpers();
    set_busy(true);
    let typing = add_msg("cat", "…", "typing");
    match call("sched:view", Value::Null).await {
        Ok(v) => {
            typing.remove();
            let chat = arr(&v, "chat");
            for m in chat.iter().skip(chat.len().saturating_sub(8)) {
                add_msg(if s(m, "role") == "user" { "user" } else { "cat" }, &s(m, "text"), "");
            }
            let tasks = arr(&v, "tasks").len();
            if tasks == 0 {
                add_msg("cat", "아직 맡은 작업이 없어요. 협업 탭에서 작업을 맡으면 몇 시부터 몇 시까지 할지 짜 드릴게요.", "");
            } else {
                let mut t = format!("맡은 작업 {tasks}개를 {}번에 나눠 잡아 봤어요. 틀린 곳이 있으면 편하게 말해 주세요. 예: \"2번 목요일 3시로 옮겨줘\", \"수요일은 안 돼\", \"자료조사 3시간이면 돼\". 괜찮으면 \"캘린더에 넣어줘\"라고 해 주세요.", arr(&v, "blocks").len());
                if !b(&v, "ai") {
                    t += " (Claude API 키를 넣으면 더 자유롭게 말해도 알아들어요.)";
                }
                add_msg("cat", &t, "");
            }
            show_sched_card(&v);
        }
        Err(e) => typing.set_text_content(Some(&format!("시간표를 불러오지 못했어요: {e}"))),
    }
    set_busy(false);
    focus_input();
    drain_queue();
}

async fn send_schedule(text: String, shown: bool) {
    if !shown {
        add_msg("user", &text, "");
    }
    set_busy(true);
    let typing = add_msg("cat", "…", "typing");
    match call("sched:chat", json!({ "text": text })).await {
        Ok(r) => {
            typing.remove();
            add_msg("cat", &s(&r, "reply"), "");
            show_sched_card(&r["view"]);
        }
        Err(e) => typing.set_text_content(Some(&format!("고치지 못했어요: {e}"))),
    }
    set_busy(false);
    focus_input();
    drain_queue();
}

pub async fn start_mode(mode: String, topic: Option<String>) {
    if mode == "promise" {
        return delivery::open("promise", "promise".into(), None, None).await;
    }
    delivery::close(false);
    if mode == "schedule" {
        return start_schedule().await;
    }
    st(|s| s.queue.clear());
    clear(&messages());
    open_composer();
    set_busy(true);
    let typing = add_msg("cat", "…", "typing");
    match call("chat:start", json!({ "mode": mode, "topic": topic })).await {
        Ok(res) => {
            st(|s| s.session = Some(res["session"].clone()));
            typing.remove();
            for r in arr(&res, "replies") {
                add_msg("cat", &s(&r, "text"), "");
            }
            update_helpers();
        }
        Err(e) => typing.set_text_content(Some(&format!("대화를 시작하지 못했어요: {e}"))),
    }
    set_busy(false);
    focus_input();
    drain_queue();
}

fn send(skip: bool) {
    if st(|s| s.session.is_none()) {
        return;
    }
    let inp = input();
    let text = val(&inp).trim().to_string();
    if text.is_empty() && !skip {
        return;
    }
    set_val(&inp, "");
    if st(|s| s.busy) {
        if !text.is_empty() {
            st(|s| s.queue.push_back(text.clone()));
            add_msg("user", &text, "queued");
        }
        return;
    }
    if session_mode() == "schedule" {
        spawn(send_schedule(text, false));
        return;
    }
    if !text.is_empty() {
        add_msg("user", &text, "");
    }
    spawn(send_now(text, skip));
}

async fn send_now(text: String, skip: bool) {
    set_busy(true);
    let typing = add_msg("cat", "…", "typing");
    let stream_id = st(|s| {
        s.stream_seq += 1;
        let id = format!("s{}", s.stream_seq);
        s.streaming = Some((id.clone(), typing.clone()));
        id
    });
    let id = st(|x| x.session.as_ref().map(|v| s(v, "id")).unwrap_or_default());
    let res = call("chat:send", json!({ "id": id, "text": text, "skip": skip, "streamId": stream_id })).await;
    st(|s| s.streaming = None);
    match res {
        Ok(res) => {
            st(|s| s.session = Some(res["session"].clone()));
            let replies = arr(&res, "replies");
            if replies.len() == 1 && b(&replies[0], "streamed") {
                toggle(&typing, "typing", false);
                typing.set_text_content(Some(&s(&replies[0], "text")));
            } else {
                typing.remove();
                for r in &replies {
                    add_msg("cat", &s(r, "text"), if b(r, "safety") { "safety" } else { "" });
                }
            }
            if res["summary"].is_object() {
                render_summary(res["summary"].clone(), &id, &messages(), false);
            }
            update_helpers();
        }
        Err(e) => {
            toggle(&typing, "typing", false);
            typing.set_text_content(Some(&format!("답하지 못했어요: {e}")));
        }
    }
    set_busy(false);
    focus_input();
    drain_queue();
}

fn drain_queue() {
    let next = st(|s| if s.busy || s.session.is_none() { None } else { s.queue.pop_front() });
    let Some(next) = next else { return };
    if let Some(q) = q_in(&messages(), ".msg.user.queued") {
        toggle(&q, "queued", false);
    }
    if session_mode() == "schedule" {
        spawn(send_schedule(next, true));
    } else {
        spawn(send_now(next, false));
    }
}

pub fn bind() {
    on(&by_id("complaintDelivery").unwrap(), "click", |_| {
        if let Some(id) = st(|x| x.session.as_ref().map(|v| s(v, "id"))) {
            spawn(delivery::open("complaint", id, None, None));
        }
    });
    on(&by_id("notesPromise").unwrap(), "click", |_| spawn(start_mode("promise".into(), None)));
    listen("chat:stream", |v| {
        let id = s(&v, "streamId");
        let Some(bubble) = st(|s| s.streaming.as_ref().filter(|(sid, _)| *sid == id).map(|(_, b)| b.clone())) else { return };
        toggle(&bubble, "typing", false);
        bubble.set_text_content(Some(&s(&v, "text")));
        st(|s| s.talking_until = now() + 600.0);
        let m = messages();
        if m.scroll_height() - m.scroll_top() - m.client_height() < 120 {
            scroll_bottom(&m);
        }
    });
    on(&by_id("modePicker").unwrap(), "click", |e| {
        if let Some(t) = event_target(&e) {
            if let Ok(Some(b)) = t.closest("button[data-mode]") {
                spawn(start_mode(b.get_attribute("data-mode").unwrap_or_default(), None));
            }
        }
    });
    on(&by_id("sendBtn").unwrap(), "click", |_| send(false));
    on(&by_id("skipBtn").unwrap(), "click", |_| send(true));
    on(&by_id("summarizeBtn").unwrap(), "click", |_| {
        let Some(id) = st(|x| x.session.as_ref().map(|v| s(v, "id"))) else { return };
        spawn(async move {
            let Ok(summary) = call("chat:summarize-now", json!(id)).await else { return };
            if !summary.is_object() {
                return;
            }
            st(|s| {
                if let Some(x) = s.session.as_mut() {
                    x["step"] = json!("done");
                }
            });
            add_msg("cat", "지금까지 들은 내용으로 정리해 봤어요. 비어 있는 부분은 나중에 채워도 괜찮아요.", "");
            render_summary(summary, &id, &messages(), false);
            update_helpers();
        });
    });
    on(&by_id("clearScheduleChat").unwrap(), "click", |_| {
        if session_mode() != "schedule" || !confirm("일정 조정 대화 기록을 지울까요? 계획한 일정은 유지됩니다.") { return; }
        spawn(async {
            match call("sched:clear-chat", Value::Null).await {
                Ok(_) if session_mode() == "schedule" => start_schedule().await,
                Ok(_) => {}
                Err(error) => { add_msg("cat", &error, "safety"); }
            }
        });
    });
    on(&by_id("newBtn").unwrap(), "click", |_| {
        st(|s| s.session = None);
        clear(&messages());
        toggle(&by_id("composer").unwrap(), "hidden", true);
        toggle(&by_id("modePicker").unwrap(), "hidden", false);
    });
    on(&input(), "keydown", |e| {
        let k: &KeyboardEvent = e.unchecked_ref();
        if k.key() == "Enter" && !k.shift_key() && !k.is_composing() {
            e.prevent_default();
            send(false);
        }
    });
}

// ---------- 정리 카드 ----------
const COMPLAINT_FIELDS: &[(&str, &str, &str)] = &[
    ("내가 이야기한 사건", "reportedEvent", "text"),
    ("함께 정한 약속이었는지", "agreement", "text"),
    ("", "agreementNote", "note"),
    ("공동 기록", "sharedRecord", "note"),
    ("감정", "emotions", "join"),
    ("실제 영향", "impact", "text"),
    ("기대·필요", "needs", "text"),
    ("가능한 행동 요청", "actionRequests", "list"),
    ("사람 대신 행동으로 바꿔 말하면", "reframes", "reframes"),
    ("아직 확인할 점", "toConfirm", "list"),
];
const EXPLAIN_FIELDS: &[(&str, &str, &str)] = &[("상황", "situation", "text"), ("그때의 사정", "reason", "text"), ("다음엔 이렇게", "nextTime", "text"), ("감정", "emotions", "join"), ("", "note", "note")];

fn str_list(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().map(|x| x.as_str().unwrap_or("").to_string()).collect()).unwrap_or_default()
}

pub fn render_summary(summary: Value, session_id: &str, container: &HtmlElement, read_only: bool) -> HtmlElement {
    let tpl: HtmlTemplateElement = by_id("summaryTpl").unwrap().unchecked_into();
    let card: HtmlElement = tpl.content().first_element_child().unwrap().clone_node_with_deep(true).unwrap().unchecked_into();
    let summary = Rc::new(RefCell::new(summary));
    let sm = summary.borrow().clone();
    let dl = q_in(&card, ".fields").unwrap();
    let explain = s(&sm, "mode") == "explain";
    for (label, key, kind) in if explain { EXPLAIN_FIELDS } else { COMPLAINT_FIELDS } {
        let v = &sm[*key];
        let empty = match v {
            Value::Null => true,
            Value::String(x) => x.is_empty(),
            Value::Array(a) => a.is_empty(),
            _ => false,
        };
        if empty {
            continue;
        }
        if !label.is_empty() {
            let _ = dl.append_child(&el("dt").text(label).0);
        }
        let dd = match *kind {
            "list" => el("dd").kid(el("ul").kids(str_list(v).iter().map(|x| el("li").text(x)))),
            "reframes" => el("dd").kid(el("ul").kids(v.as_array().unwrap().iter().map(|r| el("li").text(&format!("{} → {}", s(r, "from"), s(r, "to")))))),
            "join" => el("dd").text(&str_list(v).join(", ")),
            "note" => el("dd").cls("hint").text(v.as_str().unwrap_or("")),
            _ => el("dd").attr("data-key", key).text(v.as_str().unwrap_or("")),
        };
        let _ = dl.append_child(&dd.0);
    }
    if b(&sm, "profanity") {
        let _ = dl.append_child(&el("dd").cls("hint").text("거친 표현은 정리에서 뺐어요. 그만큼 힘들었다는 마음만 남겨 둘게요.").0);
    }
    if b(&sm, "sensitive") {
        let _ = dl.append_child(&el("dd").cls("hint").text("건강·가족·평가처럼 민감할 수 있는 내용이 있어요. 공유할 문안을 확인해 주세요.").0);
    }
    if b(&sm, "aiAssisted") {
        let _ = dl.append_child(&el("dd").cls("hint").text("AI가 정리를 도왔어요. 틀린 부분은 고쳐 주세요.").0);
    }

    // 주제 칩
    let chips = q_in(&card, ".chips").unwrap();
    let selected: Rc<RefCell<BTreeSet<String>>> = Rc::new(RefCell::new(str_list(&sm["topics"]).into_iter().collect()));
    for t in st(|s| s.topics.clone()) {
        let tid = s(&t, "id");
        let on_ = selected.borrow().contains(&tid);
        let chip = el("button").cls(if on_ { "chip on" } else { "chip" }).text(&s(&t, "label")).flag("disabled", read_only).build();
        let (c2, sel, summ, sid) = (chip.clone(), selected.clone(), summary.clone(), session_id.to_string());
        on(&chip, "click", move |_| {
            {
                let mut set = sel.borrow_mut();
                if !set.remove(&tid) {
                    set.insert(tid.clone());
                }
            }
            let on_now = sel.borrow().contains(&tid);
            toggle(&c2, "on", on_now);
            let topics: Vec<String> = sel.borrow().iter().cloned().collect();
            summ.borrow_mut()["topics"] = json!(topics);
            let sid = sid.clone();
            spawn(async move {
                let _ = call("chat:update-summary", json!({ "id": sid, "patch": { "topics": topics } })).await;
            });
        });
        let _ = chips.append_child(&chip);
    }

    let edit_btn = q_in(&card, ".edit").unwrap();
    let share_btn = q_in(&card, ".share").unwrap();
    if read_only {
        edit_btn.remove();
    } else {
        let (eb, c2, summ, sid) = (edit_btn.clone(), card.clone(), summary.clone(), session_id.to_string());
        on(&edit_btn, "click", move |_| {
            let editing = eb.get_attribute("data-editing").as_deref() == Some("1");
            let dds = q_all_in(&c2, "dd[data-key]");
            if !editing {
                for d in &dds {
                    let _ = d.set_attribute("contenteditable", "true");
                }
                eb.set_text_content(Some("고친 내용 저장"));
                let _ = eb.set_attribute("data-editing", "1");
                if let Some(d) = dds.first() {
                    let _ = d.focus();
                }
            } else {
                let mut patch = serde_json::Map::new();
                for d in &dds {
                    let _ = d.remove_attribute("contenteditable");
                    let k = d.get_attribute("data-key").unwrap_or_default();
                    let v = d.text_content().unwrap_or_default().trim().to_string();
                    summ.borrow_mut()[k.as_str()] = json!(v);
                    patch.insert(k, json!(v));
                }
                eb.set_text_content(Some("내용 고치기"));
                let _ = eb.set_attribute("data-editing", "0");
                let sid = sid.clone();
                spawn(async move {
                    let _ = call("chat:update-summary", json!({ "id": sid, "patch": patch })).await;
                });
            }
        });
    }
    if explain {
        share_btn.remove();
        if !read_only {
            add_explain_actions(&sm, &card);
        }
    } else if s(&sm, "mode") != "complaint" {
        share_btn.remove();
    } else {
        let sid = session_id.to_string();
        on(&share_btn, "click", move |_| spawn(delivery::open("complaint", sid.clone(), None, None)));
    }
    let _ = container.append_child(&card);
    scroll_bottom(container);
    card
}

fn disable_target(e: &web_sys::Event) {
    if let Some(t) = event_target(e) {
        set_disabled(&t, true);
    }
}

// ---------- 내 사정 정리 후 할 수 있는 일 ----------
fn add_explain_actions(summary: &Value, card: &HtmlElement) {
    let related = s(summary, "relatedTopic");
    let topic = if related.is_empty() { str_list(&summary["topics"]).into_iter().next().unwrap_or_default() } else { related.clone() };
    if topic.is_empty() {
        return;
    }
    let mut box_ = el("div").cls("card-actions");
    if !related.is_empty() {
        let t = topic.clone();
        box_ = box_.kid(el("button").text("이 주제 조언 잠시 멈추기").click(move |e| {
            disable_target(&e);
            let t = t.clone();
            spawn(async move {
                let held = call("team:hold-topic", json!(t)).await.map(|r| r["held"].as_f64().unwrap_or(0.0) > 0.0 || r["held"] == true).unwrap_or(false);
                let l = topic_label(&t);
                add_msg("cat", &if held { format!("\"{l}\" 조언은 잠시 멈출게요. 사정을 모른 채 같은 말을 반복하지 않도록요.") } else { format!("\"{l}\" 조언을 줄여 둘게요. (팀에서 받은 관련 주제가 없어서 내 쪽에서만 줄였어요)") }, "");
            });
        }));
    }
    let seed = format!("{}에 대해 팀 약속을 만들고 싶어요.", topic_label(&topic));
    box_ = box_.kid(el("button").cls("primary").text("팀 약속 잡기").click(move |_| {
        spawn(delivery::open("promise", "promise".into(), None, Some(seed.clone())));
    }));
    let _ = card.append_child(&box_.0);
}

// ---------- 개발용 시연 ----------
pub fn expose_demo() {
    let o = js_sys::Object::new();
    let set = |k: &str, f: JsValue| {
        let _ = js_sys::Reflect::set(&o, &JsValue::from_str(k), &f);
    };
    set("startMode", Closure::<dyn Fn(String)>::new(|m: String| spawn(start_mode(m, None))).into_js_value());
    set("startSchedule", Closure::<dyn Fn()>::new(|| spawn(start_schedule())).into_js_value());
    set(
        "send",
        Closure::<dyn Fn(String)>::new(|t: String| {
            set_val(&input(), &t);
            send(false);
        })
        .into_js_value(),
    );
    set("showTab", Closure::<dyn Fn(String)>::new(|t: String| super::show_tab(&t)).into_js_value());
    set("renderTeam", Closure::<dyn Fn()>::new(|| spawn(super::settings::render_team(None))).into_js_value());
    let _ = js_sys::Reflect::set(&window(), &JsValue::from_str("__saiDemo"), &o);
}
