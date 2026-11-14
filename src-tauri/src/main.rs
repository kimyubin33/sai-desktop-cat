// 사이 — 바탕화면 고양이 협업 도우미 (Tauri + Rust)
// Windows 에서 콘솔 창이 뜨지 않게 한다
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod advisor;
mod app;
mod cat;
mod demo;
mod ipc;
mod menu;
mod peek;
mod system;
mod updater;

use std::sync::Arc;
use tauri::Manager;

fn main() {
    tauri::Builder::default()
        // 이미 켜져 있으면 새로 켜지 않고 대화창을 연다
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(a) = app.try_state::<Arc<app::App>>() {
                app::open_chat(app, a.inner(), "chat", serde_json::json!({}));
            }
        }))
        .invoke_handler(tauri::generate_handler![ipc::call, ipc::cat_event])
        .setup(|app| {
            let a = app::setup(app.handle())?;
            app.manage(a.clone());
            demo::maybe_run(app.handle().clone(), a);
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("사이를 시작하지 못했어요")
        .run(|app, event| match event {
            // 창을 모두 닫아도 고양이는 남는다
            tauri::RunEvent::ExitRequested { api, code, .. } => {
                if code.is_none() {
                    api.prevent_exit();
                }
            }
            tauri::RunEvent::Exit => {
                if let Some(a) = app.try_state::<Arc<app::App>>() {
                    a.stores.flush_all();
                }
            }
            _ => {}
        });
}
