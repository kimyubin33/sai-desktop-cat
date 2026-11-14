//! 실제 Firebase(saigajoa)로 팀 기능이 동작하는지 확인한다. 인터넷이 필요해서 평소 테스트에서는 빠진다.
//!   실행: npm run check:firebase   (= cargo test -p sai-core --test firebase_real -- --ignored --nocapture)
//! 익명 계정 두 개로 팀을 만들고 참여해, 조언 주제·팀 약속·협업·권한 규칙을 확인한다.
use sai_core::team::Backend;
use sai_core::util::J;
use serde_json::{json, Value};

/// SAI_FIREBASE_EMULATOR 가 있으면 로컬 에뮬레이터(저장소의 firestore.rules 를 그대로 올린 것)로 점검한다.
/// 이때는 "규칙이 아직 없어요" 경고로 넘어가지 않고 실패로 본다.
fn emulator() -> bool {
    std::env::var("SAI_FIREBASE_EMULATOR").is_ok()
}

fn cfg() -> Value {
    if emulator() {
        return json!({ "apiKey": "demo-key", "projectId": "demo-sai", "emulator": { "auth": "http://127.0.0.1:9099", "firestore": "http://127.0.0.1:8080" } });
    }
    json!({ "apiKey": "AIzaSyC6MBRsYOCchzBRZT7hX2cvbxUujGG_Eq0", "projectId": "saigajoa" })
}

fn not_deployed(what: &str) {
    assert!(!emulator(), "{what}: 저장소의 규칙으로 통과해야 해요");
    println!("::warning::{what} 규칙이 서버에 아직 없어요. Firebase 콘솔 → Firestore → 규칙에 저장소의 firestore.rules 를 붙여 넣고 게시해 주세요.");
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
    let brief = json!({ "name": "점검.MD", "text": "# 공유 기획서", "by": ua, "updatedAt": 100 });
    match a.put_brief(&team, &proj, brief.clone()).await {
        Ok(()) => {
            assert_eq!(b.get_brief(&team, &proj).await.unwrap().unwrap().s("text"), "# 공유 기획서");
            assert!(b.put_brief(&team, &proj, brief.clone()).await.is_err(), "다른 팀원의 이름으로 덮어쓸 수 없어요");
            let mut too_large = brief.clone();
            too_large["text"] = json!("가".repeat(16_385));
            assert!(a.put_brief(&team, &proj, too_large).await.is_err());
            assert!(a.put_brief(&team, "missing-project", brief.clone()).await.is_err());
            assert!(backend().get_brief(&team, &proj).await.is_err(), "팀 밖에서는 기획서를 읽을 수 없어요");
        }
        Err(e) if e.contains("(403)") => not_deployed("프로젝트 공유 기획서"),
        Err(e) => panic!("{e}"),
    }
    let task = a
        .create_task(&team, &proj, json!({ "title": "점검 작업", "estimateHours": 2, "difficulty": 2, "skills": [], "dependsOn": [], "doneCriteria": "", "prep": [], "reviewer": null, "status": "todo", "assignee": null, "accepted": false, "objections": {} }))
        .await
        .unwrap();
    let before = a.list_tasks(&team, &proj).await.unwrap().into_iter().find(|t| t["id"] == task).unwrap();
    let mut new_task = before.clone();
    new_task.as_object_mut().unwrap().remove("id");
    new_task["title"] = json!("기획서에서 제안한 일");
    new_task["assignee"] = json!(ua);
    a.commit_allocation(&team, &proj, vec![("ai-first".into(), new_task.clone())], vec![(before.clone(), ub.clone())])
        .await.expect("AI 제안의 신규 업무 생성·기존 업무 배정");
    assert!(a.commit_allocation(&team, &proj, vec![("ai-second".into(), new_task.clone())], vec![(before, ua.clone())]).await.is_err());
    assert!(a.commit_allocation(&team, &proj, vec![("ai-first".into(), new_task)], vec![]).await.is_err());
    let tasks = b.list_tasks(&team, &proj).await.unwrap();
    assert_eq!(tasks.len(), 2);
    assert_eq!(tasks.iter().find(|t| t["id"] == task).unwrap()["assignee"], ub);
    println!("✓ AI 업무 배분: 신규·기존 작업 동시 등록, 변경 충돌·중복 등록 방지");
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
            not_deployed("Telegram 연결");
        }
        Err(e) => panic!("{e}"),
    }

    // 팀에 공유한 Claude 키: 공유한 사람만 끌 수 있다 (규칙을 아직 배포하지 않았으면 경고만)
    match a.set_integration(&team, "claude", json!({ "apiKey": "sk-ant-check", "sharedBy": ua })).await {
        Ok(()) => {
            assert_eq!(b.get_integration(&team, "claude").await.unwrap().unwrap().s("apiKey"), "sk-ant-check");
            assert!(b.delete_integration(&team, "claude").await.is_err(), "다른 팀원이 끄면 거절돼야 해요");
            a.delete_integration(&team, "claude").await.unwrap();
            println!("✓ Claude 키 공유: 팀원이 받고, 공유한 사람만 끔");
        }
        Err(e) if e.contains("403") => not_deployed("Claude 키 공유"),
        Err(e) => panic!("{e}"),
    }

    // 팀 자료 (Drive 링크): 올린 사람만 지운다 (규칙을 아직 배포하지 않았으면 경고만)
    match b.add_file(&team, &proj, json!({ "name": "발표.pdf", "url": "https://drive.google.com/file/d/abc/view", "driveId": "abc", "mimeType": "application/pdf", "size": 10, "taskId": task, "access": "edit", "kind": "file", "by": ub, "at": 1 })).await {
        Ok(fid) => {
            assert_eq!(a.list_files(&team, &proj).await.unwrap().len(), 1);
            assert!(a.update_file(&team, &proj, &fid, json!({ "access": "view" })).await.is_err(), "다른 팀원이 권한을 바꾸면 거절돼야 해요");
            b.update_file(&team, &proj, &fid, json!({ "access": "view" })).await.expect("올린 사람은 권한을 바꿈");
            assert!(a.delete_file(&team, &proj, &fid).await.is_err(), "다른 팀원이 지우면 거절돼야 해요");
            assert!(a.add_file(&team, &proj, json!({ "name": "x", "url": "https://drive.google.com/x", "driveId": "", "mimeType": "", "size": 0, "taskId": "", "by": ub, "at": 1 })).await.is_err(), "남의 이름으로 올리면 거절돼야 해요");
            b.delete_file(&team, &proj, &fid).await.unwrap();
            println!("✓ 팀 자료: 올린 사람 본인 이름으로만, 지우기도 본인만");
        }
        Err(e) if e.contains("403") => not_deployed("팀 자료"),
        Err(e) => panic!("{e}"),
    }

    // 팀 Drive 함: 본인 이름으로만 보내고, 보낸 사람만 지운다 (규칙을 아직 배포하지 않았으면 경고만)
    match a.add_share(&team, json!({ "text": "기획안이에요", "name": "기획안.pdf", "url": "https://drive.google.com/file/d/abc/view", "driveId": "abc", "mimeType": "application/pdf", "size": 3, "access": "edit", "by": ua, "at": 2 })).await {
        Ok(sid) => {
            assert_eq!(b.list_shares(&team).await.unwrap().len(), 1);
            let talk = a.add_share(&team, json!({ "text": "회의실에서 만나요", "by": ua, "at": 3 })).await.unwrap();
            let recent = b.recent_shares(&team, None).await.expect("팀원이 톡 알림용 기록을 읽을 수 있어야 해요");
            let ids: Vec<_> = recent.iter().map(|row| row.s("id")).collect();
            assert_eq!(ids, [sid.clone(), talk.clone()]);
            let updates = b.recent_shares(&team, Some(3)).await.unwrap();
            assert_eq!(updates.iter().map(|row| row.s("id")).collect::<Vec<_>>(), [talk.clone()]);
            assert!(b.recent_shares(&team, Some(4)).await.unwrap().is_empty());
            a.delete_share(&team, &talk).await.unwrap();
            assert!(b.delete_share(&team, &sid).await.is_err(), "다른 팀원이 지우면 거절돼야 해요");
            assert!(b.add_share(&team, json!({ "text": "x", "by": ua, "at": 3 })).await.is_err(), "남의 이름으로 보내면 거절돼야 해요");
            a.delete_share(&team, &sid).await.unwrap();
            println!("✓ 팀 Drive 함: 본인 이름으로만 보내고, 지우기도 본인만");
        }
        Err(e) if e.contains("403") => not_deployed("팀 Drive 함"),
        Err(e) => panic!("{e}"),
    }

    // 같은 이름으로는 들어올 수 없다 (공백·대소문자만 달라도)
    let c = backend();
    let uc = c.sign_in().await.unwrap();
    assert!(c.join_team(&t.s("joinCode"), " 점검b ").await.is_err(), "같은 이름이면 거절돼야 해요");
    assert_eq!(a.list_members(&team).await.unwrap().len(), 2, "실패한 참여는 흔적이 없어야 해요");
    c.join_team(&t.s("joinCode"), "점검C").await.expect("다른 이름으로 참여");
    println!("✓ 같은 이름 막기");

    // 관리자: 아이디·비밀번호를 정한 사람과, 그것으로 로그인한 기기만 팀원을 내보낸다 (규칙을 아직 배포하지 않았으면 경고만)
    if a.admin_state(&team).await.b("supported") {
        assert!(b.remove_member(&team, &uc).await.is_err(), "관리자가 아니면 내보낼 수 없어요");
        a.admin_setup(&team, "checkadmin", "check-password-1").await.expect("관리자 정하기");
        assert!(b.admin_setup(&team, "other", "other-password").await.is_err(), "관리자는 한 번만 정해요");
        assert!(b.admin_login(&team, "checkadmin", "wrong-password").await.is_err(), "틀린 비밀번호");
        assert!(!b.admin_state(&team).await.b("me"));
        assert!(b.remove_member(&team, &uc).await.is_err(), "로그인에 실패한 기기는 내보낼 수 없어요");
        b.admin_login(&team, "checkadmin", "check-password-1").await.expect("다른 기기에서 관리자 로그인");
        assert!(b.admin_state(&team).await.b("me"));
        b.remove_member(&team, &uc).await.expect("관리자가 내보내기");
        assert_eq!(a.list_members(&team).await.unwrap().len(), 2);
        assert!(c.list_members(&team).await.is_err(), "내보내진 사람은 팀을 볼 수 없어요");
        // 내보낸 이름은 다시 쓸 수 있다
        c.join_team(&t.s("joinCode"), "점검C").await.expect("내보낸 이름으로 다시 참여");
        b.admin_logout(&team).await.unwrap();
        assert!(b.remove_member(&team, &uc).await.is_err(), "로그아웃하면 권한이 없어요");
        println!("✓ 관리자: 정하기·다른 기기 로그인·내보내기, 관리자가 아니면 거절");
    } else {
        not_deployed("관리자·같은 이름 막기");
    }
    c.leave_team(&team).await.unwrap();

    // 다른 컴퓨터로 옮기기: 같은 로그인 정보로 새 기기에서 같은 계정·팀을 이어 쓴다
    let moved = Backend::firebase(&cfg(), b.session(), Box::new(|_| {})).unwrap();
    assert_eq!(moved.sign_in().await.unwrap(), ub, "옮긴 기기도 같은 계정이어야 해요");
    assert!(moved.list_members(&team).await.unwrap().iter().any(|m| m.s("uid") == ub));
    println!("✓ 다른 컴퓨터로 옮기기: 같은 계정으로 팀을 이어 씀");

    b.leave_team(&team).await.unwrap();
    a.leave_team(&team).await.unwrap();
    // 나간 사람은 더 이상 팀 데이터를 볼 수 없다
    assert!(b.list_members(&team).await.is_err());
    println!("✓ 나가기 후에는 볼 수 없음\n모두 통과했어요.");
}
