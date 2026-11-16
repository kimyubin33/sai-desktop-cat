//! 명시적으로 시작한 연결 진단만 제공한다. 앱 시작 시 LAN 포트를 자동으로 열지 않는다.
use sai_core::team::mesh::probe::{self, Probe, Status};
use sai_core::util::J;
use serde_json::{json, Value};

pub type ProbeState = tokio::sync::Mutex<Option<Probe>>;

fn session_matches(probe: &Probe, args: &Value) -> Result<(), String> {
    if probe.status().session.as_deref() != args["session"].as_str() {
        return Err("연결 실험이 바뀌었어요. 화면의 새 연결 정보를 확인해 주세요.".into());
    }
    Ok(())
}

pub async fn command(state: &ProbeState, channel: &str, args: &Value) -> Result<Value, String> {
    match channel {
        "mesh-probe:interfaces" => Ok(json!(probe::interface_options()?)),
        "mesh-probe:status" => {
            let mut slot = state.lock().await;
            let status = slot.as_ref().map(Probe::status).unwrap_or_default();
            if !status.running {
                slot.take();
            }
            Ok(json!(status))
        }
        "mesh-probe:start" => {
            if !args.b("consent") {
                return Err("선택한 LAN에서 연결 실험을 여는 데 동의해 주세요.".into());
            }
            let address = args
                .s("address")
                .parse()
                .map_err(|_| "이 PC의 LAN 주소를 골라 주세요.")?;
            let mut slot = state.lock().await;
            if slot.as_ref().is_some_and(|p| p.status().running) {
                return Err("이미 연결 실험이 실행 중이에요.".into());
            }
            slot.take();
            *slot = Some(Probe::start(address).await?);
            Ok(json!(slot.as_ref().unwrap().status()))
        }
        "mesh-probe:add" => {
            let checker = {
                let slot = state.lock().await;
                let probe = slot.as_ref().ok_or("먼저 연결 실험을 시작해 주세요.")?;
                session_matches(probe, args)?;
                probe.checker()
            };
            checker.add_peer(args.s("card")).await?;
            let slot = state.lock().await;
            let probe = slot.as_ref().ok_or("연결 실험이 중지됐어요.")?;
            session_matches(probe, args)?;
            Ok(json!(probe.status()))
        }
        "mesh-probe:check" => {
            let checker = {
                let slot = state.lock().await;
                let probe = slot.as_ref().ok_or("먼저 연결 실험을 시작해 주세요.")?;
                session_matches(probe, args)?;
                probe.checker()
            };
            checker.check(&args.s("peer")).await?;
            let slot = state.lock().await;
            Ok(json!(slot.as_ref().map(Probe::status).unwrap_or_default()))
        }
        "mesh-probe:stop" => {
            let old = {
                let mut slot = state.lock().await;
                if let Some(probe) = slot.as_ref() {
                    if probe.status().running {
                        session_matches(probe, args)?;
                    }
                }
                slot.take()
            };
            if let Some(probe) = old {
                probe.shutdown().await;
            }
            Ok(json!(Status::default()))
        }
        _ => Err("알 수 없는 연결 실험 요청이에요.".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_diagnostic_starts_inactive_and_refuses_implicit_listening() {
        let state = ProbeState::default();
        let before = command(&state, "mesh-probe:status", &Value::Null)
            .await
            .unwrap();
        assert_eq!(before["running"], false);
        assert!(before["card"].is_null());
        assert!(
            command(&state, "mesh-probe:start", &json!({"address":"127.0.0.1"}))
                .await
                .is_err()
        );
        assert!(command(
            &state,
            "mesh-probe:start",
            &json!({"address":"not-an-address","consent":true})
        )
        .await
        .is_err());
        assert!(command(&state, "mesh-probe:add", &json!({"card":"{}"}))
            .await
            .is_err());
        assert!(
            command(&state, "mesh-probe:check", &json!({"peer":"unknown"}))
                .await
                .is_err()
        );
        assert!(state.lock().await.is_none());
    }

    #[tokio::test]
    async fn stopping_an_inactive_diagnostic_is_safe_and_other_commands_are_rejected() {
        let state = ProbeState::default();
        for _ in 0..2 {
            assert_eq!(
                command(&state, "mesh-probe:stop", &Value::Null)
                    .await
                    .unwrap()["running"],
                false
            );
        }
        assert!(command(&state, "settings:get", &Value::Null).await.is_err());
        assert!(
            command(&state, "mesh-probe:exec", &json!({"command":"anything"}))
                .await
                .is_err()
        );
        assert!(state.lock().await.is_none());
    }
}
