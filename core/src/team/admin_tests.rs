//! 같은 이름 막기 · 관리자 (아이디·비밀번호) · 내보내기
use crate::store::Stores;
use crate::team::{Backend, Hub};
use crate::util::J;
use std::sync::Arc;

fn hub(shared: Arc<std::sync::Mutex<serde_json::Value>>, uid: &str) -> Hub {
    let st = Stores::memory();
    let u = uid.to_string();
    Hub::new(st.settings, st.shared_outbox, st.team_cache, Box::new(move || Some(Backend::memory_shared(shared.clone(), &u))), Some(0), Arc::new(crate::time::now_ms))
}

#[tokio::test]
async fn same_name_is_blocked() {
    let shared = Backend::new_shared_memory();
    let a = hub(shared.clone(), "ua");
    let b = hub(shared.clone(), "ub");
    let c = hub(shared.clone(), "uc");
    let t = a.create_team("팀", "유빈").await.unwrap();
    let code = t["joinCode"].as_str().unwrap().to_string();
    // 같은 이름(공백·대소문자만 다른 것 포함)은 못 들어온다
    let e = b.join_team(&code, " 유빈 ").await.unwrap_err();
    assert!(e.contains("이미"), "{e}");
    assert!(b.teams().is_empty());
    b.join_team(&code, "병헌").await.unwrap();
    assert!(c.join_team(&code, "병헌").await.is_err());
    c.join_team(&code, "민수").await.unwrap();
    let ta = a.active();
    a.sync_team(&ta).await;
    assert_eq!(ta.members(true).len(), 3);
    // 나가면 그 이름을 다시 쓸 수 있다
    let tb = b.active();
    b.leave_team(&tb.team_id().unwrap()).await.unwrap();
    let d = hub(shared.clone(), "ud");
    d.join_team(&code, "병헌").await.unwrap();
}

#[tokio::test]
async fn old_member_without_reserved_name_still_blocks() {
    let shared = Backend::new_shared_memory();
    let a = hub(shared.clone(), "ua");
    let t = a.create_team("팀", "유빈").await.unwrap();
    let code = t["joinCode"].as_str().unwrap().to_string();
    let team_id = t["teamId"].as_str().unwrap().to_string();
    // 예전 버전으로 들어온 팀원: 이름을 잡아 두지 않았다
    shared.lock().unwrap()["docs"][format!("teams/{team_id}/members/old")] = serde_json::json!({ "displayName": "병헌", "joinCode": code });
    let b = hub(shared.clone(), "ub");
    assert!(b.join_team(&code, "병헌").await.is_err());
    // 실패한 참여는 흔적을 남기지 않는다
    let ta = a.active();
    a.sync_team(&ta).await;
    assert_eq!(ta.members(true).len(), 2);
    b.join_team(&code, "병헌2").await.unwrap();
}

#[tokio::test]
async fn admin_sets_password_logs_in_elsewhere_and_removes_member() {
    let shared = Backend::new_shared_memory();
    let a = hub(shared.clone(), "ua");
    let b = hub(shared.clone(), "ub");
    let t = a.create_team("팀", "유빈").await.unwrap();
    let code = t["joinCode"].as_str().unwrap().to_string();
    b.join_team(&code, "병헌").await.unwrap();
    let (ta, tb) = (a.active(), b.active());
    // 정하기 전에는 아무도 내보낼 수 없다
    assert!(tb.remove_member("ua").await.is_err());
    assert!(ta.admin_setup("ad", "123456").await.is_err(), "너무 짧은 아이디");
    assert!(ta.admin_setup("admin", "123").await.is_err(), "너무 짧은 비밀번호");
    ta.admin_setup("admin", "correct horse").await.unwrap();
    assert!(ta.is_admin());
    // 두 번 정할 수 없다
    assert!(tb.admin_setup("other", "abcdefgh").await.is_err());
    // 팀원은 관리자가 아니다
    b.sync_team(&tb).await;
    assert!(!tb.is_admin());
    assert_eq!(tb.status()["admin"].s("adminId"), "admin");
    assert!(tb.remove_member("ua").await.is_err());
    // 틀린 비밀번호·아이디
    assert!(tb.admin_login("admin", "wrong pass").await.is_err());
    assert!(tb.admin_login("nimda", "correct horse").await.is_err());
    assert!(!tb.is_admin());
    // 다른 컴퓨터(같은 사람의 새 기기)에서 아이디·비밀번호로 관리자가 된다
    let c = hub(shared.clone(), "uc");
    c.join_team(&code, "유빈 노트북").await.unwrap();
    let tc = c.active();
    tc.admin_login("admin", "correct horse").await.unwrap();
    assert!(tc.is_admin());
    // 내보내기: 그 이름도 다시 쓸 수 있게 풀린다
    tc.remove_member("ub").await.unwrap();
    assert_eq!(tc.members(true).len(), 2);
    assert!(tc.remove_member("uc").await.is_err(), "나 자신은 내보낼 수 없다");
    let r = b.sync_team(&tb).await;
    assert!(!r.b("ok"));
    let d = hub(shared.clone(), "ud");
    d.join_team(&code, "병헌").await.unwrap();
    // 로그아웃하면 권한이 없다
    tc.admin_logout().await.unwrap();
    assert!(!tc.is_admin());
    assert!(tc.remove_member("ud").await.is_err());
}
