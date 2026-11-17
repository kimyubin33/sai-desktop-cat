//! 팀 전송 기능 없이 개인 대화와 기존 로컬 기록만 제공한다.
use super::{arr, s};
use crate::{
    api::{call, listen},
    dom::*,
};
use serde_json::{json, Value};
use std::cell::RefCell;
use wasm_bindgen::JsCast;
use web_sys::{HtmlElement, KeyboardEvent};
#[derive(Default)]
struct State {
    id: String,
    busy: bool,
    seq: u64,
    stream: Option<(String, HtmlElement)>,
}
thread_local! {static STATE:RefCell<State>=RefCell::new(State::default());}
fn state<R>(f: impl FnOnce(&mut State) -> R) -> R {
    STATE.with(|v| f(&mut v.borrow_mut()))
}
fn messages() -> HtmlElement {
    by_id("messages").unwrap()
}
fn error(text: &str) {
    set_text("aiError", text);
    toggle(&by_id("aiError").unwrap(), "hidden", text.is_empty());
}
fn busy(on: bool) {
    state(|v| v.busy = on);
    for e in q_all("#sendBtn, #newConversation, [data-prompt]") {
        set_disabled(&e, on);
    }
    let _ = messages().set_attribute("aria-busy", if on { "true" } else { "false" });
}
fn add(role: &str, text: &str) -> HtmlElement {
    let e = el("div").cls(&format!("msg {role}")).text(text).build();
    let m = messages();
    let _ = m.append_child(&e);
    m.set_scroll_top(m.scroll_height());
    e
}
fn reset() {
    if state(|v| v.busy) {
        return;
    }
    state(|v| {
        v.id.clear();
        v.stream = None;
    });
    clear(&messages());
    error("");
    let welcome = el("div")
        .cls("ai-welcome")
        .id("aiWelcome")
        .kid(el("h2").text("어떤 생각을 나눌까요?"))
        .kid(el("p").text("새로운 생각을 편하게 남겨 주세요."))
        .build();
    let _ = messages().append_child(&welcome);
    let _ = by_id("input").unwrap().focus();
}
async fn send_text(text: String) {
    error("");
    if state(|v| v.id.is_empty()) {
        match call("chat:start", json!({"mode":"free"})).await {
            Ok(v) => {
                state(|st| st.id = s(&v["session"], "id"));
                if let Some(w) = by_id("aiWelcome") {
                    w.remove();
                }
                for r in arr(&v, "replies") {
                    add("cat", &s(&r, "text"));
                }
            }
            Err(e) => {
                error(&e);
                set_val(&by_id("input").unwrap(), &text);
                busy(false);
                return;
            }
        }
    }
    add("user", &text);
    let typing = add("cat", "답을 정리하고 있어요…");
    toggle(&typing, "typing", true);
    let (id, stream_id) = state(|v| {
        v.seq += 1;
        let stream_id = format!("personal-{}", v.seq);
        v.stream = Some((stream_id.clone(), typing.clone()));
        (v.id.clone(), stream_id)
    });
    let result = call(
        "chat:send",
        json!({"id":id,"text":text,"streamId":stream_id}),
    )
    .await;
    state(|v| v.stream = None);
    toggle(&typing, "typing", false);
    match result {
        Ok(v) => {
            typing.remove();
            for reply in arr(&v, "replies") {
                add("cat", &s(&reply, "text"));
            }
        }
        Err(e) => {
            typing.remove();
            error(&e);
            set_val(&by_id("input").unwrap(), &text);
        }
    }
    busy(false);
    if q("#tab-chat.active").is_some() {
        let _ = by_id("input").unwrap().focus();
    }
}
fn send() {
    if state(|v| v.busy) {
        return;
    }
    let input = by_id("input").unwrap();
    let text = val(&input).trim().to_owned();
    if text.is_empty() {
        return;
    }
    set_val(&input, "");
    busy(true);
    spawn(send_text(text));
}
fn date(v: &Value) -> String {
    let d = js_sys::Date::new(&v.as_f64().unwrap_or(0.0).into());
    format!(
        "{}. {:02}. {:02}",
        d.get_full_year(),
        d.get_month() + 1,
        d.get_date()
    )
}
async fn history() {
    let root = by_id("sessions").unwrap();
    clear(&root);
    let list = match call("chat:list", Value::Null).await {
        Ok(v) => v.as_array().cloned().unwrap_or_default(),
        Err(e) => {
            let _ = root.append_child(&el("p").cls("history-empty").text(&e).0);
            return;
        }
    };
    if list.is_empty() {
        let _ = root.append_child(
            &el("p")
                .cls("history-empty")
                .text("이 기기에 저장된 대화가 없어요.")
                .0,
        );
    }
    for item in list {
        let id = s(&item, "id");
        let e = el("button")
            .cls("history-item")
            .kid(el("strong").text(&format!(
                "{} · {}",
                date(&item["startedAt"]),
                s(&item, "modeLabel")
            )))
            .kid(el("span").text(&s(&item, "preview")))
            .click(move |_| {
                let id = id.clone();
                spawn(async move {
                    let view = by_id("sessionView").unwrap();
                    clear(&view);
                    toggle(&view, "hidden", false);
                    match call("chat:get", json!(id)).await {
                        Ok(v) => {
                            for turn in arr(&v, "turns") {
                                let role = if s(&turn, "role") == "user" {
                                    "user"
                                } else {
                                    "cat"
                                };
                                let _ = view.append_child(
                                    &el("div")
                                        .cls(&format!("msg {role}"))
                                        .text(&s(&turn, "text"))
                                        .0,
                                );
                            }
                        }
                        Err(e) => {
                            let _ = view.append_child(&el("p").text(&e).0);
                        }
                    }
                });
            })
            .build();
        let _ = root.append_child(&e);
    }
}
pub fn bind() {
    on(&by_id("composer").unwrap(), "submit", |e| {
        e.prevent_default();
        send();
    });
    on(&by_id("input").unwrap(), "keydown", |e| {
        if let Some(key) = e.dyn_ref::<KeyboardEvent>() {
            if key.key() == "Enter" && !key.shift_key() && !key.is_composing() {
                e.prevent_default();
                send();
            }
        }
    });
    on(&by_id("newConversation").unwrap(), "click", |_| reset());
    for e in q_all("[data-prompt]") {
        let prompt = e.get_attribute("data-prompt").unwrap_or_default();
        on(&e, "click", move |_| {
            if state(|v| v.busy) {
                return;
            }
            set_val(&by_id("input").unwrap(), &prompt);
            send();
        });
    }
    listen("chat:stream", |v| {
        let id = s(&v, "streamId");
        let bubble = state(|st| {
            st.stream
                .as_ref()
                .filter(|(stream, _)| *stream == id)
                .map(|(_, e)| e.clone())
        });
        if let Some(e) = bubble {
            let root = messages();
            let near = root.scroll_height() - root.scroll_top() - root.client_height() < 120;
            toggle(&e, "typing", false);
            e.set_text_content(Some(&s(&v, "text")));
            if near {
                root.set_scroll_top(root.scroll_height());
            }
        }
    });
    on(&by_id("historyToggle").unwrap(), "click", |_| {
        let panel = by_id("historyPanel").unwrap();
        let opening = panel.class_list().contains("hidden");
        toggle(&panel, "hidden", !opening);
        let _ = by_id("historyToggle")
            .unwrap()
            .set_attribute("aria-expanded", if opening { "true" } else { "false" });
        if opening {
            toggle(&by_id("sessionView").unwrap(), "hidden", true);
            spawn(history());
        }
    });
}
