//! 팀 톡 알림창. 대기열과 창의 수명은 데스크톱 쪽에서 관리한다.
use crate::api::{call, register_listener};
use crate::dom::{by_id, on, set_text, spawn, toggle};
use serde_json::{json, Value};
use std::cell::RefCell;
use wasm_bindgen::JsCast;
use web_sys::HtmlCanvasElement;

thread_local! {
    static CURRENT: RefCell<Option<(String, String)>> = const { RefCell::new(None) };
}

fn show(notice: Value) {
    let (Some(team), Some(id), Some(text)) = (notice["teamId"].as_str(), notice["id"].as_str(), notice["text"].as_str()) else { return };
    if team.is_empty() || id.is_empty() || text.trim().is_empty() {
        return;
    }
    let key = (team.to_owned(), id.to_owned());
    let previous = CURRENT.with(|current| current.replace(Some(key.clone())));
    if previous.as_ref() == Some(&key) {
        return;
    }
    let root = by_id("peek").unwrap();
    let _ = root.remove_attribute("hidden");
    toggle(&root, "shown", false);
    set_text("sender", notice["byName"].as_str().filter(|s| !s.is_empty()).unwrap_or("팀원"));
    set_text("team", notice["teamName"].as_str().unwrap_or(""));
    set_text("message", text);
    // 다음 톡도 같은 창에서 처음부터 빼꼼 나오는 애니메이션을 보여 준다.
    let _ = root.offset_width();
    toggle(&root, "shown", true);
}

fn action(channel: &'static str) {
    let Some((team, id)) = CURRENT.with(|current| current.borrow().clone()) else { return };
    spawn(async move {
        let _ = call(channel, json!({ "teamId": team, "id": id })).await;
    });
}

pub fn start() {
    let face: HtmlCanvasElement = by_id("face").unwrap().unchecked_into();
    crate::sprite::render(&face, "peek", 0, 1, true);
    on(&by_id("peekOpen").unwrap(), "click", |_| action("peek:open"));
    on(&by_id("peekClose").unwrap(), "click", |_| action("peek:dismiss"));
    let registration = register_listener("peek:message", show);
    spawn(async move {
        let Some(registration) = registration else { return };
        if wasm_bindgen_futures::JsFuture::from(registration).await.is_err() {
            return;
        }
        if let Ok(notice) = call("peek:ready", Value::Null).await {
            // 초기 응답 전에 다음 톡 이벤트가 도착했다면 화면을 되돌리지 않는다.
            if CURRENT.with(|current| current.borrow().is_none()) {
                show(notice);
            }
        }
    });
}
