//! 실제 Firebase(saigajoa)로 팀 기능이 동작하는지 확인한다. 인터넷이 필요해서 평소 테스트에서는 빠진다.
//!   실행: npm run check:firebase   (= cargo test -p sai-core --test firebase_real -- --ignored --nocapture)
//! 익명 계정 두 개로 팀을 만들고 참여해, 조언 주제·팀 약속·협업·권한 규칙을 확인한다.
use sai_core::team::Backend;
use sai_core::util::J;
use serde_json::{json, Value};

fn cfg() -> Value {
    json!({ "apiKey": "AIzaSyC6MBRsYOCchzBRZT7hX2cvbxUujGG_Eq0", "projectId": "saigajoa" })
}

fn backend() -> Backend {
    Backend::firebase(&cfg(), None, Box::new(|_| {})).unwrap()
}

#[tokio::test]
#[ignore]
async fn firebase_contract() {
    let a = backend();
    let b = backend();
    let ua = a.sign_in().await.expect("A 익명 로그인");
    let ub = b.sign_in().await.expect("B 익명 로그인");
    println!("✓ 익명 로그인 두 명");

    let t = a.create_team("사이 점검 팀", "점검A").await.expect("팀 만들기");
    let team = t.s("teamId");
    b.join_team(&t.s("joinCode"), "점검B").await.expect("참여 코드로 참여");
    assert_eq!(a.list_members(&team).await.unwrap().len(), 2);
    println!("✓ 팀 만들기·참여");

    let sid = a.create_signal(&team, "time_promise", &ub, "2026-W46").await.expect("조언 주제 보내기");
    let inbox = b.list_signals_for_me(&team).await.expect("받은 주제 보기");
    assert!(inbox.iter().any(|s| s.s("id") == sid));
    assert!(a.list_signals_for_me(&team).await.unwrap().iter().all(|s| s.s("id") != sid), "보낸 사람 목록에는 없어야 해요");
    b.hold_signal(&team, &sid).await.expect("보류");
    assert!(a.get_signal(&team, &sid).await.unwrap().unwrap().b("held"));
    println!("✓ 조언 주제: 받는 사람만 보고, 보류 상태가 보낸 쪽에 보임");

    let pid = a.create_proposal(&team, "workload", "2026-W46").await.unwrap();
    a.respond_proposal(&team, &pid, "agree").await.unwrap();
    b.respond_proposal(&team, &pid, "agree").await.unwrap();
    let p = a.list_proposals(&team).await.unwrap().into_iter().find(|p| p.s("id") == pid).unwrap();
    assert_eq!(p["responses"].as_object().unwrap().len(), 2);
    println!("✓ 팀 약속 제안·동의");

    a.set_profile(&team, json!({ "pr": "", "skills": { "영상편집": 3 }, "learn": [], "weeklyHours": 10.5, "days": [] })).await.expect("자기소개");
    assert_eq!(b.list_profiles(&team).await.unwrap().len(), 1);
    let proj = a.create_project(&team, json!({ "title": "점검 프로젝트", "goal": "", "deliverable": "", "deadline": "2099-12-01" })).await.unwrap();
    let task = a
        .create_task(&team, &proj, json!({ "title": "점검 작업", "estimateHours": 2, "difficulty": 2, "skills": [], "dependsOn": [], "doneCriteria": "", "prep": [], "reviewer": null, "status": "todo", "assignee": null, "accepted": false, "objections": {} }))
        .await
        .unwrap();
    a.update_task(&team, &proj, &task, json!({ "assignee": ub, "accepted": false })).await.unwrap();
    // 다른 사람이 대신 수락할 수 없다
    assert!(a.update_task(&team, &proj, &task, json!({ "accepted": true })).await.is_err(), "다른 사람이 수락하면 거절돼야 해요");
    b.update_task(&team, &proj, &task, json!({ "accepted": true })).await.expect("본인 수락");
    println!("✓ 협업: 자기소개·프로젝트·작업, 수락은 본인만");

    // Telegram 규칙은 5단계에서 추가됐다. 서버에 아직 배포하지 않았으면 경고만 남긴다.
    match b.set_telegram_link(&team, json!({ "chatId": 42, "teamAlerts": true })).await {
        Ok(()) => {
            assert_eq!(a.list_telegram_links(&team).await.unwrap().len(), 1);
            b.remove_telegram_link(&team).await.unwrap();
            println!("✓ Telegram 연결 정보");
        }
        Err(e) if e.contains("403") => {
            println!("::warning::Telegram 연결 규칙이 서버에 아직 없어요. Firebase 콘솔 → Firestore → 규칙에 저장소의 firestore.rules 를 붙여 넣고 게시해 주세요.");
        }
        Err(e) => panic!("{e}"),
    }

    b.leave_team(&team).await.unwrap();
    a.leave_team(&team).await.unwrap();
    // 나간 사람은 더 이상 팀 데이터를 볼 수 없다
    assert!(b.list_members(&team).await.is_err());
    println!("✓ 나가기 후에는 볼 수 없음\n모두 통과했어요.");
    let _ = ua;
}
