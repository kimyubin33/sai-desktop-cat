//! Operator-only activation probe. Existing consent is required; only /v1/status is called.
use sai_core::{
    google::oauth::{parse_google_client, GoogleAuth},
    store::{default_settings, Store},
};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf, time::Duration};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Plan {
    expected_subject: String,
    endpoint: String,
    allowed_folder_id: String,
    denied_folder_id: String,
    daily_requests: u64,
}
fn id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}
fn origin(s: &str) -> Option<String> {
    let u = url::Url::parse(s).ok()?;
    (s.len() <= 2048
        && u.scheme() == "https"
        && u.username().is_empty()
        && u.password().is_none()
        && u.query().is_none()
        && u.fragment().is_none()
        && u.path() == "/"
        && matches!(u.host(), Some(url::Host::Domain(h)) if h != "localhost" && h.contains('.')))
    .then(|| u.origin().ascii_serialization())
}
fn valid_plan(p: &Plan) -> bool {
    id(&p.expected_subject)
        && id(&p.allowed_folder_id)
        && id(&p.denied_folder_id)
        && p.allowed_folder_id != p.denied_folder_id
        && (1..=100).contains(&p.daily_requests)
        && origin(&p.endpoint).is_some()
}
fn allowed(status: u16, v: &Value, limit: u64) -> bool {
    status == 200
        && v["ready"] == true
        && v["owner"] == true
        && v["provider"] == "claude"
        && v["analysisDisabled"] != true
        && v["dailyRequests"] == limit
        && v["used"].as_u64().is_some_and(|n| n <= 100)
}
fn denied(status: u16, v: &Value) -> bool {
    status == 403 && v["error"] == "team_not_enabled"
}
async fn status(
    http: &reqwest::Client,
    endpoint: &str,
    token: &str,
    folder: &str,
) -> Result<(u16, Value), String> {
    let mut response = http
        .post(format!("{endpoint}/v1/status"))
        .bearer_auth(token)
        .json(&json!({"folderId":folder}))
        .send()
        .await
        .map_err(|_| "The status-only request failed; no analysis was sent.")?;
    let code = response.status().as_u16();
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "Cannot finish the status response.")?
    {
        if chunk.len() > 4096usize.saturating_sub(body.len()) {
            return Err("The status response exceeded its size limit.".into());
        }
        body.extend_from_slice(&chunk);
    }
    let value = serde_json::from_slice(&body).map_err(|_| "The status response was not JSON.")?;
    Ok((code, value))
}
async fn run() -> Result<Value, String> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 3 {
        return Err(
            "Pass the existing app profile, desktop client file and explicit status plan.".into(),
        );
    }
    let root = fs::canonicalize(PathBuf::from(&args[0]))
        .map_err(|_| "Cannot open the selected app profile.")?;
    let plan: Plan = serde_json::from_slice(
        &fs::read(PathBuf::from(&args[2])).map_err(|_| "Cannot read the status plan.")?,
    )
    .map_err(|_| "The status plan is incomplete or invalid.")?;
    if !valid_plan(&plan) {
        return Err("The status plan has invalid identities, teams, limits or destination.".into());
    }
    let endpoint = origin(&plan.endpoint).ok_or("Invalid status service origin.")?;
    let settings_path = root.join("settings.json");
    let before = Sha256::digest(
        fs::read(&settings_path).map_err(|_| "Cannot read the selected app settings.")?,
    );
    let settings = Store::open(&settings_path, default_settings());
    if settings.load_error().is_some() {
        return Err("Cannot parse app settings; nothing was changed.".into());
    }
    let session = Store::memory(settings.read(
        |s| json!({"googleTokensEnc":s["googleTokensEnc"],"teamAiConsents":s["teamAiConsents"]}),
    ));
    let client = parse_google_client(
        &fs::read_to_string(PathBuf::from(&args[1]))
            .map_err(|_| "Cannot read the desktop client configuration.")?,
    )
    .ok_or("Invalid desktop client configuration.")?;
    let auth = GoogleAuth::new(&client, session);
    if !auth.has_session() {
        return Err("An existing protected session is required; no login was started.".into());
    }
    let identity = auth
        .verified_identity()
        .await
        .map_err(|_| "Cannot verify the selected Google session.")?;
    if identity.subject != plan.expected_subject {
        return Err("The selected profile belongs to a different account.".into());
    }
    let consent = format!(
        "{:x}",
        Sha256::digest(format!("team-ai-v1\n{}\n{endpoint}", identity.subject).as_bytes())
    );
    if !settings.read(|s| s["teamAiConsents"][&consent] == true) {
        return Err(
            "Existing consent for this account and origin is required; no token was forwarded."
                .into(),
        );
    }
    if !auth.has_full_drive()
        || auth.tokens().is_some_and(|t| {
            t["scope"]
                .as_str()
                .unwrap_or("")
                .split_whitespace()
                .any(|s| s.starts_with("https://www.googleapis.com/auth/calendar"))
        })
    {
        return Err("A separate full Drive session without Calendar scope is required; no token was forwarded.".into());
    }
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(25))
        .build()
        .map_err(|_| "Cannot prepare the status-only transport.")?;
    let token = zeroize::Zeroizing::new(
        auth.access_token()
            .await
            .map_err(|_| "Cannot refresh the selected session.")?,
    );
    let (allowed_code, first) =
        status(&http, &endpoint, token.as_str(), &plan.allowed_folder_id).await?;
    let (denied_code, other) =
        status(&http, &endpoint, token.as_str(), &plan.denied_folder_id).await?;
    let (repeat_code, repeat) =
        status(&http, &endpoint, token.as_str(), &plan.allowed_folder_id).await?;
    let after =
        Sha256::digest(fs::read(&settings_path).map_err(|_| "Cannot recheck app settings.")?);
    if before != after {
        return Err("App settings changed during the probe; no result was emitted.".into());
    }
    let passed = allowed(allowed_code, &first, plan.daily_requests)
        && denied(denied_code, &other)
        && allowed(repeat_code, &repeat, plan.daily_requests)
        && first["used"] == repeat["used"];
    Ok(
        json!({"mode":"authenticated_status_only","expectedIdentityVerified":true,"existingAccountOriginConsentVerified":true,
        "allowed":{"status":allowed_code,"owner":first["owner"].as_bool(),"ready":first["ready"].as_bool(),"dailyRequests":first["dailyRequests"].as_u64(),"used":first["used"].as_u64()},
        "denied":{"status":denied_code,"expectedTeamNotEnabledVerified":denied(denied_code,&other)},
        "repeat":{"status":repeat_code,"used":repeat["used"].as_u64(),"usageUnchanged":first["used"]==repeat["used"]},
        "sourceSettingsUnchanged":true,"verificationPassed":passed,"statusRequests":3,"analysisRequests":0,"permissionWrites":0}),
    )
}
#[tokio::main]
async fn main() {
    match run().await {
        Ok(result) => {
            println!("{result}");
            if result["verificationPassed"] != true {
                std::process::exit(1);
            }
        }
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(1);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn destinations_cannot_redirect_tokens_to_paths_credentials_or_local_addresses() {
        assert_eq!(
            origin("https://relay.example.test/"),
            Some("https://relay.example.test".into())
        );
        for bad in [
            "http://relay.example.test",
            "https://localhost",
            "https://127.0.0.1",
            "https://[::1]",
            "https://x@relay.example.test",
            "https://relay.example.test/analyze",
            "https://relay.example.test/?token=x",
            "https://relay.example.test/#x",
        ] {
            assert!(origin(bad).is_none());
        }
    }
    #[test]
    fn status_only_mode_missing_fields_and_wrong_limits_cannot_prove_normal_activation() {
        let valid =
            json!({"ready":true,"owner":true,"provider":"claude","dailyRequests":10,"used":0});
        assert!(allowed(200, &valid, 10));
        for (key, value) in [
            ("ready", json!(false)),
            ("owner", json!(false)),
            ("provider", json!("other")),
            ("dailyRequests", json!(20)),
            ("used", json!(null)),
            ("analysisDisabled", json!(true)),
        ] {
            let mut invalid = valid.clone();
            invalid[key] = value;
            assert!(!allowed(200, &invalid, 10));
        }
        assert!(!allowed(403, &valid, 10));
    }
    #[test]
    fn outages_and_membership_errors_are_not_a_successful_team_allowlist_check() {
        assert!(denied(403, &json!({"error":"team_not_enabled"})));
        assert!(!denied(503, &json!({"error":"team_not_enabled"})));
        assert!(!denied(403, &json!({"error":"google_access_denied"})));
        assert!(!denied(403, &json!({})));
    }
    #[test]
    fn plans_must_compare_distinct_teams_and_cannot_include_an_analysis_payload() {
        let mut p: Plan=serde_json::from_value(json!({"expectedSubject":"owner","endpoint":"https://relay.example.test","allowedFolderId":"allowed","deniedFolderId":"denied","dailyRequests":10})).unwrap();
        assert!(valid_plan(&p));
        p.denied_folder_id = p.allowed_folder_id.clone();
        assert!(!valid_plan(&p));
        assert!(serde_json::from_value::<Plan>(json!({"expectedSubject":"owner","endpoint":"https://relay.example.test","allowedFolderId":"allowed","deniedFolderId":"denied","dailyRequests":10,"text":"never send this"})).is_err());
    }
}
