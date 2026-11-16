//! 로컬 신원 준비 명령. 네트워크·기존 팀 서버와 연결하지 않는다.
use crate::app::App;
use sai_core::team::mesh::identity::{model::MAX_PACKET, Vault};
use sai_core::util::J;
use serde_json::{json, Value};
use std::sync::Arc;

pub async fn command(app: &Arc<App>, channel: &str, args: &Value) -> Result<Value, String> {
    if args.to_string().len() > MAX_PACKET + 8192 {
        return Err("장치 등록 자료가 너무 커요.".into());
    }
    let channel = channel.to_string();
    let args = args.clone();
    let path = app.mesh_identity_path.clone();
    tokio::task::spawn_blocking(move || {
        let vault = Vault::new(path);
        let mut output = None;
        let mut selected_team = args.s("teamId");
        match channel.as_str() {
            "mesh-identity:status" => {}
            "mesh-identity:create-device" => {
                vault.create_device()?;
            }
            "mesh-identity:create-team" => {
                output = Some(
                    serde_json::to_string(&vault.create_team(&args.s("name"))?)
                        .map_err(|_| "승인 정보를 만들지 못했어요.")?,
                );
            }
            "mesh-identity:request" => {
                output = Some(
                    serde_json::to_string(&vault.request(&args.s("teamId"))?)
                        .map_err(|_| "등록 요청을 만들지 못했어요.")?,
                );
            }
            "mesh-identity:export" => {
                output = Some(
                    serde_json::to_string(&vault.export(&args.s("teamId"))?)
                        .map_err(|_| "승인 정보를 만들지 못했어요.")?,
                );
            }
            "mesh-identity:import" => {
                selected_team = vault.import(&args.s("input"), &args.s("fingerprint"))?;
            }
            "mesh-identity:approve" => {
                output = Some(
                    serde_json::to_string(&vault.approve(&args.s("teamId"), &args.s("input"))?)
                        .map_err(|_| "승인 정보를 만들지 못했어요.")?,
                );
            }
            "mesh-identity:revoke" => {
                output = Some(
                    serde_json::to_string(&vault.revoke(&args.s("teamId"), &args.s("deviceId"))?)
                        .map_err(|_| "승인 정보를 만들지 못했어요.")?,
                );
            }
            _ => return Err("알 수 없는 장치 신원 요청이에요.".into()),
        }
        Ok(json!({"status":vault.status()?, "output":output, "selectedTeam":selected_team}))
    })
    .await
    .map_err(|_| "장치 신원 요청을 처리하지 못했어요.".to_string())?
}
