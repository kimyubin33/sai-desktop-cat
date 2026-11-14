//! 앱 상태와 시작: 저장소, 팀 연결, Google, AI, 대화, 시간표, 고양이, 조언, 업데이트를 하나로 묶는다.
use crate::advisor::Advisor;
use crate::cat::{self, CatState};
use crate::menu;
use crate::updater::Updater;
use sai_core::ai::Ai;
use sai_core::chat::Chat;
use sai_core::google::Google;
use sai_core::schedule::{book::Book, Schedule};
use sai_core::store::Stores;
use sai_core::team::{backend::random_code, Backend, Hub};
use sai_core::time::now_ms;
use sai_core::util::J;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

/// 이 팀이 쓰는 Firebase 프로젝트(saigajoa)의 연결 정보.
/// apiKey 는 비밀번호가 아니라 프로젝트를 가리키는 공개 값이며, 데이터 보호는 firestore.rules 가 맡는다.
pub fn builtin_firebase() -> Value {
    json!({ "apiKey": "AIzaSyC6MBRsYOCchzBRZT7hX2cvbxUujGG_Eq0", "projectId": "saigajoa" })
}

/// 설치 파일을 만들 때 함께 넣은 Google 연결 정보 (없으면 None)
pub fn builtin_google() -> Option<Value> {
    let raw = option_env!("SAI_GOOGLE_CLIENT")?;
    let j: Value = serde_json::from_str(raw).ok()?;
    let c = if j.get("installed").is_some() { &j["installed"] } else { &j };
    let id = c.os("clientId").or_else(|| c.os("client_id"))?;
    let secret = c.os("clientSecret").or_else(|| c.os("client_secret"))?;
    Some(json!({ "clientId": id, "clientSecret": secret }))
}

pub struct App {
    pub handle: AppHandle,
    pub stores: Stores,
    pub hub: Hub,
    pub google: Google,
    pub ai: Arc<Ai>,
    pub chat: Chat,
    pub schedule: Schedule,
    pub cat: Mutex<CatState>,
    pub advisor: Advisor,
    pub peeks: crate::peek::Peeks,
    pub updater: Updater,
    pub local_mode: bool,
}

pub fn setup(app: &AppHandle) -> Result<Arc<App>, Box<dyn std::error::Error>> {
    let dir = app.path().app_data_dir()?;
    let stores = Stores::open(&dir);
    // 예전 저장값 정리: 끄는 기능을 없앴으므로 빈도는 셋 중 하나로
    stores.settings.update(|s| {
        if !["low", "normal", "high"].contains(&s.s("frequency").as_str()) {
            s["frequency"] = json!("low");
        }
    });

    let local_file = std::env::var("SAI_TEAM_FILE").ok();
    let settings = stores.settings.clone();
    let make_backend: sai_core::team::MakeBackend = {
        let settings = settings.clone();
        let local_file = local_file.clone();
        Box::new(move || {
            // 로컬 시험 모드: 한 컴퓨터에서 앱 두 개를 띄워 팀처럼 시험 (SAI_TEAM_FILE=공유 파일 경로)
            if let Some(f) = &local_file {
                if settings.read(|s| s.s("localUid")).is_empty() {
                    let uid = format!("local-{}", random_code(10).to_lowercase());
                    settings.update(|s| s["localUid"] = json!(uid));
                }
                return Some(Backend::memory(Some(f.into()), &settings.read(|s| s.s("localUid"))));
            }
            let cfg = settings.read(|s| s["firebase"].clone());
            let cfg = if cfg.is_object() && !cfg.s("apiKey").is_empty() { cfg } else { builtin_firebase() };
            let session = settings.read(|s| sai_core::secret::decrypt_json_field(s, "firebaseSessionEnc"));
            let st = settings.clone();
            Backend::firebase(&cfg, session, Box::new(move |sess: &Value| {
                let enc = sai_core::secret::encrypt_json(sess);
                st.update(|s| s["firebaseSessionEnc"] = json!(enc));
            }))
            .ok()
        })
    };
    let delay = std::env::var("SAI_SEND_DELAY_MS").ok().and_then(|v| v.parse().ok());
    let hub = Hub::new(stores.settings.clone(), stores.shared_outbox.clone(), stores.team_cache.clone(), make_backend, delay, Arc::new(now_ms));
    let google = Google::new(stores.settings.clone(), stores.team_cache.clone(), hub.clone(), builtin_google(), Arc::new(|u: &str| crate::system::open_url(u)));
    let ai = Arc::new(Ai::new(stores.settings.clone()));
    let chat = Chat::new(stores.private_conv.clone(), stores.settings.clone(), ai.clone());
    let schedule = Schedule::new(Book::new(stores.schedule.clone()), hub.clone(), google.clone(), ai.clone());

    let a = Arc::new(App {
        handle: app.clone(),
        stores,
        hub,
        google,
        ai,
        chat,
        schedule,
        cat: Mutex::new(CatState::new()),
        advisor: Advisor::new(),
        peeks: crate::peek::Peeks::default(),
        updater: Updater::new(),
        local_mode: local_file.is_some(),
    });

    // 컴퓨터를 켜면 자동으로 실행 (켤 때마다 다시 등록)
    crate::system::ensure_autostart();

    cat::create(app, &a)?;

    let tray_icon = tauri::image::Image::from_bytes(include_bytes!("../../assets/tray.png"))?;
    let menu = menu::build(app, &a)?;
    let a_menu = a.clone();
    let a_click = a.clone();
    tauri::tray::TrayIconBuilder::with_id("main")
        .icon(tray_icon)
        .tooltip("사이 — 고양이 협업 도우미")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, e| menu::on_event(app, &a_menu, e))
        .on_tray_icon_event(move |tray, e| {
            if let tauri::tray::TrayIconEvent::Click { button: tauri::tray::MouseButton::Left, button_state: tauri::tray::MouseButtonState::Up, .. } = e {
                open_chat(tray.app_handle(), &a_click, "chat", json!({}));
            }
        })
        .build(app)?;
    // 메뉴의 "~까지" 시각이 맞게 가끔 새로 그린다
    let a_tray = a.clone();
    tauri::async_runtime::spawn(async move {
        let mut iv = tokio::time::interval(Duration::from_secs(60));
        loop {
            iv.tick().await;
            menu::refresh_tray(&a_tray.handle, &a_tray);
        }
    });

    // 팀 동기화: 시작 3초 뒤, 그다음 5분마다
    let a_sync = a.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(3)).await;
        account_startup(&a_sync).await;
        loop {
            sync_now(&a_sync).await;
            tokio::time::sleep(Duration::from_secs(5 * 60)).await;
        }
    });
    // 저장: 바뀐 내용을 1초마다 파일에
    let a_save = a.clone();
    tauri::async_runtime::spawn(async move {
        let mut iv = tokio::time::interval(Duration::from_secs(1));
        loop {
            iv.tick().await;
            a_save.stores.flush_all();
        }
    });

    if std::env::var("SAI_DEMO").is_err() {
        crate::peek::start(&a);
        a.advisor.start(&a);
        a.updater.start(&a);
    }
    Ok(a)
}

/// Google 계정에 맡겨 둔 팀: 이 컴퓨터에서 처음이면 되찾고, 아니면 지금 목록을 맡겨 둔다
pub async fn account_startup(a: &Arc<App>) {
    if a.local_mode {
        return;
    }
    let st = a.google.account_status();
    if !st.b("ready") {
        return;
    }
    // 처음이거나 이 컴퓨터에 팀이 하나도 없으면 되찾기부터
    if st.b("checked") && !a.hub.teams().is_empty() {
        let _ = a.google.backup_account(false).await;
    } else if a.google.restore_account().await.is_ok() {
        on_settings_changed(a);
    }
}

/// 팀 목록이 바뀐 뒤 Google 계정에도 맡겨 둔다 (뒤에서)
pub fn backup_later(a: &Arc<App>, allow_empty: bool) {
    if a.local_mode {
        return;
    }
    let a = a.clone();
    tauri::async_runtime::spawn(async move {
        let _ = a.google.backup_account(allow_empty).await;
    });
}

/// 모든 팀 동기화 + Google 할 일
pub async fn sync_now(a: &Arc<App>) -> Value {
    let results = a.hub.sync_all().await;
    // 팀에 공유된 Claude 키 (내 키가 없을 때 쓴다)
    a.ai.set_shared(a.hub.shared_claude_key());
    if !results.is_empty() {
        let _ = a.google.sync_todo().await;
    }
    menu::refresh_tray(&a.handle, a);
    results.into_iter().find(|r| !r.b("ok")).unwrap_or_else(|| json!({ "ok": false, "reason": "not-connected" }))
}

pub fn on_settings_changed(a: &Arc<App>) {
    let quiet = a.stores.settings.read(|s| s.i("quietUntil")) > now_ms();
    cat::set_calm(a, quiet);
    a.advisor.schedule(a, None);
    menu::refresh_tray(&a.handle, a);
}

/// 대화창 열기. extra: { explain: 주제 id } / { task: 작업 id } / { mode: 'schedule' }
pub fn open_chat(app: &AppHandle, a: &Arc<App>, tab: &str, extra: Value) {
    cat::hide_bubble(app, a);
    if let Some(w) = app.get_webview_window("chat") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        if let Some(team) = extra.os("teamTalk") {
            let _ = app.emit_to("chat", "chat:team-talk", json!({ "teamId": team }));
        } else {
            let _ = app.emit_to("chat", "chat:show-tab", tab);
        }
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
        let _ = WebviewWindowBuilder::new(&app, "chat", WebviewUrl::App(q.into()))
            .title("사이와 이야기")
            .inner_size(520.0, 700.0)
            .min_inner_size(380.0, 520.0)
            // 파일을 끌어다 놓으면 화면(협업 탭의 팀 자료)이 직접 받는다
            .disable_drag_drop_handler()
            .build();
    });
}
