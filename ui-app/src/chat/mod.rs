//! Google 중심 작업 공간. 첫 패치는 개인 기능과 새 화면만 제공한다.
mod agenda;
mod analysis;
mod chrome;
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

pub fn s(v: &Value, key: &str) -> String {
    v[key].as_str().map(str::to_owned).unwrap_or_default()
}
pub fn b(v: &Value, key: &str) -> bool {
    v[key].as_bool().unwrap_or(false)
}
pub(super) fn confirm_team_ai_connection() -> bool {
    confirm("Google 접근 토큰(Drive 전체 권한)을 사이 팀 AI 서비스에 전달해 팀 권한을 확인합니다.\n\n동의는 이 기기의 계정·서비스별로 저장합니다. 파일 본문은 분석할 때 따로 확인합니다.\n\n연결할까요?")
}
pub fn arr<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v[key].as_array().map(Vec::as_slice).unwrap_or_default()
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
    chrome::bind();
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
