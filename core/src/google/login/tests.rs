use super::*;
#[tokio::test]
async fn calendar_loopback_consent_binds_nonce_and_reuses_the_grant_without_reopening() {
    use crate::google::oauth::tests::{auth, server, token};
    let nonce = Arc::new(Mutex::new(String::new()));
    let captured = nonce.clone();
    let remote = server(move |r| {
        if r.starts_with("POST /token") {
            let cal = r.contains("code=calendar");
            let current_nonce = if cal {
                captured.lock().unwrap().clone()
            } else {
                "attempt".into()
            };
            let mut t = token(&current_nonce);
            t["scope"] = json!(if cal {
                format!("openid email {}", oauth::CALENDAR_SCOPE)
            } else {
                format!("{} {}", oauth::SCOPES.join(" "), oauth::DRIVE_SCOPE)
            });
            return (200, t);
        }
        (
            200,
            json!({"sub":"google-sub-a","email":"a@example.invalid","email_verified":true}),
        )
    })
    .await;
    let auth = Arc::new(auth(Store::memory(json!({})), &remote.url));
    auth.exchange_code("base", "v", "http://127.0.0.1/", "attempt")
        .await
        .unwrap();
    let epoch = auth.epoch();
    let login = Arc::new(Login::default());
    let (send, mut urls) = mpsc::unbounded_channel();
    let task = {
        let l = login.clone();
        let a = auth.clone();
        tokio::spawn(async move {
            l.run_for(
                a,
                Arc::new(move |u| {
                    send.send(u.to_owned()).unwrap();
                }),
                oauth::Purpose::Calendar,
                Some(epoch),
            )
            .await
        })
    };
    let url = url::Url::parse(&urls.recv().await.unwrap()).unwrap();
    let p = url.query_pairs().into_owned().collect::<HashMap<_, _>>();
    *nonce.lock().unwrap() = p["nonce"].clone();
    assert!(!p["scope"].contains("drive"));
    let redirect = url::Url::parse(&p["redirect_uri"]).unwrap();
    let host = format!("127.0.0.1:{}", redirect.port().unwrap());
    let mut socket = tokio::net::TcpStream::connect(&host).await.unwrap();
    socket
        .write_all(request(&format!("/?state={}&code=calendar", p["state"]), &host).as_bytes())
        .await
        .unwrap();
    let mut page = String::new();
    socket.read_to_string(&mut page).await.unwrap();
    let page = follow_completion(&host, &page).await;
    assert!(page.contains("Google Calendar 연결 완료"));
    task.await.unwrap().unwrap();
    assert_eq!(auth.epoch(), epoch);
    assert!(auth.calendar_connected());
    login
        .run_for(
            auth,
            Arc::new(|_| panic!("a valid Calendar session must not reopen consent")),
            oauth::Purpose::Calendar,
            Some(epoch),
        )
        .await
        .unwrap();
}
#[tokio::test]
async fn cancelling_calendar_consent_preserves_the_drive_login() {
    use crate::google::oauth::tests::{auth, server, token};
    let mock = server(|r| {
        if r.starts_with("POST /token") {
            let mut t = token("attempt");
            t["scope"] = json!(format!(
                "{} {}",
                oauth::SCOPES.join(" "),
                oauth::DRIVE_SCOPE
            ));
            (200, t)
        } else {
            (
                200,
                json!({"sub":"google-sub-a","email":"a@example.invalid","email_verified":true}),
            )
        }
    })
    .await;
    let auth = Arc::new(auth(Store::memory(json!({})), &mock.url));
    auth.exchange_code("base", "v", "http://127.0.0.1/", "attempt")
        .await
        .unwrap();
    let epoch = auth.epoch();
    let before = auth.tokens();
    let login = Arc::new(Login::default());
    let (send, mut urls) = mpsc::unbounded_channel();
    let task = {
        let l = login.clone();
        let a = auth.clone();
        tokio::spawn(async move {
            l.run_for(
                a,
                Arc::new(move |u| {
                    send.send(u.to_owned()).unwrap();
                }),
                oauth::Purpose::Calendar,
                Some(epoch),
            )
            .await
        })
    };
    urls.recv().await.unwrap();
    login.cancel();
    assert!(task.await.unwrap().is_err());
    assert_eq!(auth.tokens(), before);
    assert!(!auth.calendar_connected());
    assert!(auth.has_full_drive());
}

#[tokio::test]
async fn an_already_connected_full_drive_account_never_opens_a_consent_window() {
    use crate::google::oauth::tests::{auth, server, token};
    let mock = server(|request| {
        if request.starts_with("POST /token") {
            let mut t = token("attempt");
            t["scope"] = json!(format!(
                "{} {}",
                oauth::SCOPES.join(" "),
                oauth::DRIVE_SCOPE
            ));
            (200, t)
        } else {
            (
                200,
                json!({"sub":"google-sub-a","email":"user@example.invalid","email_verified":true}),
            )
        }
    })
    .await;
    let auth = Arc::new(auth(Store::memory(json!({})), &mock.url));
    auth.exchange_code("base", "v", "http://127.0.0.1/", "attempt")
        .await
        .unwrap();
    let opener: OpenUrl =
        Arc::new(|_| panic!("a saved full Drive session must not open the browser"));
    let login = Login::default();
    for _ in 0..3 {
        login
            .run_for(
                auth.clone(),
                opener.clone(),
                oauth::Purpose::Files,
                Some(auth.epoch()),
            )
            .await
            .unwrap();
    }
    assert!(!login.busy());
}

#[test]
fn picker_selection_accepts_exactly_one_id_and_never_an_arbitrary_url_or_path() {
    assert_eq!(
        selected_folder(
            "GET /?picked_file_ids=folder-123 HTTP/1.1\r\n",
            "127.0.0.1:9"
        )
        .unwrap(),
        "folder-123"
    );
    for query in [
        "picked_file_ids=a,b",
        "picked_file_ids=..%2Foutside",
        "picked_file_ids=a&picked_file_ids=b",
        "picked_file_ids=https%3A%2F%2Fevil.invalid",
        "code=only",
    ] {
        assert!(selected_folder(&format!("GET /?{query} HTTP/1.1\r\n"), "127.0.0.1:9").is_err());
    }
}

#[test]
fn file_picker_allows_ten_distinct_ids_and_rejects_duplicates_and_paths() {
    assert_eq!(
        selected_files(
            "GET /?picked_file_ids=a,b HTTP/1.1\r\n",
            "127.0.0.1:9",
            true
        )
        .unwrap(),
        vec!["a", "b"]
    );
    for ids in [
        "a,a".to_owned(),
        "a,../bad".to_owned(),
        (0..11)
            .map(|i| format!("file-{i}"))
            .collect::<Vec<_>>()
            .join(","),
    ] {
        assert!(selected_files(
            &format!("GET /?picked_file_ids={ids} HTTP/1.1\r\n"),
            "127.0.0.1:9",
            true
        )
        .is_err());
    }
    let url = oauth::auth_url_for(
        "client",
        "http://127.0.0.1:9",
        "challenge",
        "state",
        "nonce",
        oauth::Purpose::SelectedFiles,
        None,
    );
    let parsed = url::Url::parse(&url).unwrap();
    let params: std::collections::HashMap<_, _> = parsed.query_pairs().into_owned().collect();
    assert_eq!(params["scope"], oauth::FILE_SCOPE);
    assert_eq!(params["allow_multiple"], "true");
    assert_eq!(params["allow_folder_selection"], "false");
    assert!(!params.contains_key("mimetypes"));
}

#[tokio::test]
async fn desktop_picker_completes_loopback_without_replacing_the_google_login() {
    use crate::google::oauth::tests::{auth, server, token};
    let remote=server(|r| {
        if r.starts_with("POST /token ") {
            if r.contains("code=picker") { return (200,json!({"access_token":"PICKER_ONLY","refresh_token":"PICKER_REFRESH","token_type":"Bearer","expires_in":3600,"scope":oauth::FILE_SCOPE})); }
            let mut token=token("attempt"); token["scope"]=json!(format!("{} {}",oauth::SCOPES.join(" "),oauth::FILE_SCOPE)); return (200,token);
        }
        if r.starts_with("GET /userinfo ") { return (200,json!({"sub":"google-sub-a","email":"a@example.invalid","email_verified":true})); }
        if r.starts_with("GET /drive/v3/about?") { return (200,json!({"user":{"permissionId":"drive-account-a"}})); }
        panic!("Unexpected mock request");
    }).await;
    let auth = Arc::new(auth(crate::store::Store::memory(json!({})), &remote.url));
    auth.exchange_code("base", "v", "http://127.0.0.1/", "attempt")
        .await
        .unwrap();
    let epoch = auth.epoch();
    let login = Arc::new(Login::default());
    let (sender, mut urls) = mpsc::unbounded_channel();
    let task = {
        let l = login.clone();
        let a = auth.clone();
        tokio::spawn(async move {
            l.run_for(
                a,
                Arc::new(move |url| {
                    sender.send(url.to_owned()).unwrap();
                }),
                oauth::Purpose::Folder,
                Some(epoch),
            )
            .await
        })
    };
    let url = tokio::time::timeout(Duration::from_secs(5), urls.recv())
        .await
        .unwrap()
        .unwrap();
    let url = url::Url::parse(&url).unwrap();
    let params: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
    assert_eq!(params["scope"], oauth::FILE_SCOPE);
    assert_eq!(params["trigger_onepick"], "true");
    let redirect = url::Url::parse(&params["redirect_uri"]).unwrap();
    let host = format!("127.0.0.1:{}", redirect.port().unwrap());
    let mut socket = tokio::net::TcpStream::connect(&host).await.unwrap();
    socket
        .write_all(
            request(
                &format!(
                    "/?state={}&code=picker&picked_file_ids=selected-folder",
                    params["state"]
                ),
                &host,
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let mut page = String::new();
    socket.read_to_string(&mut page).await.unwrap();
    let page = follow_completion(&host, &page).await;
    assert!(page.contains("자료 선택 완료"));
    assert!(!page.contains("PICKER_ONLY"));
    let result = task.await.unwrap().unwrap();
    assert_eq!(result, json!({"folderId":"selected-folder","epoch":epoch}));
    assert_eq!(auth.tokens().unwrap()["subject"], "google-sub-a");
    assert_eq!(auth.tokens().unwrap()["refreshToken"], "REFRESH_TEST_ONLY");
    assert!(!login.busy());
}
use crate::store::Store;
use tokio::{io::AsyncWriteExt, sync::mpsc};

fn request(path: &str, host: &str) -> String {
    format!("GET {path} HTTP/1.1\r\nHost: {host}\r\n\r\n")
}

fn redirected_to(response: &str) -> &str {
    assert!(response.starts_with("HTTP/1.1 303"));
    response
        .lines()
        .find_map(|line| line.strip_prefix("Location: "))
        .unwrap()
}

async fn local_response(host: &str, request: &str) -> String {
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut socket = TcpStream::connect(host).await.unwrap();
        socket.write_all(request.as_bytes()).await.unwrap();
        let mut response = String::new();
        socket.read_to_string(&mut response).await.unwrap();
        response
    })
    .await
    .unwrap()
}

async fn follow_completion(host: &str, response: &str) -> String {
    let page = local_response(host, &request(redirected_to(response), host)).await;
    assert!(page.starts_with("HTTP/1.1 200"));
    page
}
#[test]
fn callback_rejects_wrong_state_host_duplicates_method_and_foreign_issuer() {
    let host = "127.0.0.1:1234";
    assert!(matches!(
        callback(&request("/?state=good&code=ok", host), "good", host),
        Callback::Code(_)
    ));
    for text in [
        request("/?state=bad&code=ok", host),
        request("/?state=good&code=ok", "evil.invalid"),
        request("/?state=good&state=bad&code=ok", host),
        request("/?state=good&code=ok&iss=https%3A%2F%2Fevil.invalid", host),
        request("/favicon.ico", host),
        request("/?state=good&code=ok", host).replace("GET ", "POST "),
    ] {
        assert!(matches!(callback(&text, "good", host), Callback::Ignore));
    }
    assert!(matches!(
        callback(
            &request("/?state=good&error=access_denied", host),
            "good",
            host
        ),
        Callback::Denied
    ));
}
#[tokio::test]
async fn cancel_releases_the_loopback_port_and_does_not_cancel_the_next_attempt() {
    let login = Arc::new(Login::default());
    let auth = Arc::new(GoogleAuth::new(
        &json!({"clientId":"test","clientSecret":"test"}),
        Store::memory(json!({})),
    ));
    for _ in 0..2 {
        login.cancel(); // idle cancellation must not be carried into a future attempt
        let (sender, mut urls) = mpsc::unbounded_channel();
        let opener: OpenUrl = Arc::new(move |url| {
            let _ = sender.send(url.to_owned());
        });
        let controller = login.clone();
        let auth = auth.clone();
        let task = tokio::spawn(async move { controller.run(auth, opener).await });
        let url = tokio::time::timeout(Duration::from_secs(5), urls.recv())
            .await
            .unwrap()
            .unwrap();
        let url = url::Url::parse(&url).unwrap();
        let redirect = url
            .query_pairs()
            .find(|(key, _)| key == "redirect_uri")
            .unwrap()
            .1
            .into_owned();
        let redirect = url::Url::parse(&redirect).unwrap();
        let host = format!("127.0.0.1:{}", redirect.port().unwrap());
        let mut socket = tokio::net::TcpStream::connect(&host).await.unwrap();
        socket
            .write_all(request("/?state=wrong&code=bad", &host).as_bytes())
            .await
            .unwrap();
        let mut response = String::new();
        socket.read_to_string(&mut response).await.unwrap();
        assert!(response.starts_with("HTTP/1.1 400"));
        assert!(!task.is_finished());
        login.cancel();
        assert!(task.await.unwrap().is_err());
        assert!(tokio::net::TcpStream::connect(&host).await.is_err());
        assert!(!login.busy());
    }
}
#[cfg(windows)]
#[tokio::test]
async fn a_complete_mock_login_checks_both_google_responses_before_storing_the_account() {
    use crate::google::oauth::tests::{auth, server, token};
    let nonce = Arc::new(Mutex::new(String::new()));
    let value = nonce.clone();
    let server = server(move |request| {
        if request.starts_with("POST /token") {
            let mut response = token(&value.lock().unwrap());
            response["scope"] = json!(format!(
                "{} {}",
                oauth::SCOPES.join(" "),
                oauth::DRIVE_SCOPE
            ));
            (200, response)
        } else {
            (
                200,
                json!({"sub":"google-sub-a","email":"one@example.com","email_verified":true}),
            )
        }
    })
    .await;
    let store = Store::memory(json!({}));
    let auth = Arc::new(auth(store.clone(), &server.url));
    let (reveal, mut reveals) = mpsc::unbounded_channel();
    let login = Arc::new(Login::with_return_to_app(Arc::new(move || {
        let _ = reveal.send(());
    })));
    let (tx, mut rx) = mpsc::unbounded_channel();
    let opener: OpenUrl = Arc::new(move |url| {
        let _ = tx.send(url.to_owned());
    });
    let controller = login.clone();
    let task = tokio::spawn(async move { controller.run(auth, opener).await });
    let opened = url::Url::parse(&rx.recv().await.unwrap()).unwrap();
    let query: HashMap<_, _> = opened
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    *nonce.lock().unwrap() = query["nonce"].clone();
    let redirect = url::Url::parse(&query["redirect_uri"]).unwrap();
    let host = format!("127.0.0.1:{}", redirect.port().unwrap());
    let mut socket = tokio::net::TcpStream::connect(&host).await.unwrap();
    socket
        .write_all(request(&format!("/?state={}&code=approved", query["state"]), &host).as_bytes())
        .await
        .unwrap();
    let mut body = String::new();
    socket.read_to_string(&mut body).await.unwrap();
    let completion = redirected_to(&body);
    assert!(completion.starts_with("/complete/"));
    assert!(!completion.contains("?"));
    assert!(!completion.contains(&query["state"]));
    assert!(!body.contains("approved"));
    assert_eq!(task.await.unwrap().unwrap()["email"], "one@example.com");
    assert!(!login.busy());

    let page = follow_completion(&host, &body).await;
    assert!(page.contains("Google 연결 완료"));
    assert!(page.contains("사이로 돌아가기"));
    assert!(page.contains("method=\"post\""));
    assert!(!page.contains("one@example.com"));
    assert!(!page.contains("ACCESS_TEST_ONLY"));
    tokio::time::timeout(Duration::from_secs(5), reveals.recv())
        .await
        .unwrap()
        .unwrap();

    let post = |host: &str, origin: &str| {
        format!("POST {completion} HTTP/1.1\r\nHost: {host}\r\nOrigin: {origin}\r\nContent-Length: 0\r\n\r\n")
    };
    for invalid in [
        post(&host, "https://example.invalid"),
        post("example.invalid", &format!("http://{host}")),
        post(&host, "null"),
        request(&format!("/?state={}&code=approved", query["state"]), &host),
    ] {
        let response = local_response(&host, &invalid).await;
        assert!(response.starts_with("HTTP/1.1 400"));
    }
    assert!(reveals.try_recv().is_err());

    let response = local_response(&host, &post(&host, &format!("http://{host}"))).await;
    assert_eq!(redirected_to(&response), format!("{completion}/returned"));
    tokio::time::timeout(Duration::from_secs(5), reveals.recv())
        .await
        .unwrap()
        .unwrap();
    let returned = follow_completion(&host, &response).await;
    assert!(returned.contains("사이 다시 열기"));
    assert!(!login.busy());
}

#[tokio::test]
async fn cancelling_an_exchange_in_progress_keeps_the_previous_login() {
    use crate::google::oauth::tests::auth;
    let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_url = format!("http://{}", upstream.local_addr().unwrap());
    let store = Store::memory(json!({"googleTokensEnc":"previous-value"}));
    let before = store.get();
    let auth = Arc::new(auth(store.clone(), &upstream_url));
    let login = Arc::new(Login::default());
    let (tx, mut rx) = mpsc::unbounded_channel();
    let controller = login.clone();
    let task = tokio::spawn(async move {
        controller
            .run(
                auth,
                Arc::new(move |url| {
                    let _ = tx.send(url.to_owned());
                }),
            )
            .await
    });
    let opened = url::Url::parse(&rx.recv().await.unwrap()).unwrap();
    let query: HashMap<_, _> = opened
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    let redirect = url::Url::parse(&query["redirect_uri"]).unwrap();
    let host = format!("127.0.0.1:{}", redirect.port().unwrap());
    let mut browser = tokio::net::TcpStream::connect(&host).await.unwrap();
    browser
        .write_all(request(&format!("/?state={}&code=approved", query["state"]), &host).as_bytes())
        .await
        .unwrap();
    let (_pending_response, _) = tokio::time::timeout(Duration::from_secs(5), upstream.accept())
        .await
        .unwrap()
        .unwrap();
    login.cancel();
    assert!(task.await.unwrap().is_err());
    assert_eq!(store.get(), before);
    assert!(tokio::net::TcpStream::connect(&host).await.is_err());
}
