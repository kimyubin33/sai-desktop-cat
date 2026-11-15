//! 연결 시험은 익명으로 한다. 사용자가 신뢰한 주소에만 현재 팀 계정의 단기 토큰을 보낸다.
use crate::app::{builtin_firebase, App};
use sai_core::store::team_server::scope;
use sai_core::team::{backend::random_code, desktop, Backend};
use sai_core::util::J;
use serde_json::{json, Value};
use std::sync::Arc;

pub async fn command(app: &Arc<App>, channel: &str, args: &Value) -> Result<Value, String> {
    if app.local_mode { return Err("로컬 시험 모드에서는 팀 서버를 바꿀 수 없어요.".into()); }
    match channel {
        "team:my-uid" => {
            let uid = app.hub.backend()?.sign_in().await?;
            Ok(json!(uid))
        }
        "team:host-check" => {
            let (url, id) = desktop::health(&args.s("url")).await?;
            Ok(json!({"url":url, "serverId":id}))
        }
        "team:host-plan" => {
            let mut slot = app.server_plan.lock().await;
            *slot = None;
            let target = if args["url"].is_null() { Value::Null } else {
                let (url, id) = desktop::health(&args.s("url")).await?;
                if args["serverId"] != id { return Err("서버가 바뀌었어요. 연결 확인을 다시 해 주세요.".into()); }
                json!({"url":url, "serverId":id})
            };
            let plan = make_plan(app, &target).await?;
            let summary = json!({"id":plan["id"], "teams":plan.arr("teams").iter().map(|t| t.s("name")).collect::<Vec<_>>(), "skipped":plan["skipped"]});
            *slot = Some(plan);
            Ok(summary)
        }
        "team:host-apply" => {
            let mut slot = app.server_plan.lock().await;
            if slot.as_ref().is_none_or(|p| p["id"] != args["id"]) {
                return Err("서버 연결을 다시 확인해 주세요.".into());
            }
            let plan = slot.take().unwrap();
            let current = app.stores.settings.get();
            let uid = app.hub.firebase_session().map(|s| s.s("uid"));
            if plan["sourceTeams"] != current["teams"] || plan["from"] != current["teamServer"] || plan["uid"] != json!(uid) {
                return Err("팀이나 계정이 변경됐어요. 서버 연결을 다시 확인해 주세요.".into());
            }
            app.stores.settings.update(|s| s["pendingTeamServer"] = plan);
            if let Err(e) = app.stores.settings.try_flush() {
                app.stores.settings.update(|s| s["pendingTeamServer"] = Value::Null);
                return Err(format!("서버 주소를 저장하지 못했어요: {e}"));
            }
            app.stores.flush_all();
            app.handle.request_restart();
            Ok(Value::Null)
        }
        _ => Err("알 수 없는 팀 서버 요청이에요.".into())
    }
}

async fn make_plan(app: &App, target: &Value) -> Result<Value, String> {
    let snapshot = app.stores.settings.get();
    let from = &snapshot["teamServer"];
    if from == target { return Err("이미 사용 중인 서버 주소예요.".into()); }
    let current = snapshot.arr("teams");
    let previous = &snapshot["teamServerProfiles"][scope(target)];
    let mut skipped = Vec::new();
    let (teams, preferred) = if scope(from) == scope(target) {
        (current.clone(), snapshot.s("activeTeamId"))
    } else if previous.is_object() {
        // 이전에 쓰던 서버로 돌아갈 때는 해당 서버에 따로 보관한 목록·대기함을 연다.
        (previous.arr("teams"), previous.s("activeTeamId"))
    } else if current.is_empty() {
        (Vec::new(), String::new())
    } else {
        let session = app.hub.firebase_session().ok_or("기존 팀 계정을 먼저 Google 계정에서 복원해 주세요.")?;
        let uid = session.s("uid");
        let mut config = builtin_firebase();
        if !target.is_null() {
            config["desktopServer"] = target["url"].clone();
            config["desktopServerId"] = target["serverId"].clone();
        }
        let backend = Backend::firebase(&config, Some(session), Box::new(|_| {}))?;
        if backend.sign_in().await? != uid { return Err("기존 팀 계정을 확인할 수 없어요.".into()); }
        let mut teams = Vec::new();
        for team in &current {
            if backend.carries_team(&team.s("teamId"), &team.s("joinCode"), &uid).await? {
                teams.push(team.clone());
            } else {
                skipped.push(team.s("name"));
            }
        }
        if teams.is_empty() {
            return Err("내가 참여 중인 팀을 이 서버에서 찾을 수 없어요. 사용할 방을 먼저 서버로 이전한 뒤 다시 시도해 주세요.".into());
        }
        (teams, snapshot.s("activeTeamId"))
    };
    let active = teams.iter().find(|t| t["teamId"] == preferred).or_else(|| teams.first()).map(|t| t.s("teamId"));
    let uid = app.hub.firebase_session().map(|s| s.s("uid"));
    Ok(json!({"id":random_code(20), "from":from, "to":target, "uid":uid, "sourceTeams":current, "teams":teams, "activeTeamId":active, "skipped":skipped}))
}
