use super::*;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::mpsc,
    task::JoinHandle,
};

pub(in crate::google) struct Server {
    pub url: String,
    pub received: mpsc::UnboundedReceiver<String>,
    task: JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
pub(in crate::google) async fn server(
    handler: impl Fn(&str) -> (u16, Value) + Send + Sync + 'static,
) -> Server {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (sender, received) = mpsc::unbounded_channel();
    let task = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut raw = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                let n = socket.read(&mut chunk).await.unwrap();
                if n == 0 {
                    break;
                }
                raw.extend_from_slice(&chunk[..n]);
                if let Some(header) = raw.windows(4).position(|p| p == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&raw[..header]);
                    let len = headers
                        .lines()
                        .filter_map(|line| line.split_once(':'))
                        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                        .map(|(_, v)| v.trim().parse::<usize>().unwrap())
                        .unwrap_or(0);
                    if raw.len() >= header + 4 + len {
                        break;
                    }
                }
            }
            let request = String::from_utf8(raw).unwrap();
            let _ = sender.send(request.clone());
            let (status, body) = handler(&request);
            let data = body.to_string();
            let response=format!("HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{data}",data.len());
            let _ = socket.write_all(response.as_bytes()).await;
        }
    });
    Server {
        url,
        received,
        task,
    }
}
pub(in crate::google) fn auth(settings: Store, url: &str) -> GoogleAuth {
    let mut auth = GoogleAuth::new(
        &json!({"clientId":"test-client","clientSecret":"test-secret"}),
        settings,
    );
    auth.transport = Transport {
        http: reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap(),
        token: format!("{url}/token"),
        userinfo: format!("{url}/userinfo"),
        api: url.into(),
    };
    auth
}
pub(in crate::google) fn jwt(payload: Value) -> String {
    format!(
        "{}.{}.signature",
        B64URL.encode(br#"{"alg":"RS256"}"#),
        B64URL.encode(payload.to_string())
    )
}
fn claims() -> Value {
    let now = crate::time::now_ms() / 1000;
    json!({"iss":"https://accounts.google.com","aud":"test-client","sub":"google-sub-a","nonce":"attempt","iat":now-1,"exp":now+3600})
}
pub(in crate::google) fn token(nonce: &str) -> Value {
    let mut c = claims();
    c["nonce"] = json!(nonce);
    json!({"access_token":"ACCESS_TEST_ONLY","refresh_token":"REFRESH_TEST_ONLY","token_type":"Bearer","expires_in":3600,"scope":SCOPES.join(" "),"id_token":jwt(c)})
}
fn info(subject: &str, email: &str) -> Value {
    json!({"sub":subject,"email":email,"email_verified":true})
}
fn fixture_store() -> Store {
    let s = Store::memory(json!({}));
    let old = crate::secret::encrypt_json(
        &json!({"refreshToken":"OLD_REFRESH","email":"unverified@invalid.example"}),
    );
    s.update(|v| v["googleTokensEnc"] = json!(old));
    s
}

#[tokio::test]
async fn full_drive_session_survives_restart_and_ignores_obsolete_picker_credentials() {
    let refreshes = Arc::new(AtomicUsize::new(0));
    let count = refreshes.clone();
    let mock = server(move |request| {
        if request.starts_with("POST /token") {
            assert!(!request.contains("OBSOLETE_PICKER"));
            if request.contains("grant_type=refresh_token") {
                count.fetch_add(1, Ordering::SeqCst);
            }
            let mut t = token("attempt");
            t["scope"] = json!(format!("{} {DRIVE_SCOPE}", SCOPES.join(" ")));
            return (200, t);
        }
        assert!(request.starts_with("GET /userinfo"));
        (200, info("google-sub-a", "user@example.invalid"))
    })
    .await;
    let settings = Store::memory(json!({}));
    let first = auth(settings.clone(), &mock.url);
    first
        .exchange_identity(
            "approved",
            "v",
            "http://127.0.0.1/",
            "attempt",
            None,
            DRIVE_SCOPE,
        )
        .await
        .unwrap();
    let mut stored = first.tokens().unwrap();
    stored["driveRefreshToken"] = json!("OBSOLETE_PICKER");
    stored["drivePermissionId"] = json!("old-id");
    first.save_tokens(Some(stored)).unwrap();
    let restarted = auth(settings, &mock.url);
    let epoch = restarted.epoch();
    for _ in 0..3 {
        let lease = restarted.full_drive_lease(epoch).await.unwrap();
        assert_eq!(lease.subject(), "google-sub-a");
        assert_eq!(
            restarted.file_lease().await.unwrap().subject(),
            "google-sub-a"
        );
    }
    assert!(restarted.has_full_drive());
    assert_eq!(refreshes.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn refusing_full_drive_preserves_the_existing_account_and_requires_explicit_upgrade() {
    let mock = server(|request| {
        if request.starts_with("POST /token") {
            (200, token("attempt"))
        } else {
            (200, info("google-sub-a", "user@example.invalid"))
        }
    })
    .await;
    let client = auth(Store::memory(json!({})), &mock.url);
    client
        .exchange_code("base", "v", "http://127.0.0.1/", "attempt")
        .await
        .unwrap();
    let before = client.tokens().unwrap();
    let epoch = client.epoch();
    let lease = client.lease().await.unwrap();
    assert!(client.full_drive_lease(epoch).await.is_err());
    assert!(client
        .exchange_identity(
            "declined",
            "v",
            "http://127.0.0.1/",
            "attempt",
            Some(&lease),
            DRIVE_SCOPE
        )
        .await
        .is_err());
    assert_eq!(client.tokens().unwrap(), before);
    assert_eq!(client.epoch(), epoch);
}

#[test]
fn explicit_upgrade_uses_full_drive_and_legacy_picker_stays_separate() {
    let files = url::Url::parse(&auth_url_for(
        "client",
        "http://127.0.0.1:9",
        "challenge",
        "state",
        "nonce",
        Purpose::Files,
        Some("account@example.invalid"),
    ))
    .unwrap();
    let file_params: std::collections::HashMap<_, _> = files.query_pairs().into_owned().collect();
    assert!(file_params["scope"]
        .split_whitespace()
        .any(|s| s == DRIVE_SCOPE));
    assert!(account_scopes(&file_params["scope"]));
    assert!(file_params["scope"]
        .split_whitespace()
        .any(|s| s == "https://www.googleapis.com/auth/drive"));
    let picker = url::Url::parse(&auth_url_for(
        "client",
        "http://127.0.0.1:9",
        "challenge",
        "state",
        "nonce",
        Purpose::Folder,
        Some("account@example.invalid"),
    ))
    .unwrap();
    let params: std::collections::HashMap<_, _> = picker.query_pairs().into_owned().collect();
    assert_eq!(params["scope"], FILE_SCOPE);
    assert_eq!(params["trigger_onepick"], "true");
    assert_eq!(params["allow_folder_selection"], "true");
    assert_eq!(params["allow_multiple"], "false");
    assert_eq!(params["prompt"], "consent");
    assert_eq!(params["include_granted_scopes"], "false");
    assert!(!params.contains_key("access_token"));
    assert!(!params.contains_key("nonce"));
}

#[tokio::test]
async fn picker_grant_is_bound_to_verified_drive_id_without_replacing_oidc_credentials() {
    let wrong = Arc::new(AtomicBool::new(true));
    let other = wrong.clone();
    let mock=server(move |request| {
        if request.starts_with("POST /token ") {
            if request.contains("code=picker") || request.contains("refresh_token=PICKER_REFRESH") {
                return (200,json!({"access_token":"PICKER_ACCESS","refresh_token":"PICKER_REFRESH","token_type":"Bearer","expires_in":3600,"scope":FILE_SCOPE}));
            }
            let mut value=token("attempt"); value["scope"]=json!(format!("{} {FILE_SCOPE}",SCOPES.join(" "))); return (200,value);
        }
        if request.starts_with("GET /userinfo ") { return (200,info("google-sub-a","a@example.invalid")); }
        if request.starts_with("GET /drive/v3/about?") {
            let picker=request.contains("PICKER_ACCESS");
            return (200,json!({"user":{"permissionId":if picker && other.load(Ordering::SeqCst) { "drive-b" } else { "drive-a" }}}));
        }
        panic!("Unexpected mock path");
    }).await;
    let settings = Store::memory(json!({}));
    let client = auth(settings.clone(), &mock.url);
    client
        .exchange_code("base", "v", "http://127.0.0.1/", "attempt")
        .await
        .unwrap();
    let lease = client.lease().await.unwrap();
    let original = client.tokens().unwrap();
    assert!(client
        .finish_picker(&lease, "picker", "v", "http://127.0.0.1/")
        .await
        .is_err());
    assert_eq!(client.tokens().unwrap(), original);
    wrong.store(false, Ordering::SeqCst);
    client
        .finish_picker(&lease, "picker", "v", "http://127.0.0.1/")
        .await
        .unwrap();
    assert_eq!(
        client.tokens().unwrap()["refreshToken"],
        original["refreshToken"]
    );
    assert_eq!(client.tokens().unwrap()["subject"], "google-sub-a");
    assert_eq!(
        client.file_lease().await.unwrap().access.token,
        "PICKER_ACCESS"
    );
    let reopened = auth(settings.clone(), &mock.url);
    assert_eq!(
        reopened.file_lease().await.unwrap().access.token,
        "PICKER_ACCESS"
    );
    assert_eq!(
        reopened.verified_identity().await.unwrap().subject,
        "google-sub-a"
    );
    *reopened.drive_access.lock().unwrap() = None;
    wrong.store(true, Ordering::SeqCst);
    assert!(reopened.file_lease().await.is_err());
    assert!(reopened.is_signed_in());
    assert!(!reopened.has_file_scope());
    reopened.sign_out().await.unwrap();
    assert!(settings.get()["googleTokensEnc"].is_null());
}

#[tokio::test]
async fn granting_files_for_a_different_google_subject_preserves_the_original_account() {
    let second = Arc::new(AtomicBool::new(false));
    let use_b = second.clone();
    let mock = server(move |request| {
        let sub = if use_b.load(Ordering::SeqCst) {
            "google-sub-b"
        } else {
            "google-sub-a"
        };
        if request.starts_with("POST /token ") {
            let mut c = claims();
            c["sub"] = json!(sub);
            let mut t = token("attempt");
            t["id_token"] = json!(jwt(c));
            t["scope"] = json!(format!("{} {FILE_SCOPE}", SCOPES.join(" ")));
            return (200, t);
        }
        (200, info(sub, "same-visible-name@example.invalid"))
    })
    .await;
    let client = auth(Store::memory(json!({})), &mock.url);
    client
        .exchange_code("base", "v", "http://127.0.0.1/", "attempt")
        .await
        .unwrap();
    let original = client.tokens().unwrap();
    let lease = client.lease().await.unwrap();
    second.store(true, Ordering::SeqCst);
    assert!(client
        .exchange_identity(
            "enable",
            "v",
            "http://127.0.0.1/",
            "attempt",
            Some(&lease),
            FILE_SCOPE
        )
        .await
        .is_err());
    assert_eq!(client.tokens().unwrap(), original);
    assert_eq!(client.active_identity().unwrap().subject, "google-sub-a");
}

#[test]
fn initial_login_asks_only_identity_and_app_settings_with_independent_state_nonce_and_pkce() {
    let (verifier, challenge) = pkce();
    assert_eq!(verifier.len(), 43);
    assert_eq!(
        B64URL.encode(Sha256::digest(verifier.as_bytes())),
        challenge
    );
    assert_ne!(random_token(), random_token());
    let url = url::Url::parse(&auth_url(
        "client",
        "http://127.0.0.1:1234",
        &challenge,
        "state",
        "nonce",
    ))
    .unwrap();
    let params: std::collections::HashMap<_, _> = url.query_pairs().collect();
    // Least privilege: full Drive is requested only when the person turns on team Drive.
    assert_eq!(params["scope"], SCOPES.join(" "));
    assert_eq!(params["nonce"], "nonce");
    assert_eq!(params["state"], "state");
    assert_eq!(params["code_challenge_method"], "S256");
    assert!(!params["scope"].contains("calendar"));
    assert!(!params["scope"].contains("tasks"));
    assert!(!params["scope"]
        .split_whitespace()
        .any(|s| s == "https://www.googleapis.com/auth/drive"));
    assert!(account_scopes(&format!(
        "openid https://www.googleapis.com/auth/userinfo.email {APPDATA_SCOPE}"
    )));
}
#[test]
fn direct_google_claims_are_bound_to_issuer_client_time_nonce_and_access_hash() {
    let now = crate::time::now_ms() / 1000;
    let original = claims();
    assert_eq!(
        identity::claims_from_google_response(
            &jwt(original.clone()),
            "test-client",
            Some("attempt"),
            "access",
            now
        )
        .unwrap(),
        "google-sub-a"
    );
    for (key, value) in [
        ("iss", json!("https://evil.invalid")),
        ("aud", json!("wrong-client")),
        ("nonce", json!("wrong-attempt")),
        ("exp", json!(now)),
        ("iat", json!(now + 120)),
        ("sub", json!("")),
    ] {
        let mut c = original.clone();
        c[key] = value;
        assert!(
            identity::claims_from_google_response(
                &jwt(c),
                "test-client",
                Some("attempt"),
                "access",
                now
            )
            .is_err(),
            "{key}"
        );
    }
    let mut c = original.clone();
    c["aud"] = json!(["test-client", "another"]);
    assert!(identity::claims_from_google_response(
        &jwt(c.clone()),
        "test-client",
        Some("attempt"),
        "access",
        now
    )
    .is_err());
    c["azp"] = json!("test-client");
    assert!(identity::claims_from_google_response(
        &jwt(c.clone()),
        "test-client",
        Some("attempt"),
        "access",
        now
    )
    .is_ok());
    c["at_hash"] = json!("wrong");
    assert!(identity::claims_from_google_response(
        &jwt(c.clone()),
        "test-client",
        Some("attempt"),
        "access",
        now
    )
    .is_err());
    c["at_hash"] = json!(B64URL.encode(&Sha256::digest(b"access")[..16]));
    assert!(identity::claims_from_google_response(
        &jwt(c),
        "test-client",
        Some("attempt"),
        "access",
        now
    )
    .is_ok());
    assert!(identity::user_identity(&info("other", "a@example.com"), "google-sub-a").is_err());
    assert!(identity::user_identity(
        &json!({"sub":"google-sub-a","email":"a@example.com","email_verified":false}),
        "google-sub-a"
    )
    .is_err());
}
#[test]
fn legacy_records_do_not_authenticate_and_google_endpoints_cannot_be_overridden() {
    let store = fixture_store();
    let before = store.get();
    let auth = GoogleAuth::new(
        &json!({"clientId":"client","clientSecret":"secret","token_uri":"http://evil.invalid","userinfo":"http://evil.invalid"}),
        store.clone(),
    );
    assert!(!auth.has_session());
    assert!(!auth.is_signed_in());
    assert!(auth.needs_reauth());
    assert_eq!(store.get(), before);
    assert_eq!(auth.transport.token, TOKEN_URL);
    assert_eq!(auth.transport.userinfo, USERINFO_URL);
}
#[cfg(windows)]
#[tokio::test]
async fn successful_login_uses_google_subject_and_os_protected_tokens_and_local_logout() {
    let mut server = server(|request| {
        if request.starts_with("POST /token") {
            (200, token("attempt"))
        } else {
            assert!(request
                .to_ascii_lowercase()
                .contains("authorization: bearer access_test_only"));
            (200, info("google-sub-a", "one@example.com"))
        }
    })
    .await;
    let store = fixture_store();
    let auth = auth(store.clone(), &server.url);
    let identity = auth
        .exchange_code("code", "verifier", "http://127.0.0.1", "attempt")
        .await
        .unwrap();
    assert_eq!(identity.subject, "google-sub-a");
    assert!(auth.is_signed_in());
    assert!(store.get()["googleTokensEnc"]
        .as_str()
        .unwrap()
        .starts_with("dpapi:"));
    assert!(!store.get().to_string().contains("REFRESH_TEST_ONLY"));
    assert_eq!(auth.tokens().unwrap()["subject"], "google-sub-a");
    assert_eq!(auth.access_token().await.unwrap(), "ACCESS_TEST_ONLY");
    auth.sign_out().await.unwrap();
    assert!(!auth.has_credentials());
    assert!(!auth.is_signed_in());
    assert!(server
        .received
        .try_recv()
        .unwrap()
        .starts_with("POST /token"));
    assert!(server
        .received
        .try_recv()
        .unwrap()
        .starts_with("GET /userinfo"));
    assert!(server.received.try_recv().is_err());
}
#[cfg(windows)]
#[tokio::test]
async fn failed_nonce_or_partial_permissions_keep_previous_credentials_unchanged() {
    for partial in [false, true] {
        let server = server(move |_| {
            let mut d = token(if partial { "attempt" } else { "another" });
            if partial {
                d["scope"] = json!("openid email");
            }
            (200, d)
        })
        .await;
        let store = fixture_store();
        let before = store.get();
        let auth = auth(store.clone(), &server.url);
        assert!(auth
            .exchange_code("code", "verifier", "redirect", "attempt")
            .await
            .is_err());
        assert_eq!(store.get(), before);
        assert!(!auth.is_signed_in());
    }
}
#[cfg(windows)]
#[tokio::test]
async fn same_subject_with_changed_email_is_refreshed_but_a_different_subject_is_rejected() {
    for switch_subject in [false, true] {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let server=server(move |request|if request.starts_with("POST /token"){
            if request.contains("grant_type=refresh_token"){(200,json!({"access_token":"NEW_ACCESS","token_type":"Bearer","expires_in":3600}))}else{(200,token("attempt"))}
        }else{let next=counter.fetch_add(1,Ordering::SeqCst)>0;(200,info(if next&&switch_subject{"google-sub-b"}else{"google-sub-a"},if next{"renamed@example.com"}else{"one@example.com"}))}).await;
        let store = fixture_store();
        let auth = auth(store.clone(), &server.url);
        auth.exchange_code("code", "verifier", "redirect", "attempt")
            .await
            .unwrap();
        let before = store.get();
        auth.access.lock().unwrap().as_mut().unwrap().expires = 0;
        let result = auth.verified_identity().await;
        if switch_subject {
            assert!(result.is_err());
            assert_eq!(store.get(), before);
            assert!(!auth.is_signed_in());
            assert!(auth.needs_reauth());
        } else {
            assert_eq!(result.unwrap().subject, "google-sub-a");
            assert_eq!(auth.tokens().unwrap()["email"], "renamed@example.com");
        }
    }
}
#[tokio::test]
async fn provider_error_body_is_not_exposed_to_the_user() {
    let server=server(|_|(400,json!({"error":"invalid_request","error_description":"DO_NOT_EXPOSE_TOKEN_OR_PRIVATE_INPUT"}))).await;
    let auth = auth(fixture_store(), &server.url);
    let error = auth
        .exchange_code("code", "verifier", "redirect", "attempt")
        .await
        .unwrap_err();
    assert!(!error.contains("DO_NOT_EXPOSE"));
}

#[cfg(windows)]
#[tokio::test]
async fn unsuccessful_userinfo_does_not_replace_an_existing_login() {
    for verified in [true, false] {
        let server = server(move |request| if request.starts_with("POST /token") { (200, token("attempt")) }
            else { (200, json!({"sub":if verified {"another-subject"} else {"google-sub-a"},"email":"one@example.com","email_verified":verified})) }).await;
        let store = fixture_store();
        let before = store.get();
        let auth = auth(store.clone(), &server.url);
        assert!(auth
            .exchange_code("code", "verifier", "redirect", "attempt")
            .await
            .is_err());
        assert_eq!(store.get(), before);
        assert!(!auth.is_signed_in());
    }
}

#[cfg(windows)]
#[tokio::test]
async fn expired_refresh_requires_login_without_erasing_original_credentials() {
    let server = server(|request| {
        if request.starts_with("POST /token") {
            if request.contains("grant_type=refresh_token") {
                (400, json!({"error":"invalid_grant"}))
            } else {
                (200, token("attempt"))
            }
        } else {
            (200, info("google-sub-a", "one@example.com"))
        }
    })
    .await;
    let store = fixture_store();
    let auth = auth(store.clone(), &server.url);
    auth.exchange_code("code", "verifier", "redirect", "attempt")
        .await
        .unwrap();
    let before = store.get();
    auth.access.lock().unwrap().as_mut().unwrap().expires = 0;
    assert!(auth.access_token().await.is_err());
    assert!(auth.needs_reauth());
    assert!(!auth.is_signed_in());
    assert_eq!(store.get(), before);
}
#[test]
fn failed_atomic_store_commit_keeps_both_memory_and_original_bytes() {
    let root = std::env::temp_dir().join(format!("sai-login-commit-{}", rand::random::<u64>()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("settings.json");
    let original = br#"{"googleTokensEnc":"old","unknown":42}"#;
    std::fs::write(&path, original).unwrap();
    let store = Store::open(&path, json!({}));
    let before = store.get();
    std::fs::create_dir(path.with_extension("json.tmp")).unwrap();
    assert!(store
        .commit(|s| s["googleTokensEnc"] = json!("new"))
        .is_err());
    assert_eq!(store.get(), before);
    assert_eq!(std::fs::read(path).unwrap(), original);
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(windows)]
#[tokio::test]
async fn wrong_client_or_unprotected_session_is_rejected_before_any_network_request() {
    use base64::engine::general_purpose::STANDARD;
    let mut server = server(|_| (500, json!({}))).await;
    for protected in [true, false] {
        let store = Store::memory(json!({}));
        let auth = auth(store.clone(), &server.url);
        let record = json!({"version":2,"clientId":if protected {"other-client"} else {"test-client"},
            "subject":"google-sub-a","refreshToken":"TEST_ONLY_REFRESH","scope":SCOPES.join(" ")});
        if protected {
            auth.save_tokens(Some(record)).unwrap();
        } else {
            store.update(|s| {
                s["googleTokensEnc"] = json!(format!("raw:{}", STANDARD.encode(record.to_string())))
            });
        }
        let before = store.get();
        assert!(!auth.has_session());
        assert!(auth.needs_reauth());
        assert!(auth.access_token().await.is_err());
        assert_eq!(store.get(), before);
        assert!(server.received.try_recv().is_err());
    }
}
