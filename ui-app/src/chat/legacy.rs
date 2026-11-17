use super::s;
use crate::{api::call, dom::*};
use serde_json::Value;
use std::cell::Cell;
thread_local! { static BUSY: Cell<bool> = const { Cell::new(false) }; }
fn draw(v: &Value) {
    let count = |key: &str| v[key].as_u64().unwrap_or(0);
    set_text(
        "legacyCounts",
        &format!(
            "이 기기에 남아 있는 팀 {}개 · 프로젝트 {}개 · 업무 {}개 · 파일 링크 {}개",
            count("teams"),
            count("projects"),
            count("tasks"),
            count("files")
        ),
    );
    set_disabled(
        &by_id("legacyExport").unwrap(),
        count("teams") == 0 || BUSY.with(Cell::get),
    );
    toggle(
        &by_id("legacyPcNotice").unwrap(),
        "hidden",
        !v["pcArchivePresent"].as_bool().unwrap_or(false),
    );
}
async fn load() {
    if BUSY.with(|busy| busy.replace(true)) {
        return;
    }
    set_disabled(&by_id("legacyExport").unwrap(), true);
    let result = call("legacy:status", Value::Null).await;
    BUSY.with(|busy| busy.set(false));
    match result {
        Ok(v) => draw(&v),
        Err(error) => {
            set_text("legacyCounts", &error);
            set_disabled(&by_id("legacyExport").unwrap(), true);
        }
    }
}
pub fn bind() {
    let details = by_id("legacySection").unwrap();
    let section = details.clone();
    on(&details, "toggle", move |_| {
        if section.has_attribute("open") {
            spawn(load());
        }
    });
    on(&by_id("legacyExport").unwrap(), "click", |_| {
        if BUSY.with(|busy| busy.replace(true)) {
            return;
        }
        set_disabled(&by_id("legacyExport").unwrap(), true);
        set_text(
            "legacyMessage",
            "원본을 그대로 두고 보관 사본을 만들고 있어요…",
        );
        spawn(async {
            let result = call("legacy:export", Value::Null).await;
            BUSY.with(|busy| busy.set(false));
            match result {
                Ok(v) => {
                    draw(&v["summary"]);
                    set_text(
                        "legacyMessage",
                        &format!("{} 파일을 이 기기에 저장했어요.", s(&v, "fileName")),
                    );
                    toggle(&by_id("legacyOpenExports").unwrap(), "hidden", false);
                }
                Err(error) => {
                    set_text("legacyMessage", &error);
                    load().await;
                }
            }
        });
    });
    for (id, channel) in [
        ("legacyOpenOriginal", "legacy:open-original"),
        ("legacyOpenExports", "legacy:open-exports"),
    ] {
        on(&by_id(id).unwrap(), "click", move |_| {
            spawn(async move {
                if let Err(e) = call(channel, Value::Null).await {
                    set_text("legacyMessage", &e);
                }
            })
        });
    }
}
