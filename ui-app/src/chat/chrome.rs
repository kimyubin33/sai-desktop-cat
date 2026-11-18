//! Small native window controls and a fixed documentation destination.
use crate::{api::call, dom::*};
use serde_json::Value;
use wasm_bindgen::JsCast;
use web_sys::MouseEvent;

pub fn bind() {
    for (id, channel) in [
        ("windowMinimize", "window:minimize"),
        ("windowMaximize", "window:maximize"),
        ("windowClose", "window:close"),
        ("helpManual", "help:open"),
    ] {
        on(&by_id(id).unwrap(), "click", move |_| {
            spawn(async move {
                let _ = call(channel, Value::Null).await;
            });
        });
    }
    on(&by_id("windowResize").unwrap(), "mousedown", |event| {
        let Some(mouse) = event.dyn_ref::<MouseEvent>() else {
            return;
        };
        if mouse.button() == 0 {
            event.prevent_default();
            crate::api::resize_workspace();
        }
    });
}
