//! 개발용: SAI_DEMO=시나리오 이름이면 대화창을 열고 정해진 동작을 차례로 실행한다.
//! 화면이 없는 환경에서 화면이 제대로 그려지는지 확인할 때 쓴다 (스크린샷은 바깥에서 찍는다).
//! 각 단계는 SAI_DEMO_STEP_MS 간격으로 진행되고, 진행 상황을 표준 출력에 남긴다.
use crate::app::{open_chat, App};
use sai_core::util::J;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Manager};

pub fn maybe_run(app: AppHandle, a: Arc<App>) {
    let Ok(name) = std::env::var("SAI_DEMO") else { return };
    let step: u64 = std::env::var("SAI_DEMO_STEP_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(2500);
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_millis(1500)).await;
        let js = |code: &str| {
            if let Some(w) = app.get_webview_window("chat") {
                let _ = w.eval(code);
            }
        };
        let mark = |s: &str| println!("DEMO-STEP {s}");
        match name.as_str() {
            "team" => {
                let r = a.hub.create_team("BION 홍보팀", "유빈").await;
                println!("DEMO create_team {:?}", r.as_ref().map(|v| v.s("joinCode")));
                let t = a.hub.active();
                let _ = t.save_profile(&json!({ "skills": [{ "name": "영상편집", "level": 3 }], "weeklyHours": 12 })).await;
                let pid = t.create_project(&json!({ "title": "학생회 홍보 영상", "deadline": sai_core::time::to_ymd(sai_core::time::now_ms() + 6 * sai_core::time::DAY) })).await.unwrap_or_default();
                let x = t.add_task(&pid, &json!({ "title": "자료조사", "estimateHours": 3 })).await.unwrap_or_default();
                let y = t.add_task(&pid, &json!({ "title": "컷 편집", "estimateHours": 5, "dependsOn": [x.clone()] })).await.unwrap_or_default();
                for id in [&x, &y] {
                    let _ = t.patch_task(&pid, id, json!({ "assignee": t.my_uid(), "accepted": false })).await;
                    let _ = t.accept(id).await;
                }
                open_chat(&app, &a, "collab", json!({}));
                mark("collab");
                tokio::time::sleep(Duration::from_millis(step)).await;
                js("__saiDemo.startSchedule()");
                mark("schedule-chat");
                tokio::time::sleep(Duration::from_millis(step)).await;
                js("__saiDemo.send('2번 금요일 오후 3시로 옮겨줘')");
                mark("schedule-move");
                tokio::time::sleep(Duration::from_millis(step)).await;
                js("__saiDemo.send('내일은 안 돼')");
                mark("schedule-unavail");
                tokio::time::sleep(Duration::from_millis(step)).await;
                js("__saiDemo.showTab('settings')");
                mark("settings");
            }
            "chat" => {
                open_chat(&app, &a, "chat", json!({}));
                tokio::time::sleep(Duration::from_millis(step)).await;
                js("__saiDemo.startMode('complaint')");
                tokio::time::sleep(Duration::from_millis(step / 2)).await;
                js("__saiDemo.send('시간도 안 지키고 맡은 일도 제대로 안 해서 너무 화나요. 진짜 무책임해요')");
                mark("chat-1");
                tokio::time::sleep(Duration::from_millis(step)).await;
                for t in ["어제 같이 검토하기로 했는데 연락이 없었어요", "사정은 모르겠어요. 물어보진 않았어요", "제 다음 작업이 하루 밀렸어요", "미리 알려주기만 했어도 괜찮았을 것 같아요"] {
                    js(&format!("__saiDemo.send({})", serde_json::to_string(t).unwrap()));
                    tokio::time::sleep(Duration::from_millis(step / 2)).await;
                }
                mark("chat-summary");
            }
            "cat" => {
                a.advisor.speak_now(&a);
                mark("bubble");
            }
            _ => {}
        }
    });
}
