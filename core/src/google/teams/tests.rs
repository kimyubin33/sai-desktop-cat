use super::*;
use crate::{
    google::oauth::{
        self,
        tests::{auth, server, token, Server},
    },
    store::Store,
};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("sai-team-test-{}", oauth::random_token()));
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
            .starts_with("sai-team-test-"));
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
#[derive(Default)]
struct Api {
    folders: Mutex<std::collections::BTreeMap<String, Value>>,
    journal: Mutex<std::collections::BTreeMap<String, Value>>,
    permissions: Mutex<Vec<Value>>,
    ids: AtomicUsize,
    creates: AtomicUsize,
    invites: AtomicUsize,
    fail_create: AtomicBool,
    fail_invite: AtomicBool,
    revoked: AtomicBool,
    not_owner: AtomicBool,
    basic_scope: AtomicBool,
}
fn file_token() -> Value {
    let mut t = token("attempt");
    t["scope"] = json!(format!("{} {}", oauth::SCOPES.join(" "), oauth::FILE_SCOPE));
    t
}
impl Api {
    fn handle(&self, req: &str) -> (u16, Value) {
        let line = req.lines().next().unwrap();
        let parts: Vec<_> = line.split_whitespace().collect();
        let method = parts[0];
        let path = parts[1];
        if path == "/token" {
            return (
                200,
                if self.basic_scope.load(Ordering::SeqCst) {
                    token("attempt")
                } else {
                    file_token()
                },
            );
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
            return (
                200,
                json!({"ids":[format!("file-{}",self.ids.fetch_add(1,Ordering::SeqCst))]}),
            );
        }
        if method == "GET" && path.starts_with("/drive/v3/files?spaces=appDataFolder") {
            return (
                200,
                json!({"files":self.journal.lock().unwrap().iter().map(|(id,r)|json!({"id":id,"name":format!("sai-profile-v1-{}.json",r["opId"].as_str().unwrap())})).collect::<Vec<_>>()}),
            );
        }
        if method == "POST" && path.starts_with("/upload/drive/v3/files?") {
            let parts: Vec<_> = req
                .split_once("\r\n\r\n")
                .unwrap()
                .1
                .split("\r\n\r\n")
                .collect();
            let meta: Value =
                serde_json::from_str(parts[1].split("\r\n--").next().unwrap()).unwrap();
            let record: Value =
                serde_json::from_str(parts[2].split("\r\n--").next().unwrap()).unwrap();
            let fid = meta["id"].as_str().unwrap().to_owned();
            self.journal.lock().unwrap().insert(fid.clone(), record);
            return (200, json!({"id":fid}));
        }
        if method == "POST" && path.starts_with("/drive/v3/files?fields=") {
            let mut body: Value =
                serde_json::from_str(req.split_once("\r\n\r\n").unwrap().1).unwrap();
            assert_eq!(body["mimeType"], FOLDER);
            assert!(body.get("permissions").is_none());
            let fid = body["id"].as_str().unwrap().to_owned();
            let mut folders = self.folders.lock().unwrap();
            if folders.contains_key(&fid) {
                return (409, json!({"error":"exists"}));
            }
            body["trashed"] = json!(false);
            body["ownedByMe"] = json!(true);
            body["capabilities"] =
                json!({"canListChildren":true,"canAddChildren":true,"canShare":true});
            folders.insert(fid, body.clone());
            self.creates.fetch_add(1, Ordering::SeqCst);
            if self.fail_create.swap(false, Ordering::SeqCst) {
                return (503, json!({"error":"lost after commit"}));
            }
            return (200, body);
        }
        if path.contains("/permissions?") {
            if method == "GET" {
                return (
                    200,
                    json!({"permissions":self.permissions.lock().unwrap().clone()}),
                );
            }
            let body: Value = serde_json::from_str(req.split_once("\r\n\r\n").unwrap().1).unwrap();
            assert_eq!(body["type"], "user");
            assert_eq!(body["role"], "writer");
            assert!(path.contains("sendNotificationEmail=true"));
            let mut p = body;
            p["id"] = json!("permission-user");
            self.permissions.lock().unwrap().push(p.clone());
            self.invites.fetch_add(1, Ordering::SeqCst);
            if self.fail_invite.swap(false, Ordering::SeqCst) {
                return (503, json!({"error":"lost after commit"}));
            }
            return (200, p);
        }
        if path.contains("/permissions/permission-user?") && method == "PATCH" {
            let mut ps = self.permissions.lock().unwrap();
            ps[0]["role"] = json!("writer");
            return (200, ps[0].clone());
        }
        if let Some(rest) = path.strip_prefix("/drive/v3/files/") {
            let fid = rest.split('?').next().unwrap();
            if path.ends_with("?alt=media") {
                return self
                    .journal
                    .lock()
                    .unwrap()
                    .get(fid)
                    .cloned()
                    .map(|v| (200, v))
                    .unwrap_or((404, json!({})));
            }
            if self.revoked.load(Ordering::SeqCst) {
                return (404, json!({}));
            }
            return self
                .folders
                .lock()
                .unwrap()
                .get(fid)
                .cloned()
                .map(|mut v| {
                    if self.not_owner.load(Ordering::SeqCst) {
                        v["ownedByMe"] = json!(false);
                    }
                    (200, v)
                })
                .unwrap_or((404, json!({})));
        }
        panic!("Unexpected test request: {line}");
    }
}
async fn fixture(api: Arc<Api>, root: &Temp) -> (Server, GoogleAuth, Profiles) {
    let server = server(move |r| api.handle(r)).await;
    let settings = Store::memory(json!({}));
    let auth = auth(settings.clone(), &server.url);
    auth.exchange_code("code", "verifier", "http://127.0.0.1/", "attempt")
        .await
        .unwrap();
    let profiles = Profiles::new(root.0.clone());
    profiles.sync(&auth, &settings).await.unwrap();
    (server, auth, profiles)
}

#[tokio::test]
async fn in_app_team_browser_uses_live_marked_folders_and_omits_connected_references() {
    let root = Temp::new();
    let api = Arc::new(Api::default());
    let remote = api.clone();
    for fid in ["connected", "invited"] {
        api.folders.lock().unwrap().insert(fid.into(),json!({"id":fid,"name":fid,"mimeType":FOLDER,"trashed":false,"properties":{"saiTeam":"1"},"capabilities":{"canListChildren":true,"canAddChildren":true}}));
    }
    let mock=server(move |request|{
        if request.starts_with("POST /token") {let mut t=file_token();t["scope"]=json!(format!("{} {}",oauth::SCOPES.join(" "),oauth::DRIVE_SCOPE));return (200,t);}
        let path=request.lines().next().unwrap().split_whitespace().nth(1).unwrap();
        if path.starts_with("/drive/v3/files?spaces=drive") {
            let parsed=url::Url::parse(&format!("http://mock{path}")).unwrap();let params:std::collections::BTreeMap<_,_>=parsed.query_pairs().into_owned().collect();
            assert!(params["q"].contains("properties has { key='saiTeam'"));assert!(params["q"].contains(FOLDER));
            return (200,json!({"files":remote.folders.lock().unwrap().values().cloned().collect::<Vec<_>>()}));
        }
        remote.handle(request)
    }).await;
    let settings = Store::memory(json!({}));
    let client = auth(settings.clone(), &mock.url);
    client
        .exchange_code("base", "v", "http://127.0.0.1/", "attempt")
        .await
        .unwrap();
    let profiles = Profiles::new(root.0.clone());
    profiles.sync(&client, &settings).await.unwrap();
    let teams = Teams::default();
    teams
        .join(&client, &profiles, client.epoch(), "connected")
        .await
        .unwrap();
    let rows = teams
        .browse(&client, &profiles, client.epoch(), "")
        .await
        .unwrap();
    assert_eq!(rows["entries"].as_array().unwrap().len(), 1);
    assert_eq!(rows["entries"][0]["id"], "invited");
    assert!(teams
        .browse(&client, &profiles, client.epoch() - 1, "")
        .await
        .is_err());
    assert_eq!(api.creates.load(Ordering::SeqCst), 0);
    assert_eq!(api.invites.load(Ordering::SeqCst), 0);
}

#[test]
fn external_objects_do_not_become_teams_without_folder_marker_and_current_access() {
    let mut value = json!({"id":"folder","name":"Team","mimeType":FOLDER,"trashed":false,"properties":{"saiTeam":"1"},"ownedByMe":true,"capabilities":{"canListChildren":true,"canShare":true,"canAddChildren":true}});
    assert_eq!(folder(value.clone(), "folder").unwrap()["canInvite"], true);
    value["ownedByMe"] = json!(false);
    assert_eq!(folder(value.clone(), "folder").unwrap()["canInvite"], false);
    value["capabilities"]["canListChildren"] = json!(false);
    assert!(folder(value.clone(), "folder").is_err());
    value["capabilities"]["canListChildren"] = json!(true);
    value["trashed"] = json!(true);
    assert!(folder(value.clone(), "folder").is_err());
    value["trashed"] = json!(false);
    value["mimeType"] = json!("application/pdf");
    assert!(folder(value.clone(), "folder").is_err());
}

#[tokio::test]
async fn ambiguous_create_reopens_same_draft_and_generates_one_folder() {
    let root = Temp::new();
    let api = Arc::new(Api::default());
    let (_server, auth, profiles) = fixture(api.clone(), &root).await;
    let teams = Teams::default();
    api.fail_create.store(true, Ordering::SeqCst);
    assert!(teams
        .create(&auth, &profiles, auth.epoch(), "Our team")
        .await
        .is_err());
    assert!(profiles.team_state("google-sub-a").unwrap()["draft"]["fileId"].is_string());
    assert!(teams
        .create(&auth, &profiles, auth.epoch(), "Different team")
        .await
        .is_err());
    drop(profiles);
    let profiles = Profiles::new(root.0.clone());
    let team = teams
        .create(&auth, &profiles, auth.epoch(), "")
        .await
        .unwrap();
    assert_eq!(team["name"], "Our team");
    assert_eq!(team["accessVerified"], true);
    assert_eq!(api.creates.load(Ordering::SeqCst), 1);
    assert_eq!(api.ids.load(Ordering::SeqCst), 1);
    assert!(profiles.team_state("google-sub-a").unwrap()["draft"].is_null());
    assert_eq!(
        profiles.team_state("google-sub-a").unwrap()["teams"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn restored_reference_requires_live_access_and_removal_preserves_drive_files() {
    let root = Temp::new();
    let pc2 = Temp::new();
    let api = Arc::new(Api::default());
    let (_server, auth, profiles) = fixture(api.clone(), &root).await;
    let teams = Teams::default();
    let team = teams
        .create(&auth, &profiles, auth.epoch(), "Team")
        .await
        .unwrap();
    let fid = team["folderId"].as_str().unwrap();
    let settings = Store::memory(json!({}));
    profiles.sync(&auth, &settings).await.unwrap();
    let restored = Profiles::new(pc2.0.clone());
    restored.sync(&auth, &settings).await.unwrap();
    assert_eq!(
        restored.team_state("google-sub-a").unwrap()["teams"][0]["accessVerified"],
        false
    );
    assert_eq!(
        teams.view(&auth, &restored).await.unwrap()["teams"][0]["accessVerified"],
        true
    );
    api.revoked.store(true, Ordering::SeqCst);
    assert_eq!(
        teams.view(&auth, &restored).await.unwrap()["teams"][0]["accessVerified"],
        false
    );
    assert!(teams
        .open(&auth, &restored, auth.epoch(), fid)
        .await
        .is_err());
    assert!(teams
        .invite(
            &auth,
            &restored,
            auth.epoch(),
            fid,
            "person@example.invalid"
        )
        .await
        .is_err());
    teams
        .remove(&auth, &restored, auth.epoch(), fid)
        .await
        .unwrap();
    assert!(api.folders.lock().unwrap().contains_key(fid));
    assert!(restored.team_state("google-sub-a").unwrap()["teams"]
        .as_array()
        .unwrap()
        .is_empty());
    restored.sync(&auth, &settings).await.unwrap();
    profiles.sync(&auth, &settings).await.unwrap();
    assert!(profiles.team_state("google-sub-a").unwrap()["teams"]
        .as_array()
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn only_owner_invites_explicit_user_and_retry_does_not_resend_after_uncertain_response() {
    let root = Temp::new();
    let api = Arc::new(Api::default());
    let (_server, auth, profiles) = fixture(api.clone(), &root).await;
    let teams = Teams::default();
    let team = teams
        .create(&auth, &profiles, auth.epoch(), "Team")
        .await
        .unwrap();
    let fid = team["folderId"].as_str().unwrap();
    api.not_owner.store(true, Ordering::SeqCst);
    assert!(teams
        .invite(
            &auth,
            &profiles,
            auth.epoch(),
            fid,
            "person@example.invalid"
        )
        .await
        .is_err());
    assert_eq!(api.invites.load(Ordering::SeqCst), 0);
    api.not_owner.store(false, Ordering::SeqCst);
    api.fail_invite.store(true, Ordering::SeqCst);
    assert!(teams
        .invite(
            &auth,
            &profiles,
            auth.epoch(),
            fid,
            "person@example.invalid"
        )
        .await
        .is_err());
    let result = teams
        .invite(
            &auth,
            &profiles,
            auth.epoch(),
            fid,
            "PERSON@example.invalid",
        )
        .await
        .unwrap();
    assert_eq!(result["alreadyShared"], true);
    assert_eq!(api.invites.load(Ordering::SeqCst), 1);
    assert!(teams
        .invite(
            &auth,
            &profiles,
            auth.epoch(),
            fid,
            "person@example.invalid\r\nother"
        )
        .await
        .is_err());
}

#[tokio::test]
async fn reader_upgrade_is_explicit_and_arbitrary_ids_or_non_team_objects_are_rejected() {
    let root = Temp::new();
    let api = Arc::new(Api::default());
    let (_server, auth, profiles) = fixture(api.clone(), &root).await;
    let teams = Teams::default();
    let team = teams
        .create(&auth, &profiles, auth.epoch(), "Team")
        .await
        .unwrap();
    let fid = team["folderId"].as_str().unwrap();
    api.permissions.lock().unwrap().push(json!({"id":"permission-user","type":"user","role":"reader","emailAddress":"reader@example.invalid"}));
    teams
        .invite(
            &auth,
            &profiles,
            auth.epoch(),
            fid,
            "reader@example.invalid",
        )
        .await
        .unwrap();
    assert_eq!(api.permissions.lock().unwrap()[0]["role"], "writer");
    assert_eq!(api.invites.load(Ordering::SeqCst), 0);
    assert!(teams
        .open(&auth, &profiles, auth.epoch(), "not-remembered")
        .await
        .is_err());
    assert!(teams
        .join(&auth, &profiles, auth.epoch(), "../bad")
        .await
        .is_err());
    let mut fake = api.folders.lock().unwrap()[fid].clone();
    fake["id"] = json!("ordinary");
    fake["properties"] = json!({});
    api.folders.lock().unwrap().insert("ordinary".into(), fake);
    assert!(teams
        .join(&auth, &profiles, auth.epoch(), "ordinary")
        .await
        .is_err());
    let epoch = auth.epoch();
    auth.sign_out().await.unwrap();
    assert!(teams
        .create(&auth, &profiles, epoch, "Stale")
        .await
        .is_err());
}

#[tokio::test]
async fn basic_login_does_not_silently_request_or_use_drive_file_access() {
    let root = Temp::new();
    let api = Arc::new(Api::default());
    api.basic_scope.store(true, Ordering::SeqCst);
    let (_server, auth, profiles) = fixture(api.clone(), &root).await;
    let teams = Teams::default();
    assert_eq!(
        teams.view(&auth, &profiles).await.unwrap()["driveConnected"],
        false
    );
    assert!(teams
        .create(&auth, &profiles, auth.epoch(), "Team")
        .await
        .is_err());
    assert_eq!(api.creates.load(Ordering::SeqCst), 0);
    assert_eq!(api.ids.load(Ordering::SeqCst), 0);
}
