use super::{b, s};
use crate::{api::call, dom::*};
use serde_json::{json, Value};
use std::cell::Cell;
use web_sys::Element;
thread_local! {
    static SAVING: Cell<bool> = const { Cell::new(false) };
    static DIRTY: Cell<bool> = const { Cell::new(false) };
    static LOCAL_FREQUENCY: Cell<bool> = const { Cell::new(true) };
}
fn form() -> Element {
    by_id("settingsForm").unwrap().into()
}
pub fn update_engine(v: &Value) {
    let name = s(v, "catName");
    if !name.is_empty() {
        set_text("catName", &name);
    }
    set_text(
        "engine",
        if !b(&v["ai"], "enabled") {
            "기본 대화 · 개인 AI 꺼짐"
        } else if b(v, "hasApiKey") {
            "개인 Claude 연결됨"
        } else if b(v, "hasOpenAiKey") {
            "기본 대화 · OpenAI 연결 보관됨"
        } else {
            "기본 대화 · 개인 AI 미연결"
        },
    );
}
fn draw(v: &Value) {
    if !super::profile::draw(&v["googleProfile"]) {
        return;
    }
    LOCAL_FREQUENCY.with(|local| local.set(!b(&v["googleProfile"], "signedIn")));
    let warning = s(v, "storageWarning");
    set_text("storageWarning", &warning);
    toggle(
        &by_id("storageWarning").unwrap(),
        "hidden",
        warning.is_empty(),
    );
    update_engine(v);
    if DIRTY.with(Cell::get) {
        return;
    }
    let f = form();
    set_val(&field(&f, "frequency"), &s(v, "frequency"));
    set_val(
        &field(&f, "retentionDays"),
        &v["retentionDays"].as_i64().unwrap_or(0).to_string(),
    );
    set_checked(&field(&f, "aiEnabled"), b(&v["ai"], "enabled"));
    set_val(&field(&f, "model"), &s(&v["ai"], "model"));
    set_val(&field(&f, "openAiModel"), &s(v, "openAiModel"));
    for (name, flag, button) in [
        ("apiKey", "hasOwnKey", "removeClaude"),
        ("openAiKey", "hasOpenAiKey", "removeOpenAi"),
    ] {
        set_val(&field(&f, name), "");
        let _ = field(&f, name).set_attribute(
            "placeholder",
            if b(v, flag) {
                "저장된 키가 있어요. 바꿀 때만 입력해 주세요."
            } else {
                "개인 API 키 입력"
            },
        );
        toggle(&by_id(button).unwrap(), "hidden", !b(v, flag));
    }
}
pub async fn load() {
    if SAVING.with(Cell::get) {
        return;
    }
    match call("settings:get", Value::Null).await {
        Ok(v) => draw(&v),
        Err(e) => set_text("settingsMessage", &e),
    }
}
async fn save() {
    let f = form();
    let mut patch = json!({"retentionDays":val(&field(&f,"retentionDays")).parse::<i64>().unwrap_or(0),"ai":{"enabled":checked(&field(&f,"aiEnabled")),"model":val(&field(&f,"model"))}});
    if LOCAL_FREQUENCY.with(Cell::get) {
        patch["frequency"] = json!(val(&field(&f, "frequency")));
    }
    let claude = val(&field(&f, "apiKey")).trim().to_owned();
    let openai = val(&field(&f, "openAiKey")).trim().to_owned();
    let mut ai_patch = json!({"model":val(&field(&f,"openAiModel"))});
    if !openai.is_empty() {
        ai_patch["key"] = json!(openai);
    }
    let result = async {
        let mut latest = call("settings:set", patch).await?;
        if !claude.is_empty() {
            latest = call("ai:set-key", json!(claude)).await?;
        }
        if !s(&ai_patch, "model").trim().is_empty() || ai_patch.get("key").is_some() {
            latest = call("ai:set-openai", ai_patch).await?;
        }
        Ok::<_, String>(latest)
    }
    .await;
    match result {
        Ok(v) => {
            DIRTY.with(|d| d.set(false));
            draw(&v);
            set_text("settingsMessage", "변경 사항을 저장했어요.");
        }
        Err(e) => set_text("settingsMessage", &e),
    }
    SAVING.with(|busy| busy.set(false));
    for element in q_all("#settingsForm input, #settingsForm select, #settingsForm button") {
        set_disabled(&element, false);
    }
}
pub fn render_update(v: Value) {
    set_text("appVersion", &s(&v, "current"));
    let status = s(&v, "status");
    let error = s(&v, "error");
    set_text(
        "updateStatus",
        if !error.is_empty() {
            &error
        } else {
            match status.as_str() {
                "checking" => "새 버전을 확인하고 있어요.",
                "downloading" => "새 버전을 받고 있어요.",
                "ready" => "새 버전을 적용할 수 있어요.",
                _ => "필요할 때 새 버전을 확인할 수 있어요.",
            }
        },
    );
    if let Some(button) = by_id("updateInstall") {
        toggle(&button, "hidden", status != "ready");
    }
    if let Some(button) = by_id("updateCheck") {
        toggle(&button, "hidden", status == "unsupported");
        set_disabled(&button, status == "checking" || status == "downloading");
    }
}
pub async fn load_update() {
    if let Ok(v) = call("update:status", Value::Null).await {
        render_update(v);
    }
}
pub fn bind() {
    for event in ["input", "change"] {
        on(&by_id("settingsForm").unwrap(), event, |_| {
            DIRTY.with(|d| d.set(true))
        });
    }
    on(&by_id("settingsForm").unwrap(), "submit", |e| {
        e.prevent_default();
        if SAVING.with(|busy| busy.replace(true)) {
            return;
        }
        set_text("settingsMessage", "저장하고 있어요…");
        for element in q_all("#settingsForm input, #settingsForm select, #settingsForm button") {
            set_disabled(&element, true);
        }
        spawn(save());
    });
    for (button, channel, payload) in [
        ("removeClaude", "ai:set-key", json!("")),
        ("removeOpenAi", "ai:set-openai", json!({"key":""})),
    ] {
        on(&by_id(button).unwrap(), "click", move |_| {
            if SAVING.with(|busy| busy.replace(true)) {
                return;
            }
            if !confirm("이 기기에 저장한 개인 API 키를 지울까요?") {
                SAVING.with(|busy| busy.set(false));
                return;
            }
            let payload = payload.clone();
            spawn(async move {
                match call(channel, payload).await {
                    Ok(v) => {
                        update_engine(&v);
                        // Removing a stored key must not erase other unsaved fields.
                        let field_name = if channel == "ai:set-key" {
                            "apiKey"
                        } else {
                            "openAiKey"
                        };
                        set_val(&field(&form(), field_name), "");
                        let _ = field(&form(), field_name)
                            .set_attribute("placeholder", "개인 API 키 입력");
                        toggle(
                            &by_id(if channel == "ai:set-key" {
                                "removeClaude"
                            } else {
                                "removeOpenAi"
                            })
                            .unwrap(),
                            "hidden",
                            true,
                        );
                        set_text("settingsMessage", "이 기기의 개인 키를 지웠어요.");
                    }
                    Err(e) => set_text("settingsMessage", &e),
                }
                SAVING.with(|busy| busy.set(false));
            });
        });
    }
    on(&by_id("updateCheck").unwrap(), "click", |_| {
        spawn(async {
            match call("update:check", Value::Null).await {
                Ok(v) => render_update(v),
                Err(e) => set_text("updateStatus", &e),
            }
        })
    });
    on(&by_id("updateInstall").unwrap(), "click", |_| {
        spawn(async {
            if let Err(e) = call("update:install", Value::Null).await {
                set_text("updateStatus", &e);
            }
        })
    });
}
