use super::{b, s, settings};
use crate::{api::call, dom::*};
use serde_json::{json, Value};
use std::cell::Cell;
thread_local! {
    static EPOCH: Cell<u64> = const { Cell::new(0) };
    static BUSY: Cell<bool> = const { Cell::new(false) };
    static DIRTY: Cell<bool> = const { Cell::new(false) };
    static FORM_EPOCH: Cell<u64> = const { Cell::new(0) };
}
pub fn accept(v: &Value) -> bool {
    let epoch = v["epoch"].as_u64().unwrap_or(0);
    EPOCH.with(|current| {
        if epoch < current.get() {
            return false;
        }
        if epoch != current.get() {
            DIRTY.with(|d| d.set(false));
        }
        current.set(epoch);
        true
    })
}
pub fn draw(v: &Value) -> bool {
    if !accept(v) {
        return false;
    }
    let signed = b(v, "signedIn");
    let ready = signed && b(v, "ready");
    toggle(&by_id("profileSection").unwrap(), "hidden", !signed);
    toggle(&by_id("localFrequency").unwrap(), "hidden", signed);
    let busy = BUSY.with(Cell::get);
    for field in q_all("#profileForm input, #profileForm select, #profileForm button") {
        set_disabled(&field, !ready || busy);
    }
    set_disabled(&by_id("profileSync").unwrap(), busy || !signed);
    let changed_account = FORM_EPOCH.with(|e| e.replace(v["epoch"].as_u64().unwrap_or(0)))
        != v["epoch"].as_u64().unwrap_or(0);
    if changed_account || !DIRTY.with(Cell::get) && !busy {
        let p = &v["profile"];
        for (id, key) in [
            ("profileName", "displayName"),
            ("profileCat", "catName"),
            ("profileFrequency", "frequency"),
        ] {
            set_val(&by_id(id).unwrap(), &s(p, key));
        }
    }
    let error = s(v, "error");
    let pending = v["pending"].as_u64().unwrap_or(0);
    let message = if !error.is_empty() {
        if pending > 0 {
            format!("변경 내용은 이 기기에 보관했어요. {error}")
        } else {
            error
        }
    } else if !ready {
        "동기화 필요".into()
    } else if pending > 0 {
        "동기화 중…".into()
    } else {
        "동기화됨".into()
    };
    if !busy {
        set_text("profileMessage", &message);
    }
    true
}
pub async fn load() {
    if let Ok(v) = call("profile:status", Value::Null).await {
        draw(&v);
    }
}
async fn run(save: bool) {
    let request_epoch = EPOCH.with(Cell::get);
    let payload = if save {
        json!({"epoch":EPOCH.with(Cell::get),"patch":{"displayName":val(&by_id("profileName").unwrap()),"catName":val(&by_id("profileCat").unwrap()),"frequency":val(&by_id("profileFrequency").unwrap())}})
    } else {
        Value::Null
    };
    for field in q_all("#profileForm input, #profileForm select, #profileForm button, #profileSync")
    {
        set_disabled(&field, true);
    }
    set_text(
        "profileMessage",
        if save {
            "저장 중…"
        } else {
            "동기화 중…"
        },
    );
    let result = call(
        if save {
            "profile:update"
        } else {
            "profile:sync"
        },
        payload,
    )
    .await;
    BUSY.with(|busy| busy.set(false));
    if request_epoch != EPOCH.with(Cell::get) {
        load().await;
        return;
    }
    match result {
        Ok(v) => {
            if save {
                DIRTY.with(|d| d.set(false));
            }
            if draw(&v) {
                settings::load().await;
            }
        }
        Err(e) => {
            load().await;
            set_text("profileMessage", &e);
        }
    }
}
pub fn bind() {
    for event in ["input", "change"] {
        on(&by_id("profileForm").unwrap(), event, |_| {
            DIRTY.with(|d| d.set(true))
        });
    }
    on(&by_id("profileForm").unwrap(), "submit", |e| {
        e.prevent_default();
        if !BUSY.with(|busy| busy.replace(true)) {
            spawn(run(true));
        }
    });
    on(&by_id("profileSync").unwrap(), "click", |_| {
        if !BUSY.with(|busy| busy.replace(true)) {
            spawn(run(false));
        }
    });
}
