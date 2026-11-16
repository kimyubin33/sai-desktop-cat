//! 설정의 준비 기록과 승인 장치 연결. 대화 화면의 PC 톡은 mesh_talk의 별도 API를 사용한다.
use crate::app::App;
use sai_core::team::mesh::{
    identity::{journal::MAX_TEXT, Vault},
    runtime::{Config, Mode},
};
use sai_core::util::J;
use serde_json::{json, Value};
use std::sync::Arc;

pub async fn command(app: &Arc<App>, channel: &str, args: &Value) -> Result<Value, String> {
    if channel.starts_with("mesh-journal:") {
        if args.to_string().len() > MAX_TEXT * 6 + 8192 {
            return Err("로컬 톡 자료가 너무 커요.".into());
        }
        let path = app.mesh_identity_path.clone();
        let args = args.clone();
        let channel = channel.to_string();
        let append = channel == "mesh-journal:append";
        let result = tokio::task::spawn_blocking(move || {
            let vault = Vault::new(path);
            match channel.as_str() {
                "mesh-journal:draft-id" => Ok(json!(Vault::local_draft_id()?)),
                "mesh-journal:status" => Ok(json!(vault.local_status(&args.s("teamId"))?)),
                "mesh-journal:append" => {
                    let id = vault.append_local(
                        &args.s("teamId"),
                        &args.s("requestId"),
                        &args.s("text"),
                    )?;
                    Ok(json!({"eventId":id,"status":vault.local_status(&args.s("teamId"))?}))
                }
                _ => Err("알 수 없는 로컬 톡 요청이에요.".into()),
            }
        })
        .await
        .map_err(|_| "로컬 톡을 처리하지 못했어요.".to_string())?;
        if append && result.is_ok() {
            app.mesh_runtime.wake().await;
        }
        return result;
    }
    if channel == "mesh-session:start" {
        if args.b("courierConsent") && !args.b("autoConsent") {
            return Err("중계 참여에는 자동 연결 동의도 필요해요.".into());
        }
        if args.b("autoConsent") && !args.b("exchangeConsent") {
            return Err("자동 연결에는 암호화 기록 교환 동의도 필요해요.".into());
        }
        if args.b("restoreConsent") && !args.b("autoConsent") {
            return Err("자동 복구에는 자동 연결 동의가 필요해요.".into());
        }
        if !args.b("consent") {
            return Err("선택한 LAN에서 승인 장치 연결 확인을 여는 데 동의해 주세요.".into());
        }
        let address = args
            .s("address")
            .parse()
            .map_err(|_| "이 PC의 LAN 주소를 골라 주세요.")?;
        let mode = if args.b("courierConsent") {
            Mode::Courier
        } else if args.b("autoConsent") {
            Mode::Automatic
        } else if args.b("exchangeConsent") {
            Mode::Transfer
        } else {
            Mode::Approved
        };
        return Ok(json!(
            app.mesh_runtime
                .start(Config {
                    team: args.s("teamId"),
                    address,
                    mode,
                    restore: args.b("restoreConsent")
                })
                .await?
        ));
    }
    if args.to_string().len() > 20000 {
        return Err("연결 자료가 너무 커요.".into());
    }
    match channel {
        "mesh-session:status" => return Ok(json!(app.mesh_runtime.status().await)),
        "mesh-session:interfaces" => {
            return Ok(json!(sai_core::team::mesh::probe::interface_options()?))
        }
        "mesh-session:stop" => {
            return Ok(json!(
                app.mesh_runtime.stop(args["session"].as_str()).await?
            ))
        }
        _ => {}
    }
    let session = args.s("session");
    let checker = app.mesh_runtime.checker(&session).await?;
    let report = match channel {
        "mesh-session:exchange" => Some(checker.exchange(&args.s("peer")).await?),
        "mesh-session:add" => {
            checker.add_peer(args.s("card")).await?;
            None
        }
        "mesh-session:check" => {
            checker.check(&args.s("peer")).await?;
            None
        }
        _ => return Err("허용되지 않은 PC 팀 준비 요청이에요.".into()),
    };
    app.mesh_runtime.checker(&session).await?;
    let status = app.mesh_runtime.status().await;
    if let Some(report) = report {
        Ok(json!({"session":status,"report":report}))
    } else {
        Ok(json!(status))
    }
}
