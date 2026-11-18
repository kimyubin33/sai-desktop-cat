use super::*;
use crate::{
    google::oauth::{
        self,
        tests::{auth, server, token, Server},
    },
    store::Store,
};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

const ME: &str = "a@example.invalid";

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("sai-mediation-test-{}", oauth::random_token()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        assert!(self.0.file_name().unwrap().to_string_lossy().starts_with("sai-mediation-test-"));
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A tiny Drive: files with metadata, JSON content and permissions, seen from the account `ME`.
#[derive(Default)]
struct World {
    files: Mutex<BTreeMap<String, (Value, Value, Vec<Value>)>>,
    journal: Mutex<BTreeMap<String, Value>>,
    ids: AtomicUsize,
    fail_share: AtomicBool,
    shares: Mutex<Vec<String>>,
}

fn owner(email: &str) -> Value {
    json!([{"emailAddress":email,"displayName":email.split('@').next().unwrap()}])
}
fn body(req: &str) -> &str {
    req.split_once("\r\n\r\n").unwrap().1
}
fn multipart(req: &str) -> (Value, Value) {
    let parts: Vec<_> = body(req).split("\r\n\r\n").collect();
    let meta = serde_json::from_str(parts[1].split("\r\n--").next().unwrap()).unwrap();
    let content = serde_json::from_str(parts[2].split("\r\n--").next().unwrap()).unwrap();
    (meta, content)
}

impl World {
    fn add(&self, id: &str, meta: Value, content: Value, perms: Vec<Value>) {
        let mut meta = meta;
        meta["id"] = json!(id);
        if meta.get("trashed").is_none() {
            meta["trashed"] = json!(false);
        }
        self.files.lock().unwrap().insert(id.into(), (meta, content, perms));
    }
    fn folder(&self, id: &str, mine: bool, owner_email: &str, perms: Vec<Value>) {
        self.add(
            id,
            json!({"name":id,"mimeType":FOLDER,"ownedByMe":mine,"writersCanShare":false,"properties":{"saiTeam":"1"},"owners":owner(owner_email),
                "capabilities":{"canListChildren":true,"canAddChildren":true,"canShare":mine}}),
            Value::Null,
            perms,
        );
    }
    fn mine_where(&self, f: impl Fn(&Value) -> bool) -> Vec<(Value, Value, Vec<Value>)> {
        self.files.lock().unwrap().values().filter(|(m, _, _)| f(m)).cloned().collect()
    }
    fn handle(&self, req: &str) -> (u16, Value) {
        let line = req.lines().next().unwrap();
        let (method, path) = {
            let p: Vec<_> = line.split_whitespace().collect();
            (p[0], p[1])
        };
        if path == "/token" {
            let mut t = token("attempt");
            t["scope"] = json!(format!("{} {} {}", oauth::SCOPES.join(" "), oauth::FILE_SCOPE, oauth::DRIVE_SCOPE));
            return (200, t);
        }
        if path == "/userinfo" {
            return (200, json!({"sub":"google-sub-a","email":ME,"email_verified":true}));
        }
        if path.starts_with("/drive/v3/files/generateIds?") {
            return (200, json!({"ids":[format!("gen{}", self.ids.fetch_add(1, Ordering::SeqCst))]}));
        }
        if method == "GET" && path.starts_with("/drive/v3/files?spaces=appDataFolder") {
            let rows: Vec<_> = self
                .journal
                .lock()
                .unwrap()
                .iter()
                .map(|(id, r)| json!({"id":id,"name":format!("sai-profile-v1-{}.json",r["opId"].as_str().unwrap())}))
                .collect();
            return (200, json!({ "files": rows }));
        }
        if method == "POST" && path.starts_with("/upload/drive/v3/files?") {
            let (mut meta, content) = multipart(req);
            if let Some(id) = meta["id"].as_str().map(str::to_owned) {
                self.journal.lock().unwrap().insert(id.clone(), content);
                return (200, json!({ "id": id }));
            }
            let id = format!("new{}", self.ids.fetch_add(1, Ordering::SeqCst));
            meta["ownedByMe"] = json!(true);
            meta["owners"] = owner(ME);
            meta["lastModifyingUser"] = owner(ME)[0].clone();
            self.add(&id, meta, content, vec![]);
            return (200, json!({ "id": id }));
        }
        if method == "PATCH" && path.starts_with("/upload/drive/v3/files/") {
            let id = path.trim_start_matches("/upload/drive/v3/files/").split('?').next().unwrap();
            let mut files = self.files.lock().unwrap();
            files.get_mut(id).unwrap().1 = serde_json::from_str(body(req)).unwrap();
            return (200, json!({ "id": id }));
        }
        if method == "POST" && path.starts_with("/drive/v3/files?fields=id") {
            let mut meta: Value = serde_json::from_str(body(req)).unwrap();
            let id = format!("new{}", self.ids.fetch_add(1, Ordering::SeqCst));
            meta["ownedByMe"] = json!(true);
            meta["owners"] = owner(ME);
            self.add(&id, meta, Value::Null, vec![]);
            return (200, json!({ "id": id }));
        }
        if method == "GET" && path.starts_with("/drive/v3/files?") {
            let url = url::Url::parse(&format!("http://mock{path}")).unwrap();
            let params: BTreeMap<_, _> = url.query_pairs().into_owned().collect();
            let q = params["q"].clone();
            let parent = q.split('\'').nth(1).filter(|_| q.contains("in parents")).map(str::to_owned);
            let rows: Vec<_> = self
                .mine_where(|m| {
                    m["trashed"] == false
                        && parent.as_ref().is_none_or(|p| m["parents"].as_array().is_some_and(|ps| ps.iter().any(|x| x == p)))
                        && (!q.contains("saiRoster") || m["properties"]["saiRoster"] == "1")
                        && (!q.contains("saiOutbox") || (m["properties"]["saiOutbox"] == "1" && m["ownedByMe"] == true))
                        && (!q.contains("sharedWithMe") || (m["properties"]["saiMessage"] == "1" && m["ownedByMe"] == false))
                })
                .into_iter()
                .map(|(m, _, _)| m)
                .collect();
            return (200, json!({ "files": rows }));
        }
        if let Some(rest) = path.strip_prefix("/drive/v3/files/") {
            let id = rest.split(['/', '?']).next().unwrap().to_owned();
            if rest.contains("/permissions") {
                let mut files = self.files.lock().unwrap();
                let file = files.get_mut(&id).unwrap();
                if method == "GET" {
                    return (200, json!({"permissions": file.2}));
                }
                assert!(path.contains("sendNotificationEmail=false"), "{path}");
                let p: Value = serde_json::from_str(body(req)).unwrap();
                assert_eq!((p["type"].as_str(), p["role"].as_str()), (Some("user"), Some("reader")));
                if self.fail_share.swap(false, Ordering::SeqCst) {
                    return (500, json!({}));
                }
                self.shares.lock().unwrap().push(p.s("emailAddress"));
                file.2.push(p.clone());
                return (200, p);
            }
            if method == "DELETE" {
                self.files.lock().unwrap().remove(&id);
                return (204, json!({}));
            }
            if path.ends_with("?alt=media") {
                if let Some(v) = self.journal.lock().unwrap().get(&id) {
                    return (200, v.clone());
                }
                return self.files.lock().unwrap().get(&id).map(|f| (200, f.1.clone())).unwrap_or((404, json!({})));
            }
            return self.files.lock().unwrap().get(&id).map(|f| (200, f.0.clone())).unwrap_or((404, json!({})));
        }
        panic!("Unexpected test request: {line}");
    }
}

async fn fixture(world: Arc<World>, root: &Temp, join: &[&str]) -> (Server, GoogleAuth, Profiles, Teams) {
    let w = world.clone();
    let server = server(move |r| w.handle(r)).await;
    let settings = Store::memory(json!({}));
    let auth = auth(settings.clone(), &server.url);
    auth.exchange_code("code", "verifier", "http://127.0.0.1/", "attempt").await.unwrap();
    let profiles = Profiles::new(root.0.clone());
    profiles.sync(&auth, &settings).await.unwrap();
    let teams = Teams::default();
    for fid in join {
        teams.join(&auth, &profiles, auth.epoch(), fid).await.unwrap();
    }
    (server, auth, profiles, teams)
}

fn perm(email: &str, role: &str) -> Value {
    json!({"id":format!("p-{}", email.split('@').next().unwrap()),"type":"user","role":role,"emailAddress":email,"displayName":email.split('@').next().unwrap()})
}

#[tokio::test]
async fn owner_publishes_roster_and_sends_only_to_current_members_without_notification() {
    let root = Temp::new();
    let world = Arc::new(World::default());
    world.folder(
        "team1",
        true,
        ME,
        vec![perm(ME, "owner"), perm("b@example.com", "writer"), perm("c@example.com", "reader"), json!({"id":"anyone","type":"anyone","role":"reader"})],
    );
    let (_s, auth, profiles, teams) = fixture(world.clone(), &root, &["team1"]).await;
    let epoch = auth.epoch();

    let t = targets(&auth, &profiles, &teams, epoch).await.unwrap();
    let emails: Vec<_> = t["teams"][0]["members"].as_array().unwrap().iter().map(|m| m.s("email")).collect();
    assert_eq!(emails, ["b@example.com", "c@example.com"]);
    let roster = world.mine_where(|m| m["properties"]["saiRoster"] == "1");
    assert_eq!(roster.len(), 1);
    assert_eq!(roster[0].0["parents"], json!(["team1"]));
    assert_eq!(roster[0].1["members"].as_array().unwrap().len(), 3, "public link access is not a member");

    let sent = send(&auth, &profiles, &teams, epoch, "team1", "B@example.com", "자료를 하루 전에 공유해 줄 수 있을까요?").await.unwrap();
    assert_eq!(sent["to"], "b@example.com");
    assert_eq!(sent["toName"], "b");
    let messages = world.mine_where(|m| m["properties"]["saiMessage"] == "1");
    assert_eq!(messages.len(), 1);
    let (meta, content, perms) = &messages[0];
    assert_eq!(content["type"], "request");
    assert_eq!(content["team"], "team1");
    assert_eq!(perms.len(), 1, "shared with exactly one person");
    let outboxes = world.mine_where(|m| m["properties"]["saiOutbox"] == "1");
    assert_eq!(meta["parents"], json!([outboxes[0].0["id"]]));

    // The outbox folder is reused, strangers and I are refused, and nothing is created for them.
    send(&auth, &profiles, &teams, epoch, "team1", "c@example.com", "다음 회의 전에 진행 상황을 알려 줄래요?").await.unwrap();
    assert_eq!(world.mine_where(|m| m["properties"]["saiOutbox"] == "1").len(), 1);
    for to in ["x@example.com", ME, "not-an-email"] {
        assert!(send(&auth, &profiles, &teams, epoch, "team1", to, "부탁해요").await.is_err(), "{to}");
    }
    assert!(send(&auth, &profiles, &teams, epoch, "team1", "b@example.com", "").await.is_err());
    assert_eq!(world.mine_where(|m| m["properties"]["saiMessage"] == "1").len(), 2);

    // A removed member drops out of the roster the next time the owner's app refreshes it.
    world.files.lock().unwrap().get_mut("team1").unwrap().2.retain(|p| p["emailAddress"] != "c@example.com");
    refresh_roster(&auth, &profiles, &teams, epoch, "team1").await.unwrap();
    let roster = world.mine_where(|m| m["properties"]["saiRoster"] == "1");
    assert_eq!(roster.len(), 1, "updated in place");
    assert_eq!(roster[0].1["members"].as_array().unwrap().len(), 2);
    assert!(send(&auth, &profiles, &teams, epoch, "team1", "c@example.com", "부탁해요").await.is_err());

    // If sharing fails the message file is removed rather than left half-sent.
    world.fail_share.store(true, Ordering::SeqCst);
    assert!(send(&auth, &profiles, &teams, epoch, "team1", "b@example.com", "부탁해요").await.is_err());
    assert_eq!(world.mine_where(|m| m["properties"]["saiMessage"] == "1").len(), 2);
    assert_eq!(*world.shares.lock().unwrap(), ["b@example.com", "c@example.com"]);

    react(&auth, &profiles, &teams, epoch, "team1", "b@example.com", "abc", "ok").await.unwrap();
    assert!(react(&auth, &profiles, &teams, epoch, "team1", "b@example.com", "abc", "great").await.is_err());
    assert!(send(&auth, &profiles, &teams, epoch + 1, "team1", "b@example.com", "부탁해요").await.is_err());
}

#[tokio::test]
async fn member_receives_only_from_people_on_the_owners_roster() {
    let root = Temp::new();
    let world = Arc::new(World::default());
    let o = "o@example.com";
    world.folder("team1", false, o, vec![]);
    world.folder("team2", false, o, vec![]);
    let roster = |members: &[&str]| json!({"v":1,"team":"team1","members":members.iter().map(|e| json!({"email":e,"name":"","role":"writer"})).collect::<Vec<_>>()});
    let roster_meta = |by: &str, edited: &str| json!({"name":ROSTER,"mimeType":"application/json","parents":["team1"],"properties":{"saiRoster":"1"},"ownedByMe":false,"owners":owner(by),"lastModifyingUser":owner(edited)[0]});
    // A member planted a file named like the roster, and an editor rewrote a copy of the owner's roster.
    // Only a roster the folder owner made and last changed counts.
    world.add("r0-forged", roster_meta("z@example.com", "z@example.com"), roster(&["z@example.com"]), vec![]);
    world.add("r1-edited", roster_meta(o, "z@example.com"), roster(&["z@example.com"]), vec![]);
    world.add("r2-roster", roster_meta(o, o), roster(&[ME, "b@example.com"]), vec![]);
    let shared = |from: &str, content: Value| (json!({"name":MESSAGE,"mimeType":"application/json","properties":{"saiMessage":"1"},"ownedByMe":false,"owners":owner(from)}), content);
    let req = |team: &str, id: &str| json!({"v":1,"type":"request","id":id,"team":team,"text":"자료를 미리 공유해 줄래요?","createdAt":1});
    for (fid, (meta, content)) in [
        ("m1", shared("b@example.com", req("team1", "q1"))),
        ("m2", shared("z@example.com", req("team1", "q2"))),
        ("m3", shared("b@example.com", req("elsewhere", "q3"))),
        ("m4", shared("b@example.com", json!({"v":1,"type":"spam","id":"q4","team":"team1"}))),
        ("m5", shared("b@example.com", json!({"v":1,"type":"reaction","id":"q5","re":"s1","team":"team1","reaction":"ok"}))),
        ("m6", shared("b@example.com", req("team2", "q6"))),
    ] {
        world.add(fid, meta, content, vec![]);
    }
    let (_s, auth, profiles, teams) = fixture(world.clone(), &root, &["team1", "team2"]).await;
    let epoch = auth.epoch();

    let got = fetch(&auth, &profiles, &teams, epoch, &[]).await.unwrap();
    let items: Vec<_> = got["items"].as_array().unwrap().iter().map(|i| i.s("fileId")).collect();
    assert_eq!(items, ["m1", "m5"]);
    assert_eq!(got["rejected"], json!(["m2", "m3", "m4"]));
    let first = &got["items"][0];
    assert_eq!((first.s("from"), first.s("fromName"), first.s("teamName")), ("b@example.com".into(), "b".into(), "team1".into()));

    // m6's team has no roster yet: it is neither accepted nor rejected, so it is checked again later.
    let seen: Vec<String> = ["m1", "m2", "m3", "m4", "m5"].map(String::from).to_vec();
    let again = fetch(&auth, &profiles, &teams, epoch, &seen).await.unwrap();
    assert_eq!(again, json!({"items":[],"rejected":[]}));

    // Members see the owner as a possible recipient even if the owner's own entry is missing.
    let t = targets(&auth, &profiles, &teams, epoch).await.unwrap();
    let team1 = t["teams"].as_array().unwrap().iter().find(|r| r["folderId"] == "team1").unwrap();
    let emails: Vec<_> = team1["members"].as_array().unwrap().iter().map(|m| m.s("email")).collect();
    assert_eq!(emails, ["b@example.com", o]);
    let team2 = t["teams"].as_array().unwrap().iter().find(|r| r["folderId"] == "team2").unwrap();
    assert_eq!(team2["rosterMissing"], true);
    assert!(send(&auth, &profiles, &teams, epoch, "team2", o, "부탁해요").await.is_err());
}

#[test]
fn incoming_trusts_drive_owner_not_file_content() {
    let meta = json!({"id":"f","mimeType":"application/json","ownedByMe":false,"trashed":false,"properties":{"saiMessage":"1"},"owners":[{"emailAddress":"B@Example.com","displayName":"비"}]});
    let content = json!({"v":1,"type":"request","id":"q","team":"t","text":"부탁해요","from":"ceo@example.com","fromName":"대표"});
    let item = incoming(&meta, &content).unwrap();
    assert_eq!((item.s("from"), item.s("fromName")), ("b@example.com".into(), "비".into()));
    let mut two = meta.clone();
    two["owners"] = json!([{"emailAddress":"b@example.com"},{"emailAddress":"c@example.com"}]);
    assert!(incoming(&two, &content).is_none());
    let mut mine = meta.clone();
    mine["ownedByMe"] = json!(true);
    assert!(incoming(&mine, &content).is_none());
    for bad in [
        json!({"v":2,"type":"request","id":"q","team":"t","text":"부탁"}),
        json!({"v":1,"type":"request","id":"q/../x","team":"t","text":"부탁"}),
        json!({"v":1,"type":"request","id":"q","team":"t","text":"가".repeat(401)}),
        json!({"v":1,"type":"reaction","id":"q","team":"t","re":"s","reaction":"love"}),
    ] {
        assert!(incoming(&meta, &bad).is_none(), "{bad}");
    }
}
