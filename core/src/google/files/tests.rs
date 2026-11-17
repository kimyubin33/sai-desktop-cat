use super::*;
use crate::{
    google::{
        oauth::{
            self,
            tests::{auth, token},
        },
        profile::model::TeamRef,
    },
    store::Store,
};
use sha2::Digest;
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Mutex,
    },
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("sai-files-test-{}", oauth::random_token()));
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
            .starts_with("sai-files-test-"));
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
#[derive(Default)]
struct Api {
    files: Mutex<BTreeMap<String, (Value, Vec<u8>)>>,
    ids: AtomicUsize,
    posts: AtomicUsize,
    downloads: AtomicUsize,
    ambiguous: AtomicBool,
    revoked: AtomicBool,
    read_only: AtomicBool,
    changed: AtomicBool,
    mode: AtomicUsize,
    second_account: AtomicBool,
    copies: AtomicUsize,
    hide_copy_results: AtomicBool,
    copy_denied: AtomicBool,
    ai_calls: AtomicUsize,
    ai_fail: AtomicBool,
    ai_changed: AtomicBool,
    ai_revoked: AtomicBool,
    calendar_events: Mutex<BTreeMap<String, Value>>,
    calendar_posts: AtomicUsize,
    calendar_ambiguous: AtomicBool,
    calendar_denied: AtomicBool,
    calendar_revoke_before_write: AtomicBool,
    calendar_changes: AtomicUsize,
    calendar_deletes: AtomicUsize,
    calendar_attempts: AtomicUsize,
    calendar_revision: AtomicUsize,
    calendar_race: AtomicBool,
    calendar_change_ambiguous: AtomicBool,
    calendar_hide_after_change: AtomicBool,
    calendar_reads_fail: AtomicBool,
    profile_list_fail: AtomicBool,
}
struct Reply {
    status: u16,
    body: Vec<u8>,
    headers: String,
    length: Option<usize>,
}
impl Reply {
    fn json(status: u16, value: Value) -> Self {
        Self {
            status,
            body: value.to_string().into_bytes(),
            headers: "Content-Type: application/json\r\n".into(),
            length: None,
        }
    }
}
impl Api {
    fn add(&self, id: &str, folder: &str, bytes: &[u8]) -> Value {
        let meta = json!({"id":id,"name":"example.bin","mimeType":"application/octet-stream","size":bytes.len().to_string(),"parents":[folder],"trashed":false,"version":"1","sha256Checksum":digest(bytes),"capabilities":{"canDownload":true,"canCopy":true}});
        self.files
            .lock()
            .unwrap()
            .insert(id.into(), (meta.clone(), bytes.to_vec()));
        meta
    }
    fn respond(&self, method: &str, path: &str, body: &[u8]) -> Reply {
        self.respond_headers(method, path, body, "")
    }
    fn respond_headers(&self, method: &str, path: &str, body: &[u8], headers: &str) -> Reply {
        if path == "/token" {
            let form = String::from_utf8_lossy(body);
            if form.contains("code=calendar")
                || form.contains("refresh_token=CAL_REFRESH_TEST_ONLY")
            {
                let mut t = token("attempt");
                t["scope"] = json!(format!("openid email {}", oauth::CALENDAR_SCOPE));
                t["access_token"] = json!("CAL_ACCESS_TEST_ONLY");
                t["refresh_token"] = json!("CAL_REFRESH_TEST_ONLY");
                return Reply::json(200, t);
            }
            let mut t = token("attempt");
            t["scope"] = json!(format!(
                "{} {}",
                oauth::SCOPES.join(" "),
                oauth::DRIVE_SCOPE
            ));
            if self.second_account.load(Ordering::SeqCst) {
                let now = crate::time::now_ms() / 1000;
                t["id_token"] = json!(oauth::tests::jwt(
                    json!({"iss":"https://accounts.google.com","aud":"test-client","sub":"google-sub-b","nonce":"attempt","iat":now-1,"exp":now+3600})
                ));
            }
            return Reply::json(200, t);
        }
        if path == "/userinfo" {
            return Reply::json(
                200,
                json!({"sub":if self.second_account.load(Ordering::SeqCst){"google-sub-b"}else{"google-sub-a"},"email":"user@example.invalid","email_verified":true}),
            );
        }
        if path.starts_with("/calendar/v3/calendars/primary/events") {
            if self.calendar_denied.load(Ordering::SeqCst) {
                return Reply::json(403, json!({"error":"private upstream information"}));
            }
            if method == "GET" {
                if self.calendar_reads_fail.load(Ordering::SeqCst) {
                    return Reply::json(503, json!({}));
                }
                let id = path.rsplit('/').next().unwrap();
                if let Some(v) = self.calendar_events.lock().unwrap().get(id).cloned() {
                    return Reply::json(200, v);
                }
                if self.calendar_revoke_before_write.load(Ordering::SeqCst) {
                    self.revoked.store(true, Ordering::SeqCst);
                }
                return Reply::json(404, json!({}));
            }
            if ["PATCH", "DELETE"].contains(&method) {
                self.calendar_attempts.fetch_add(1, Ordering::SeqCst);
                assert!(path.ends_with("?sendUpdates=none"));
                let revision = headers
                    .lines()
                    .filter_map(|s| s.split_once(':'))
                    .find(|(k, _)| k.eq_ignore_ascii_case("if-match"))
                    .map(|(_, v)| v.trim())
                    .expect("every mutation must send If-Match");
                let id = path.split('?').next().unwrap().rsplit('/').next().unwrap();
                let mut events = self.calendar_events.lock().unwrap();
                let Some(current) = events.get_mut(id) else {
                    return Reply::json(404, json!({}));
                };
                if self.calendar_race.swap(false, Ordering::SeqCst) {
                    current["etag"] = json!("\"external-race\"");
                    current["summary"] = json!("Changed during request");
                }
                if current["etag"] != revision {
                    return Reply::json(412, json!({}));
                }
                let new_revision = format!(
                    "\"edit-{}\"",
                    self.calendar_revision.fetch_add(1, Ordering::SeqCst)
                );
                if method == "DELETE" {
                    assert!(body.is_empty());
                    *current = json!({"id":id,"status":"cancelled","etag":new_revision});
                    self.calendar_deletes.fetch_add(1, Ordering::SeqCst);
                } else {
                    let patch: Value = serde_json::from_slice(body).unwrap();
                    assert_eq!(patch.as_object().unwrap().len(), 4);
                    for k in ["summary", "start", "end", "extendedProperties"] {
                        current[k] = patch[k].clone();
                    }
                    current["etag"] = json!(new_revision);
                    self.calendar_changes.fetch_add(1, Ordering::SeqCst);
                }
                if self
                    .calendar_hide_after_change
                    .swap(false, Ordering::SeqCst)
                {
                    self.calendar_reads_fail.store(true, Ordering::SeqCst);
                    return Reply::json(503, json!({}));
                }
                if self.calendar_change_ambiguous.swap(false, Ordering::SeqCst) {
                    return Reply::json(503, json!({}));
                }
                return if method == "DELETE" {
                    Reply {
                        status: 204,
                        body: vec![],
                        headers: String::new(),
                        length: Some(0),
                    }
                } else {
                    Reply::json(200, current.clone())
                };
            }
            assert_eq!(method, "POST");
            assert!(path.ends_with("?sendUpdates=none"));
            let mut v: Value = serde_json::from_slice(body).unwrap();
            assert!(v.get("attendees").is_none());
            assert!(v.get("description").is_none());
            assert_eq!(v["visibility"], "private");
            assert_eq!(v["transparency"], "transparent");
            assert_eq!(v["reminders"]["useDefault"], false);
            let id = v["id"].as_str().unwrap().to_owned();
            assert!(id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'v').contains(&b)));
            let mut events = self.calendar_events.lock().unwrap();
            if events.contains_key(&id) {
                return Reply::json(409, json!({}));
            }
            v["status"] = json!("confirmed");
            v["etag"] = json!(format!(
                "\"event-{}\"",
                self.calendar_revision.fetch_add(1, Ordering::SeqCst)
            ));
            events.insert(id, v.clone());
            self.calendar_posts.fetch_add(1, Ordering::SeqCst);
            if self.calendar_ambiguous.swap(false, Ordering::SeqCst) {
                return Reply::json(503, json!({}));
            }
            return Reply::json(200, v);
        }
        if path == "/v1/status" {
            return Reply::json(
                200,
                json!({"ready":true,"provider":"claude","owner":false,"dailyRequests":20,"used":self.ai_calls.load(Ordering::SeqCst)}),
            );
        }
        if path == "/v1/analyze" {
            self.ai_calls.fetch_add(1, Ordering::SeqCst);
            let body: Value = serde_json::from_slice(body).unwrap();
            assert!(body.get("apiKey").is_none());
            assert!(body.get("refreshToken").is_none());
            assert_eq!(body["folderId"], "team-folder");
            if self.ai_fail.load(Ordering::SeqCst) {
                return Reply::json(503, json!({"error":"analysis_result_unknown"}));
            }
            if self.ai_changed.load(Ordering::SeqCst) {
                if let Some((m, _)) = self.files.lock().unwrap().get_mut("source-file") {
                    m["version"] = json!("2");
                }
            }
            if self.ai_revoked.load(Ordering::SeqCst) {
                self.revoked.store(true, Ordering::SeqCst);
            }
            return Reply::json(
                200,
                json!({"requestId":body["requestId"],"summary":"A request to review the brief","candidates":[{"title":"Review brief","assignee":"","dueDate":"","evidence":"Review the brief","uncertain":true},{"title":"Share notes","assignee":"","dueDate":"2026-11-20","evidence":"Share notes","uncertain":false}]}),
            );
        }
        if path.starts_with("/drive/v3/files?spaces=appDataFolder") {
            if self.profile_list_fail.load(Ordering::SeqCst) {
                return Reply::json(503, json!({}));
            }
            let subject = if self.second_account.load(Ordering::SeqCst) {
                "google-sub-b"
            } else {
                "google-sub-a"
            };
            return Reply::json(
                200,
                json!({"files":self.files.lock().unwrap().values().filter(|(m,b)|m["parents"]==json!(["appDataFolder"])&&m["appProperties"]["saiProfile"]=="1"&&serde_json::from_slice::<Value>(b).is_ok_and(|v|v["subject"]==subject)).map(|(m,_)|m.clone()).collect::<Vec<_>>() }),
            );
        }
        if path.starts_with("/drive/v3/files/generateIds?") {
            return Reply::json(
                200,
                json!({"ids":[format!("new-file-{}",self.ids.fetch_add(1,Ordering::SeqCst))]}),
            );
        }
        if path.starts_with("/drive/v3/files/team-folder?") {
            return if self.revoked.load(Ordering::SeqCst) {
                Reply::json(404, json!({}))
            } else {
                Reply::json(
                    200,
                    json!({"id":"team-folder","name":"Team","mimeType":"application/vnd.google-apps.folder","trashed":false,"properties":{"saiTeam":"1"},"capabilities":{"canListChildren":true,"canAddChildren":!self.read_only.load(Ordering::SeqCst)}}),
                )
            };
        }
        if path.starts_with("/drive/v3/files?spaces=drive") {
            let parsed = url::Url::parse(&format!("http://mock{path}")).unwrap();
            let params: BTreeMap<_, _> = parsed.query_pairs().into_owned().collect();
            let q = &params["q"];
            if q.contains("saiImport") {
                assert_eq!(params["pageSize"], "2");
                let rows = self
                    .files
                    .lock()
                    .unwrap()
                    .values()
                    .filter(|(m, _)| {
                        !self.hide_copy_results.load(Ordering::SeqCst)
                            && m["appProperties"]["saiImport"]
                                .as_str()
                                .is_some_and(|op| q.contains(op))
                    })
                    .map(|(m, _)| m.clone())
                    .collect::<Vec<_>>();
                return Reply::json(200, json!({"files":rows}));
            }
            if q.contains("saiCollab") && !q.contains("not properties") {
                assert!(path.contains("pageSize=50"));
                return Reply::json(
                    200,
                    json!({"files":self.files.lock().unwrap().values().filter(|(m,_)|m["properties"]["saiCollab"]=="1").map(|(m,_)|m.clone()).collect::<Vec<_>>() }),
                );
            }
            assert!(path.contains("pageSize=25"));
            if self.mode.load(Ordering::SeqCst) == 4 {
                return Reply::json(200, json!({"incompleteSearch":true,"files":[]}));
            }
            return Reply::json(
                200,
                json!({"files":self.files.lock().unwrap().values().filter(|(m,_)|m["parents"]!=json!(["appDataFolder"])&&(!q.contains("not properties")||m["properties"]["saiCollab"]!="1")&&(if q.starts_with("'team-folder'"){m["parents"]==json!(["team-folder"])}else{m["parents"]!=json!(["team-folder"])})).map(|(m,_)|m.clone()).collect::<Vec<_>>()}),
            );
        }
        if method == "POST" && path.contains("/copy?") {
            assert!(path.contains("supportsAllDrives=true"));
            self.copies.fetch_add(1, Ordering::SeqCst);
            if self.copy_denied.load(Ordering::SeqCst) {
                return Reply::json(403, json!({}));
            }
            let source = path
                .strip_prefix("/drive/v3/files/")
                .unwrap()
                .split('/')
                .next()
                .unwrap();
            let mut files = self.files.lock().unwrap();
            let Some((mut meta, bytes)) = files.get(source).cloned() else {
                return Reply::json(404, json!({}));
            };
            let body: Value = serde_json::from_slice(body).unwrap();
            let native = meta["mimeType"]
                .as_str()
                .unwrap()
                .starts_with("application/vnd.google-apps.");
            assert_eq!(body.get("id").is_none(), native);
            let fid = body["id"].as_str().map(str::to_owned).unwrap_or_else(|| {
                format!("native-copy-{}", self.ids.fetch_add(1, Ordering::SeqCst))
            });
            if files.contains_key(&fid) {
                return Reply::json(409, json!({}));
            }
            meta["id"] = json!(fid);
            meta["name"] = body["name"].clone();
            meta["parents"] = body["parents"].clone();
            meta["appProperties"] = body["appProperties"].clone();
            files.insert(fid, (meta.clone(), bytes));
            return if self.ambiguous.swap(false, Ordering::SeqCst) {
                Reply::json(503, json!({}))
            } else {
                Reply::json(200, meta)
            };
        }
        if method == "POST" && path.starts_with("/upload/drive/v3/files?") {
            let sep = b"\r\n\r\n";
            let breaks: Vec<_> = body
                .windows(4)
                .enumerate()
                .filter(|(_, p)| *p == sep)
                .map(|(i, _)| i)
                .collect();
            let meta_start = breaks[0] + 4;
            let meta_end = body[meta_start..]
                .windows(4)
                .position(|p| p == b"\r\n--")
                .unwrap()
                + meta_start;
            let mut meta: Value = serde_json::from_slice(&body[meta_start..meta_end]).unwrap();
            let boundary_len = body.windows(2).position(|p| p == b"\r\n").unwrap();
            let data = body[breaks[1] + 4..body.len() - boundary_len - 6].to_vec();
            let fid = meta["id"].as_str().unwrap().to_owned();
            let mut files = self.files.lock().unwrap();
            if files.contains_key(&fid) {
                return Reply::json(409, json!({}));
            }
            meta["size"] = json!(data.len().to_string());
            meta["sha256Checksum"] = json!(digest(&data));
            meta["version"] = json!("1");
            meta["trashed"] = json!(false);
            meta["capabilities"] = json!({"canDownload":true});
            files.insert(fid, (meta.clone(), data));
            self.posts.fetch_add(1, Ordering::SeqCst);
            return if self.ambiguous.swap(false, Ordering::SeqCst) {
                Reply::json(503, json!({"error":"lost after commit"}))
            } else {
                Reply::json(200, meta)
            };
        }
        if let Some(rest) = path.strip_prefix("/drive/v3/files/") {
            if rest.contains("/export?mimeType=text%2Fplain") {
                let fid = rest.split('/').next().unwrap();
                let files = self.files.lock().unwrap();
                let (_, bytes) = files.get(fid).unwrap();
                return Reply {
                    status: 200,
                    body: bytes.clone(),
                    headers: "Content-Type: text/plain\r\n".into(),
                    length: None,
                };
            }
            let fid = rest.split('?').next().unwrap();
            if path.ends_with("?alt=media") {
                self.downloads.fetch_add(1, Ordering::SeqCst);
                let mut files = self.files.lock().unwrap();
                let Some((meta, data)) = files.get_mut(fid) else {
                    return Reply::json(404, json!({}));
                };
                if self.changed.swap(false, Ordering::SeqCst) {
                    meta["version"] = json!("2");
                }
                return match self.mode.load(Ordering::SeqCst) {
                    1 => Reply {
                        status: 200,
                        body: vec![1],
                        headers: String::new(),
                        length: Some(MAX_BYTES + 1),
                    },
                    2 => Reply {
                        status: 200,
                        body: vec![1],
                        headers: String::new(),
                        length: Some(100),
                    },
                    3 => Reply {
                        status: 302,
                        body: vec![],
                        headers: "Location: https://evil.invalid/steal\r\n".into(),
                        length: None,
                    },
                    _ => Reply {
                        status: 200,
                        body: data.clone(),
                        headers: "Content-Type: application/octet-stream\r\n".into(),
                        length: None,
                    },
                };
            }
            return self
                .files
                .lock()
                .unwrap()
                .get(fid)
                .map(|(m, _)| Reply::json(200, m.clone()))
                .unwrap_or_else(|| Reply::json(404, json!({})));
        }
        panic!("Unexpected mock request {method} {path}");
    }
}
#[path = "../agenda/tests.rs"]
mod calendar_workflow;
#[path = "../review/tests.rs"]
mod review_workflow;
#[path = "../workspace_tests.rs"]
mod workspace_workflow;
struct Server {
    url: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn server(handler: impl Fn(&str, &str, &[u8]) -> Reply + Send + Sync + 'static) -> Server {
    server_headers(move |m, p, b, _| handler(m, p, b)).await
}
async fn server_headers(
    handler: impl Fn(&str, &str, &[u8], &str) -> Reply + Send + Sync + 'static,
) -> Server {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut raw = vec![];
            let mut chunk = [0u8; 8192];
            let header_end = loop {
                let n = socket.read(&mut chunk).await.unwrap();
                if n == 0 {
                    break None;
                }
                raw.extend_from_slice(&chunk[..n]);
                if let Some(end) = raw.windows(4).position(|p| p == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&raw[..end]);
                    let len = headers
                        .lines()
                        .filter_map(|s| s.split_once(':'))
                        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                        .map(|(_, v)| v.trim().parse::<usize>().unwrap())
                        .unwrap_or(0);
                    if raw.len() >= end + 4 + len {
                        break Some(end);
                    }
                }
            };
            let Some(end) = header_end else { continue };
            let head = String::from_utf8_lossy(&raw[..end]);
            let first: Vec<_> = head.lines().next().unwrap().split_whitespace().collect();
            let reply = handler(first[0], first[1], &raw[end + 4..], &head);
            let header = format!(
                "HTTP/1.1 {} Test\r\n{}Content-Length: {}\r\nConnection: close\r\n\r\n",
                reply.status,
                reply.headers,
                reply.length.unwrap_or(reply.body.len())
            );
            let _ = socket.write_all(header.as_bytes()).await;
            let _ = socket.write_all(&reply.body).await;
        }
    });
    Server { url, task }
}
struct Fixture {
    root: Temp,
    _server: Server,
    api: Arc<Api>,
    auth: Arc<GoogleAuth>,
    profiles: Arc<Profiles>,
    teams: Arc<Teams>,
    files: Arc<Files>,
}

#[tokio::test]
async fn collaboration_records_are_hidden_from_selection_and_cannot_be_copied_as_team_files() {
    let f = Fixture::new().await;
    f.api.add("internal", "root", b"journal");
    f.api.files.lock().unwrap().get_mut("internal").unwrap().0["properties"] =
        json!({"saiCollab":"1"});
    let listed = f
        .files
        .browse(&f.auth, f.auth.epoch(), "folder", "", "", "")
        .await
        .unwrap();
    assert_eq!(listed["entries"], json!([]));
    assert!(f.import(&["internal"]).await.is_err());
    assert_eq!(f.api.copies.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn source_browser_escapes_search_and_keeps_pagination_and_shared_queries_separate() {
    let mock = server(|method, path, _| {
        if path == "/token" {
            let mut t = token("attempt");
            t["scope"] = json!(format!(
                "{} {}",
                oauth::SCOPES.join(" "),
                oauth::DRIVE_SCOPE
            ));
            return Reply::json(200, t);
        }
        if path == "/userinfo" {
            return Reply::json(
                200,
                json!({"sub":"google-sub-a","email":"user@example.invalid","email_verified":true}),
            );
        }
        assert_eq!(method, "GET");
        let parsed = url::Url::parse(&format!("http://mock{path}")).unwrap();
        let p: BTreeMap<_, _> = parsed.query_pairs().into_owned().collect();
        let q = &p["q"];
        if p["pageToken"] == "page/a+b" {
            assert_eq!(q, "sharedWithMe = true and trashed = false");
        } else {
            assert_eq!(q, "name contains 'a\\'b\\\\c' and trashed = false");
        }
        Reply::json(200, json!({"files":[],"nextPageToken":"another-page"}))
    })
    .await;
    let client = auth(Store::memory(json!({})), &mock.url);
    client
        .exchange_code("base", "v", "http://127.0.0.1/", "attempt")
        .await
        .unwrap();
    let root = Temp::new();
    let files = Files::new(root.0.join("files"));
    assert_eq!(
        files
            .browse(&client, client.epoch(), "search", "", "a'b\\c", "")
            .await
            .unwrap()["nextPage"],
        "another-page"
    );
    files
        .browse(&client, client.epoch(), "shared", "", "", "page/a+b")
        .await
        .unwrap();
}

#[tokio::test]
async fn importing_from_personal_drive_creates_a_team_copy_and_keeps_the_original() {
    let f = Fixture::new().await;
    let original = f.api.add(
        "personal-source",
        "personal-folder",
        b"private source bytes",
    );
    let result = f.import(&["personal-source"]).await.unwrap();
    assert_eq!(result["added"], 1);
    let files = f.api.files.lock().unwrap();
    assert_eq!(files["personal-source"].0, original);
    assert_eq!(files.len(), 2);
    let (copy, data) = files
        .values()
        .find(|(m, _)| m["parents"] == json!(["team-folder"]))
        .unwrap();
    assert_ne!(copy["id"], original["id"]);
    assert_eq!(data, b"private source bytes");
    assert!(copy["appProperties"]["saiImport"].is_string());
    assert_eq!(f.api.copies.load(Ordering::SeqCst), 1);
    assert_eq!(f.files.pending("google-sub-a", None).unwrap(), json!([]));
}

#[tokio::test]
async fn an_existing_team_file_is_not_copied_and_invalid_selection_is_preflighted() {
    let f = Fixture::new().await;
    f.api.add("already-here", "team-folder", b"same");
    let value = f.import(&["already-here"]).await.unwrap();
    assert_eq!(value["existing"], 1);
    assert_eq!(value["added"], 0);
    f.api.add("personal", "root", b"copy");
    f.api.add("restricted", "root", b"restricted");
    f.api.files.lock().unwrap().get_mut("restricted").unwrap().0["capabilities"]["canCopy"] =
        json!(false);
    assert!(f.import(&["personal", "restricted"]).await.is_err());
    assert!(f.import(&["personal", "personal"]).await.is_err());
    assert_eq!(f.api.copies.load(Ordering::SeqCst), 0);
    assert!(f
        .files
        .pending("google-sub-a", None)
        .unwrap()
        .as_array()
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn binary_copy_recovers_by_reserved_id_after_a_lost_response_without_a_duplicate() {
    let f = Fixture::new().await;
    f.api.add("personal", "root", b"binary");
    f.api.ambiguous.store(true, Ordering::SeqCst);
    f.api.hide_copy_results.store(true, Ordering::SeqCst);
    assert!(f.import(&["personal"]).await.is_err());
    let pending = f
        .files
        .pending("google-sub-a", Some("team-folder"))
        .unwrap();
    assert_eq!(pending[0]["origin"], "drive");
    let reopened = Files::new(f.root.0.join("files"));
    reopened
        .retry_import(
            &f.auth,
            &f.profiles,
            &f.teams,
            f.auth.epoch(),
            "team-folder",
            pending[0]["operation"].as_str().unwrap(),
            silent(),
        )
        .await
        .unwrap();
    assert_eq!(f.api.copies.load(Ordering::SeqCst), 1);
    assert_eq!(reopened.pending("google-sub-a", None).unwrap(), json!([]));
}

#[tokio::test]
async fn native_document_copy_keeps_an_uncertain_journal_and_never_blindly_reposts() {
    let f = Fixture::new().await;
    f.api.add("document", "root", b"");
    {
        let mut files = f.api.files.lock().unwrap();
        let meta = &mut files.get_mut("document").unwrap().0;
        meta["mimeType"] = json!("application/vnd.google-apps.document");
        meta.as_object_mut().unwrap().remove("size");
        meta.as_object_mut().unwrap().remove("sha256Checksum");
    }
    f.api.ambiguous.store(true, Ordering::SeqCst);
    f.api.hide_copy_results.store(true, Ordering::SeqCst);
    assert!(f.import(&["document"]).await.is_err());
    let reopened = Files::new(f.root.0.join("files"));
    let pending = reopened.pending("google-sub-a", None).unwrap();
    assert_eq!(pending[0]["uncertain"], true);
    let op = pending[0]["operation"].as_str().unwrap();
    assert!(reopened
        .retry_import(
            &f.auth,
            &f.profiles,
            &f.teams,
            f.auth.epoch(),
            "team-folder",
            op,
            silent()
        )
        .await
        .is_err());
    assert_eq!(f.api.copies.load(Ordering::SeqCst), 1);
    f.api.hide_copy_results.store(false, Ordering::SeqCst);
    reopened
        .retry_import(
            &f.auth,
            &f.profiles,
            &f.teams,
            f.auth.epoch(),
            "team-folder",
            op,
            silent(),
        )
        .await
        .unwrap();
    assert_eq!(f.api.copies.load(Ordering::SeqCst), 1);
    assert_eq!(reopened.pending("google-sub-a", None).unwrap(), json!([]));
}

#[tokio::test]
async fn copy_denial_and_read_only_team_keep_sources_and_pending_records() {
    let f = Fixture::new().await;
    f.api.add("personal", "root", b"safe");
    f.api.read_only.store(true, Ordering::SeqCst);
    assert!(f.import(&["personal"]).await.is_err());
    assert_eq!(f.api.copies.load(Ordering::SeqCst), 0);
    f.api.read_only.store(false, Ordering::SeqCst);
    f.api.copy_denied.store(true, Ordering::SeqCst);
    assert!(f.import(&["personal"]).await.is_err());
    let pending = f.files.pending("google-sub-a", None).unwrap();
    assert_eq!(pending.as_array().unwrap().len(), 1);
    assert_eq!(f.api.files.lock().unwrap().len(), 1);
    f.api.copy_denied.store(false, Ordering::SeqCst);
    f.retry_import(pending[0]["operation"].as_str().unwrap())
        .await
        .unwrap();
    assert_eq!(f.api.files.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn source_browser_lists_personal_files_and_never_changes_them() {
    let f = Fixture::new().await;
    let original = f.api.add("personal", "root", b"bytes");
    f.api.add("team-file", "team-folder", b"other");
    let result = f
        .files
        .browse(&f.auth, f.auth.epoch(), "folder", "", "", "")
        .await
        .unwrap();
    assert_eq!(result["entries"].as_array().unwrap().len(), 1);
    assert_eq!(result["entries"][0]["id"], "personal");
    assert_eq!(result["entries"][0]["canCopy"], true);
    assert_eq!(f.api.files.lock().unwrap()["personal"].0, original);
    assert_eq!(f.api.copies.load(Ordering::SeqCst), 0);
    assert!(f
        .files
        .browse(&f.auth, f.auth.epoch() - 1, "folder", "", "", "")
        .await
        .is_err());
    assert!(f
        .files
        .browse(&f.auth, f.auth.epoch(), "folder", "invalid/id", "", "")
        .await
        .is_err());
    assert!(f
        .files
        .browse(&f.auth, f.auth.epoch(), "search", "", "\n", "")
        .await
        .is_err());
}

#[tokio::test]
async fn import_records_cannot_cross_accounts_or_teams_and_discard_keeps_cloud_files() {
    let f = Fixture::new().await;
    f.api.add("personal", "root", b"source");
    f.api.ambiguous.store(true, Ordering::SeqCst);
    assert!(f.import(&["personal"]).await.is_err());
    let pending = f.files.pending("google-sub-a", None).unwrap();
    let op = pending[0]["operation"].as_str().unwrap();
    assert_eq!(f.files.pending("google-sub-b", None).unwrap(), json!([]));
    assert!(f
        .files
        .discard_import(&f.auth, f.auth.epoch(), "other-team", op)
        .await
        .is_err());
    f.files
        .discard_import(&f.auth, f.auth.epoch(), "team-folder", op)
        .await
        .unwrap();
    assert_eq!(f.api.files.lock().unwrap().len(), 2);
    assert_eq!(f.files.pending("google-sub-a", None).unwrap(), json!([]));
}
impl Fixture {
    async fn import(&self, ids: &[&str]) -> Result<Value> {
        self.files
            .import(
                &self.auth,
                &self.profiles,
                &self.teams,
                self.auth.epoch(),
                "team-folder",
                &ids.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
                silent(),
            )
            .await
    }
    async fn retry_import(&self, op: &str) -> Result<Value> {
        self.files
            .retry_import(
                &self.auth,
                &self.profiles,
                &self.teams,
                self.auth.epoch(),
                "team-folder",
                op,
                silent(),
            )
            .await
    }
    async fn new() -> Self {
        let root = Temp::new();
        let api = Arc::new(Api::default());
        let remote = api.clone();
        let server = server_headers(move |m, p, b, h| remote.respond_headers(m, p, b, h)).await;
        Self::connect(root, api, server).await
    }
    async fn connect(root: Temp, api: Arc<Api>, server: Server) -> Self {
        let settings = Store::memory(json!({}));
        let auth = auth(settings.clone(), &server.url);
        auth.exchange_code("base", "v", "http://127.0.0.1/", "attempt")
            .await
            .unwrap();
        let profiles = Profiles::new(root.0.join("profile"));
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
        let files = Files::new(root.0.join("files"));
        Self {
            root,
            _server: server,
            api,
            auth: Arc::new(auth),
            profiles: Arc::new(profiles),
            teams: Arc::new(Teams::default()),
            files: Arc::new(files),
        }
    }
    async fn stage(&self, bytes: &[u8]) -> String {
        self.files
            .stage(
                &self.auth,
                &self.profiles,
                &self.teams,
                self.auth.epoch(),
                "team-folder",
                "my-file.bin",
                bytes,
            )
            .await
            .unwrap()
    }
    async fn retry(&self, op: &str) -> Result<Value> {
        self.files
            .retry(
                &self.auth,
                &self.profiles,
                &self.teams,
                self.auth.epoch(),
                op,
                silent(),
            )
            .await
    }
}
#[test]
fn filenames_cannot_select_a_directory_or_reserved_windows_device() {
    assert_eq!(safe_name("../../CON.txt"), ".._.._CON.txt");
    assert_eq!(safe_name("CON.txt"), "_CON.txt");
    assert_eq!(safe_name("LPT1"), "_LPT1");
    assert_eq!(safe_name("COM¹.txt"), "_COM¹.txt");
    assert_eq!(safe_name(" .. "), "download.bin");
    assert!(!safe_name("a/b\\c:*?.txt").contains('/'));
}

#[tokio::test]
async fn pending_actions_must_match_both_the_current_account_and_selected_team() {
    let f = Fixture::new().await;
    let operation = f.stage(b"team A copy").await;
    assert!(f
        .files
        .check_pending_target(&f.auth, f.auth.epoch(), "another-team", &operation)
        .await
        .is_err());
    f.files
        .check_pending_target(&f.auth, f.auth.epoch(), "team-folder", &operation)
        .await
        .unwrap();
    assert_eq!(
        f.files.outbox.pending("google-sub-a", None).unwrap().len(),
        1
    );
    assert_eq!(f.api.posts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn binary_upload_is_verified_and_keeps_the_same_contents_without_public_permissions() {
    let f = Fixture::new().await;
    let bytes = b"\0\xffbinary\r\n bytes";
    let operation = f.stage(bytes).await;
    let uploaded = f.retry(&operation).await.unwrap();
    let fid = uploaded["file"]["id"].as_str().unwrap();
    let files = f.api.files.lock().unwrap();
    assert_eq!(files[fid].1, bytes);
    assert!(files[fid].0.get("permissions").is_none());
    assert_eq!(f.api.posts.load(Ordering::SeqCst), 1);
    assert!(f
        .files
        .outbox
        .pending("google-sub-a", None)
        .unwrap()
        .is_empty());
}
#[tokio::test]
async fn lost_upload_response_and_restart_reuse_one_id_even_when_local_copy_is_missing() {
    let f = Fixture::new().await;
    let op = f.stage(b"original").await;
    f.api.ambiguous.store(true, Ordering::SeqCst);
    assert!(f.retry(&op).await.is_err());
    let stored = f.files.outbox.get("google-sub-a", &op).unwrap();
    let path = f
        .files
        .outbox
        .blob("google-sub-a", &stored.operation)
        .unwrap();
    std::fs::remove_file(path).unwrap();
    let reopened = Files::new(f.root.0.join("files"));
    reopened
        .retry(
            &f.auth,
            &f.profiles,
            &f.teams,
            f.auth.epoch(),
            &op,
            silent(),
        )
        .await
        .unwrap();
    assert_eq!(f.api.posts.load(Ordering::SeqCst), 1);
    assert_eq!(f.api.ids.load(Ordering::SeqCst), 1);
    assert!(reopened
        .outbox
        .pending("google-sub-a", None)
        .unwrap()
        .is_empty());
}
#[tokio::test]
async fn modified_staging_copy_and_provider_id_collision_are_never_overwritten() {
    let f = Fixture::new().await;
    let op = f.stage(b"original").await;
    let u = f.files.outbox.get("google-sub-a", &op).unwrap();
    let path = f.files.outbox.blob("google-sub-a", &u.operation).unwrap();
    std::fs::write(path, b"tampered").unwrap();
    assert!(f.retry(&op).await.is_err());
    assert_eq!(f.api.posts.load(Ordering::SeqCst), 0);
    let op2 = f.stage(b"new bytes").await;
    f.files
        .outbox
        .file_id("google-sub-a", &op2, "collision")
        .unwrap();
    f.api.add("collision", "team-folder", b"unrelated");
    assert!(f.retry(&op2).await.is_err());
    assert_eq!(f.api.files.lock().unwrap()["collision"].1, b"unrelated");
    assert_eq!(
        f.files.outbox.pending("google-sub-a", None).unwrap().len(),
        2
    );
}
#[tokio::test]
async fn revoked_or_read_only_team_cannot_upload_but_own_pending_copy_can_be_discarded() {
    let f = Fixture::new().await;
    let op = f.stage(b"original").await;
    f.api.revoked.store(true, Ordering::SeqCst);
    assert!(f.retry(&op).await.is_err());
    let view = f
        .files
        .list(
            &f.auth,
            &f.profiles,
            &f.teams,
            f.auth.epoch(),
            "team-folder",
            "",
        )
        .await
        .unwrap();
    assert_eq!(view["pending"].as_array().unwrap().len(), 1);
    assert!(view["error"].is_string());
    f.files.discard(&f.auth, f.auth.epoch(), &op).await.unwrap();
    assert_eq!(f.api.posts.load(Ordering::SeqCst), 0);
    f.api.revoked.store(false, Ordering::SeqCst);
    f.api.read_only.store(true, Ordering::SeqCst);
    assert!(f
        .files
        .stage(
            &f.auth,
            &f.profiles,
            &f.teams,
            f.auth.epoch(),
            "team-folder",
            "file",
            b"new"
        )
        .await
        .is_err());
}
#[tokio::test]
async fn successful_download_validates_hash_and_rejects_changed_or_truncated_content() {
    let f = Fixture::new().await;
    f.api.add("download", "team-folder", b"download bytes");
    let data = f
        .files
        .download(
            &f.auth,
            &f.profiles,
            &f.teams,
            f.auth.epoch(),
            "team-folder",
            "download",
            silent(),
        )
        .await
        .unwrap();
    assert_eq!(data.bytes, b"download bytes");
    f.api.changed.store(true, Ordering::SeqCst);
    assert!(f
        .files
        .download(
            &f.auth,
            &f.profiles,
            &f.teams,
            f.auth.epoch(),
            "team-folder",
            "download",
            silent()
        )
        .await
        .is_err());
    f.api.files.lock().unwrap().get_mut("download").unwrap().0["sha256Checksum"] =
        json!("0".repeat(64));
    assert!(f
        .files
        .download(
            &f.auth,
            &f.profiles,
            &f.teams,
            f.auth.epoch(),
            "team-folder",
            "download",
            silent()
        )
        .await
        .is_err());
}
#[tokio::test]
async fn oversized_truncated_and_redirected_media_are_rejected_without_following_redirects() {
    let f = Fixture::new().await;
    f.api.add("download", "team-folder", b"some data");
    for mode in [1, 2, 3] {
        f.api.mode.store(mode, Ordering::SeqCst);
        assert!(f
            .files
            .download(
                &f.auth,
                &f.profiles,
                &f.teams,
                f.auth.epoch(),
                "team-folder",
                "download",
                silent()
            )
            .await
            .is_err());
    }
    assert_eq!(f.api.downloads.load(Ordering::SeqCst), 3);
}
#[tokio::test]
async fn files_outside_the_team_shortcuts_and_google_documents_are_not_binary_downloads() {
    let f = Fixture::new().await;
    f.api.add("other", "elsewhere", b"private");
    assert!(f
        .files
        .info(
            &f.auth,
            &f.profiles,
            &f.teams,
            f.auth.epoch(),
            "team-folder",
            "other"
        )
        .await
        .is_err());
    let mut doc = f.api.add("doc", "team-folder", b"");
    doc["mimeType"] = json!("application/vnd.google-apps.document");
    doc.as_object_mut().unwrap().remove("size");
    doc.as_object_mut().unwrap().remove("sha256Checksum");
    f.api.files.lock().unwrap().get_mut("doc").unwrap().0 = doc;
    assert_eq!(
        f.files
            .info(
                &f.auth,
                &f.profiles,
                &f.teams,
                f.auth.epoch(),
                "team-folder",
                "doc"
            )
            .await
            .unwrap()["canDownload"],
        false
    );
    assert!(f
        .files
        .download(
            &f.auth,
            &f.profiles,
            &f.teams,
            f.auth.epoch(),
            "team-folder",
            "doc",
            silent()
        )
        .await
        .is_err());
    assert!(f
        .files
        .selected(
            &f.auth,
            &f.profiles,
            &f.teams,
            f.auth.epoch(),
            "team-folder",
            &["other".into()]
        )
        .await
        .is_err());
}
#[tokio::test]
async fn queue_is_bounded_and_empty_files_are_valid_and_never_uploaded_as_defaults() {
    let f = Fixture::new().await;
    for _ in 0..MAX_PENDING {
        f.stage(b"").await;
    }
    assert!(f
        .files
        .stage(
            &f.auth,
            &f.profiles,
            &f.teams,
            f.auth.epoch(),
            "team-folder",
            "extra",
            b"data"
        )
        .await
        .is_err());
    assert_eq!(f.api.posts.load(Ordering::SeqCst), 0);
    let op = f.files.outbox.pending("google-sub-a", None).unwrap()[0]
        .operation
        .clone();
    f.retry(&op).await.unwrap();
    assert_eq!(f.api.posts.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn corrupted_local_queue_is_read_only_and_incomplete_list_is_not_an_empty_folder() {
    let f = Fixture::new().await;
    let _ = f.stage(b"data").await;
    let dir = f
        .root
        .0
        .join("files")
        .join(format!("{:x}", sha2::Sha256::digest(b"google-sub-a")));
    let path = dir.join("uploads.json");
    std::fs::write(&path, b"{broken").unwrap();
    let reopened = Files::new(f.root.0.join("files"));
    assert!(reopened.outbox.pending("google-sub-a", None).is_err());
    assert_eq!(std::fs::read(path).unwrap(), b"{broken");
    f.api.mode.store(4, Ordering::SeqCst);
    let view = f
        .files
        .list(
            &f.auth,
            &f.profiles,
            &f.teams,
            f.auth.epoch(),
            "team-folder",
            "",
        )
        .await
        .unwrap();
    assert!(view["error"].is_string());
}

async fn paused_upload() -> (
    Fixture,
    tokio::sync::mpsc::UnboundedReceiver<()>,
    std::sync::mpsc::Sender<()>,
) {
    let api = Arc::new(Api::default());
    let remote = api.clone();
    let (entered, received) = tokio::sync::mpsc::unbounded_channel();
    let (release, wait) = std::sync::mpsc::channel();
    let wait = Mutex::new(wait);
    let server = server(move |m, p, b| {
        let result = remote.respond(m, p, b);
        if m == "POST" && p.starts_with("/upload/drive/v3/files?") {
            entered.send(()).unwrap();
            wait.lock()
                .unwrap()
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
        }
        result
    })
    .await;
    (
        Fixture::connect(Temp::new(), api, server).await,
        received,
        release,
    )
}
fn run_upload(f: &Fixture, operation: String) -> tokio::task::JoinHandle<Result<Value>> {
    let files = f.files.clone();
    let auth = f.auth.clone();
    let profiles = f.profiles.clone();
    let teams = f.teams.clone();
    let epoch = auth.epoch();
    tokio::spawn(async move {
        files
            .retry(&auth, &profiles, &teams, epoch, &operation, silent())
            .await
    })
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_request_keeps_its_copy_and_retry_confirms_the_provider_result_once() {
    let (f, mut entered, release) = paused_upload().await;
    let operation = f.stage(b"retry after cancel").await;
    let task = run_upload(&f, operation.clone());
    tokio::time::timeout(std::time::Duration::from_secs(3), entered.recv())
        .await
        .unwrap()
        .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    release.send(()).unwrap();
    assert_eq!(
        f.files.outbox.pending("google-sub-a", None).unwrap().len(),
        1
    );
    f.retry(&operation).await.unwrap();
    assert_eq!(f.api.posts.load(Ordering::SeqCst), 1);
    assert_eq!(f.api.ids.load(Ordering::SeqCst), 1);
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn logout_during_upload_preserves_a_queue_that_another_account_cannot_resume() {
    let (f, mut entered, release) = paused_upload().await;
    let operation = f.stage(b"private A copy").await;
    let task = run_upload(&f, operation.clone());
    tokio::time::timeout(std::time::Duration::from_secs(3), entered.recv())
        .await
        .unwrap()
        .unwrap();
    let old_epoch = f.auth.epoch();
    f.auth.sign_out().await.unwrap();
    release.send(()).unwrap();
    assert!(task.await.unwrap().is_err());
    assert_eq!(
        f.files.outbox.pending("google-sub-a", None).unwrap().len(),
        1
    );
    f.api.second_account.store(true, Ordering::SeqCst);
    f.auth
        .exchange_code("new", "v", "http://127.0.0.1/", "attempt")
        .await
        .unwrap();
    assert_ne!(old_epoch, f.auth.epoch());
    assert!(f.retry(&operation).await.is_err());
    let view = f
        .files
        .list(
            &f.auth,
            &f.profiles,
            &f.teams,
            f.auth.epoch(),
            "team-folder",
            "",
        )
        .await
        .unwrap();
    assert_eq!(view["pending"], json!([]));
    assert_eq!(f.api.posts.load(Ordering::SeqCst), 1);
}
