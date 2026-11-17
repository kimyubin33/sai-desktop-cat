mod dialog;
mod disk;
use crate::app::App;
use sai_core::{google::FileProgress, util::J};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tauri::{Emitter, WebviewWindow};
use tokio::sync::{watch, Mutex as AsyncMutex};

#[derive(Default)]
pub struct Jobs {
    gate: AsyncMutex<()>,
    cancel: Mutex<Option<watch::Sender<bool>>>,
}
impl Jobs {
    pub fn cancel(&self) {
        if let Some(sender) = &*self.cancel.lock().unwrap() {
            let _ = sender.send(true);
        }
    }
}
fn progress(window: &WebviewWindow, epoch: u64, folder: String, request: String) -> FileProgress {
    let window = window.clone();
    let last = Mutex::new((String::new(), 0u64));
    Arc::new(move |phase, done, total| {
        let mut last = last.lock().unwrap();
        if last.0 == phase && done != total && done.saturating_sub(last.1) < 256 * 1024 {
            return;
        }
        *last = (phase.into(), done);
        let _=window.emit_to("chat","files:progress",json!({"epoch":epoch,"folderId":folder,"request":request,"phase":phase,"done":done,"total":total}));
    })
}
pub async fn command(
    a: &Arc<App>,
    window: &WebviewWindow,
    ch: &str,
    args: Value,
) -> Result<Value, String> {
    if ch == "files:cancel" {
        a.file_jobs.cancel();
        return Ok(json!(true));
    }
    let epoch = args["epoch"]
        .as_u64()
        .ok_or("계정 정보를 다시 확인해 주세요.")?;
    let folder = args.s("folderId");
    if ch == "files:sources" {
        return a
            .google
            .browse_drive_files(
                epoch,
                &args.s("mode"),
                &args.s("parent"),
                &args.s("query"),
                &args.s("page"),
            )
            .await;
    }
    if ch == "files:list" {
        return a
            .google
            .list_team_files(epoch, &folder, &args.s("page"))
            .await;
    }
    if ch == "files:open" {
        let fid = args.s("fileId");
        a.google.team_file_info(epoch, &folder, &fid).await?;
        return a
            .google
            .with_current_account(epoch, || {
                crate::system::open_url(&format!("https://drive.google.com/open?id={fid}"));
                Ok(json!(true))
            })
            .await;
    }
    let _guard = a.file_jobs.gate.try_lock().map_err(|_| {
        "다른 파일 전송을 진행하고 있어요. 완료하거나 취소한 뒤 다시 시도해 주세요."
    })?;
    let (cancel, mut cancelled) = watch::channel(false);
    *a.file_jobs.cancel.lock().unwrap() = Some(cancel);
    let progress = progress(
        window,
        epoch,
        folder.clone(),
        args.s("request").chars().take(128).collect(),
    );
    let operation = async {
        match ch {
            "files:upload" => {
                a.google.check_file_upload(epoch, &folder).await?;
                let Some(path) = dialog::choose(window, None).await? else {
                    return Ok(json!({"cancelled":true}));
                };
                progress("preparing", 0, 0);
                let (name, bytes) = tokio::task::spawn_blocking(move || disk::source(&path))
                    .await
                    .map_err(|_| "파일을 읽는 작업을 마치지 못했어요.")??;
                a.google
                    .upload_team_file(epoch, &folder, &name, &bytes, progress)
                    .await
            }
            "files:retry" => {
                a.google
                    .retry_team_file(epoch, &folder, &args.s("operation"), progress)
                    .await
            }
            "files:discard" => {
                a.google
                    .discard_team_upload(epoch, &folder, &args.s("operation"))
                    .await?;
                Ok(json!(true))
            }
            "files:import" => {
                let ids: Vec<String> = args["fileIds"]
                    .as_array()
                    .filter(|v| !v.is_empty() && v.len() <= 10 && v.iter().all(|id| id.is_string()))
                    .ok_or("파일을 1~10개 선택해 주세요.")?
                    .iter()
                    .map(|id| id.as_str().unwrap().to_owned())
                    .collect();
                a.google
                    .import_drive_files(epoch, &folder, &ids, progress)
                    .await
            }
            "files:retry-import" => {
                a.google
                    .retry_drive_import(epoch, &folder, &args.s("operation"), progress)
                    .await
            }
            "files:discard-import" => {
                a.google
                    .discard_drive_import(epoch, &folder, &args.s("operation"))
                    .await?;
                Ok(json!(true))
            }
            "files:download" => {
                let fid = args.s("fileId");
                let info = a.google.team_file_info(epoch, &folder, &fid).await?;
                if !info.b("canDownload") {
                    return Err("이 파일은 Drive에서 열어 주세요. 다운로드 권한·형식·25MB 한도를 확인할 수 있어요.".into());
                }
                let name = sai_core::google::safe_download_name(&info.s("name"));
                let Some(path) = dialog::choose(window, Some(name)).await? else {
                    return Ok(json!({"cancelled":true}));
                };
                disk::destination(&path)?;
                let data = a
                    .google
                    .download_team_file(epoch, &folder, &fid, progress)
                    .await?;
                a.google
                    .with_current_account(epoch, || {
                        Ok(json!({"savedName":disk::save(&path,&data.bytes)?}))
                    })
                    .await
            }
            _ => Err("허용되지 않은 파일 요청이에요.".into()),
        }
    };
    let result = tokio::select! { biased;
        _=cancelled.changed()=>Err("파일 작업을 중단했어요. 전송을 시작한 파일은 대기 목록에서 결과를 다시 확인할 수 있어요.".into()),
        result=operation=>result,
    };
    *a.file_jobs.cancel.lock().unwrap() = None;
    result
}
