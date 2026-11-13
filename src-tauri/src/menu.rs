//! 트레이 아이콘과 고양이 오른쪽 클릭 메뉴 (같은 메뉴).
//! 쉬는 방법은 모두 정해진 시간이 지나면 저절로 끝난다 — 영구히 끄는 메뉴는 없다.
use crate::app::{on_settings_changed, open_chat, App};
use crate::cat;
use sai_core::time::{fmt_time, now_ms, MIN};
use sai_core::util::J;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tauri::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Wry};

pub fn build(app: &AppHandle, a: &Arc<App>) -> tauri::Result<Menu<Wry>> {
    let s = a.stores.settings.get();
    let name = s.s("catName");
    let now = now_ms();
    let quieting = s.i("quietUntil") > now;
    let focusing = s.i("focusUntil") > now;
    let visible = a.cat.lock().unwrap().visible;
    let freq = s.s("frequency");

    let item = |id: &str, text: &str| MenuItem::with_id(app, id, text, true, None::<&str>);
    let focus = Submenu::with_items(
        app,
        if focusing { format!("집중 모드 중 ({}까지)", fmt_time(s.i("focusUntil"))) } else { "집중 모드".into() },
        true,
        &[
            &item("focus-30", "30분")?,
            &item("focus-60", "1시간")?,
            &item("focus-120", "2시간")?,
            &MenuItem::with_id(app, "focus-0", "집중 모드 끝내기", focusing, None::<&str>)?,
        ],
    )?;
    let fr = |id: &str, label: &str| CheckMenuItem::with_id(app, format!("freq-{id}"), label, true, freq == id, None::<&str>);
    let freq_menu = Submenu::with_items(app, "말 거는 빈도", true, &[&fr("low", "가끔")?, &fr("normal", "보통")?, &fr("high", "자주")?])?;
    let update_label = if a.updater.is_ready() { format!("새 버전 {} 적용하기", a.updater.latest().unwrap_or_default()) } else { format!("업데이트 확인 (지금 {})", a.updater.current) };
    Menu::with_items(
        app,
        &[
            &item("chat", &format!("{name}와 이야기하기"))?,
            &item("speak", "조언 한마디 듣기")?,
            &PredefinedMenuItem::separator(app)?,
            &item("quiet", &if quieting { format!("잠시 조용히 하는 중 ({}까지)", fmt_time(s.i("quietUntil"))) } else { "잠시 조용히 (1시간)".into() })?,
            &focus,
            &freq_menu,
            &item("notes", "정리함")?,
            &item("collab", "협업 (업무·일정)")?,
            &item("settings", "설정")?,
            &item("update", &update_label)?,
            &PredefinedMenuItem::separator(app)?,
            &item("hide", &if visible { format!("{name} 1시간 숨기기") } else { format!("{name} 다시 보이기") })?,
            &item("quit", "종료")?,
        ],
    )
}

pub fn refresh_tray(app: &AppHandle, a: &Arc<App>) {
    if let Some(tray) = app.tray_by_id("main") {
        if let Ok(m) = build(app, a) {
            let _ = tray.set_menu(Some(m));
        }
    }
}

pub fn on_event(app: &AppHandle, a: &Arc<App>, e: MenuEvent) {
    let id = e.id().as_ref().to_string();
    match id.as_str() {
        "chat" => open_chat(app, a, "chat", json!({})),
        "speak" => {
            cat::hide_bubble(app, a);
            a.advisor.speak_now(a);
        }
        "quiet" => {
            let q = a.stores.settings.read(|s| s.i("quietUntil")) > now_ms();
            quiet_for(app, a, if q { 0 } else { 60 });
        }
        "notes" => open_chat(app, a, "notes", json!({})),
        "collab" => open_chat(app, a, "collab", json!({})),
        "settings" => open_chat(app, a, "settings", json!({})),
        "update" => {
            if a.updater.is_ready() {
                a.updater.install(a);
            } else {
                let a2 = a.clone();
                tauri::async_runtime::spawn(async move {
                    a2.updater.check(&a2, true).await;
                    refresh_tray(&a2.handle, &a2);
                });
            }
        }
        "hide" => {
            let visible = a.cat.lock().unwrap().visible;
            hide_for(app, a, if visible { 60 } else { 0 });
        }
        "quit" => {
            a.updater.install_on_quit();
            a.stores.flush_all();
            app.exit(0);
        }
        x if x.starts_with("focus-") => {
            let m: i64 = x[6..].parse().unwrap_or(0);
            let until = if m > 0 { now_ms() + m * MIN } else { 0 };
            a.stores.settings.update(|s| s["focusUntil"] = json!(until));
            on_settings_changed(a);
        }
        x if x.starts_with("freq-") => {
            let f = x[5..].to_string();
            a.stores.settings.update(|s| s["frequency"] = json!(f));
            on_settings_changed(a);
        }
        _ => {}
    }
}

/// 잠시 조용히: 최대 1시간, 지나면 저절로 다시 말을 건다
pub fn quiet_for(_app: &AppHandle, a: &Arc<App>, minutes: i64) {
    let m = minutes.min(60);
    let until = if m > 0 { now_ms() + m * MIN } else { 0 };
    a.stores.settings.update(|s| s["quietUntil"] = json!(until));
    on_settings_changed(a);
    if m > 0 {
        let a2 = a.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_millis((m * MIN + 1000) as u64)).await;
            on_settings_changed(&a2);
        });
    }
}

/// 숨기기도 1시간 뒤 저절로 다시 나타난다
pub fn hide_for(app: &AppHandle, a: &Arc<App>, minutes: i64) {
    cat::set_visible(app, a, minutes == 0);
    refresh_tray(app, a);
    if minutes > 0 {
        let a2 = a.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_millis((minutes * MIN) as u64)).await;
            cat::set_visible(&a2.handle, &a2, true);
            refresh_tray(&a2.handle, &a2);
        });
    }
}
