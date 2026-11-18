use super::*;
use crate::google::{
    oauth::{
        self,
        tests::{auth, server, token, Server},
    },
    profile::model::TeamRef,
};
use model::{Entity, Kind, Status};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("sai-work-test-{}", oauth::random_token()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        assert_eq!(self.0.parent(), Some(std::env::temp_dir().as_path()));
        assert!(self
            .0
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("sai-work-test-"));
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn entity(id: &str, title: &str) -> Entity {
    Entity {
        id: id.into(),
        kind: Kind::Task,
        title: title.into(),
        project_id: String::new(),
        assignee: String::new(),
        due_date: "2026-11-20".into(),
        status: Status::Todo,
    }
}
fn record(op: &str, parents: &[&str], title: &str) -> Record {
    Record {
        version: 1,
        folder_id: "team-folder".into(),
        op_id: op.into(),
        parents: parents.iter().map(|s| s.to_string()).collect(),
        entity: entity("task-1", title),
    }
}
#[test]
fn concurrent_changes_remain_visible_until_a_record_explicitly_resolves_all_heads() {
    let a = record("root", &[], "Original");
    let b = record("left", &["root"], "Left change");
    let c = record("right", &["root"], "Right change");
    let mut other = record("independent", &[], "Separate task");
    other.entity.id = "task-2".into();
    let mut records = vec![other, c, b, a];
    model::validate(&records, "team-folder").unwrap();
    let view = model::view(&records);
    assert_eq!(view[0]["id"], "task-1");
    assert_eq!(view[0]["conflict"], true);
    assert_eq!(view[0]["heads"], json!(["left", "right"]));
    assert_eq!(view[0]["variants"][0]["operation"], "left");
    assert_eq!(view[0]["variants"][1]["operation"], "right");
    assert_eq!(view[1]["id"], "task-2");
    assert_eq!(view[1]["conflict"], false);
    assert_eq!(view[1]["heads"], json!(["independent"]));
    records.push(record("resolved", &["left", "right"], "Reviewed result"));
    model::validate(&records, "team-folder").unwrap();
    let view = model::view(&records);
    assert_eq!(view[0]["conflict"], false);
    assert_eq!(view[0]["variants"][0]["entity"]["title"], "Reviewed result");
}
#[test]
fn malformed_graphs_unknown_versions_cross_team_and_invalid_dates_are_rejected() {
    for records in [
        vec![record("a", &["missing"], "x")],
        vec![record("a", &["b"], "x"), record("b", &["a"], "x")],
        vec![record("a", &[], "x"), record("a", &[], "x")],
    ] {
        assert!(model::validate(&records, "team-folder").is_err());
    }
    let mut r = record("a", &[], "x");
    assert!(model::validate(&[r.clone()], "other-folder").is_err());
    r.entity.due_date = "2026-02-30".into();
    assert!(!r.valid("team-folder"));
    r.entity.due_date = String::new();
    r.version = 2;
    assert!(!r.valid("team-folder"));
}
#[derive(Default)]
struct Api {
    files: Mutex<BTreeMap<String, (Value, Value)>>,
    ids: AtomicUsize,
    posts: AtomicUsize,
    ambiguous: AtomicBool,
    denied: AtomicBool,
    read_only: AtomicBool,
    revoked: AtomicBool,
    incomplete: AtomicBool,
    hide: AtomicBool,
    second: AtomicBool,
}
impl Api {
    fn seed(&self, fid: &str, r: Record) {
        let meta = json!({"id":fid,"name":format!("sai-work-v1-{}.json",r.op_id),"mimeType":"application/json","parents":["team-folder"],"trashed":false,"version":"1","properties":{"saiCollab":"1"}});
        self.files
            .lock()
            .unwrap()
            .insert(fid.into(), (meta, serde_json::to_value(r).unwrap()));
    }
    fn handle(&self, request: &str) -> (u16, Value) {
        let line = request.lines().next().unwrap();
        let mut parts = line.split_whitespace();
        let method = parts.next().unwrap();
        let path = parts.next().unwrap();
        if path == "/token" {
            let mut t = token("attempt");
            t["scope"] = json!(format!(
                "{} {}",
                oauth::SCOPES.join(" "),
                oauth::DRIVE_SCOPE
            ));
            if self.second.load(Ordering::SeqCst) {
                let now = crate::time::now_ms() / 1000;
                t["id_token"] = json!(oauth::tests::jwt(
                    json!({"iss":"https://accounts.google.com","aud":"test-client","sub":"google-sub-b","nonce":"attempt","iat":now-1,"exp":now+3600})
                ));
            }
            return (200, t);
        }
        if path == "/userinfo" {
            return (
                200,
                json!({"sub":if self.second.load(Ordering::SeqCst){"google-sub-b"}else{"google-sub-a"},"email":"user@example.invalid","email_verified":true}),
            );
        }
        if path.starts_with("/drive/v3/files?spaces=appDataFolder") {
            return (200, json!({"files":[]}));
        }
        if path.starts_with("/drive/v3/files/team-folder?") {
            return if self.revoked.load(Ordering::SeqCst) {
                (403, json!({}))
            } else {
                (
                    200,
                    json!({"id":"team-folder","name":"Team","mimeType":"application/vnd.google-apps.folder","trashed":false,"properties":{"saiTeam":"1"},"capabilities":{"canListChildren":true,"canAddChildren":!self.read_only.load(Ordering::SeqCst)}}),
                )
            };
        }
        if path.starts_with("/drive/v3/files/generateIds?") {
            return (
                200,
                json!({"ids":[format!("work-file-{}",self.ids.fetch_add(1,Ordering::SeqCst))]}),
            );
        }
        if path.starts_with("/drive/v3/files?spaces=drive") {
            let parsed = url::Url::parse(&format!("http://mock{path}")).unwrap();
            let params: BTreeMap<_, _> = parsed.query_pairs().into_owned().collect();
            assert!(params["q"].contains("'team-folder' in parents"));
            assert!(params["q"].contains("key='saiCollab'"));
            let rows = if self.hide.load(Ordering::SeqCst) {
                vec![]
            } else {
                self.files
                    .lock()
                    .unwrap()
                    .values()
                    .map(|(m, _)| m.clone())
                    .collect::<Vec<_>>()
            };
            return (
                200,
                json!({"files":rows,"incompleteSearch":self.incomplete.load(Ordering::SeqCst)}),
            );
        }
        if method == "POST" && path.starts_with("/upload/drive/v3/files?") {
            if self.denied.load(Ordering::SeqCst) {
                return (403, json!({}));
            }
            let parts = request
                .split_once("\r\n\r\n")
                .unwrap()
                .1
                .split("\r\n\r\n")
                .collect::<Vec<_>>();
            let mut metadata: Value =
                serde_json::from_str(parts[1].split("\r\n--").next().unwrap()).unwrap();
            let content: Value =
                serde_json::from_str(parts[2].split("\r\n--").next().unwrap()).unwrap();
            assert_eq!(metadata["parents"], json!(["team-folder"]));
            assert_eq!(metadata["properties"]["saiCollab"], "1");
            assert!(metadata.get("permissions").is_none());
            let fid = metadata["id"].as_str().unwrap().to_owned();
            let mut files = self.files.lock().unwrap();
            if files.contains_key(&fid) {
                return (409, json!({}));
            }
            metadata["version"] = json!("1");
            metadata["trashed"] = json!(false);
            files.insert(fid.clone(), (metadata, content));
            self.posts.fetch_add(1, Ordering::SeqCst);
            return if self.ambiguous.swap(false, Ordering::SeqCst) {
                (503, json!({}))
            } else {
                (200, json!({"id":fid}))
            };
        }
        if let Some(rest) = path.strip_prefix("/drive/v3/files/") {
            let fid = rest.split('?').next().unwrap();
            return self
                .files
                .lock()
                .unwrap()
                .get(fid)
                .map(|(meta, body)| {
                    (
                        200,
                        if path.ends_with("?alt=media") {
                            body.clone()
                        } else {
                            meta.clone()
                        },
                    )
                })
                .unwrap_or((404, json!({})));
        }
        panic!("Unexpected mock request {method} {path}");
    }
}
struct Fixture {
    root: Temp,
    _server: Server,
    auth: GoogleAuth,
    profiles: Profiles,
    teams: Teams,
    api: Arc<Api>,
    work: Collaboration,
}

#[tokio::test]
async fn a_team_cannot_be_forgotten_while_its_collaboration_intent_is_pending() {
    let f = Fixture::new().await;
    f.api.denied.store(true, Ordering::SeqCst);
    f.save(entity("", "Pending"), json!([])).await.unwrap();
    assert!(f
        .work
        .remove_team(
            &f.auth,
            &f.profiles,
            &f.teams,
            f.auth.epoch(),
            "team-folder"
        )
        .await
        .is_err());
    assert_eq!(
        f.profiles.team_state("google-sub-a").unwrap()["teams"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    f.api.denied.store(false, Ordering::SeqCst);
    f.load(true).await.unwrap();
    f.work
        .remove_team(
            &f.auth,
            &f.profiles,
            &f.teams,
            f.auth.epoch(),
            "team-folder",
        )
        .await
        .unwrap();
    assert_eq!(
        f.profiles.team_state("google-sub-a").unwrap()["teams"],
        json!([])
    );
    assert_eq!(f.api.files.lock().unwrap().len(), 1);
}
impl Fixture {
    async fn new() -> Self {
        let root = Temp::new();
        let api = Arc::new(Api::default());
        let remote = api.clone();
        let server = server(move |r| remote.handle(r)).await;
        let settings = Store::memory(json!({}));
        let auth = auth(settings.clone(), &server.url);
        auth.exchange_code("base", "v", "http://127.0.0.1/", "attempt")
            .await
            .unwrap();
        let profiles = Profiles::new(root.0.join("profiles"));
        profiles.sync(&auth, &settings).await.unwrap();
        let lease = auth.lease().await.unwrap();
        profiles
            .remember_team(
                &auth,
                &lease,
                TeamRef {
                    folder_id: "team-folder".into(),
                    name: "Team".into(),
                    removed: false,
                },
                None,
            )
            .await
            .unwrap();
        let work = Collaboration::new(root.0.join("work"));
        Self {
            root,
            _server: server,
            auth,
            profiles,
            teams: Teams::default(),
            api,
            work,
        }
    }
    async fn load(&self, sync: bool) -> Result<Value> {
        self.work
            .load(
                &self.auth,
                &self.profiles,
                &self.teams,
                self.auth.epoch(),
                "team-folder",
                sync,
            )
            .await
    }
    async fn save(&self, entity: Entity, heads: Value) -> Result<Value> {
        self.work
            .save(
                &self.auth,
                &self.profiles,
                &self.teams,
                self.auth.epoch(),
                "team-folder",
                json!({"entity":entity,"heads":heads}),
            )
            .await
    }
}
#[tokio::test]
async fn project_and_task_save_restore_on_another_pc_without_rewriting_other_files() {
    let f = Fixture::new().await;
    assert_eq!(f.load(false).await.unwrap()["items"], json!([]));
    let project = Entity {
        id: String::new(),
        kind: Kind::Project,
        title: "Launch".into(),
        project_id: String::new(),
        assignee: String::new(),
        due_date: String::new(),
        status: Status::Doing,
    };
    let v = f.save(project, json!([])).await.unwrap();
    let project_id = v["items"][0]["id"].as_str().unwrap();
    let mut task = entity("", "Draft brief");
    task.project_id = project_id.into();
    task.assignee = "Team member".into();
    let v = f.save(task, json!([])).await.unwrap();
    assert_eq!(v["pending"], 0);
    assert_eq!(v["items"].as_array().unwrap().len(), 2);
    let another = Collaboration::new(f.root.0.join("pc-two"));
    let restored = another
        .load(
            &f.auth,
            &f.profiles,
            &f.teams,
            f.auth.epoch(),
            "team-folder",
            false,
        )
        .await
        .unwrap();
    assert_eq!(restored["items"], v["items"]);
    assert_eq!(f.api.posts.load(Ordering::SeqCst), 2);
}
#[tokio::test]
async fn stale_form_is_rejected_and_does_not_overwrite_the_newer_remote_change() {
    let f = Fixture::new().await;
    f.api.seed("root-file", record("root", &[], "Original"));
    let initial = f.load(false).await.unwrap();
    f.api
        .seed("new-file", record("new", &["root"], "Other PC changed"));
    assert!(f
        .save(
            entity("task-1", "My old form"),
            initial["items"][0]["heads"].clone()
        )
        .await
        .is_err());
    assert_eq!(f.api.posts.load(Ordering::SeqCst), 0);
    let current = f.load(false).await.unwrap();
    assert_eq!(
        current["items"][0]["variants"][0]["entity"]["title"],
        "Other PC changed"
    );
}
#[tokio::test]
async fn conflict_is_visible_and_user_reviewed_resolution_retains_history() {
    let f = Fixture::new().await;
    f.api.seed("root-file", record("root", &[], "Original"));
    f.api.seed("left-file", record("left", &["root"], "Left"));
    f.api
        .seed("right-file", record("right", &["root"], "Right"));
    let v = f.load(false).await.unwrap();
    assert_eq!(v["items"][0]["conflict"], true);
    let resolved = f
        .save(entity("task-1", "Reviewed"), v["items"][0]["heads"].clone())
        .await
        .unwrap();
    assert_eq!(resolved["items"][0]["conflict"], false);
    assert_eq!(f.api.files.lock().unwrap().len(), 4);
}
#[tokio::test]
async fn lost_upload_response_recovers_after_restart_without_a_duplicate() {
    let f = Fixture::new().await;
    f.api.ambiguous.store(true, Ordering::SeqCst);
    let v = f.save(entity("", "Draft"), json!([])).await.unwrap();
    assert_eq!(v["pending"], 1);
    assert!(v["error"].is_string());
    let reopened = Collaboration::new(f.root.0.join("work"));
    let v = reopened
        .load(
            &f.auth,
            &f.profiles,
            &f.teams,
            f.auth.epoch(),
            "team-folder",
            true,
        )
        .await
        .unwrap();
    assert_eq!(v["pending"], 0);
    assert_eq!(f.api.posts.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn failed_write_preserves_local_intent_and_reuses_its_reserved_file_id() {
    let f = Fixture::new().await;
    f.api.denied.store(true, Ordering::SeqCst);
    let v = f.save(entity("", "Draft"), json!([])).await.unwrap();
    assert_eq!(v["pending"], 1);
    assert_eq!(f.api.ids.load(Ordering::SeqCst), 1);
    f.api.denied.store(false, Ordering::SeqCst);
    let v = f.load(true).await.unwrap();
    assert_eq!(v["pending"], 0);
    assert_eq!(f.api.ids.load(Ordering::SeqCst), 1);
    assert_eq!(f.api.posts.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn incomplete_deleted_or_modified_journals_never_become_an_empty_board_to_overwrite() {
    let f = Fixture::new().await;
    f.api.seed("root-file", record("root", &[], "Original"));
    f.load(false).await.unwrap();
    f.api.incomplete.store(true, Ordering::SeqCst);
    assert!(f.save(entity("", "New"), json!([])).await.is_err());
    f.api.incomplete.store(false, Ordering::SeqCst);
    f.api.hide.store(true, Ordering::SeqCst);
    assert!(f.load(true).await.is_err());
    f.api.hide.store(false, Ordering::SeqCst);
    {
        let mut files = f.api.files.lock().unwrap();
        let (meta, body) = files.get_mut("root-file").unwrap();
        meta["version"] = json!("2");
        body["entity"]["title"] = json!("Tampered");
    }
    assert!(f.load(true).await.is_err());
    assert_eq!(f.api.posts.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn read_only_revoked_and_stale_accounts_cannot_publish_changes() {
    let f = Fixture::new().await;
    f.api.read_only.store(true, Ordering::SeqCst);
    assert!(f.save(entity("", "No write"), json!([])).await.is_err());
    assert_eq!(f.load(false).await.unwrap()["team"]["canUpload"], false);
    f.api.revoked.store(true, Ordering::SeqCst);
    assert!(f.load(false).await.is_err());
    let epoch = f.auth.epoch();
    f.auth.sign_out().await.unwrap();
    assert!(f
        .work
        .save(
            &f.auth,
            &f.profiles,
            &f.teams,
            epoch,
            "team-folder",
            json!({"entity":entity("","x"),"heads":[]})
        )
        .await
        .is_err());
    assert_eq!(f.api.posts.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn corrupt_local_state_and_another_accounts_pending_are_not_published() {
    let f = Fixture::new().await;
    f.api.denied.store(true, Ordering::SeqCst);
    f.save(entity("", "Pending"), json!([])).await.unwrap();
    let other = f
        .work
        .board("google-sub-b", "team-folder", false, |b| {
            Ok(b.pending.len())
        })
        .unwrap();
    assert_eq!(other, 0);
    let key = format!("{:x}", Sha256::digest(b"google-sub-a\nteam-folder"));
    let path = f.root.0.join("work").join(format!("{key}.json"));
    std::fs::write(&path, b"{broken").unwrap();
    let reopened = Collaboration::new(f.root.0.join("work"));
    assert!(reopened
        .load(
            &f.auth,
            &f.profiles,
            &f.teams,
            f.auth.epoch(),
            "team-folder",
            true
        )
        .await
        .is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"{broken");
    assert_eq!(f.api.posts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn signing_in_as_another_member_does_not_publish_the_previous_accounts_private_pending() {
    let f = Fixture::new().await;
    f.api.denied.store(true, Ordering::SeqCst);
    f.save(entity("", "A pending"), json!([])).await.unwrap();
    f.auth.sign_out().await.unwrap();
    f.api.second.store(true, Ordering::SeqCst);
    f.api.denied.store(false, Ordering::SeqCst);
    f.auth
        .exchange_code("second", "v", "http://127.0.0.1/", "attempt")
        .await
        .unwrap();
    let settings = Store::memory(json!({}));
    f.profiles.sync(&f.auth, &settings).await.unwrap();
    let lease = f.auth.lease().await.unwrap();
    f.profiles
        .remember_team(
            &f.auth,
            &lease,
            TeamRef {
                folder_id: "team-folder".into(),
                name: "Team".into(),
                removed: false,
            },
            None,
        )
        .await
        .unwrap();
    assert_eq!(f.load(true).await.unwrap()["items"], json!([]));
    assert_eq!(f.api.posts.load(Ordering::SeqCst), 0);
    assert_eq!(
        f.work
            .board("google-sub-a", "team-folder", false, |b| Ok(b
                .pending
                .len()))
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn remote_metadata_must_match_the_team_and_the_record_name() {
    let f = Fixture::new().await;
    f.api.seed("record-file", record("root", &[], "Original"));
    f.api
        .files
        .lock()
        .unwrap()
        .get_mut("record-file")
        .unwrap()
        .0["parents"] = json!(["different-team"]);
    assert!(f.load(false).await.is_err());
    {
        let mut files = f.api.files.lock().unwrap();
        let meta = &mut files.get_mut("record-file").unwrap().0;
        meta["parents"] = json!(["team-folder"]);
        meta["name"] = json!("unrelated.json");
    }
    assert!(f.load(false).await.is_err());
    assert_eq!(f.api.posts.load(Ordering::SeqCst), 0);
}
