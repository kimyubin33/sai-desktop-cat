//! Read-only operator probe for an explicitly selected member account and synthetic team file.
//! Never signs in, grants consent, changes permissions, or calls an analysis endpoint.
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
    folder_id: String,
    owner_subject: String,
    owner_permission_id: String,
    member_email: String,
    source_file_id: String,
    source_file_name: String,
    source_sha256: String,
    endpoint: String,
}
fn id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
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
fn member_matches(email: &str, subject: &str, plan: &Plan) -> bool {
    email.eq_ignore_ascii_case(&plan.member_email) && subject != plan.owner_subject
}
fn valid_folder(v: &Value, p: &Plan) -> bool {
    v["id"] == p.folder_id
        && v["mimeType"] == "application/vnd.google-apps.folder"
        && v["trashed"] == false
        && v["properties"]["saiTeam"] == "1"
        && v["driveId"].is_null()
        && v["capabilities"]["canListChildren"] == true
        && v["owners"]
            .as_array()
            .is_some_and(|a| a.len() == 1 && a[0]["permissionId"] == p.owner_permission_id)
}
async fn bytes(mut response: reqwest::Response, limit: usize) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "Cannot finish the bounded response.")?
    {
        if chunk.len() > limit.saturating_sub(body.len()) {
            return Err("The response exceeded its allowed size.".into());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}
fn phase_met(phase: &str, v: &Value) -> bool {
    let denied = |key: &str| matches!(v[key].as_u64(), Some(403 | 404));
    let gateway = &v["gateway"];
    match phase {
        "member" => {
            v["folderStatus"] == 200
                && v["fileMetadataStatus"] == 200
                && v["sourceBytesVerified"] == true
                && gateway["requested"] == true
                && gateway["status"] == 200
                && gateway["owner"] == false
                && gateway["analysisDisabled"] == true
                && gateway["ready"] == false
        }
        "unshared" => denied("folderStatus") && denied("fileMetadataStatus"),
        "revoked" => {
            denied("folderStatus")
                && denied("fileMetadataStatus")
                && gateway["requested"] == true
                && gateway["status"] == 403
                && matches!(
                    gateway["error"].as_str(),
                    Some("membership_required" | "team_access_denied" | "google_access_denied")
                )
        }
        _ => false,
    }
}
async fn run() -> Result<Value, String> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 4 {
        return Err(
            "Pass app data directory, desktop client file, verification plan and phase.".into(),
        );
    }
    let phase = args[3]
        .to_str()
        .filter(|s| ["unshared", "member", "revoked"].contains(s))
        .ok_or("Invalid verification phase.")?;
    let root = fs::canonicalize(PathBuf::from(&args[0]))
        .map_err(|_| "Cannot open the selected app profile.")?;
    let client_file = fs::canonicalize(PathBuf::from(&args[1]))
        .map_err(|_| "Cannot open the desktop client file.")?;
    let plan: Plan = serde_json::from_slice(
        &fs::read(PathBuf::from(&args[2])).map_err(|_| "Cannot read the verification plan.")?,
    )
    .map_err(|_| "The verification plan is incomplete or invalid.")?;
    let endpoint = origin(&plan.endpoint).ok_or("Invalid verification service origin.")?;
    if ![
        &plan.folder_id,
        &plan.owner_subject,
        &plan.owner_permission_id,
        &plan.source_file_id,
    ]
    .iter()
    .all(|s| id(s))
        || plan.member_email.len() > 254
        || !plan.member_email.contains('@')
        || plan.member_email.chars().any(char::is_whitespace)
        || plan.source_file_name != "Sai-File-Check-20261117-083131.txt"
        || plan.source_sha256.len() != 64
        || !plan.source_sha256.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err("The explicit member, team or synthetic source is invalid.".into());
    }
    let settings_path = root.join("settings.json");
    let before = Sha256::digest(
        fs::read(&settings_path).map_err(|_| "Cannot read the selected app settings.")?,
    );
    let settings = Store::open(&settings_path, default_settings());
    if settings.load_error().is_some() {
        return Err("Cannot parse the selected app settings; nothing was changed.".into());
    }
    let session = Store::memory(settings.read(
        |s| json!({"googleTokensEnc":s["googleTokensEnc"],"teamAiConsents":s["teamAiConsents"]}),
    ));
    let client = parse_google_client(
        &fs::read_to_string(client_file)
            .map_err(|_| "Cannot read the desktop client configuration.")?,
    )
    .ok_or("Invalid desktop client configuration.")?;
    let auth = GoogleAuth::new(&client, session);
    if !auth.has_session() {
        return Err("Sign in to the separate member preview first; no login was started.".into());
    }
    let identity = auth
        .verified_identity()
        .await
        .map_err(|_| "Cannot verify the selected Google session.")?;
    if !member_matches(&identity.email, &identity.subject, &plan) {
        return Err(
            "The selected profile is not the explicitly chosen separate member account.".into(),
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
        return Err(
            "A separate Drive session without Calendar scope is required; no token was forwarded."
                .into(),
        );
    }
    let folder_url = format!("https://www.googleapis.com/drive/v3/files/{}?fields=id,mimeType,trashed,driveId,owners(permissionId),properties,capabilities(canListChildren)", plan.folder_id);
    let folder = auth.api("GET", &folder_url, None).await;
    if folder.as_ref().is_ok_and(|v| !valid_folder(v, &plan)) {
        return Err("The accessible folder does not match the verified team and owner.".into());
    }
    let source_url = format!(
        "https://www.googleapis.com/drive/v3/files/{}?fields=id,name,mimeType,size,parents,trashed",
        plan.source_file_id
    );
    let source = auth.api("GET", &source_url, None).await;
    let source_expected = source.as_ref().is_ok_and(|v| {
        v["id"] == plan.source_file_id
            && v["name"] == plan.source_file_name
            && v["mimeType"] == "text/plain"
            && v["size"] == "136"
            && v["trashed"] == false
            && v["parents"]
                .as_array()
                .is_some_and(|p| p.iter().any(|v| v == &plan.folder_id))
    });
    if source.is_ok() && !source_expected {
        return Err("The accessible source is not the reviewed synthetic team file.".into());
    }
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(25))
        .build()
        .map_err(|_| "Cannot prepare the read-only transport.")?;
    let mut result = json!({"phase":phase,"memberIdentityVerified":true,"separateFromOwner":true,
        "folderStatus":folder.as_ref().map(|_|200).unwrap_or_else(|e|e.0),
        "fileMetadataStatus":source.as_ref().map(|_|200).unwrap_or_else(|e|e.0),
        "sourceBytesVerified":false,"gateway":{"requested":false,"reason":"account_origin_consent_required"},
        "analysisRequests":0,"permissionWrites":0});
    let token = zeroize::Zeroizing::new(
        auth.access_token()
            .await
            .map_err(|_| "Cannot refresh the selected protected session.")?,
    );
    if folder.is_ok() && source_expected {
        let response = http
            .get(format!(
                "https://www.googleapis.com/drive/v3/files/{}?alt=media",
                plan.source_file_id
            ))
            .bearer_auth(token.as_str())
            .send()
            .await
            .map_err(|_| "Cannot read the synthetic file.")?;
        result["fileDownloadStatus"] = json!(response.status().as_u16());
        if response.status().is_success() {
            let content = bytes(response, 136).await?;
            result["sourceBytesVerified"] = json!(
                content.len() == 136
                    && format!("{:x}", Sha256::digest(&content))
                        .eq_ignore_ascii_case(&plan.source_sha256)
            );
        }
    }
    let consent_key = format!(
        "{:x}",
        Sha256::digest(format!("team-ai-v1\n{}\n{endpoint}", identity.subject).as_bytes())
    );
    if settings.read(|s| s["teamAiConsents"][&consent_key] == true) {
        let response = http
            .post(format!("{endpoint}/v1/status"))
            .bearer_auth(token.as_str())
            .json(&json!({"folderId":plan.folder_id}))
            .send()
            .await
            .map_err(|_| "Cannot query the verification service status.")?;
        let status = response.status().as_u16();
        let value: Value = serde_json::from_slice(&bytes(response, 4096).await?)
            .map_err(|_| "The status response is not valid JSON.")?;
        let code = match value["error"].as_str().unwrap_or("") {
            c @ ("team_not_enabled"
            | "membership_required"
            | "team_access_denied"
            | "google_access_denied"
            | "login_required"
            | "invalid_identity"
            | "invalid_membership"
            | "not_configured"
            | "service_unavailable") => c,
            "" if status == 200 => "",
            _ => "unclassified_service_error",
        };
        result["gateway"] = json!({"requested":true,"status":status,"error":code,"owner":value["owner"].as_bool(),"analysisDisabled":value["analysisDisabled"]==true,"ready":value["ready"].as_bool(),"used":value["used"].as_u64(),"dailyRequests":value["dailyRequests"].as_u64()});
    }
    let after = Sha256::digest(
        fs::read(&settings_path).map_err(|_| "Cannot recheck the selected app settings.")?,
    );
    if before != after {
        return Err("App settings changed during the read; no result was emitted.".into());
    }
    result["sourceSettingsUnchanged"] = json!(true);
    result["phaseExpectationMet"] = json!(phase_met(phase, &result));
    Ok(result)
}
#[tokio::main]
async fn main() {
    match run().await {
        Ok(v) => println!("{v}"),
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn plan() -> Plan {
        Plan {
            folder_id: "team".into(),
            owner_subject: "owner-subject".into(),
            owner_permission_id: "owner-permission".into(),
            member_email: "member@example.test".into(),
            source_file_id: "source".into(),
            source_file_name: "Sai-File-Check-20261117-083131.txt".into(),
            source_sha256: "a".repeat(64),
            endpoint: "https://gateway.example.test".into(),
        }
    }
    #[test]
    fn wrong_account_and_owner_cannot_be_mistaken_for_a_member() {
        let p = plan();
        assert!(member_matches("MEMBER@example.test", "member-subject", &p));
        assert!(!member_matches(
            "another@example.test",
            "member-subject",
            &p
        ));
        assert!(!member_matches("member@example.test", "owner-subject", &p));
    }
    #[test]
    fn missing_consent_wrong_roles_and_global_shutdown_do_not_prove_member_access_or_revocation() {
        let valid = json!({"folderStatus":200,"fileMetadataStatus":200,"sourceBytesVerified":true,
            "gateway":{"requested":true,"status":200,"owner":false,"analysisDisabled":true,"ready":false}});
        assert!(phase_met("member", &valid));
        for (key, value) in [
            ("requested", json!(false)),
            ("owner", json!(true)),
            ("analysisDisabled", json!(false)),
            ("ready", json!(true)),
        ] {
            let mut invalid = valid.clone();
            invalid["gateway"][key] = value;
            assert!(!phase_met("member", &invalid));
        }
        let mut invalid = valid;
        invalid["sourceBytesVerified"] = json!(false);
        assert!(!phase_met("member", &invalid));
        let mut revoked = json!({"folderStatus":404,"fileMetadataStatus":404,"gateway":{"requested":true,"status":403,"error":"google_access_denied"}});
        assert!(phase_met("revoked", &revoked));
        assert!(phase_met("unshared", &revoked));
        revoked["gateway"]["error"] = json!("team_not_enabled");
        assert!(!phase_met("revoked", &revoked));
        revoked["fileMetadataStatus"] = json!(200);
        assert!(!phase_met("unshared", &revoked));
    }
    #[test]
    fn unsafe_or_nonorigin_destinations_cannot_receive_tokens() {
        assert_eq!(
            origin("https://gateway.example.test/"),
            Some("https://gateway.example.test".into())
        );
        for s in [
            "http://gateway.example.test",
            "https://127.0.0.1",
            "https://localhost",
            "https://user@gateway.example.test",
            "https://gateway.example.test/path",
            "https://gateway.example.test/?key=secret",
            "https://gateway.example.test/#fragment",
        ] {
            assert!(origin(s).is_none());
        }
    }
    #[test]
    fn unmarked_foreign_owned_and_shared_drive_folders_are_rejected() {
        let p = plan();
        let v = json!({"id":"team","mimeType":"application/vnd.google-apps.folder","trashed":false,"owners":[{"permissionId":"owner-permission"}],"properties":{"saiTeam":"1"},"capabilities":{"canListChildren":true}});
        assert!(valid_folder(&v, &p));
        for (key, value) in [
            ("properties", json!({})),
            ("owners", json!([{"permissionId":"other-owner"}])),
            ("driveId", json!("shared-drive")),
            ("trashed", json!(true)),
            ("id", json!("other-team")),
        ] {
            let mut invalid = v.clone();
            invalid[key] = value;
            assert!(!valid_folder(&invalid, &p));
        }
    }
}
