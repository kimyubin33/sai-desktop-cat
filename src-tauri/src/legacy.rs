use crate::app::App;
use sai_core::store::legacy::{export_folder, Archive};
use serde_json::{json, Value};
use std::sync::Arc;

pub async fn command(a: &Arc<App>, channel: &str) -> Result<Value, String> {
    match channel {
        "legacy:status" => {
            let dir = a.data_dir.clone();
            tauri::async_runtime::spawn_blocking(move || {
                Archive::read(&dir).map(|archive| json!(archive.summary))
            })
            .await
            .map_err(|_| "이전 자료 확인을 마치지 못했어요.")?
        }
        "legacy:export" => {
            let _guard = a
                .legacy_export
                .try_lock()
                .map_err(|_| "이전 자료를 저장하고 있어요. 잠시 기다려 주세요.")?;
            let dir = a.data_dir.clone();
            tauri::async_runtime::spawn_blocking(move || {
                let archive = Archive::read(&dir)?;
                let path = archive.export(&dir)?;
                Ok(json!({"fileName":path.file_name().and_then(|n| n.to_str()),"summary":archive.summary}))
            }).await.map_err(|_| "이전 자료 내보내기를 마치지 못했어요. 원본은 유지돼요.")?
        }
        "legacy:open-original" | "legacy:open-exports" => {
            let path = if channel == "legacy:open-exports" {
                export_folder(&a.data_dir, false)?
            } else {
                std::fs::canonicalize(&a.data_dir).map_err(|_| "자료 보관 폴더를 찾지 못했어요.")?
            };
            open::that_detached(path).map_err(|_| "자료 폴더를 열지 못했어요.")?;
            Ok(Value::Null)
        }
        _ => Err("지원하지 않는 자료 보관 요청이에요.".into()),
    }
}
