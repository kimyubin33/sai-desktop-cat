use super::*;
#[test]
fn conditional_mutation_revisions_require_one_strong_nonempty_etag() {
    assert!(event_revision("\"123456\""));
    for value in [
        "*",
        "W/\"123\"",
        "\"\"",
        "123",
        "\"one\",\"two\"",
        "\"one\r\nother: value\"",
    ] {
        assert!(!event_revision(value));
    }
    assert!(event_path(
        "/calendar/v3/calendars/primary/events/5a1abcdef?sendUpdates=none",
        "?sendUpdates=none"
    ));
    assert!(!event_path(
        "/calendar/v3/calendars/primary/events/../other?sendUpdates=none",
        "?sendUpdates=none"
    ));
}
use crate::google::oauth::tests::{auth, server, token};
use std::sync::{atomic::AtomicUsize, Arc};
fn cal_token() -> Value {
    let mut v = token("attempt");
    v["scope"] = json!(format!("openid email {CALENDAR_SCOPE}"));
    v["access_token"] = json!("CAL_ACCESS_TEST_ONLY");
    v["refresh_token"] = json!("CAL_REFRESH_TEST_ONLY");
    v
}
async fn connected(
    mode: Arc<AtomicUsize>,
) -> (GoogleAuth, crate::google::oauth::tests::Server, Store) {
    let mock=server(move|r|{
        if r.starts_with("POST /token") {
            if r.contains("code=calendar")||r.contains("refresh_token=CAL_REFRESH_TEST_ONLY") {
                let mut v=cal_token();match mode.load(Ordering::SeqCst){1=>v["scope"]=json!("openid email"),2=>{let now=crate::time::now_ms()/1000;v["id_token"]=json!(crate::google::oauth::tests::jwt(json!({"iss":"https://accounts.google.com","aud":"test-client","sub":"other-subject","nonce":"attempt","iat":now-1,"exp":now+3600})));},3=>return (400,json!({"error":"invalid_grant"})),4=>v["id_token"]=json!("bad-token"),_=>{}}return (200,v);
            }
            let mut v=token("attempt");v["scope"]=json!(format!("{} {DRIVE_SCOPE}",SCOPES.join(" ")));return (200,v);
        }
        if r.starts_with("GET /userinfo"){return (200,json!({"sub":"google-sub-a","email":"user@example.invalid","email_verified":true}));}
        if r.starts_with("GET /calendar/"){assert!(r.to_ascii_lowercase().contains("authorization: bearer cal_access_test_only"));return (200,json!({"id":"test-only"}));}
        if r.starts_with("POST /v1/status"){assert!(!r.contains("CAL_ACCESS_TEST_ONLY"));return (200,json!({}));}
        (404,json!({}))
    }).await;
    let settings = Store::memory(json!({}));
    let auth = auth(settings.clone(), &mock.url);
    auth.exchange_code("base", "v", "http://127.0.0.1/", "attempt")
        .await
        .unwrap();
    (auth, mock, settings)
}
#[test]
fn calendar_consent_is_separate_from_initial_drive_login() {
    let url = url::Url::parse(&auth_url_for(
        "test",
        "http://127.0.0.1",
        "p",
        "s",
        "n",
        Purpose::Calendar,
        Some("user@example.invalid"),
    ))
    .unwrap();
    let p = url
        .query_pairs()
        .collect::<std::collections::BTreeMap<_, _>>();
    assert_eq!(p["scope"], format!("openid email {CALENDAR_SCOPE}"));
    assert_eq!(p["include_granted_scopes"], "false");
    assert_eq!(p["nonce"], "n");
    assert!(!p["scope"].contains("drive"));
    let initial = auth_url("test", "http://127.0.0.1", "p", "s", "n");
    assert!(!initial.contains("calendar"));
}
#[tokio::test]
async fn separate_calendar_grant_preserves_drive_epoch_and_never_returns_a_secret() {
    let (auth, _mock, settings) = connected(Arc::new(AtomicUsize::new(0))).await;
    let lease = auth.lease().await.unwrap();
    let original = auth.tokens().unwrap();
    assert!(!auth.calendar_connected());
    let identity = auth
        .finish_calendar(&lease, "calendar", "v", "http://127.0.0.1/", "attempt")
        .await
        .unwrap();
    assert_eq!(identity.subject, lease.subject());
    assert_eq!(auth.epoch(), lease.epoch());
    assert_eq!(auth.tokens().unwrap(), original);
    assert!(auth.calendar_connected());
    assert!(settings.read(|s| s["googleCalendarTokensEnc"]
        .as_str()
        .unwrap()
        .starts_with("dpapi:")));
    assert!(!settings.get().to_string().contains("CAL_REFRESH_TEST_ONLY"));
    let cal = auth.calendar_lease(lease.epoch()).await.unwrap();
    assert_eq!(
        auth.calendar_request(
            &cal,
            "GET",
            "/calendar/v3/calendars/primary/events/abcde",
            None
        )
        .await
        .unwrap()["id"],
        "test-only"
    );
    auth.sign_out().await.unwrap();
    assert!(!auth.calendar_connected());
    assert!(settings.get()["googleCalendarTokensEnc"].is_null());
    assert!(auth
        .calendar_request(
            &cal,
            "GET",
            "/calendar/v3/calendars/primary/events/abcde",
            None
        )
        .await
        .is_err());
}
#[tokio::test]
async fn refused_wrong_subject_and_invalid_nonce_grants_preserve_existing_credentials() {
    for mode in [1, 2, 4] {
        let state = Arc::new(AtomicUsize::new(0));
        let (auth, _mock, settings) = connected(state.clone()).await;
        let lease = auth.lease().await.unwrap();
        auth.finish_calendar(&lease, "calendar", "v", "http://127.0.0.1/", "attempt")
            .await
            .unwrap();
        let before = settings.get();
        state.store(mode, Ordering::SeqCst);
        assert!(auth
            .finish_calendar(&lease, "calendar", "v", "http://127.0.0.1/", "attempt")
            .await
            .is_err());
        assert_eq!(settings.get(), before);
    }
}
#[tokio::test]
async fn restart_refreshes_calendar_without_a_browser_and_revocation_keeps_drive() {
    let mode = Arc::new(AtomicUsize::new(0));
    let (a, mock, settings) = connected(mode.clone()).await;
    let lease = a.lease().await.unwrap();
    a.finish_calendar(&lease, "calendar", "v", "http://127.0.0.1/", "attempt")
        .await
        .unwrap();
    let restarted = auth(settings.clone(), &mock.url);
    let main = restarted.lease().await.unwrap();
    restarted.calendar_lease(main.epoch()).await.unwrap();
    let revoked = auth(settings, &mock.url);
    let main = revoked.lease().await.unwrap();
    mode.store(3, Ordering::SeqCst);
    assert!(revoked.calendar_lease(main.epoch()).await.is_err());
    assert!(!revoked.calendar_connected());
    assert!(revoked.has_full_drive());
    assert!(revoked.lease_current(&main));
}
#[tokio::test]
async fn stale_epochs_and_another_account_cannot_reuse_calendar_credentials() {
    let (a, _mock, settings) = connected(Arc::new(AtomicUsize::new(0))).await;
    let lease = a.lease().await.unwrap();
    a.finish_calendar(&lease, "calendar", "v", "http://127.0.0.1/", "attempt")
        .await
        .unwrap();
    assert!(a.calendar_lease(lease.epoch() + 1).await.is_err());
    a.with_lease(&lease, || {
        let mut t = a.tokens().unwrap();
        t["subject"] = json!("google-sub-b");
        a.save_tokens(Some(t))
    })
    .await
    .unwrap();
    assert!(!a.calendar_connected());
    assert!(a.calendar_tokens().is_some());
    assert!(!settings.get().to_string().contains("CAL_REFRESH_TEST_ONLY"));
}
#[tokio::test]
async fn calendar_scopes_in_a_drive_token_are_not_forwarded_to_the_team_gateway() {
    let (a, mock, _) = connected(Arc::new(AtomicUsize::new(0))).await;
    let lease = a.lease().await.unwrap();
    a.set_team_ai_consent(&lease, &mock.url, true)
        .await
        .unwrap();
    a.with_lease(&lease, || {
        let mut t = a.tokens().unwrap();
        t["scope"] = json!(format!("{} {CALENDAR_SCOPE}", t.s("scope")));
        a.save_tokens(Some(t))
    })
    .await
    .unwrap();
    assert!(a
        .team_ai_request(&lease, &mock.url, "/v1/status", json!({}))
        .await
        .unwrap_err()
        .contains("Calendar"));
}
