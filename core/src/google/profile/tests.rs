use super::*;
use crate::google::oauth::tests::{auth, server, token};
use model::{Change, Record, TeamRef};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "sai-profile-test-{}",
            crate::google::oauth::random_token()
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        assert!(self.0.parent() == Some(std::env::temp_dir().as_path()));
        assert!(self
            .0
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("sai-profile-test-"));
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
#[derive(Default)]
struct Drive {
    files: Mutex<BTreeMap<String, Value>>,
    posts: AtomicUsize,
    ids: AtomicUsize,
    fail_list: AtomicBool,
    fail_after_commit: AtomicBool,
}
impl Drive {
    fn handle(&self, req: &str) -> (u16, Value) {
        let path = req
            .lines()
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap();
        if path == "/token" {
            return (200, token("attempt"));
        }
        if path == "/userinfo" {
            return (
                200,
                json!({"sub":"google-sub-a","email":"a@example.invalid","email_verified":true}),
            );
        }
        assert!(req
            .to_lowercase()
            .contains("authorization: bearer access_test_only"));
        if path.starts_with("/drive/v3/files/generateIds?") {
            assert!(path.contains("space=appDataFolder"));
            return (
                200,
                json!({"ids":[format!("file-{}", self.ids.fetch_add(1, Ordering::SeqCst))]}),
            );
        }
        if path.starts_with("/drive/v3/files?") {
            if self.fail_list.load(Ordering::SeqCst) {
                return (503, json!({"error":"offline"}));
            }
            assert!(path.contains("spaces=appDataFolder"));
            return (
                200,
                json!({"files":self.files.lock().unwrap().iter().map(|(id,r)|json!({"id":id,"name":format!("sai-profile-v1-{}.json",r["opId"].as_str().unwrap())})).collect::<Vec<_>>()}),
            );
        }
        if let Some(fid) = path
            .strip_prefix("/drive/v3/files/")
            .and_then(|p| p.strip_suffix("?alt=media"))
        {
            return self
                .files
                .lock()
                .unwrap()
                .get(fid)
                .map(|r| (200, r.clone()))
                .unwrap_or((404, json!({"error":"missing"})));
        }
        if path.starts_with("/upload/drive/v3/files?") {
            let body = req.split_once("\r\n\r\n").unwrap().1;
            let parts: Vec<_> = body.split("\r\n\r\n").collect();
            let meta: Value =
                serde_json::from_str(parts[1].split("\r\n--").next().unwrap()).unwrap();
            let record: Value =
                serde_json::from_str(parts[2].split("\r\n--").next().unwrap()).unwrap();
            assert_eq!(meta["parents"], json!(["appDataFolder"]));
            assert_eq!(meta["appProperties"]["saiProfile"], "1");
            self.posts.fetch_add(1, Ordering::SeqCst);
            let mut files = self.files.lock().unwrap();
            let fid = meta["id"].as_str().unwrap().to_owned();
            if files.contains_key(&fid) {
                return (409, json!({"error":"exists"}));
            }
            files.insert(fid.clone(), record);
            if self.fail_after_commit.swap(false, Ordering::SeqCst) {
                return (503, json!({"error":"response unavailable"}));
            }
            return (200, json!({"id":fid}));
        }
        panic!("Unexpected test request: {}", req.lines().next().unwrap());
    }
}
fn rec(op: &str, actor: &str, clock: u64, changes: Vec<Change>) -> Record {
    Record {
        version: 1,
        subject: "google-sub-a".into(),
        op_id: op.into(),
        actor: actor.into(),
        clock,
        changes,
    }
}
fn patch(value: Value) -> model::Patch {
    serde_json::from_value(value).unwrap()
}
async fn connected(drive: Arc<Drive>) -> (crate::google::oauth::tests::Server, GoogleAuth, Store) {
    let server = server(move |req| drive.handle(req)).await;
    let settings = Store::memory(
        json!({"catName":"Local cat","frequency":"low","apiKeyEnc":"PRIVATE_KEY","retentionDays":7}),
    );
    let auth = auth(settings.clone(), &server.url);
    auth.exchange_code("code", "verifier", "http://127.0.0.1/", "attempt")
        .await
        .unwrap();
    (server, auth, settings)
}
#[test]
fn two_devices_merge_independent_changes_and_tombstones_without_clock_time() {
    let mut a = Account::empty("google-sub-a");
    a.checked = true;
    let mut b = a.clone();
    a.edit("device-a", patch(json!({"displayName":"Alice"})))
        .unwrap();
    b.edit("device-b", patch(json!({"frequency":"high"})))
        .unwrap();
    let old = rec(
        "join",
        "device-a",
        1,
        vec![Change::Team(TeamRef {
            folder_id: "folder-1".into(),
            name: "Team".into(),
            removed: false,
        })],
    );
    let leave = rec(
        "leave",
        "device-a",
        2,
        vec![Change::Team(TeamRef {
            folder_id: "folder-1".into(),
            name: "Team".into(),
            removed: true,
        })],
    );
    let stale = rec(
        "stale",
        "device-z",
        2,
        vec![Change::Team(TeamRef {
            folder_id: "folder-1".into(),
            name: "Team".into(),
            removed: false,
        })],
    );
    a.merge(vec![old.clone(), leave.clone(), stale.clone()])
        .unwrap();
    b.merge(vec![stale, leave, old]).unwrap();
    a.merge(b.records.clone()).unwrap();
    b.merge(a.records.clone()).unwrap();
    assert_eq!(a.view(), b.view());
    assert_eq!(a.view()["displayName"], "Alice");
    assert_eq!(a.view()["frequency"], "high");
    assert_eq!(a.view()["teams"], json!([]));
}
#[test]
fn concurrent_same_field_has_deterministic_winner_and_explicit_rejoin_remains_possible() {
    let mut a = Account::empty("google-sub-a");
    let one = rec("one", "a", 1, vec![Change::CatName("A".into())]);
    let two = rec("two", "b", 1, vec![Change::CatName("B".into())]);
    a.merge(vec![one.clone(), two.clone()]).unwrap();
    let mut b = Account::empty("google-sub-a");
    b.merge(vec![two, one]).unwrap();
    assert_eq!(a.view(), b.view());
    assert_eq!(a.view()["catName"], "B");
    a.merge(vec![
        rec(
            "leave",
            "a",
            2,
            vec![Change::Team(TeamRef {
                folder_id: "folder".into(),
                name: "T".into(),
                removed: true,
            })],
        ),
        rec(
            "rejoin",
            "a",
            3,
            vec![Change::Team(TeamRef {
                folder_id: "folder".into(),
                name: "T".into(),
                removed: false,
            })],
        ),
    ])
    .unwrap();
    assert_eq!(a.view()["teams"][0]["accessVerified"], false);
}
#[test]
fn rejects_unchecked_edits_unknown_secrets_wrong_subject_and_conflicting_operations() {
    let mut account = Account::empty("google-sub-a");
    assert!(account
        .edit("a", patch(json!({"catName":"Changed"})))
        .is_err());
    assert!(serde_json::from_value::<Patch>(json!({"apiKeyEnc":"secret"})).is_err());
    let r = rec("op", "a", 1, vec![Change::CatName("First".into())]);
    account.merge(vec![r.clone()]).unwrap();
    let mut other = r;
    other.changes = vec![Change::CatName("Other".into())];
    assert!(account.merge(vec![other.clone()]).is_err());
    other.op_id = "new".into();
    other.subject = "google-sub-b".into();
    assert!(account.merge(vec![other]).is_err());
    assert_eq!(account.view()["catName"], "First");
}
#[tokio::test]
async fn new_pc_reads_without_uploading_defaults_and_only_explicit_fields_leave_device() {
    let temp = Temp::new();
    let drive = Arc::new(Drive::default());
    let (_server, auth, settings) = connected(drive.clone()).await;
    let profiles = Profiles::new(temp.0.clone());
    profiles.sync(&auth, &settings).await.unwrap();
    assert!(profiles.status("google-sub-a", auth.epoch())["ready"] == true);
    assert_eq!(drive.posts.load(Ordering::SeqCst), 0);
    profiles
        .update(
            &auth,
            &settings,
            auth.epoch(),
            json!({"displayName":"Alice","catName":"Ink","frequency":"high"}),
        )
        .await
        .unwrap();
    profiles.sync(&auth, &settings).await.unwrap();
    assert_eq!(settings.get()["catName"], "Ink");
    assert_eq!(settings.get()["apiKeyEnc"], "PRIVATE_KEY");
    assert_eq!(settings.get()["retentionDays"], 7);
    let raw = serde_json::to_string(&*drive.files.lock().unwrap()).unwrap();
    for private in [
        "PRIVATE_KEY",
        "refreshToken",
        "googleTokensEnc",
        "retentionDays",
        "conversation",
        "Local cat",
    ] {
        assert!(!raw.contains(private));
    }
    let pc2 = Temp::new();
    let second = Profiles::new(pc2.0.clone());
    second.sync(&auth, &settings).await.unwrap();
    assert_eq!(
        second.status("google-sub-a", auth.epoch())["profile"]["displayName"],
        "Alice"
    );
    assert_eq!(drive.posts.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn ambiguous_upload_survives_restart_and_reuses_the_same_file_id() {
    let temp = Temp::new();
    let drive = Arc::new(Drive::default());
    let (_server, auth, settings) = connected(drive.clone()).await;
    let profiles = Profiles::new(temp.0.clone());
    profiles.sync(&auth, &settings).await.unwrap();
    profiles
        .update(
            &auth,
            &settings,
            auth.epoch(),
            json!({"displayName":"Alice"}),
        )
        .await
        .unwrap();
    drive.fail_after_commit.store(true, Ordering::SeqCst);
    assert!(profiles.sync(&auth, &settings).await.is_err());
    assert_eq!(profiles.status("google-sub-a", auth.epoch())["pending"], 1);
    drop(profiles);
    let reopened = Profiles::new(temp.0.clone());
    reopened.sync(&auth, &settings).await.unwrap();
    assert_eq!(reopened.status("google-sub-a", auth.epoch())["pending"], 0);
    assert_eq!(drive.posts.load(Ordering::SeqCst), 1);
    assert_eq!(drive.ids.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn failed_restore_does_not_mark_ready_or_publish_local_defaults() {
    let temp = Temp::new();
    let drive = Arc::new(Drive::default());
    drive.fail_list.store(true, Ordering::SeqCst);
    let (_server, auth, settings) = connected(drive.clone()).await;
    let profiles = Profiles::new(temp.0.clone());
    assert!(profiles.sync(&auth, &settings).await.is_err());
    assert_eq!(
        profiles.status("google-sub-a", auth.epoch())["ready"],
        false
    );
    assert!(profiles
        .update(
            &auth,
            &settings,
            auth.epoch(),
            json!({"displayName":"Alice"})
        )
        .await
        .is_err());
    assert_eq!(drive.posts.load(Ordering::SeqCst), 0);
    assert_eq!(settings.get()["catName"], "Local cat");
}
#[tokio::test]
async fn account_logout_invalidates_lease_and_stale_forms_and_removes_overlay() {
    let temp = Temp::new();
    let drive = Arc::new(Drive::default());
    let (_server, auth, settings) = connected(drive).await;
    let profiles = Profiles::new(temp.0.clone());
    profiles.sync(&auth, &settings).await.unwrap();
    let epoch = auth.epoch();
    let lease = auth.lease().await.unwrap();
    profiles
        .update(&auth, &settings, epoch, json!({"catName":"Account cat"}))
        .await
        .unwrap();
    auth.sign_out().await.unwrap();
    assert_eq!(settings.get()["catName"], "Local cat");
    assert!(!auth.lease_current(&lease));
    assert!(auth
        .with_lease(&lease, || -> Result<()> {
            panic!("old account must not commit")
        })
        .await
        .is_err());
    assert!(profiles
        .update(&auth, &settings, epoch, json!({"displayName":"stale"}))
        .await
        .is_err());
    auth.exchange_code("code", "verifier", "http://127.0.0.1/", "attempt")
        .await
        .unwrap();
    assert!(profiles
        .update(&auth, &settings, epoch, json!({"displayName":"stale"}))
        .await
        .is_err());
}
#[test]
fn overlay_never_enters_persisted_settings_and_corrupt_account_is_not_replaced() {
    let temp = Temp::new();
    let path = temp.0.join("settings.json");
    let store = Store::open(&path, json!({"catName":"Local","secret":"private"}));
    store.set_overlay(Some(json!({"catName":"Account"})));
    store.commit(|v| v["retentionDays"] = json!(30)).unwrap();
    assert_eq!(store.get()["catName"], "Account");
    assert_eq!(Store::open(&path, json!({})).get()["catName"], "Local");
    store.set_overlay(None);
    assert_eq!(store.get()["catName"], "Local");
    let profiles = Profiles::new(temp.0.clone());
    let dir = temp.0.join("accounts");
    std::fs::create_dir(&dir).unwrap();
    let account_path = dir.join(format!("{:x}.json", Sha256::digest(b"google-sub-a")));
    std::fs::write(&account_path, b"{broken").unwrap();
    assert!(profiles
        .account(
            "google-sub-a",
            |a| {
                a.checked = true;
                Ok(())
            },
            true
        )
        .is_err());
    assert_eq!(std::fs::read(&account_path).unwrap(), b"{broken");
}

#[tokio::test]
async fn offline_edits_from_two_pcs_converge_and_pending_changes_survive_bad_remote_data() {
    let first = Temp::new();
    let second = Temp::new();
    let drive = Arc::new(Drive::default());
    let (_server, auth, settings) = connected(drive.clone()).await;
    let a = Profiles::new(first.0.clone());
    let b = Profiles::new(second.0.clone());
    a.sync(&auth, &settings).await.unwrap();
    b.sync(&auth, &settings).await.unwrap();
    drive.fail_list.store(true, Ordering::SeqCst);
    a.update(
        &auth,
        &settings,
        auth.epoch(),
        json!({"displayName":"Alice"}),
    )
    .await
    .unwrap();
    b.update(&auth, &settings, auth.epoch(), json!({"catName":"Ink"}))
        .await
        .unwrap();
    assert!(a.sync(&auth, &settings).await.is_err());
    assert!(b.sync(&auth, &settings).await.is_err());
    drive.fail_list.store(false, Ordering::SeqCst);
    a.sync(&auth, &settings).await.unwrap();
    b.sync(&auth, &settings).await.unwrap();
    a.sync(&auth, &settings).await.unwrap();
    assert_eq!(
        a.status("google-sub-a", auth.epoch())["profile"],
        b.status("google-sub-a", auth.epoch())["profile"]
    );
    a.update(&auth, &settings, auth.epoch(), json!({"frequency":"high"}))
        .await
        .unwrap();
    let mut wrong = serde_json::to_value(rec(
        "wrong",
        "z",
        1,
        vec![Change::DisplayName("Other".into())],
    ))
    .unwrap();
    wrong["subject"] = json!("google-sub-b");
    drive
        .files
        .lock()
        .unwrap()
        .insert("wrong-account-file".into(), wrong);
    let count = drive.posts.load(Ordering::SeqCst);
    assert!(a.sync(&auth, &settings).await.is_err());
    assert_eq!(a.status("google-sub-a", auth.epoch())["pending"], 1);
    assert_eq!(
        a.status("google-sub-a", auth.epoch())["profile"]["frequency"],
        "high"
    );
    assert_eq!(drive.posts.load(Ordering::SeqCst), count);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn logout_during_remote_read_prevents_old_account_commit() {
    let temp = Temp::new();
    let drive = Arc::new(Drive::default());
    let pause = Arc::new(AtomicBool::new(false));
    let (started, mut received) = tokio::sync::mpsc::unbounded_channel();
    let (release, wait) = std::sync::mpsc::channel();
    let wait = Mutex::new(wait);
    let pause_request = pause.clone();
    let remote = drive.clone();
    let server = server(move |req| {
        if req.starts_with("GET /drive/v3/files?") && pause_request.load(Ordering::SeqCst) {
            started.send(()).unwrap();
            wait.lock()
                .unwrap()
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
        }
        remote.handle(req)
    })
    .await;
    let settings = Store::memory(json!({"catName":"Local"}));
    let auth = Arc::new(auth(settings.clone(), &server.url));
    auth.exchange_code("code", "verifier", "http://127.0.0.1/", "attempt")
        .await
        .unwrap();
    let profiles = Arc::new(Profiles::new(temp.0.clone()));
    profiles.sync(&auth, &settings).await.unwrap();
    pause.store(true, Ordering::SeqCst);
    let task = {
        let p = profiles.clone();
        let a = auth.clone();
        let s = settings.clone();
        tokio::spawn(async move { p.sync(&a, &s).await })
    };
    tokio::time::timeout(std::time::Duration::from_secs(3), received.recv())
        .await
        .unwrap()
        .unwrap();
    auth.sign_out().await.unwrap();
    release.send(()).unwrap();
    assert!(task.await.unwrap().is_err());
    assert_eq!(settings.get()["catName"], "Local");
    assert_eq!(drive.posts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn switching_google_subject_keeps_profile_names_outbox_and_settings_isolated() {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    let temp = Temp::new();
    let drive = Arc::new(Drive::default());
    let second = Arc::new(AtomicBool::new(false));
    let account_b = second.clone();
    let remote = drive.clone();
    let server = server(move |req| {
        if req.starts_with("POST /token ") && account_b.load(Ordering::SeqCst) {
            let mut value = token("attempt");
            let encoded = value["id_token"]
                .as_str()
                .unwrap()
                .split('.')
                .nth(1)
                .unwrap();
            let mut claims: Value =
                serde_json::from_slice(&URL_SAFE_NO_PAD.decode(encoded).unwrap()).unwrap();
            claims["sub"] = json!("google-sub-b");
            value["id_token"] = json!(crate::google::oauth::tests::jwt(claims));
            return (200, value);
        }
        if req.starts_with("GET /userinfo ") && account_b.load(Ordering::SeqCst) {
            return (
                200,
                json!({"sub":"google-sub-b","email":"b@example.invalid","email_verified":true}),
            );
        }
        remote.handle(req)
    })
    .await;
    let settings = Store::memory(json!({"catName":"Local"}));
    let auth = auth(settings.clone(), &server.url);
    let profiles = Profiles::new(temp.0.clone());
    auth.exchange_code("code", "v", "http://127.0.0.1/", "attempt")
        .await
        .unwrap();
    profiles.sync(&auth, &settings).await.unwrap();
    let old_epoch = auth.epoch();
    profiles
        .update(
            &auth,
            &settings,
            old_epoch,
            json!({"displayName":"Alice","catName":"A cat"}),
        )
        .await
        .unwrap();
    second.store(true, Ordering::SeqCst);
    auth.exchange_code("code", "v", "http://127.0.0.1/", "attempt")
        .await
        .unwrap();
    assert_eq!(settings.get()["catName"], "Local");
    profiles.sync(&auth, &settings).await.unwrap();
    assert_eq!(
        profiles.status("google-sub-b", auth.epoch())["profile"]["displayName"],
        ""
    );
    assert_eq!(profiles.status("google-sub-b", auth.epoch())["pending"], 0);
    assert_eq!(profiles.status("google-sub-a", old_epoch)["pending"], 1);
    assert!(profiles
        .update(
            &auth,
            &settings,
            old_epoch,
            json!({"displayName":"stale A"})
        )
        .await
        .is_err());
    assert_eq!(drive.posts.load(Ordering::SeqCst), 0);
}
