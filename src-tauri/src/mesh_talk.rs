//! 실제 PC 톡 화면은 승인 저장소만 사용한다. 기존 서버·Google 계정과 연결하지 않는다.
use crate::app::App;
use sai_core::{
    team::mesh::identity::{talk::TalkScope, Vault},
    util::J,
};
use serde_json::{json, Value};
use std::sync::Arc;

pub async fn command(app: &Arc<App>, channel: &str, args: &Value) -> Result<Value, String> {
    if args.to_string().len() > 16384 {
        return Err("PC 톡 요청이 너무 커요.".into());
    }
    let path = app.mesh_identity_path.clone();
    let channel = channel.to_owned();
    let args = args.clone();
    let write = matches!(channel.as_str(), "mesh-talk:post" | "mesh-talk:delete");
    let feed = channel == "mesh-talk:feed";
    let mut result = tokio::task::spawn_blocking(move || {
        let vault = Vault::new(path);
        match channel.as_str() {
            "mesh-talk:teams" => {
                let status = vault.status()?;
                Ok(json!(status
                    .teams
                    .iter()
                    .map(|t| json!({"teamId":t.team_id,"name":t.name}))
                    .collect::<Vec<_>>()))
            }
            "mesh-talk:draft-id" => Ok(json!(Vault::local_draft_id()?)),
            "mesh-talk:feed" => {
                let offset = args["offset"].as_u64().unwrap_or(0);
                if offset > 2048 {
                    return Err("PC 톡 목록 위치를 확인해 주세요.".into());
                }
                Ok(json!(vault.talk_feed(&args.s("teamId"), offset as usize)?))
            }
            "mesh-talk:post" | "mesh-talk:delete" => {
                if args["consent"] != true {
                    return Err("표시한 승인 범위를 확인하고 PC 톡 사용을 선택해 주세요.".into());
                }
                let scope: TalkScope = serde_json::from_value(args["scope"].clone())
                    .map_err(|_| "PC 톡의 승인 범위를 새로 확인해 주세요.")?;
                let id = if channel == "mesh-talk:post" {
                    vault.post_talk(&scope, &args.s("requestId"), &args.s("text"))?
                } else {
                    vault.delete_talk(
                        &scope,
                        &args.s("requestId"),
                        &args.s("eventId"),
                        args["sequence"]
                            .as_u64()
                            .ok_or("삭제할 원본 순번을 확인해 주세요.")?,
                    )?
                };
                Ok(json!({"eventId":id}))
            }
            _ => Err("허용되지 않은 PC 톡 요청이에요.".into()),
        }
    })
    .await
    .map_err(|_| "PC 톡을 처리하지 못했어요.")?;
    if write && result.is_ok() {
        app.mesh_runtime.wake().await;
    }
    if feed {
        if let Ok(value) = &mut result {
            let network = app.mesh_runtime.status().await;
            value["connection"] = json!({"running":network.session.running,"teamId":network.session.team_id,"exchangeEnabled":network.session.exchange_enabled,"peers":network.session.peers.len(),"notice":network.session.notice,"lifecycle":network.lifecycle});
        }
    }
    result
}
