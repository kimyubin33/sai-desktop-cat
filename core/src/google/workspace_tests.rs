//! Google facade workflow against isolated PCs and a credential-free Google/Claude mock.
use super::*;
use crate::google::Google;
struct Pc {
    google: Google,
    settings: Store,
}
impl Pc {
    async fn boot(root: &std::path::Path, server: &str, login: bool) -> Self {
        let settings = Store::open(
            root.join("settings.json"),
            json!({"catName":"Local cat","frequency":"low","ai":{"enabled":false},"retentionDays":7}),
        );
        let hub = crate::team::Hub::new(
            Store::memory(json!({"teams":[]})),
            Store::memory(json!({"items":[]})),
            Store::memory(json!({"teams":{}})),
            Box::new(|| panic!("the retired team backend must never be used")),
            None,
            Arc::new(crate::time::now_ms),
        );
        let google = Google::new(
            settings.clone(),
            Store::memory(json!({})),
            hub,
            Some(json!({"clientId":"test-client","clientSecret":"test-secret"})),
            Arc::new(|_| panic!("a restored session must not open a browser")),
            root.join("google"),
            Arc::new(|| panic!("a restored session must not request foreground")),
        );
        let auth = Arc::new(auth(settings.clone(), server));
        *google.0.auth.lock().unwrap() = Some(auth.clone());
        if login {
            auth.exchange_code("base", "v", "http://127.0.0.1/", "attempt")
                .await
                .unwrap();
        }
        Self { google, settings }
    }
    fn epoch(&self) -> u64 {
        self.google.profile_status()["epoch"].as_u64().unwrap()
    }
    async fn calendar(&self) {
        let auth = self.google.auth().unwrap();
        let lease = auth.lease().await.unwrap();
        auth.finish_calendar(&lease, "calendar", "v", "http://127.0.0.1/", "attempt")
            .await
            .unwrap();
        self.google.connect_calendar(self.epoch()).await.unwrap();
    }
}
async fn backend() -> (Temp, Arc<Api>, Server) {
    let root = Temp::new();
    let api = Arc::new(Api::default());
    let copy = api.clone();
    let server = server_headers(move |m, p, b, h| copy.respond_headers(m, p, b, h)).await;
    (root, api, server)
}
async fn join(pc: &Pc) {
    pc.google.resume().await.unwrap();
    pc.google.sync_profile().await.unwrap();
    pc.google
        .connect_team_folder(pc.epoch(), "team-folder")
        .await
        .unwrap();
    pc.google.sync_profile().await.unwrap();
}
const TEXT: &str = "Review the brief. Share notes by 2026-11-20.";
#[tokio::test]
async fn restored_google_account_links_files_ai_tasks_and_calendar_across_two_pc_stores() {
    let (root, api, server) = backend().await;
    let a = Pc::boot(&root.0.join("pc-a"), &server.url, true).await;
    a.google.sync_profile().await.unwrap();
    assert_eq!(api.posts.load(Ordering::SeqCst), 0);
    join(&a).await;
    a.google
        .update_profile(
            a.epoch(),
            json!({"displayName":"Team member","catName":"Ink cat","frequency":"high"}),
        )
        .await
        .unwrap();
    a.google.sync_profile().await.unwrap();
    let upload = a
        .google
        .upload_team_file(
            a.epoch(),
            "team-folder",
            "brief.txt",
            TEXT.as_bytes(),
            silent(),
        )
        .await
        .unwrap();
    let fid = upload["file"]["id"].as_str().unwrap();
    let p = a
        .google
        .prepare_ai_review(
            a.epoch(),
            "team-folder",
            json!({"kind":"drive","fileId":fid}),
        )
        .await
        .unwrap();
    assert_eq!(p["text"], TEXT);
    assert_eq!(api.ai_calls.load(Ordering::SeqCst), 0);
    let auth = a.google.auth().unwrap();
    let lease = auth.lease().await.unwrap();
    auth.set_team_ai_consent(&lease, &server.url, true)
        .await
        .unwrap();
    let review = p["reviewId"].as_str().unwrap();
    a.google
        .run_ai_review(a.epoch(), "team-folder", review, true, Some(&server.url))
        .await
        .unwrap();
    let saved=a.google.commit_ai_review(a.epoch(),"team-folder",review,json!([{"index":0,"title":"Reviewed task","assignee":"Member","dueDate":"2026-11-20","projectId":""}]),true).await.unwrap();
    let task = saved["state"]["items"][0]["id"].as_str().unwrap();
    assert_eq!(saved["state"]["pending"], 0);
    a.calendar().await;
    let before = a
        .google
        .calendar_preview(a.epoch(), "team-folder", task)
        .await
        .unwrap();
    a.google
        .add_to_calendar(
            a.epoch(),
            "team-folder",
            json!({"taskId":task,"heads":before["heads"],"date":"2026-11-20","confirmed":true}),
        )
        .await
        .unwrap();
    let old = a
        .google
        .calendar_preview(a.epoch(), "team-folder", task)
        .await
        .unwrap();
    let posts = api.posts.load(Ordering::SeqCst);
    let b = Pc::boot(&root.0.join("pc-b"), &server.url, true).await;
    b.google.login().await.unwrap();
    b.google.resume().await.unwrap();
    b.google.sync_profile().await.unwrap();
    assert_eq!(api.posts.load(Ordering::SeqCst), posts);
    assert_eq!(
        b.google.profile_status()["profile"]["displayName"],
        "Team member"
    );
    assert_eq!(b.settings.get()["catName"], "Ink cat");
    assert_eq!(
        b.google.team_status().await.unwrap()["teams"][0]["folderId"],
        "team-folder"
    );
    let files = b
        .google
        .list_team_files(b.epoch(), "team-folder", "")
        .await
        .unwrap();
    assert_eq!(files["files"].as_array().unwrap().len(), 1);
    let downloaded = b
        .google
        .download_team_file(b.epoch(), "team-folder", fid, silent())
        .await
        .unwrap();
    assert_eq!(downloaded.bytes, TEXT.as_bytes());
    let work = b
        .google
        .collaboration(b.epoch(), "team-folder", false)
        .await
        .unwrap();
    assert_eq!(
        work["items"][0]["variants"][0]["entity"]["title"],
        "Reviewed task"
    );
    b.calendar().await;
    let current = b
        .google
        .calendar_preview(b.epoch(), "team-folder", task)
        .await
        .unwrap();
    assert_eq!(current["calendar"]["state"], "existing");
    assert_eq!(b.google.change_calendar(b.epoch(),"team-folder",json!({"reviewId":current["reviewId"],"action":{"kind":"update","date":"2026-11-21"},"confirmed":true})).await.unwrap()["state"],"updated");
    assert_eq!(
        a.google
            .change_calendar(
                a.epoch(),
                "team-folder",
                json!({"reviewId":old["reviewId"],"action":{"kind":"delete"},"confirmed":true})
            )
            .await
            .unwrap()["state"],
        "conflict"
    );
    assert_eq!(api.calendar_events.lock().unwrap().len(), 1);
    assert_eq!(api.ai_calls.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn cold_restart_reuses_protected_login_and_restores_teams_without_publishing_defaults() {
    let (root, api, server) = backend().await;
    let path = root.0.join("pc-a");
    let a = Pc::boot(&path, &server.url, true).await;
    join(&a).await;
    a.google
        .update_profile(a.epoch(), json!({"displayName":"Restored member"}))
        .await
        .unwrap();
    a.google.sync_profile().await.unwrap();
    let before = api.posts.load(Ordering::SeqCst);
    drop(a);
    let restored = Pc::boot(&path, &server.url, false).await;
    assert_eq!(restored.google.status()["signedIn"], false);
    assert_eq!(restored.google.status()["hasSession"], true);
    restored.google.resume().await.unwrap();
    restored.google.sync_profile().await.unwrap();
    restored.google.login().await.unwrap();
    assert_eq!(restored.google.status()["signedIn"], true);
    assert_eq!(
        restored.google.profile_status()["profile"]["displayName"],
        "Restored member"
    );
    assert_eq!(
        restored.google.team_status().await.unwrap()["teams"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(api.posts.load(Ordering::SeqCst), before);
}
#[tokio::test]
async fn a_failed_new_pc_restore_cannot_publish_empty_settings_or_enable_team_operations() {
    let (root, api, server) = backend().await;
    let a = Pc::boot(&root.0.join("pc-a"), &server.url, true).await;
    join(&a).await;
    a.google
        .update_profile(a.epoch(), json!({"displayName":"Remote member"}))
        .await
        .unwrap();
    a.google.sync_profile().await.unwrap();
    let before = api.posts.load(Ordering::SeqCst);
    api.profile_list_fail.store(true, Ordering::SeqCst);
    let b = Pc::boot(&root.0.join("pc-b"), &server.url, true).await;
    assert!(b.google.sync_profile().await.is_err());
    assert_eq!(b.google.profile_status()["ready"], false);
    assert!(b
        .google
        .update_profile(b.epoch(), json!({"displayName":""}))
        .await
        .is_err());
    let denied = b
        .google
        .list_team_files(b.epoch(), "team-folder", "")
        .await
        .unwrap();
    assert_eq!(denied["files"], json!([]));
    assert!(!denied["error"].as_str().unwrap().is_empty());
    assert!(b
        .google
        .check_file_upload(b.epoch(), "team-folder")
        .await
        .is_err());
    assert_eq!(api.posts.load(Ordering::SeqCst), before);
    api.profile_list_fail.store(false, Ordering::SeqCst);
    b.google.sync_profile().await.unwrap();
    assert_eq!(
        b.google.profile_status()["profile"]["displayName"],
        "Remote member"
    );
    assert_eq!(
        b.google.team_status().await.unwrap()["teams"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(api.posts.load(Ordering::SeqCst), before);
}
#[tokio::test]
async fn signing_out_or_changing_google_account_hides_old_teams_without_removing_drive_files() {
    let (root, api, server) = backend().await;
    let a = Pc::boot(&root.0.join("pc-a"), &server.url, true).await;
    join(&a).await;
    a.google
        .upload_team_file(
            a.epoch(),
            "team-folder",
            "brief.txt",
            TEXT.as_bytes(),
            silent(),
        )
        .await
        .unwrap();
    let files = api.files.lock().unwrap().len();
    let posts = api.posts.load(Ordering::SeqCst);
    let old = a.epoch();
    a.google.logout().await.unwrap();
    assert_eq!(a.google.status()["hasCredentials"], false);
    assert_eq!(a.google.profile_status()["signedIn"], false);
    assert!(a
        .google
        .list_team_files(old, "team-folder", "")
        .await
        .is_err());
    assert_eq!(api.files.lock().unwrap().len(), files);
    api.second_account.store(true, Ordering::SeqCst);
    let b = Pc::boot(&root.0.join("pc-b"), &server.url, true).await;
    b.google.sync_profile().await.unwrap();
    assert_eq!(b.google.team_status().await.unwrap()["teams"], json!([]));
    let denied = b
        .google
        .list_team_files(b.epoch(), "team-folder", "")
        .await
        .unwrap();
    assert_eq!(denied["files"], json!([]));
    assert!(!denied["error"].as_str().unwrap().is_empty());
    assert_eq!(denied["pending"], json!([]));
    assert!(b
        .google
        .check_file_upload(b.epoch(), "team-folder")
        .await
        .is_err());
    assert_eq!(api.posts.load(Ordering::SeqCst), posts);
    assert_eq!(api.files.lock().unwrap().len(), files);
}
