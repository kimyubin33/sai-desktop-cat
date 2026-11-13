//! 대화창·고양이 창과 앱 사이의 통신.
//! 대화창은 `call(채널, 값)` 하나로 모든 기능을 부르고, 채널 이름은 예전 형식을 그대로 쓴다.
use crate::app::{on_settings_changed, sync_now, App};
use crate::cat;
use sai_core::advice::{dnd_span_minutes, MAX_DND_MINUTES};
use sai_core::collab::OBJECTION_REASONS;
use sai_core::privacy::{build_preview, create_outbox_items, leaks_raw_text, merge_into_outbox};
use sai_core::team::backend::parse_firebase_config;
use sai_core::time::now_ms;
use sai_core::topics::{topic, TOPICS};
use sai_core::util::{arr_mut, J};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tauri::{Emitter, State, WebviewWindow};

type R = Result<Value, String>;

#[tauri::command]
pub async fn call(window: WebviewWindow, state: State<'_, Arc<App>>, ch: String, args: Option<Value>) -> R {
    if window.label() != "chat" {
        return Err("허용되지 않은 요청이에요.".into());
    }
    let a = state.inner().clone();
    dispatch(&a, &window, &ch, args.unwrap_or(Value::Null)).await
}

#[tauri::command]
pub async fn cat_event(window: WebviewWindow, state: State<'_, Arc<App>>, ev: String, args: Option<Value>) -> R {
    if window.label() != "cat" {
        return Err("허용되지 않은 요청이에요.".into());
    }
    let a = state.inner().clone();
    let args = args.unwrap_or(Value::Null);
    let app = &a.handle;
    match ev.as_str() {
        "regions" => cat::regions(&a, &args),
        "drag-start" => cat::drag_start(app, &a),
        "drag-end" => cat::drag_end(app, &a),
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
    let hub = &a.hub;
    match ch {
        // ---------- 대화 ----------
        "chat:start" => {
            let (mode, t) = if x.is_string() { (x.as_str().unwrap().to_string(), None) } else { (x.s("mode"), x.os("topic")) };
            Ok(a.chat.start(&mode, t.as_deref()).await)
        }
        "chat:send" => {
            let text: String = x.s("text").chars().take(50000).collect();
            let stream_id = x.os("streamId");
            let w = win.clone();
            let on_text = move |full: &str| {
                if let Some(id) = &stream_id {
                    let _ = w.emit_to("chat", "chat:stream", json!({ "streamId": id, "text": full }));
                }
            };
            let cb: &(dyn Fn(&str) + Send + Sync) = &on_text;
            a.chat.send(&x.s("id"), &text, x.b("skip"), if x.os("streamId").is_some() { Some(cb) } else { None }).await
        }
        "chat:summarize-now" => Ok(a.chat.summarize_now(x.as_str().unwrap_or(""))),
        "chat:update-summary" => Ok(a.chat.update_summary(&x.s("id"), &x["patch"])),
        "chat:list" => Ok(a.chat.list()),
        "chat:get" => Ok(a.chat.get(x.as_str().unwrap_or(""))),
        "chat:delete" => {
            a.chat.delete(x.as_str().unwrap_or(""));
            ok()
        }
        "chat:delete-all" => {
            a.chat.delete_all();
            a.schedule.clear_chat();
            ok()
        }
        "topics:list" => Ok(json!(TOPICS.iter().map(|t| json!({ "id": t.id, "label": t.label })).collect::<Vec<_>>())),

        // ---------- 공유 (여러 팀) ----------
        "share:preview" => {
            let Some(s) = a.chat.find(&x.s("id")) else { return Ok(json!({ "items": [] })) };
            let Some(summary) = s.summary.clone() else { return Ok(json!({ "items": [] })) };
            // 받을 팀원 목록이 오래됐으면 먼저 새로 받아 온다
            let stale = hub.all().iter().any(|t| now_ms() - t.cache_get().i("lastSync") > 2 * 60 * 1000);
            if !hub.all().is_empty() && stale {
                sync_now(a).await;
            }
            let mut preview = build_preview(&summary, &s.id);
            let teams: Vec<Value> = hub
                .all()
                .iter()
                .map(|t| json!({ "teamId": t.id, "name": t.name(), "memberCount": t.members(true).len(), "members": t.members(false) }))
                .collect();
            let active = hub.active();
            let info = teams.iter().find(|t| t["teamId"] == json!(active.id)).cloned();
            preview["teams"] = json!(teams);
            preview["defaultTeamId"] = json!(active.id);
            preview["team"] = info.as_ref().map(|i| json!({ "name": i["name"], "memberCount": i["memberCount"] })).unwrap_or(Value::Null);
            preview["members"] = info.as_ref().map(|i| i["members"].clone()).unwrap_or(json!([]));
            // 팀 규모와 상관없이 안내하되, 작은 팀이면 더 분명하게
            if info.map(|i| i.i("memberCount") > 3).unwrap_or(false) {
                preview["smallTeamWarning"] = Value::Null;
            }
            Ok(preview)
        }
        "share:save" => {
            let Some(s) = a.chat.find(&x.s("id")) else { return Ok(json!({ "saved": 0 })) };
            let Some(summary) = s.summary.clone().filter(|_| s.safety_hold.is_none()) else { return Ok(json!({ "saved": 0 })) };
            let t = hub.team(x.os("teamId").as_deref().or(hub.active().id.as_deref())).unwrap_or_else(|| hub.active());
            let raw: Vec<String> = s.turns.iter().filter(|t| t.role == "user").map(|t| t.text.clone()).collect();
            let choices = x.arr("choices");
            let recipients: HashMap<String, Value> = choices.iter().map(|c| (c.s("topic"), c.get("toMember").cloned().unwrap_or(Value::Null))).collect();
            let items: Vec<Value> = create_outbox_items(&summary, &choices, now_ms(), &s.id)
                .into_iter()
                .map(|mut it| {
                    it["toMember"] = if it.s("target") == "person" { recipients.get(&it.s("topic")).cloned().unwrap_or(Value::Null) } else { Value::Null };
                    it
                })
                // 혹시라도 원문 조각이 섞였다면 다른 일반 문안으로 바꾸고, 그래도 겹치면 저장하지 않는다
                .filter_map(|mut it| {
                    if !leaks_raw_text(&it, &raw, 12) {
                        return Some(it);
                    }
                    let tp = topic(&it.s("topic"))?;
                    let alt = tp.advice.iter().find(|adv| {
                        let mut c = it.clone();
                        c["advice"] = json!(adv);
                        !leaks_raw_text(&c, &raw, 12)
                    })?;
                    it["advice"] = json!(alt);
                    Some(it)
                })
                .collect();
            let items: Vec<Value> = t
                .prepare_items(items)
                .into_iter()
                .map(|mut it| {
                    it["teamId"] = json!(t.id);
                    it
                })
                .collect();
            let n = items.len();
            a.stores.shared_outbox.update(|d| {
                let merged = merge_into_outbox(&d.arr("items"), &items);
                d["items"] = json!(merged);
            });
            if t.is_connected() {
                let a2 = a.clone();
                tauri::async_runtime::spawn(async move {
                    sync_now(&a2).await;
                });
            }
            Ok(json!({ "saved": n, "connected": t.is_connected(), "teamName": t.name() }))
        }
        "share:confirm" | "share:set-recipient" => {
            let item_id = x.s("itemId");
            let team_id = a.stores.shared_outbox.read(|d| d.arr("items").into_iter().find(|i| i.s("id") == item_id).and_then(|i| i.os("teamId")));
            if let Some(t) = hub.team(team_id.as_deref()) {
                if ch == "share:confirm" {
                    t.confirm_review(&item_id, x.os("toMember"));
                } else {
                    t.set_recipient(&item_id, &x.s("toMember"));
                }
            }
            let a2 = a.clone();
            tauri::async_runtime::spawn(async move {
                sync_now(&a2).await;
            });
            ok()
        }
        "share:outbox" => {
            let names: HashMap<String, String> = hub.teams().iter().map(|t| (t.s("teamId"), t.s("name"))).collect();
            Ok(json!(a
                .stores
                .shared_outbox
                .read(|d| d.arr("items"))
                .into_iter()
                .map(|mut it| {
                    it["teamName"] = match it.os("teamId") {
                        Some(id) => json!(names.get(&id).cloned().unwrap_or_else(|| "나간 팀".into())),
                        None => Value::Null,
                    };
                    it
                })
                .collect::<Vec<_>>()))
        }
        "share:remove" => {
            let id = x.as_str().unwrap_or("").to_string();
            a.stores.shared_outbox.update(|d| arr_mut(d, "items").retain(|i| i.s("id") != id));
            ok()
        }

        // ---------- 팀 ----------
        "team:status" => Ok(team_view(a)),
        "team:configure" => {
            let Some(cfg) = parse_firebase_config(x.as_str().unwrap_or("")) else {
                return Err("apiKey 와 projectId 를 찾지 못했어요. Firebase 콘솔의 firebaseConfig 를 그대로 붙여 넣어 주세요.".into());
            };
            let prev = a.stores.settings.read(|s| s["firebase"].s("projectId"));
            a.stores.settings.update(|s| {
                if prev != cfg.s("projectId") {
                    // 다른 프로젝트로 바꾸면 이전 로그인과 팀 연결은 쓸 수 없다
                    s["firebaseSessionEnc"] = Value::Null;
                    s["teams"] = json!([]);
                    s["activeTeamId"] = Value::Null;
                }
                s["firebase"] = cfg.clone();
            });
            hub.reset_backend();
            after_team(a)
        }
        "team:create" => {
            hub.create_team(&x.s("name"), &x.s("displayName")).await?;
            after_team(a)
        }
        "team:join" => {
            hub.join_team(&x.s("code"), &x.s("displayName")).await?;
            after_team(a)
        }
        "team:leave" => {
            let id = x.as_str().map(String::from).or(hub.active().id);
            if let Some(id) = id {
                hub.leave_team(&id).await?;
            }
            after_team(a)
        }
        "team:switch" => {
            hub.set_active(x.as_str().unwrap_or(""))?;
            after_team(a)
        }
        "team:sync" => {
            sync_now(a).await;
            after_team(a)
        }
        "team:respond" => {
            hub.respond(&x.s("proposalId"), &x.s("response")).await?;
            after_team(a)
        }
        "team:hold-topic" => Ok(json!({ "held": hub.hold_topic(x.as_str().unwrap_or("")).await })),
        "team:propose" => {
            let ids = hub.propose_team(x.as_str().unwrap_or(""));
            if !ids.is_empty() {
                let a2 = a.clone();
                tauri::async_runtime::spawn(async move {
                    sync_now(&a2).await;
                });
            }
            Ok(json!({ "queued": !ids.is_empty(), "connected": !hub.all().is_empty() }))
        }

        // ---------- 협업 ----------
        "collab:view" => {
            if x.b("refresh") && hub.active().is_connected() {
                sync_now(a).await;
            }
            Ok(collab_view(a))
        }
        "collab:plan" => {
            if hub.active().is_connected() {
                sync_now(a).await;
            }
            let t = hub.active();
            let pid = x.s("projectId");
            let Some(mut p) = t.plan(&pid) else { return Ok(Value::Null) };
            let names: HashMap<String, String> = t.collab_members().iter().map(|m| (m.s("uid"), m.s("name"))).collect();
            let titles: HashMap<String, String> = t.tasks_of(&pid).iter().map(|x| (x.s("id"), x.s("title"))).collect();
            for asg in arr_mut(&mut p, "assignments") {
                asg["name"] = json!(names.get(&asg.s("uid")));
                asg["title"] = json!(titles.get(&asg.s("taskId")));
            }
            p["missingProfiles"] = json!(t.collab_members().iter().filter(|m| !m.b("hasProfile")).map(|m| m.s("name")).collect::<Vec<_>>());
            Ok(p)
        }
        "collab:apply-plan" => {
            let t = hub.active();
            let n = t.apply_plan(&x.s("projectId"), &x.arr("assignments")).await?;
            let a2 = a.clone();
            tauri::async_runtime::spawn(async move {
                sync_now(&a2).await;
            });
            Ok(json!({ "applied": n, "view": collab_view(a) }))
        }
        c if c.starts_with("collab:") => {
            let t = hub.active();
            match c {
                "collab:save-profile" => drop(t.save_profile(&x).await?),
                "collab:create-project" => drop(t.create_project(&x).await?),
                "collab:update-project" => t.update_project(&x.s("id"), &x).await?,
                "collab:set-active" => t.set_active_project(&x.s("id")),
                "collab:add-task" => drop(t.add_task(&x.s("projectId"), &x).await?),
                "collab:edit-task" => t.edit_task(&x.s("projectId"), &x.s("id"), &x).await?,
                "collab:delete-task" => t.delete_task(&x.s("projectId"), &x.s("id")).await?,
                "collab:accept" => {
                    t.accept(&x.s("id")).await?;
                    // 작업을 맡으면 내 시간표를 다시 짠다
                    a.schedule.view(false).await;
                }
                "collab:object" => t.object(&x.s("id"), &x.s("reason")).await?,
                "collab:status" => {
                    t.set_status(&x.s("id"), &x.s("status")).await?;
                    let a2 = a.clone();
                    tauri::async_runtime::spawn(async move {
                        a2.schedule.view(false).await;
                    });
                }
                "collab:toggle-prep" => t.toggle_prep(&x.s("id"), x.i("index").max(0) as usize).await?,
                _ => return Err(format!("알 수 없는 요청: {c}")),
            }
            Ok(collab_view(a))
        }

        // ---------- 외부 연결 ----------
        "ext:status" => Ok(ext_view(a)),
        "tg:set-bot" => {
            let t = hub.active();
            t.tg_set_bot(x.as_str().unwrap_or("")).await?;
            t.tg_sync().await?;
            Ok(ext_view(a))
        }
        "tg:start-link" => {
            let r = hub.active().tg_start_link()?;
            crate::system::open_url(&r.s("url"));
            Ok(json!({ "url": r.s("url"), "view": ext_view(a) }))
        }
        "tg:check-link" => {
            let mut r = hub.active().tg_check_link().await?;
            r["view"] = ext_view(a);
            Ok(r)
        }
        "tg:team-alerts" => {
            hub.active().tg_set_team_alerts(x.as_bool().unwrap_or(false)).await?;
            Ok(ext_view(a))
        }
        "tg:away" => {
            let on = x.as_bool().unwrap_or(false);
            a.stores.settings.update(|s| s["telegramAway"] = json!(on));
            Ok(ext_view(a))
        }
        "tg:unlink" => {
            hub.active().tg_unlink().await?;
            Ok(ext_view(a))
        }
        "tg:test" => {
            hub.active().tg_test().await?;
            Ok(ext_view(a))
        }
        "g:configure" => {
            a.google.configure(x.as_str().unwrap_or(""))?;
            Ok(ext_view(a))
        }
        "g:login" => {
            a.google.login().await?;
            Ok(ext_view(a))
        }
        "g:logout" => {
            a.google.logout().await?;
            Ok(ext_view(a))
        }
        "g:sync" => Ok(json!({ "result": a.google.sync_todo().await?, "view": ext_view(a) })),
        "g:availability" => a.google.availability(x.os("projectId").as_deref()).await,

        // ---------- 내 작업 시간표 ----------
        "sched:view" => Ok(a.schedule.view(x.b("refresh")).await),
        "sched:replan" => Ok(a.schedule.replan().await),
        "sched:apply" => a.schedule.apply().await,
        "sched:edit" => a.schedule.edit(&x).await,
        "sched:chat" => a.schedule.chat(&x.s("text")).await,
        "sched:clear-chat" => {
            a.schedule.clear_chat();
            Ok(a.schedule.view(false).await)
        }

        // ---------- 업데이트 ----------
        "update:status" => Ok(a.updater.state()),
        "update:check" => Ok(a.updater.check(a, true).await),
        "update:install" => {
            a.updater.install(a);
            ok()
        }

        // ---------- 설정 ----------
        "settings:get" => Ok(public_settings(a)),
        "settings:set" => {
            let p = x;
            let mut error = None;
            a.stores.settings.update(|s| {
                let f = p.s("frequency");
                if ["low", "normal", "high"].contains(&f.as_str()) {
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
            a.chat.purge_old();
            on_settings_changed(a);
            Ok(public_settings(a))
        }
        "settings:reset-dampened" => ok(),
        "ai:set-key" => {
            a.ai.set_key(x.as_str().unwrap_or(""));
            ok()
        }
        "ai:test" => Ok(a.ai.test().await),
        _ => Err(format!("알 수 없는 요청: {ch}")),
    }
}

fn public_settings(a: &Arc<App>) -> Value {
    let s = a.stores.settings.get();
    json!({
        "catName": s["catName"], "frequency": s["frequency"], "quietUntil": s["quietUntil"], "dnd": s["dnd"],
        "retentionDays": s["retentionDays"], "ai": s["ai"], "hasApiKey": a.ai.has_key(),
    })
}

fn after_team(a: &Arc<App>) -> R {
    on_settings_changed(a);
    Ok(team_view(a))
}

fn team_view(a: &Arc<App>) -> Value {
    let hub = &a.hub;
    let t = hub.active();
    let c = t.cache_get();
    let me = c.s("myUid");
    let cfg = a.stores.settings.read(|s| s["firebase"].clone());
    let cfg = if cfg.is_object() { cfg } else { crate::app::builtin_firebase() };
    let mut v = t.status();
    v["configured"] = json!(a.local_mode || !cfg.s("apiKey").is_empty());
    v["projectId"] = json!(if a.local_mode { "로컬 시험 모드".into() } else { cfg.s("projectId") });
    v["localMode"] = json!(a.local_mode);
    v["teams"] = json!(hub
        .all()
        .iter()
        .map(|b| {
            let st = b.state().unwrap_or(Value::Null);
            let c = b.cache_get();
            json!({
                "teamId": b.id, "name": st.s("name"), "joinCode": st.s("joinCode"), "displayName": st.s("displayName"),
                "active": b.id == t.id, "memberCount": c.arr("members").len(), "lastSync": c.i("lastSync"), "lastError": c["lastError"],
            })
        })
        .collect::<Vec<_>>());
    v["activeTeamId"] = json!(t.id);
    v["members"] = json!(t
        .members(true)
        .into_iter()
        .map(|mut m| {
            m["me"] = json!(m.s("uid") == me);
            m
        })
        .collect::<Vec<_>>());
    v["proposals"] = json!(t.proposals());
    v["inboxCount"] = json!(c.arr("inbox").len());
    v
}

fn collab_view(a: &Arc<App>) -> Value {
    let t = a.hub.active();
    if !t.is_connected() {
        return json!({ "connected": false });
    }
    let d = t.collab_data();
    let members = t.collab_members();
    let name_of: HashMap<String, String> = members.iter().map(|m| (m.s("uid"), m.s("name"))).collect();
    let project = t.active_project();
    let tasks: Vec<Value> = project
        .as_ref()
        .map(|p| {
            t.tasks_of(&p.s("id"))
                .into_iter()
                .map(|mut x| {
                    let assignee = x.s("assignee");
                    x["assigneeName"] = if assignee.is_empty() { Value::Null } else { json!(name_of.get(&assignee).cloned().unwrap_or_else(|| "나간 팀원".into())) };
                    x["objectionNames"] = json!(x.obj("objections").keys().map(|u| name_of.get(u).cloned().unwrap_or_else(|| "팀원".into())).collect::<Vec<_>>());
                    x
                })
                .collect()
        })
        .unwrap_or_default();
    json!({
        "connected": true,
        "teamId": t.id,
        "teamName": t.name(),
        "teams": a.hub.teams(),
        "myUid": t.my_uid(),
        "myProfile": t.my_profile(),
        "members": members,
        "projects": d.arr("projects"),
        "project": project,
        "tasks": tasks,
        "objectionReasons": OBJECTION_REASONS,
    })
}

fn ext_view(a: &Arc<App>) -> Value {
    let t = a.hub.active();
    json!({
        "teamConnected": t.is_connected(),
        "telegram": t.tg_status(),
        "telegramAway": a.stores.settings.read(|s| s.b("telegramAway")),
        "google": a.google.status(),
    })
}
