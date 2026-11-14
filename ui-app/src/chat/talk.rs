//! 대화 탭: 말 주고받기, 답 이어 받기(스트리밍), 정리 카드, 공유 범위, 일정 조정 대화.
use super::{arr, b, s, schedule, st, topic_label};
use crate::api::{call, listen};
use crate::dom::*;
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{Element, HtmlElement, HtmlTemplateElement, KeyboardEvent};

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
    for id in ["skipBtn", "summarizeBtn"] {
        if let Some(e) = by_id(id) {
            set_disabled(&e, on);
        }
    }
}

fn focus_input() {
    let _ = input().focus();
}

fn update_helpers() {
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
        let _ = dl.append_child(&el("dd").cls("hint").text("건강·가족·평가처럼 민감할 수 있는 내용이 보여서, 자동 반영은 기본으로 꺼 둘게요.").0);
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
        let (c2, summ, sid) = (card.clone(), summary.clone(), session_id.to_string());
        on(&share_btn, "click", move |_| spawn(open_share_panel(sid.clone(), summ.clone(), c2.clone())));
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
    let t = topic.clone();
    box_ = box_.kid(el("button").cls("primary").text("팀 약속으로 맞춰 보자고 제안").click(move |e| {
        disable_target(&e);
        let t = t.clone();
        spawn(async move {
            let connected = call("team:propose", json!(t)).await.map(|r| b(&r, "connected")).unwrap_or(false);
            add_msg("cat", &if connected { format!("\"{}\"에 대해 팀이 함께 맞출 약속을 제안해 둘게요. 누가 제안했는지는 보이지 않아요.", topic_label(&t)) } else { "팀에 연결되면 이 제안이 전달돼요. 설정에서 팀을 연결해 주세요.".into() }, "");
        });
    }));
    let _ = card.append_child(&box_.0);
}

// ---------- 공유 범위 ----------
const SCOPES: &[(&str, &str, &str)] = &[("private", "비공개로 두기", "이 주제는 어디에도 반영하지 않아요."), ("review", "검토 후 공유", "보내기 전에 일반화된 문안을 한 번 더 확인해요."), ("auto", "허용한 주제로 자동 반영", "상대의 고양이가 평소 조언 속에 자연스럽게 섞어 말해요.")];
const TARGETS: &[(&str, &str)] = &[("person", "관련된 팀원의 고양이에게"), ("team", "팀 공통 약속으로 제안")];

#[derive(Clone)]
struct Choice {
    scope: String,
    target: String,
    to_member: Option<String>,
}

fn member_options(sel: &Element, members: &[Value], empty_text: &str) {
    clear(sel);
    let _ = sel.append_child(&el("option").attr("value", "").text(empty_text).0);
    for m in members {
        let _ = sel.append_child(&el("option").attr("value", &s(m, "uid")).text(&s(m, "displayName")).0);
    }
}

fn radio(name: &str, value: &str, checked_: bool, disabled: bool, on_change: impl FnMut(web_sys::Event) + 'static) -> El {
    el("input").attr("type", "radio").attr("name", name).attr("value", value).flag("checked", checked_).flag("disabled", disabled).on("change", on_change)
}

async fn open_share_panel(session_id: String, summary: Rc<RefCell<Value>>, card: HtmlElement) {
    if let Some(old) = q_in(&card, ".share-panel") {
        old.remove();
        return;
    }
    if str_list(&summary.borrow()["topics"]).is_empty() {
        let _ = card.append_child(&el("p").cls("hint share-panel").text("먼저 위에서 반영할 일반 주제를 하나 이상 골라 주세요.").0);
        return;
    }
    let preview = match call("share:preview", json!({ "id": session_id })).await {
        Ok(p) => p,
        Err(e) => {
            add_msg("cat", &e, "safety");
            return;
        }
    };
    let panel = el("div").cls("share-panel").build();
    let _ = panel.append_child(&el("p").cls("hint").text("전달되는 건 아래의 일반적인 문안뿐이에요. 내가 쓴 원문, 이름, 날짜, 구체적인 사건은 들어가지 않아요.").0);
    let choices: Rc<RefCell<Vec<(String, Choice)>>> = Rc::new(RefCell::new(vec![]));
    let share_team = Rc::new(RefCell::new(s(&preview, "defaultTeamId")));
    let teams = arr(&preview, "teams");
    let team_by_id: Rc<HashMap<String, Value>> = Rc::new(teams.iter().map(|t| (s(t, "teamId"), t.clone())).collect());
    if teams.len() > 1 {
        // 팀플이 여럿이면 어느 팀 이야기인지 고른다. 받을 팀원 목록도 그 팀으로 바뀐다
        let (p2, ch, stid, tb) = (panel.clone(), choices.clone(), share_team.clone(), team_by_id.clone());
        let sel = el("select").cls("team-pick").build();
        for t in &teams {
            let _ = sel.append_child(&el("option").attr("value", &s(t, "teamId")).text(&format!("{} ({}명)", s(t, "name"), s(t, "memberCount"))).flag("selected", s(t, "teamId") == *share_team.borrow()).0);
        }
        let sel2 = sel.clone();
        on(&sel, "change", move |_| {
            let id = val(&sel2);
            *stid.borrow_mut() = id.clone();
            let members = tb.get(&id).map(|t| arr(t, "members")).unwrap_or_default();
            for r in q_all_in(&p2, "select.recipient") {
                member_options(&r, &members, "받을 팀원 고르기");
            }
            for (_, c) in ch.borrow_mut().iter_mut() {
                c.to_member = None;
            }
        });
        let _ = panel.append_child(&el("div").cls("label").text("어느 팀의 이야기인가요?").0);
        let _ = panel.append_child(&sel);
    } else if preview["team"].is_object() {
        let _ = panel.append_child(&el("p").cls("hint").text(&format!("연결된 팀: {} ({}명)", s(&preview["team"], "name"), s(&preview["team"], "memberCount"))).0);
    } else {
        let _ = panel.append_child(&el("div").cls("warn").text("아직 팀에 연결되지 않았어요. 저장해 두면 팀에 연결한 뒤 받을 사람을 골라 보낼 수 있어요.").0);
    }
    for (idx, p) in arr(&preview, "items").into_iter().enumerate() {
        let topic = s(&p, "topic");
        let sensitive = b(&p, "sensitive");
        let default_scope = s(&p, "defaultScope");
        choices.borrow_mut().push((topic.clone(), Choice { scope: default_scope.clone(), target: "person".into(), to_member: None }));
        let opt = el("div").cls("option").kid(el("div").cls("topic-name").text(&s(&p, "label"))).build();
        let mut scopes = el("div").cls("scopes");
        for (v, label, desc) in SCOPES {
            let disabled = *v == "auto" && sensitive;
            let ch = choices.clone();
            let lbl = el("label")
                .attr("title", desc)
                .kid(radio(&format!("scope-{topic}"), v, default_scope == *v, disabled, move |_| ch.borrow_mut()[idx].1.scope = v.to_string()))
                .txt(&format!(" {label}"))
                .txt(if disabled { " (민감할 수 있어 제외)" } else { "" });
            scopes = scopes.kid(lbl);
        }
        let mut targets = el("div").cls("scopes");
        for (v, label) in TARGETS {
            let ch = choices.clone();
            targets = targets.kid(el("label").kid(radio(&format!("target-{topic}"), v, *v == "person", false, move |_| ch.borrow_mut()[idx].1.target = v.to_string())).txt(&format!(" {label}")));
        }
        let _ = opt.append_child(&scopes.0);
        let _ = opt.append_child(&el("div").cls("label").text("전달 방식").0);
        let _ = opt.append_child(&targets.0);
        // 팀에 연결돼 있으면 항상 그린다 (고른 팀에 따라 목록이 바뀌므로)
        if !teams.is_empty() {
            let ms = team_by_id.get(&*share_team.borrow()).map(|t| arr(t, "members")).unwrap_or_else(|| arr(&preview, "members"));
            let sel = el("select").cls("recipient").build();
            member_options(&sel, &ms, if ms.is_empty() { "이 팀에는 아직 다른 팀원이 없어요" } else { "받을 팀원 고르기" });
            let (ch, s2) = (choices.clone(), sel.clone());
            on(&sel, "change", move |_| {
                let v = val(&s2);
                ch.borrow_mut()[idx].1.to_member = if v.is_empty() { None } else { Some(v) };
            });
            let _ = opt.append_child(&el("div").cls("label").text("관련된 팀원 (상대에게 내 이름은 보이지 않아요)").0);
            let _ = opt.append_child(&sel);
        }
        let _ = opt.append_child(&el("div").cls("preview").text(&format!("상대의 고양이가 이렇게 말할 수 있어요: “{}”", s(&p, "advice"))).0);
        let _ = opt.append_child(&el("div").cls("preview").text(&format!("팀 공통 제안으로는: “{}”", s(&p, "teamProposal"))).0);
        if sensitive {
            let _ = opt.append_child(&el("div").cls("warn").text("민감할 수 있는 내용이 있어 자동 반영은 할 수 없어요.").0);
        }
        let _ = panel.append_child(&opt);
    }
    if !s(&preview, "smallTeamWarning").is_empty() {
        let _ = panel.append_child(&el("div").cls("warn").text(&s(&preview, "smallTeamWarning")).0);
    }
    let p2 = panel.clone();
    let save = el("button").cls("primary").text("공유 대기함에 저장").click(move |_| {
        let list: Vec<Value> = choices.borrow().iter().map(|(t, c)| json!({ "topic": t, "scope": c.scope, "target": c.target, "toMember": c.to_member })).collect();
        let args = json!({ "id": session_id, "teamId": *share_team.borrow(), "choices": list });
        let p3 = p2.clone();
        spawn(async move {
            match call("share:save", args).await {
                Ok(res) => {
                    p3.remove();
                    let saved = res["saved"].as_i64().unwrap_or(0);
                    let team_name = s(&res, "teamName");
                    let text = if saved == 0 {
                        "모두 비공개로 두었어요. 언제든 정리함에서 다시 정할 수 있어요.".to_string()
                    } else if b(&res, "connected") {
                        format!("{}{saved}개 주제를 공유 대기함에 넣었어요. 원문과 이름은 들어가지 않고, 시간을 조금 두고 전달돼요. \"검토 후 공유\"는 정리함에서 확인한 뒤에 보내져요.", if team_name.is_empty() { String::new() } else { format!("\"{team_name}\" 팀으로 ") })
                    } else {
                        format!("{saved}개 주제를 공유 대기함에 넣어 두었어요. 팀에 연결되면 전달돼요.")
                    };
                    add_msg("cat", &text, "");
                }
                Err(e) => {
                    add_msg("cat", &e, "safety");
                }
            }
        });
    });
    let _ = panel.append_child(&el("div").cls("card-actions").kid(el("span")).kid(save).0);
    let _ = card.append_child(&panel);
    scroll_into_view(&panel, true);
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
