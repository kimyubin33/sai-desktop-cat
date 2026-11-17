//! Google 중심 작업 공간. 첫 패치는 개인 기능과 새 화면만 제공한다.
mod agenda;
mod analysis;
mod collab;
mod drive_picker;
mod files;
mod legacy;
mod profile;
mod settings;
mod talk;
mod teams;
mod workspace;
use crate::{api::listen, dom::*};
use serde_json::Value;
use wasm_bindgen::JsCast;
use web_sys::HtmlCanvasElement;

pub fn s(v: &Value, key: &str) -> String {
    v[key].as_str().map(str::to_owned).unwrap_or_default()
}
pub fn b(v: &Value, key: &str) -> bool {
    v[key].as_bool().unwrap_or(false)
}
pub(super) fn confirm_team_ai_connection() -> bool {
    confirm("팀 권한을 확인하려면 짧은 수명의 Google 접근 토큰을 사이 팀 AI 서비스에 보내야 해요. 이 토큰에는 현재 허용한 Drive 전체 접근 권한이 포함돼요.\n\n이 PC의 같은 Google 계정과 같은 서비스에 대한 동의는 저장돼요. 파일 분석은 이후 선택한 내용을 별도로 확인한 뒤 보내요.\n\n팀 AI 연결을 허용할까요?")
}
pub fn arr(v: &Value, key: &str) -> Vec<Value> {
    v[key].as_array().cloned().unwrap_or_default()
}

pub fn show_tab(tab: &str) {
    let (tab, label) = match tab {
        "collab" => ("collab", "협업"),
        "chat" | "notes" => ("chat", "AI"),
        "settings" => ("settings", "설정"),
        _ => ("drive", "파일"),
    };
    for button in q_all(".tabs button") {
        let active = button.get_attribute("data-tab").as_deref() == Some(tab);
        toggle(&button, "active", active);
        if active {
            let _ = button.set_attribute("aria-current", "page");
        } else {
            let _ = button.remove_attribute("aria-current");
        }
    }
    for pane in q_all(".tab") {
        let active = pane.id() == format!("tab-{tab}");
        toggle(&pane, "active", active);
        if active {
            let _ = pane.remove_attribute("hidden");
            pane.set_scroll_top(0);
        } else {
            let _ = pane.set_attribute("hidden", "");
        }
    }
    set_text("pageLabel", label);
    if tab == "settings" {
        spawn(settings::load());
        spawn(settings::load_update());
        spawn(workspace::load());
    }
    if tab == "drive" {
        spawn(teams::load());
    }
    if tab == "collab" {
        spawn(async {
            teams::load().await;
            collab::load(false).await;
        });
    }
}

pub fn start() {
    if let Some(avatar) = by_id("avatar") {
        crate::sprite::render(
            &avatar.unchecked_into::<HtmlCanvasElement>(),
            "sit",
            0,
            -1,
            true,
        );
    }
    for button in q_all("[data-tab], [data-go]") {
        let tab = button
            .get_attribute("data-tab")
            .or_else(|| button.get_attribute("data-go"))
            .unwrap_or_default();
        if !button.has_attribute("aria-label") {
            if let Some(label) = button.text_content() {
                let _ = button.set_attribute("aria-label", label.trim());
            }
        }
        on(&button, "click", move |event| {
            event.prevent_default();
            show_tab(&tab);
        });
    }
    listen("chat:show-tab", |v| show_tab(v.as_str().unwrap_or("drive")));
    listen("update:state", settings::render_update);
    talk::bind();
    settings::bind();
    legacy::bind();
    workspace::bind();
    profile::bind();
    teams::bind();
    files::bind();
    drive_picker::bind();
    analysis::bind();
    agenda::bind();
    collab::bind();
    listen("profile:changed", |_| {
        spawn(async {
            workspace::load().await;
            settings::load().await;
        });
    });
    show_tab(&query_param("tab").unwrap_or_else(|| "drive".into()));
    spawn(async {
        if let Ok(value) = crate::api::call("settings:get", Value::Null).await {
            settings::update_engine(&value);
        }
        workspace::load().await;
    });
}
