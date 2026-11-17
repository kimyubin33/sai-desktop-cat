use super::*;
use crate::{
    google::{
        oauth::tests::{auth, server, token, Server},
        profile::model::TeamRef,
    },
    store::Store,
};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "sai-team-ai-test-{}",
            super::super::oauth::random_token()
        ));
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
            .starts_with("sai-team-ai-test-"));
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
struct Fixture {
    _root: Temp,
    server: Server,
    auth: GoogleAuth,
    profiles: Profiles,
    teams: Teams,
    requests: Arc<AtomicUsize>,
    settings: Store,
}
impl Fixture {
    async fn new(status: u16) -> Self {
        let root = Temp::new();
        let requests = Arc::new(AtomicUsize::new(0));
        let calls = requests.clone();
        let server=server(move |r|{
            let path=r.lines().next().unwrap().split_whitespace().nth(1).unwrap();
            if path=="/token" {let mut t=token("attempt");t["scope"]=json!(format!("{} {}",crate::google::oauth::SCOPES.join(" "),crate::google::oauth::DRIVE_SCOPE));return (200,t);}
            if path=="/userinfo" {return (200,json!({"sub":"google-sub-a","email":"user@example.invalid","email_verified":true}));}
            if path.starts_with("/drive/v3/files?spaces=appDataFolder"){return (200,json!({"files":[]}));}
            if path.starts_with("/drive/v3/files/team-folder?"){return (200,json!({"id":"team-folder","name":"Team","mimeType":"application/vnd.google-apps.folder","trashed":false,"properties":{"saiTeam":"1"},"capabilities":{"canListChildren":true,"canAddChildren":true}}));}
            if path.starts_with("/v1/") {
                calls.fetch_add(1,Ordering::SeqCst);assert!(r.to_lowercase().contains("authorization: bearer access_test_only"));assert!(!r.contains("REFRESH_TEST_ONLY"));
                let body:Value=serde_json::from_str(r.split_once("\r\n\r\n").unwrap().1).unwrap();assert_eq!(body["folderId"],"team-folder");assert!(body.get("subject").is_none());assert!(body.get("apiKey").is_none());
                if status!=200{return (status,json!({"error":"unknown","detail":"SECRET_SHOULD_NOT_ESCAPE"}));}
                if path=="/v1/status"{assert_eq!(body.as_object().unwrap().len(),1);return (200,json!({"ready":true,"provider":"claude","owner":false,"dailyRequests":20,"used":2,"privateKey":"SECRET_SHOULD_NOT_ESCAPE"}));}
                return (200,json!({"requestId":body["requestId"],"summary":"Review requested","candidates":[{"title":"Review","assignee":"","dueDate":"","evidence":"Please review the brief.","uncertain":true}]}));
            }panic!("Unexpected mock path {path}");
        }).await;
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
        auth.set_team_ai_consent(&lease, &server.url, true)
            .await
            .unwrap();
        Self {
            _root: root,
            server,
            auth,
            profiles,
            teams: Teams::default(),
            requests,
            settings,
        }
    }
    fn client(&self) -> Client {
        Client::new(Some(&self.server.url)).unwrap()
    }
    fn attempt(&self) -> Attempt {
        Attempt {
            request_id: "r".repeat(40),
            created_at: crate::time::now_ms(),
            text: "Please review the brief.".into(),
        }
    }
}
#[tokio::test]
async fn missing_developer_endpoint_is_honestly_unavailable_without_transmission() {
    let f = Fixture::new(200).await;
    let v = Client::new(None)
        .unwrap()
        .status(
            &f.auth,
            &f.profiles,
            &f.teams,
            f.auth.epoch(),
            "team-folder",
        )
        .await
        .unwrap();
    assert_eq!(v["configured"], false);
    assert_eq!(f.requests.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn status_exposes_only_verified_counts_and_never_gateway_secrets() {
    let f = Fixture::new(200).await;
    let v = f
        .client()
        .status(
            &f.auth,
            &f.profiles,
            &f.teams,
            f.auth.epoch(),
            "team-folder",
        )
        .await
        .unwrap();
    assert_eq!(v["ready"], true);
    assert_eq!(v["used"], 2);
    assert!(!v.to_string().contains("SECRET_SHOULD_NOT_ESCAPE"));
}
#[tokio::test]
async fn analysis_is_bound_to_the_account_team_and_cited_source() {
    let f = Fixture::new(200).await;
    let v = f
        .client()
        .analyze(
            &f.auth,
            &f.profiles,
            &f.teams,
            f.auth.epoch(),
            "team-folder",
            &f.attempt(),
        )
        .await
        .unwrap();
    assert_eq!(v["result"]["candidates"][0]["title"], "Review");
    assert_eq!(f.requests.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn failures_are_not_retried_or_returned_with_upstream_details() {
    let f = Fixture::new(503).await;
    let e = f
        .client()
        .analyze(
            &f.auth,
            &f.profiles,
            &f.teams,
            f.auth.epoch(),
            "team-folder",
            &f.attempt(),
        )
        .await
        .unwrap_err();
    assert!(!e.contains("SECRET_SHOULD_NOT_ESCAPE"));
    assert_eq!(f.requests.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn stale_account_unknown_team_and_oversize_input_never_reach_the_gateway() {
    let f = Fixture::new(200).await;
    assert!(f
        .client()
        .status(
            &f.auth,
            &f.profiles,
            &f.teams,
            f.auth.epoch() - 1,
            "team-folder"
        )
        .await
        .is_err());
    assert!(f
        .client()
        .status(&f.auth, &f.profiles, &f.teams, f.auth.epoch(), "other-team")
        .await
        .is_err());
    let mut a = f.attempt();
    a.text = "a".repeat(24577);
    assert!(f
        .client()
        .analyze(
            &f.auth,
            &f.profiles,
            &f.teams,
            f.auth.epoch(),
            "team-folder",
            &a
        )
        .await
        .is_err());
    f.auth.sign_out().await.unwrap();
    assert!(f
        .client()
        .analyze(
            &f.auth,
            &f.profiles,
            &f.teams,
            f.auth.epoch(),
            "team-folder",
            &f.attempt()
        )
        .await
        .is_err());
    assert_eq!(f.requests.load(Ordering::SeqCst), 0);
}
#[test]
fn only_a_bundled_https_origin_is_accepted() {
    for value in [
        "http://example.com",
        "https://u:p@example.com",
        "https://example.com/a",
        "https://example.com?key=x",
        "https://example.com#x",
        "https://127.0.0.1",
    ] {
        assert!(Client::new(Some(value)).is_err());
    }
    assert!(Client::new(Some("https://team-ai.example.com")).is_ok());
}
#[test]
fn fabricated_evidence_extra_actions_and_invalid_dates_are_rejected() {
    let a = Attempt {
        request_id: "r".repeat(40),
        created_at: 1,
        text: "Actual source".into(),
    };
    let mut v = json!({"requestId":a.request_id,"summary":"x","candidates":[{"title":"x","assignee":"","dueDate":"2026-02-30","evidence":"Actual source","uncertain":true}]});
    assert!(validate_result(v.clone(), &a).is_err());
    v["candidates"][0]["dueDate"] = json!("");
    v["candidates"][0]["evidence"] = json!("Invented");
    assert!(validate_result(v.clone(), &a).is_err());
    v["candidates"][0]["evidence"] = json!("Actual source");
    assert!(validate_result(v.clone(), &a).is_ok());
    let multi = Attempt {
        text: "Actual source\nSecond line".into(),
        ..a
    };
    v["candidates"][0]["evidence"] = json!(multi.text);
    assert!(validate_result(v.clone(), &multi).is_ok());
    v["candidates"][0]["action"] = json!("share");
    assert!(validate_result(v, &multi).is_err());
}

#[tokio::test]
async fn opening_a_board_never_transmits_a_token_before_explicit_gateway_consent() {
    let f = Fixture::new(200).await;
    let lease = f.auth.lease().await.unwrap();
    f.auth
        .set_team_ai_consent(&lease, &f.server.url, false)
        .await
        .unwrap();
    let v = f
        .client()
        .status(
            &f.auth,
            &f.profiles,
            &f.teams,
            f.auth.epoch(),
            "team-folder",
        )
        .await
        .unwrap();
    assert_eq!(v["requiresConsent"], true);
    assert_eq!(f.requests.load(Ordering::SeqCst), 0);
    assert!(f
        .client()
        .analyze(
            &f.auth,
            &f.profiles,
            &f.teams,
            f.auth.epoch(),
            "team-folder",
            &f.attempt()
        )
        .await
        .is_err());
    assert_eq!(f.requests.load(Ordering::SeqCst), 0);
    f.client()
        .connect(
            &f.auth,
            &f.profiles,
            &f.teams,
            f.auth.epoch(),
            "team-folder",
        )
        .await
        .unwrap();
    assert_eq!(f.requests.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn consent_survives_restart_for_the_same_account_and_origin_only() {
    let f = Fixture::new(200).await;
    let restarted = auth(f.settings.clone(), &f.server.url);
    let lease = restarted.lease().await.unwrap();
    assert!(restarted.team_ai_consented(&lease, &f.server.url));
    assert!(!restarted.team_ai_consented(&lease, "https://another.example.com"));
}

#[tokio::test]
async fn changing_google_accounts_never_reuses_the_previous_accounts_gateway_consent() {
    let f = Fixture::new(200).await;
    f.auth.sign_out().await.unwrap();
    let second=server(|request|if request.starts_with("POST /token"){
        let mut t=token("attempt");let now=crate::time::now_ms()/1000;t["scope"]=json!(format!("{} {}",crate::google::oauth::SCOPES.join(" "),crate::google::oauth::DRIVE_SCOPE));t["id_token"]=json!(crate::google::oauth::tests::jwt(json!({"iss":"https://accounts.google.com","aud":"test-client","sub":"google-sub-b","nonce":"attempt","iat":now-1,"exp":now+3600})));(200,t)
    }else{(200,json!({"sub":"google-sub-b","email":"b@example.invalid","email_verified":true}))}).await;
    let other = auth(f.settings.clone(), &second.url);
    other
        .exchange_code("other", "v", "http://127.0.0.1/", "attempt")
        .await
        .unwrap();
    let lease = other.lease().await.unwrap();
    assert!(!other.team_ai_consented(&lease, &f.server.url));
    assert_eq!(f.requests.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn gateway_redirects_do_not_forward_the_google_bearer_token() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let f = Fixture::new(200).await;
    let redirected = Arc::new(AtomicUsize::new(0));
    let calls = redirected.clone();
    let other = server(move |_| {
        calls.fetch_add(1, Ordering::SeqCst);
        (200, json!({}))
    })
    .await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let location = other.url.clone();
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buffer = [0u8; 4096];
        let _ = socket.read(&mut buffer).await.unwrap();
        let raw=format!("HTTP/1.1 302 Found\r\nLocation: {location}/steal\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}");
        socket.write_all(raw.as_bytes()).await.unwrap();
    });
    let lease = f.auth.lease().await.unwrap();
    f.auth
        .set_team_ai_consent(&lease, &endpoint, true)
        .await
        .unwrap();
    assert!(Client::new(Some(&endpoint))
        .unwrap()
        .status(
            &f.auth,
            &f.profiles,
            &f.teams,
            f.auth.epoch(),
            "team-folder"
        )
        .await
        .is_err());
    task.await.unwrap();
    assert_eq!(redirected.load(Ordering::SeqCst), 0);
}
