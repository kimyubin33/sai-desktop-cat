//! 이전 미리보기에는 서버에서 읽은 자료만 쓰며, 저장 요청은 선택 ID와 미리보기 토큰만 받는다.
use crate::app::App;
use sai_core::util::J;
use serde_json::{json, Value};
use std::sync::Arc;
pub async fn command(app: &Arc<App>, channel: &str, args: &Value) -> Result<Value, String> {
    if args.to_string().len() > 32768 {
        return Err("이전 선택 자료가 너무 커요.".into());
    }
    match channel {
        "mesh-migration:sources" => Ok(json!(app
            .hub
            .teams()
            .iter()
            .map(|t| json!({"teamId":t.s("teamId"),"name":t.s("name")}))
            .collect::<Vec<_>>())),
        "mesh-migration:archive" => {
            let offset = args.get("offset").and_then(Value::as_u64).unwrap_or(0);
            if offset > 1024 {
                return Err("이전 보관 목록 위치를 확인해 주세요.".into());
            }
            Ok(json!(
                app.mesh_migration
                    .page(&args.s("targetTeam"), offset as usize)
                    .await?
            ))
        }
        "mesh-migration:preview" | "mesh-migration:commit" => {
            let source = args.s("sourceTeam");
            let target = args.s("targetTeam");
            if app.hub.team(Some(&source)).is_none() {
                return Err("이 앱에 등록된 기존 팀을 선택해 주세요.".into());
            }
            let backend = app.hub.backend()?;
            if channel == "mesh-migration:preview" {
                return Ok(json!(
                    app.mesh_migration
                        .preview(backend, &source, &target)
                        .await?
                ));
            }
            let selected = args
                .get("selected")
                .and_then(Value::as_array)
                .ok_or("이전할 톡을 선택해 주세요.")?;
            if selected.len() > 200 {
                return Err("한 번에 200개까지 선택해 주세요.".into());
            }
            let ids = selected
                .iter()
                .map(|v| {
                    v.as_str()
                        .map(str::to_owned)
                        .ok_or_else(|| "이전 선택 ID 형식이 올바르지 않아요.".to_string())
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(json!(
                app.mesh_migration
                    .commit(
                        backend,
                        &source,
                        &target,
                        &args.s("token"),
                        ids,
                        args.b("consent")
                    )
                    .await?
            ))
        }
        _ => Err("허용되지 않은 이전 요청이에요.".into()),
    }
}
