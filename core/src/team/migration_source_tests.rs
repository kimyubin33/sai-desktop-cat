use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const ROOT: &str = "projects/test/databases/(default)/documents";
const SERVER_ID: &str = "5608a7b4-f9da-4ad7-a98f-646be56ac567";
struct Reply {
    status: &'static str,
    body: Vec<u8>,
    length: Option<usize>,
}
impl Reply {
    fn json(value: Value) -> Self {
        let body = value.to_string().into_bytes();
        Self {
            status: "200 OK",
            length: Some(body.len()),
            body,
        }
    }
}
struct Server {
    backend: Backend,
    seen: Arc<Mutex<Vec<String>>>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Server {
    async fn start(reply: impl Fn(&str, usize) -> Reply + Send + 'static) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let requests = seen.clone();
        let task = tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                loop {
                    let mut buf = [0; 2048];
                    let n = socket.read(&mut buf).await.unwrap();
                    assert!(n > 0 && request.len() < 16384);
                    request.extend_from_slice(&buf[..n]);
                    if request.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                let request = String::from_utf8(request).unwrap();
                let lower = request.to_ascii_lowercase();
                assert!(request.starts_with("GET "));
                assert!(lower.contains("authorization: bearer fake-test-token\r\n"));
                assert!(lower.contains(&format!("x-sai-server-id: {SERVER_ID}\r\n")));
                let path = request.split_whitespace().nth(1).unwrap().to_owned();
                let count = {
                    let mut seen = requests.lock().unwrap();
                    seen.push(path.clone());
                    seen.len()
                };
                let response = reply(&path, count);
                let length = response
                    .length
                    .map(|n| format!("Content-Length: {n}\r\n"))
                    .unwrap_or_default();
                let header=format!("HTTP/1.1 {}\r\nContent-Type: application/json\r\n{length}Connection: close\r\n\r\n",response.status);
                let _ = socket.write_all(header.as_bytes()).await;
                let _ = socket.write_all(&response.body).await;
            }
        });
        let backend = Backend::Firebase(Firebase {
            api_key: "not-used".into(),
            auth_base: "http://invalid.test".into(),
            token_url: "http://invalid.test".into(),
            docs_base: format!("{address}/v1/{ROOT}"),
            desktop: true,
            server_id: Some(SERVER_ID.into()),
            session: Mutex::new(Some(("owner".into(), "not-used".into()))),
            id_token: Mutex::new(Some((
                "fake-test-token".into(),
                crate::time::now_ms() + 3_600_000,
            ))),
            auth_lock: Default::default(),
            save_session: Box::new(|_| panic!("test must not renew credentials")),
        });
        Self {
            backend,
            seen,
            task,
        }
    }
    fn firebase(&self) -> &Firebase {
        let Backend::Firebase(f) = &self.backend else {
            unreachable!()
        };
        f
    }
}
fn doc(path: &str, fields: Value) -> Value {
    json!({"name":format!("{ROOT}/{path}"),"fields":encode_fields(fields.as_object().unwrap())})
}
fn row(id: &str) -> Value {
    doc(
        &format!("teams/old/shares/{id}"),
        json!({"by":"writer","at":1000,"text":"public text"}),
    )
}
fn normal(path: &str) -> Reply {
    let relative = path.strip_prefix(&format!("/v1/{ROOT}/")).unwrap();
    match relative {
        "teams/old" => Reply::json(doc(relative, json!({"name":"Old team"}))),
        "teams/old/members/owner" => Reply::json(doc(relative, json!({"displayName":"Owner"}))),
        "teams/old/admins/owner" => Reply::json(doc(relative, json!({"key":"role-not-copied"}))),
        _ => panic!("unexpected source request: {relative}"),
    }
}
fn shares(path: &str) -> bool {
    path.contains("/teams/old/shares?")
}

#[tokio::test]
async fn transport_reads_all_pages_with_encoded_tokens_and_only_public_paths() {
    let server = Server::start(|path, _| {
        if shares(path) {
            if path.ends_with("?pageSize=100") {
                Reply::json(json!({"documents":[row("a")],"nextPageToken":"next&/+#"}))
            } else {
                assert!(path.ends_with("?pageSize=100&pageToken=next%26%2F%2B%23"));
                Reply::json(json!({"documents":[row("b")]}))
            }
        } else {
            normal(path)
        }
    })
    .await;
    let snapshot = server.backend.migration_snapshot("old").await.unwrap();
    assert_eq!(snapshot.scanned, 2);
    assert_eq!(snapshot.rows.len(), 2);
    assert_eq!(snapshot.source.reader_uid, "owner");
    let paths = server.seen.lock().unwrap();
    assert_eq!(paths.len(), 7);
    assert!(paths.iter().all(|p| !p.contains("signals")
        && !p.contains("integrations")
        && !p.contains("private")
        && !p.contains("telegram")
        && !p.contains("adminKeys")));
    assert!(!serde_json::to_string(&snapshot.rows)
        .unwrap()
        .contains("role-not-copied"));
}

#[tokio::test]
async fn repeated_pages_duplicate_ids_and_foreign_document_paths_fail_closed() {
    for case in 0..6 {
        let server=Server::start(move |path,_|if shares(path){Reply::json(match case {
            0=>json!({"documents":[row("a")],"nextPageToken":"repeat"}),
            1=>json!({"documents":[row("a"),row("a")]}),
            2=>json!({"documents":[doc("teams/other/shares/a",json!({"by":"writer","at":1,"text":"wrong team"}))]}),
            3=>json!({"documents":[doc("teams/old/signals/a/private/body",json!({"text":"not public"}))]}),
            4=>json!({"documents":{}}),
            _=>json!({"documents":[{"name":format!("{ROOT}/teams/old/shares/a"),"fields":[]}]}),
        })}else{normal(path)}).await;
        assert!(
            server.backend.migration_snapshot("old").await.is_err(),
            "case {case}"
        );
        assert!(server.seen.lock().unwrap().len() <= 6);
    }
}

#[tokio::test]
async fn source_role_is_rechecked_after_pagination_and_must_match_the_requested_path() {
    for case in 0..2 {
        let server = Server::start(move |path, count| {
            if shares(path) {
                Reply::json(json!({"documents":[row("a")]}))
            } else if path.ends_with("/admins/owner") && (case == 1 || count > 4) {
                if case == 1 {
                    Reply::json(doc("teams/other/admins/owner", json!({"key":"other-team"})))
                } else {
                    Reply {
                        status: "404 Not Found",
                        body: Vec::new(),
                        length: Some(0),
                    }
                }
            } else {
                normal(path)
            }
        })
        .await;
        assert!(server.backend.migration_snapshot("old").await.is_err());
        if case == 1 {
            assert_eq!(server.seen.lock().unwrap().len(), 2);
        }
    }
}

#[tokio::test]
async fn oversized_advertised_and_streamed_bodies_are_rejected_without_error_body_leaks() {
    for case in 0..3 {
        let server = Server::start(move |_, _| match case {
            0 => Reply {
                status: "200 OK",
                length: Some(2 * 1024 * 1024 + 1),
                body: Vec::new(),
            },
            1 => Reply {
                status: "200 OK",
                length: None,
                body: vec![b' '; 2 * 1024 * 1024 + 1],
            },
            _ => Reply {
                status: "403 Forbidden",
                length: None,
                body: b"remote-sensitive-details-do-not-display".to_vec(),
            },
        })
        .await;
        let error = server
            .firebase()
            .migration_get("/teams/old", "")
            .await
            .unwrap_err();
        assert!(!error.contains("remote-sensitive"));
        assert!(!error.contains("fake-test-token"));
    }
}

#[tokio::test]
async fn row_byte_and_page_caps_abort_the_fresh_read() {
    for case in 0..3 {
        let server=Server::start(move |path,count|if shares(path){Reply::json(match case {
            0=>json!({"documents":(0..1001).map(|n|row(&format!("m{n}"))).collect::<Vec<_>>()}),
            1=>json!({"documents":(0..100).map(|n|doc(&format!("teams/old/shares/m{count}_{n}"),json!({"by":"writer","at":1,"text":"x".repeat(15_000)}))).collect::<Vec<_>>(),"nextPageToken":format!("page{count}")}),
            _=>json!({"documents":[],"nextPageToken":format!("page{count}")}),
        })}else{normal(path)}).await;
        assert!(
            server.backend.migration_snapshot("old").await.is_err(),
            "case {case}"
        );
        assert!(server.seen.lock().unwrap().len() <= 19);
    }
}

#[tokio::test]
async fn malformed_json_or_redirect_is_not_retried_as_cached_data() {
    for status in ["200 OK", "302 Found"] {
        let server = Server::start(move |_, _| Reply {
            status,
            body: b"not-json".to_vec(),
            length: Some(8),
        })
        .await;
        assert!(server.backend.migration_snapshot("old").await.is_err());
        assert_eq!(server.seen.lock().unwrap().len(), 1);
    }
}
