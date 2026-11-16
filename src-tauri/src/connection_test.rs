//! 기존 사이의 저장소·자동 시작·외부 계정에 접근하지 않는 휴대용 연결 진단 앱.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod mesh;

use serde_json::Value;
use tauri::{Manager, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

#[tauri::command]
async fn probe_call(
    window: WebviewWindow,
    state: State<'_, mesh::ProbeState>,
    ch: String,
    args: Option<Value>,
) -> Result<Value, String> {
    if window.label() != "connection-test" || !ch.starts_with("mesh-probe:") {
        return Err("연결 진단에서 허용되지 않은 요청이에요.".into());
    }
    mesh::command(state.inner(), &ch, &args.unwrap_or(Value::Null)).await
}

fn main() {
    let mut context = tauri::generate_context!();
    context.config_mut().identifier = "com.kimyubin33.sai.connection-test".into();
    context.config_mut().product_name = Some("사이 PC 연결 진단".into());
    tauri::Builder::default()
        .manage(mesh::ProbeState::default())
        .invoke_handler(tauri::generate_handler![probe_call])
        .setup(|app| {
            WebviewWindowBuilder::new(
                app,
                "connection-test",
                WebviewUrl::App("probe/index.html".into()),
            )
            .title("사이 PC 연결 진단")
            .inner_size(680.0, 800.0)
            .min_inner_size(420.0, 520.0)
            .build()?;
            Ok(())
        })
        .build(context)
        .expect("연결 진단 앱을 시작하지 못했어요")
        .run(|app, event| {
            if matches!(event, tauri::RunEvent::Exit) {
                if let Some(state) = app.try_state::<mesh::ProbeState>() {
                    if let Ok(mut slot) = state.try_lock() {
                        slot.take();
                    }
                }
            }
        });
}
