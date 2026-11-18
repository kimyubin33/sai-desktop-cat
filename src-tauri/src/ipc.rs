use crate::{
    app::{on_settings_changed, App},
    cat,
};
use sai_core::{
    advice::{dnd_span_minutes, MAX_DND_MINUTES},
    util::J,
};
use serde_json::{json, Value};
use std::sync::Arc;
use tauri::{Emitter, State, WebviewWindow};
type R = Result<Value, String>;

#[tauri::command]
pub async fn call(
    window: WebviewWindow,
    state: State<'_, Arc<App>>,
    ch: String,
    args: Option<Value>,
) -> R {
    if window.label() != "chat" {
        return Err("허용되지 않은 요청이에요.".into());
    }
    dispatch(state.inner(), &window, &ch, args.unwrap_or(Value::Null)).await
}

#[tauri::command]
pub async fn cat_event(
    window: WebviewWindow,
    state: State<'_, Arc<App>>,
    ev: String,
    args: Option<Value>,
) -> R {
    if window.label() != "cat" {
        return Err("허용되지 않은 요청이에요.".into());
    }
    let a = state.inner().clone();
    let args = args.unwrap_or(Value::Null);
    let app = &a.handle;
    match ev.as_str() {
        "regions" => cat::regions(&a, &args),
        "drag-start" => cat::drag_start(app, &a),
        "drag-end" => cat::drag_end(app, &a, args.as_bool().unwrap_or(false)),
        "context-menu" => cat::context_menu(app, &a),
        "bubble-hover" => cat::bubble_hover(&a, args.as_bool().unwrap_or(false)),
        "bubble-action" => cat::bubble_action(app, &a, &args.s("id"), &args.s("action")),
        "ready" => cat::resend_state(app, &a),
        _ => {}
    }
    Ok(Value::Null)
}

fn ok() -> R {
    Ok(json!(true))
}

async fn dispatch(a: &Arc<App>, win: &WebviewWindow, ch: &str, x: Value) -> R {
    match ch {
        "window:minimize" => {
            win.minimize().map_err(|_| "창을 최소화하지 못했어요.")?;
            ok()
        }
        "window:maximize" => {
            if win.is_maximized().unwrap_or(false) {
                win.unmaximize().map_err(|_| "창을 복원하지 못했어요.")?;
            } else {
                win.maximize().map_err(|_| "창을 최대화하지 못했어요.")?;
            }
            ok()
        }
        "window:close" => {
            win.close().map_err(|_| "창을 닫지 못했어요.")?;
            ok()
        }
        "help:open" => {
            crate::system::open_url("https://github.com/kimyubin33/sai-desktop-cat/blob/work/google-workspace-preview-20261118/docs/user-guide.md");
            ok()
        }
        // 옛 팀/톡/호스트/공유 키 요청은 이 허용 목록에 포함하지 않는다.
        "workspace:status" => Ok(workspace_status(a)),
        "calendar:preview" => {
            a.google
                .calendar_preview(
                    x["epoch"].as_u64().ok_or("계정을 확인해 주세요.")?,
                    &x.s("folderId"),
                    &x.s("taskId"),
                )
                .await
        }
        "calendar:connect" => {
            a.google
                .connect_calendar(x["epoch"].as_u64().ok_or("계정을 확인해 주세요.")?)
                .await
        }
        "calendar:add" => {
            a.google
                .add_to_calendar(
                    x["epoch"].as_u64().ok_or("계정을 확인해 주세요.")?,
                    &x.s("folderId"),
                    x["add"].clone(),
                )
                .await
        }
        "calendar:cancel" => {
            a.google.cancel_login().await;
            Ok(Value::Null)
        }
        "calendar:change" => {
            a.google
                .change_calendar(
                    x["epoch"].as_u64().ok_or("계정을 확인해 주세요.")?,
                    &x.s("folderId"),
                    x["change"].clone(),
                )
                .await
        }
        "calendar:check-change" => {
            a.google
                .check_calendar_change(
                    x["epoch"].as_u64().ok_or("계정을 확인해 주세요.")?,
                    &x.s("folderId"),
                    &x.s("reviewId"),
                )
                .await
        }
        "calendar:forget" => {
            a.google.forget_calendar(
                x["epoch"].as_u64().unwrap_or(0),
                &x.s("folderId"),
                &x.s("reviewId"),
            );
            Ok(Value::Null)
        }
        "analysis:prepare" => {
            a.google
                .prepare_ai_review(
                    x["epoch"].as_u64().ok_or("계정을 확인해 주세요.")?,
                    &x.s("folderId"),
                    x["source"].clone(),
                )
                .await
        }
        "analysis:run" => {
            a.google
                .run_ai_review(
                    x["epoch"].as_u64().ok_or("계정을 확인해 주세요.")?,
                    &x.s("folderId"),
                    &x.s("reviewId"),
                    x["confirmed"] == true,
                    option_env!("SAI_TEAM_AI_ENDPOINT"),
                )
                .await
        }
        "analysis:commit" => {
            a.google
                .commit_ai_review(
                    x["epoch"].as_u64().ok_or("계정을 확인해 주세요.")?,
                    &x.s("folderId"),
                    &x.s("reviewId"),
                    x["choices"].clone(),
                    x["confirmed"] == true,
                )
                .await
        }
        "analysis:forget" => {
            a.google.forget_ai_review(
                x["epoch"].as_u64().unwrap_or(0),
                &x.s("folderId"),
                &x.s("reviewId"),
            );
            Ok(Value::Null)
        }
        "team-ai:status" => {
            a.google
                .team_ai_status(
                    x["epoch"].as_u64().ok_or("계정을 다시 확인해 주세요.")?,
                    &x.s("folderId"),
                    option_env!("SAI_TEAM_AI_ENDPOINT"),
                )
                .await
        }
        "team-ai:connect" => {
            a.google
                .connect_team_ai(
                    x["epoch"].as_u64().ok_or("계정을 다시 확인해 주세요.")?,
                    &x.s("folderId"),
                    option_env!("SAI_TEAM_AI_ENDPOINT"),
                )
                .await
        }
        "collab:load" | "collab:sync" => {
            a.google
                .collaboration(
                    x["epoch"].as_u64().ok_or("계정을 다시 확인해 주세요.")?,
                    &x.s("folderId"),
                    ch == "collab:sync",
                )
                .await
        }
        "collab:save" => {
            a.google
                .save_collaboration(
                    x["epoch"].as_u64().ok_or("계정을 다시 확인해 주세요.")?,
                    &x.s("folderId"),
                    x["edit"].clone(),
                )
                .await
        }
        "files:list"
        | "files:sources"
        | "files:upload"
        | "files:retry"
        | "files:discard"
        | "files:download"
        | "files:import"
        | "files:retry-import"
        | "files:discard-import"
        | "files:open"
        | "files:cancel" => crate::file_transfer::command(a, win, ch, x).await,
        "teams:status" => a.google.team_status().await,
        "teams:browse" => {
            a.google
                .browse_team_folders(
                    x["epoch"].as_u64().ok_or("계정 연결을 확인해 주세요.")?,
                    &x.s("page"),
                )
                .await
        }
        "teams:enable" | "teams:create" | "teams:connect" | "teams:invite" | "teams:remove"
        | "teams:open" => {
            let epoch = x["epoch"].as_u64().ok_or("팀 목록을 다시 확인해 주세요.")?;
            let result = match ch {
                "teams:enable" => {
                    a.google.enable_team_drive(epoch).await?;
                    let _ = a.google.sync_profile().await;
                    Value::Null
                }
                "teams:create" => a.google.create_team_folder(epoch, &x.s("name")).await?,
                "teams:connect" => {
                    a.google
                        .connect_team_folder(epoch, &x.s("folderId"))
                        .await?
                }
                "teams:invite" => {
                    a.google
                        .invite_team_folder(epoch, &x.s("folderId"), &x.s("email"))
                        .await?
                }
                "teams:remove" => {
                    a.google.remove_team_folder(epoch, &x.s("folderId")).await?;
                    Value::Null
                }
                "teams:open" => {
                    let url = a.google.open_team_folder(epoch, &x.s("folderId")).await?;
                    if a.google.profile_status()["epoch"] != epoch {
                        return Err("계정 연결이 바뀌었어요.".into());
                    }
                    crate::system::open_url(&url);
                    return ok();
                }
                _ => unreachable!(),
            };
            let sync = a.clone();
            tauri::async_runtime::spawn(async move {
                let _ = sync.google.sync_profile().await;
                let _ = sync.handle.emit_to("chat", "profile:changed", ());
            });
            on_settings_changed(a);
            Ok(json!({"result":result,"state":a.google.team_status().await?}))
        }
        "legacy:status" | "legacy:export" | "legacy:open-original" | "legacy:open-exports" => {
            crate::legacy::command(a, ch).await
        }
        "g:login" => {
            readable(&a.stores.settings)?;
            a.google.login().await?;
            persist(&a.stores.settings)?;
            // 상태 캐시의 오류가 로그인·로그아웃 자체를 막지 않게 한다.
            a.stores.google_connection.flush();
            let _ = a.google.sync_profile().await;
            on_settings_changed(a);
            Ok(workspace_status(a))
        }
        "g:resume" => {
            readable(&a.stores.settings)?;
            a.google.resume().await?;
            let _ = a.google.sync_profile().await;
            on_settings_changed(a);
            Ok(workspace_status(a))
        }
        "g:upgrade" => {
            readable(&a.stores.settings)?;
            a.google
                .enable_team_drive(
                    x["epoch"]
                        .as_u64()
                        .ok_or("계정 정보를 다시 확인해 주세요.")?,
                )
                .await?;
            let _ = a.google.sync_profile().await;
            on_settings_changed(a);
            Ok(workspace_status(a))
        }
        "g:cancel-login" => {
            a.google.cancel_login().await;
            Ok(workspace_status(a))
        }
        "g:logout" => {
            a.file_jobs.cancel();
            readable(&a.stores.settings)?;
            a.google.logout().await?;
            persist(&a.stores.settings)?;
            a.stores.google_connection.flush();
            on_settings_changed(a);
            Ok(workspace_status(a))
        }
        "profile:status" => Ok(a.google.profile_status()),
        "profile:sync" => {
            let _ = a.google.sync_profile().await;
            on_settings_changed(a);
            Ok(a.google.profile_status())
        }
        "profile:update" => {
            a.google
                .update_profile(
                    x["epoch"]
                        .as_u64()
                        .ok_or("계정 정보를 다시 확인해 주세요.")?,
                    x["patch"].clone(),
                )
                .await?;
            on_settings_changed(a);
            let sync = a.clone();
            tauri::async_runtime::spawn(async move {
                let _ = sync.google.sync_profile().await;
                let _ = sync.handle.emit_to("chat", "profile:changed", ());
            });
            Ok(a.google.profile_status())
        }
        "app:open-drive" => {
            crate::system::open_url("https://drive.google.com/drive/u/0/my-drive");
            ok()
        }
        "chat:start" => {
            readable(&a.stores.private_conv)?;
            if x.s("mode") != "free" {
                return Err("지금은 개인 AI 대화만 사용할 수 있어요.".into());
            }
            let result = a.chat.start("free", None).await;
            persist(&a.stores.private_conv)?;
            Ok(result)
        }
        "chat:send" => {
            readable(&a.stores.private_conv)?;
            let session = a.chat.find(&x.s("id")).ok_or("대화를 찾을 수 없어요.")?;
            if session.mode != "free" {
                return Err("이전 대화는 기록에서 확인하고 새 대화를 시작해 주세요.".into());
            }
            let text: String = x.s("text").chars().take(50000).collect();
            let stream_id = x.s("streamId");
            let w = win.clone();
            let on_text = move |full: &str| {
                let _ = w.emit_to(
                    "chat",
                    "chat:stream",
                    json!({"streamId":stream_id,"text":full}),
                );
            };
            let result = a
                .chat
                .send(&session.id, &text, false, Some(&on_text))
                .await?;
            persist(&a.stores.private_conv)?;
            Ok(result)
        }
        "chat:list" => {
            readable(&a.stores.private_conv)?;
            Ok(a.chat.list())
        }
        "chat:get" => {
            readable(&a.stores.private_conv)?;
            Ok(a.chat.get(x.as_str().unwrap_or("")))
        }
        "update:status" => Ok(a.updater.state()),
        "update:check" => Ok(a.updater.check_in_background(a)),
        "update:install" => {
            a.updater.install(a);
            ok()
        }
        "settings:get" => Ok(public_settings(a)),
        "settings:set" => {
            readable(&a.stores.settings)?;
            readable(&a.stores.private_conv)?;
            let p = x;
            // Google reads the settings store too; resolve this before taking its write lock.
            let local_frequency = !a.google.is_ready();
            let mut error = None;
            a.stores.settings.update(|s| {
                let f = p.s("frequency");
                if local_frequency && ["low", "normal", "high"].contains(&f.as_str()) {
                    s["frequency"] = json!(f);
                }
                if p.get("dnd").is_some() {
                    let d = &p["dnd"];
                    let dnd = if !d.s("start").is_empty() && !d.s("end").is_empty() { json!({ "start": d.s("start"), "end": d.s("end") }) } else { Value::Null };
                    // 방해 금지는 잠·휴식 시간용: 하루 10시간까지만 (그 이상이면 사실상 끄기가 되므로)
                    if !dnd.is_null() && dnd_span_minutes(&dnd) > MAX_DND_MINUTES {
                        error = Some("방해 금지 시간은 하루 10시간까지 정할 수 있어요.".to_string());
                    } else {
                        s["dnd"] = dnd;
                    }
                }
                if let Some(r) = p.get("retentionDays").and_then(|v| v.as_i64()) {
                    if [0, 7, 30, 90].contains(&r) {
                        s["retentionDays"] = json!(r);
                    }
                }
                if p["ai"].is_object() {
                    let model = p["ai"].os("model").filter(|m| !m.trim().is_empty()).unwrap_or_else(|| s["ai"].s("model"));
                    s["ai"] = json!({ "enabled": p["ai"].b("enabled"), "model": model.chars().take(80).collect::<String>() });
                }
            });
            if let Some(e) = error {
                return Err(e);
            }
            persist(&a.stores.settings)?;
            a.chat.purge_old();
            persist(&a.stores.private_conv)?;
            on_settings_changed(a);
            Ok(public_settings(a))
        }
        "settings:reset-dampened" => ok(),
        "ai:set-key" => {
            readable(&a.stores.settings)?;
            a.ai.set_key(x.as_str().unwrap_or(""));
            persist(&a.stores.settings)?;
            Ok(public_settings(a))
        }
        "ai:test" => Ok(a.ai.test().await),
        "ai:set-openai" => {
            readable(&a.stores.settings)?;
            if !x.is_object()
                || ["key", "model"]
                    .iter()
                    .any(|k| x.get(*k).is_some_and(|v| !v.is_string()))
            {
                return Err("GPT 연결 설정의 형식이 올바르지 않아요.".into());
            }
            a.ai.configure_openai(x["key"].as_str(), x["model"].as_str())?;
            persist(&a.stores.settings)?;
            Ok(public_settings(a))
        }
        "ai:test-openai" => Ok(a.ai.test_openai().await),
        _ => Err("이 기능은 새 Google 작업 공간으로 전환 중이에요.".into()),
    }
}

fn workspace_status(a: &Arc<App>) -> Value {
    json!({ "google": a.google.status(), "profile": a.google.profile_status(), "teamsReady": false, "filesReady": false, "teamAiReady": false })
}

fn readable(store: &sai_core::store::Store) -> Result<(), String> {
    match store.load_error() {
        Some(error) => Err(error),
        None => Ok(()),
    }
}
fn persist(store: &sai_core::store::Store) -> Result<(), String> {
    store.try_flush().map_err(|_| "기기에 저장하지 못했어요. 기존 원본은 덮어쓰지 않아요. 저장 공간과 파일 상태를 확인해 주세요.".into())
}

fn public_settings(a: &Arc<App>) -> Value {
    let s = a.stores.settings.get();
    json!({
        "catName":s["catName"], "frequency":s["frequency"], "dnd":s["dnd"],
        "googleProfile":a.google.profile_status(),
        "retentionDays":s["retentionDays"], "ai":s["ai"],
        "hasApiKey":a.ai.has_key(), "hasOwnKey":a.ai.own_key().is_some(),
        "hasOpenAiKey":a.ai.has_openai_key(), "openAiModel":a.ai.openai_model(),
        "openAiKeyPersistent":cfg!(windows),
        "storageWarning": a.stores.settings.load_error().or_else(|| a.stores.private_conv.load_error()).or_else(|| a.stores.google_connection.load_error()),
    })
}
