use crate::{
    advisor::Advisor,
    cat::{self, CatState},
    menu,
    updater::Updater,
};
use sai_core::{
    ai::Ai,
    chat::Chat,
    google::Google,
    store::{Store, Stores},
    team::Hub,
    time::now_ms,
    util::J,
};
use serde_json::{json, Value};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

pub fn builtin_google() -> Option<Value> {
    let raw = option_env!("SAI_GOOGLE_CLIENT")?;
    let j: Value = serde_json::from_str(raw).ok()?;
    let c = if j.get("installed").is_some() {
        &j["installed"]
    } else {
        &j
    };
    let id = c.os("clientId").or_else(|| c.os("client_id"))?;
    let secret = c.os("clientSecret").or_else(|| c.os("client_secret"))?;
    Some(json!({ "clientId": id, "clientSecret": secret }))
}

pub struct App {
    pub handle: AppHandle,
    pub stores: Stores,
    pub google: Google,
    pub ai: Arc<Ai>,
    pub chat: Chat,
    pub cat: Mutex<CatState>,
    pub advisor: Advisor,
    pub updater: Updater,
    pub data_dir: std::path::PathBuf,
    pub legacy_export: tokio::sync::Mutex<()>,
    pub file_jobs: crate::file_transfer::Jobs,
}

pub fn setup(app: &AppHandle) -> Result<Arc<App>, Box<dyn std::error::Error>> {
    let config = app.config();
    crate::preview::validate_identity(
        crate::preview::BUILD,
        &config.identifier,
        config.product_name.as_deref(),
        config.main_binary_name.as_deref(),
    )?;
    // 화면 검증은 실제 사용자 자료·자동 실행 등록을 건드리지 않는다.
    let preview = std::env::var_os("SAI_PREVIEW_DIR").map(std::path::PathBuf::from);
    let isolated = crate::preview::isolated();
    let dir = match &preview {
        Some(dir) => dir.clone(),
        None => app.path().app_data_dir()?,
    };
    let stores = Stores::open(&dir);
    // 기존 자료는 보존하되 옛 Firebase / PC 서버에는 연결하지 않는다.
    let hub = Hub::new(
        Store::memory(json!({"teams":[]})),
        Store::memory(json!({"items":[]})),
        Store::memory(json!({"teams":{}})),
        Box::new(|| None),
        None,
        Arc::new(now_ms),
    );
    let google = Google::new(
        stores.settings.clone(),
        stores.google_connection.clone(),
        hub,
        builtin_google(),
        Arc::new(crate::system::open_url),
        preview
            .clone()
            .unwrap_or(app.path().app_local_data_dir()?)
            .join("google-profile-v1"),
    );
    let ai = Arc::new(Ai::new(stores.settings.clone()));
    let chat = Chat::new(
        stores.private_conv.clone(),
        stores.settings.clone(),
        ai.clone(),
    );
    let a = Arc::new(App {
        handle: app.clone(),
        stores,
        google,
        ai,
        chat,
        cat: Mutex::new(CatState::new()),
        advisor: Advisor::new(),
        updater: Updater::new(),
        data_dir: dir,
        legacy_export: tokio::sync::Mutex::new(()),
        file_jobs: crate::file_transfer::Jobs::default(),
    });
    if !isolated {
        crate::system::ensure_autostart();
    }
    if !isolated {
        cat::create(app, &a)?;
    }
    let a_menu = a.clone();
    let a_click = a.clone();
    tauri::tray::TrayIconBuilder::with_id("main")
        .icon(tauri::image::Image::from_bytes(include_bytes!(
            "../../assets/tray.png"
        ))?)
        .tooltip(if crate::preview::BUILD {
            crate::preview::TITLE
        } else {
            "사이 · 함께하는 작업 공간"
        })
        .menu(&menu::build(app, &a)?)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, e| menu::on_event(app, &a_menu, e))
        .on_tray_icon_event(move |tray, e| {
            if let tauri::tray::TrayIconEvent::Click {
                button: tauri::tray::MouseButton::Left,
                button_state: tauri::tray::MouseButtonState::Up,
                ..
            } = e
            {
                open_chat(tray.app_handle(), &a_click, "drive", json!({}));
            }
        })
        .build(app)?;
    let save = a.clone();
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        loop {
            interval.tick().await;
            save.stores.flush_all();
        }
    });
    let sync = a.clone();
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        loop {
            interval.tick().await;
            if sync.google.status()["hasSession"] == true {
                let _ = sync.google.sync_profile().await;
                on_settings_changed(&sync);
                let _ = sync.handle.emit_to("chat", "profile:changed", ());
            }
        }
    });
    if !isolated {
        a.advisor.start(&a);
        a.updater.start(&a);
    }
    if isolated {
        create_workspace(app, "chat/index.html?tab=drive".into())?;
    } else {
        open_chat(app, &a, "drive", json!({}));
    }
    Ok(a)
}

pub fn on_settings_changed(a: &Arc<App>) {
    cat::set_calm(a, a.stores.settings.read(|s| s.i("quietUntil")) > now_ms());
    a.advisor.schedule(a, None);
    menu::refresh_tray(&a.handle, a);
}

pub fn open_chat(app: &AppHandle, a: &Arc<App>, tab: &str, extra: Value) {
    cat::hide_bubble(app, a);
    if let Some(w) = app.get_webview_window("chat") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        let _ = app.emit_to("chat", "chat:show-tab", tab);
        if let Some(t) = extra.os("explain") {
            let _ = app.emit_to("chat", "chat:start-explain", t);
        }
        if let Some(t) = extra.os("task") {
            let _ = app.emit_to("chat", "chat:focus-task", t);
        }
        return;
    }
    let mut q = format!("chat/index.html?tab={tab}");
    for k in ["explain", "task"] {
        if let Some(v) = extra.os(k) {
            q.push_str(&format!("&{k}={}", sai_core::util::take_chars(&v, 100)));
        }
    }
    // 창 만들기는 메뉴·트레이 이벤트 처리 안에서 바로 하면 Windows 에서 멈춘다 (WebView2 교착, tauri#4121).
    // 그래서 따로 떨어진 작업으로 넘겨 만든다.
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if app.get_webview_window("chat").is_some() {
            return;
        }
        match create_workspace(&app, q) {
            Ok(window) => {
                let _ = window.show();
                let _ = window.set_focus();
            }
            Err(error) => eprintln!("작업 공간을 열지 못했어요: {error}"),
        }
    });
}

fn create_workspace(app: &AppHandle, url: String) -> tauri::Result<tauri::WebviewWindow> {
    let builder = WebviewWindowBuilder::new(app, "chat", WebviewUrl::App(url.into()))
        .title(if crate::preview::BUILD {
            crate::preview::TITLE
        } else {
            "사이"
        })
        .inner_size(1100.0, 780.0)
        .min_inner_size(420.0, 580.0)
        .disable_drag_drop_handler();
    #[cfg(windows)]
    let builder = if let Some(dir) = std::env::var_os("SAI_PREVIEW_DIR") {
        builder.data_directory(std::path::PathBuf::from(dir).join("webview"))
    } else {
        builder
    };
    builder.build()
}
