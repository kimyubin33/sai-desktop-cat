//! Explicit operator-only temporary grant/revoke. No notifications, ownership changes or file deletion.
use sai_core::{
    google::oauth::{parse_google_client, GoogleAuth},
    store::{default_settings, Store},
};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs,
    io::Write,
    path::{Path, PathBuf},
};
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Plan {
    folder_id: String,
    owner_subject: String,
    owner_permission_id: String,
    member_email: String,
}
fn id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}
fn target(p: &Value, plan: &Plan) -> bool {
    p["type"] == "user"
        && p["role"] == "writer"
        && p["deleted"] != true
        && p["emailAddress"]
            .as_str()
            .is_some_and(|e| e.eq_ignore_ascii_case(&plan.member_email))
}
fn baseline(p: &[Value], plan: &Plan) -> bool {
    p.len() == 1
        && p[0]["id"] == plan.owner_permission_id
        && p[0]["type"] == "user"
        && p[0]["role"] == "owner"
}
fn removable(receipt: &Value, p: &Value, plan: &Plan) -> bool {
    receipt["folderId"] == plan.folder_id
        && receipt["ownerSubject"] == plan.owner_subject
        && receipt["ownerPermissionId"] == plan.owner_permission_id
        && receipt["memberEmail"] == plan.member_email
        && receipt["permissionId"] == p["id"]
        && p["id"] != plan.owner_permission_id
        && target(p, plan)
        && receipt["baselinePermissionIds"]
            .as_array()
            .is_some_and(|a| !a.contains(&p["id"]))
}
fn save(path: &Path, v: &Value) -> Result<(), String> {
    fs::write(
        path,
        serde_json::to_vec_pretty(v).map_err(|_| "Cannot serialize the permission receipt.")?,
    )
    .map_err(|_| "Cannot save the permission receipt.".into())
}
async fn permissions(auth: &GoogleAuth, fid: &str) -> Result<Vec<Value>, String> {
    let mut result = vec![];
    let mut token = String::new();
    let mut seen = BTreeSet::new();
    for _ in 0..20 {
        let continuation = if token.is_empty() {
            String::new()
        } else {
            format!("&pageToken={}", urlencoding::encode(&token))
        };
        let v=auth.api("GET",&format!("https://www.googleapis.com/drive/v3/files/{fid}/permissions?pageSize=100&fields=nextPageToken,permissions(id,type,role,emailAddress,deleted){continuation}"),None).await.map_err(|_|"Cannot read the current team permission list.")?;
        let page = v["permissions"]
            .as_array()
            .filter(|a| a.len() <= 100)
            .ok_or("Invalid permission list.")?;
        result.extend(page.iter().cloned());
        if v["nextPageToken"].is_null() {
            return Ok(result);
        }
        token = v["nextPageToken"]
            .as_str()
            .filter(|s| !s.is_empty() && s.len() <= 2048)
            .ok_or("Invalid permission continuation.")?
            .into();
        if !seen.insert(token.clone()) {
            return Err("Repeated permission continuation.".into());
        }
    }
    Err("The permission list exceeded its bounded page limit.".into())
}
async fn run() -> Result<Value, String> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 4 {
        return Err(
            "Pass owner profile, desktop client file, member plan and grant/revoke action.".into(),
        );
    }
    let action = args[3]
        .to_str()
        .filter(|s| ["grant", "revoke"].contains(s))
        .ok_or("Invalid permission action.")?;
    let root =
        fs::canonicalize(PathBuf::from(&args[0])).map_err(|_| "Cannot open the owner profile.")?;
    let plan_path =
        fs::canonicalize(PathBuf::from(&args[2])).map_err(|_| "Cannot open the member plan.")?;
    let plan: Plan =
        serde_json::from_slice(&fs::read(&plan_path).map_err(|_| "Cannot read the member plan.")?)
            .map_err(|_| "The explicit member plan is incomplete.")?;
    if ![
        &plan.folder_id,
        &plan.owner_subject,
        &plan.owner_permission_id,
    ]
    .iter()
    .all(|s| id(s))
        || plan.member_email.len() > 254
        || !plan.member_email.contains('@')
        || plan.member_email.chars().any(char::is_whitespace)
    {
        return Err("The explicit team or recipient is invalid.".into());
    }
    let receipt_path = plan_path
        .parent()
        .ok_or("Cannot locate the receipt directory.")?
        .join("member-grant-receipt.json");
    let settings_path = root.join("settings.json");
    let before =
        Sha256::digest(fs::read(&settings_path).map_err(|_| "Cannot read the owner settings.")?);
    let source = Store::open(&settings_path, default_settings());
    if source.load_error().is_some() {
        return Err("Cannot parse the owner settings.".into());
    }
    let session = Store::memory(source.read(|s| json!({"googleTokensEnc":s["googleTokensEnc"]})));
    let client = parse_google_client(
        &fs::read_to_string(PathBuf::from(&args[1]))
            .map_err(|_| "Cannot read the desktop client file.")?,
    )
    .ok_or("Invalid desktop client configuration.")?;
    let auth = GoogleAuth::new(&client, session);
    if !auth.has_session() {
        return Err("An existing protected owner session is required.".into());
    }
    let identity = auth
        .verified_identity()
        .await
        .map_err(|_| "Cannot verify the owner session.")?;
    if identity.subject != plan.owner_subject
        || identity.email.eq_ignore_ascii_case(&plan.member_email)
        || !auth.has_full_drive()
    {
        return Err("The selected session is not the distinct verified owner.".into());
    }
    let about = auth
        .api(
            "GET",
            "https://www.googleapis.com/drive/v3/about?fields=user(permissionId)",
            None,
        )
        .await
        .map_err(|_| "Cannot verify the Drive owner.")?;
    let folder=auth.api("GET",&format!("https://www.googleapis.com/drive/v3/files/{}?fields=id,mimeType,trashed,ownedByMe,driveId,properties,owners(permissionId,me)",plan.folder_id),None).await.map_err(|_|"Cannot verify the selected team folder.")?;
    if about["user"]["permissionId"] != plan.owner_permission_id
        || folder["id"] != plan.folder_id
        || folder["mimeType"] != "application/vnd.google-apps.folder"
        || folder["trashed"] != false
        || folder["ownedByMe"] != true
        || !folder["driveId"].is_null()
        || folder["properties"]["saiTeam"] != "1"
        || !folder["owners"].as_array().is_some_and(|a| {
            a.len() == 1 && a[0]["permissionId"] == plan.owner_permission_id && a[0]["me"] == true
        })
    {
        return Err("The selected folder is not the verified owner's Sai test team.".into());
    }
    let current = permissions(&auth, &plan.folder_id).await?;
    if before
        != Sha256::digest(
            fs::read(&settings_path).map_err(|_| "Cannot recheck the owner settings.")?,
        )
    {
        return Err("Owner settings changed; no permission mutation was started.".into());
    }
    let mut receipt;
    if action == "grant" {
        if !baseline(&current, &plan) {
            return Err(
                "The test team is no longer owner-only; existing permissions were preserved."
                    .into(),
            );
        }
        receipt = json!({"folderId":plan.folder_id,"ownerSubject":plan.owner_subject,"ownerPermissionId":plan.owner_permission_id,"memberEmail":plan.member_email,"baselinePermissionIds":current.iter().map(|p|p["id"].clone()).collect::<Vec<_>>(),"notificationEmailSent":false,"state":"grant_intent_no_automatic_retry"});
        let mut file=fs::OpenOptions::new().write(true).create_new(true).open(&receipt_path).map_err(|_|"A permission receipt already exists or cannot be created; inspect it before another grant.")?;
        file.write_all(
            serde_json::to_string_pretty(&receipt)
                .map_err(|_| "Cannot serialize the grant intent.")?
                .as_bytes(),
        )
        .and_then(|_| file.sync_all())
        .map_err(|_| "Cannot persist the grant intent; no grant was sent.")?;
        drop(file);
        let granted=auth.api("POST",&format!("https://www.googleapis.com/drive/v3/files/{}/permissions?sendNotificationEmail=false&fields=id,type,role,emailAddress",plan.folder_id),Some(json!({"type":"user","role":"writer","emailAddress":plan.member_email}))).await;
        let created = match granted {
            Ok(v) => v,
            Err(_) => {
                receipt["state"] = json!("grant_outcome_unknown_inspect_permissions_do_not_repeat");
                save(&receipt_path, &receipt)?;
                return Err(
                    "The grant outcome is uncertain; inspect current permissions before recovery."
                        .into(),
                );
            }
        };
        receipt["permissionId"] = created["id"].clone();
        receipt["state"] = json!("grant_response_received");
        save(&receipt_path, &receipt)?;
        if !created["id"].as_str().is_some_and(id) {
            return Err(
                "The grant response has no valid permission ID; inspect current permissions."
                    .into(),
            );
        }
        let after = permissions(&auth, &plan.folder_id).await?;
        if !after.iter().any(|p| removable(&receipt, p, &plan)) {
            return Err(
                "The new member grant could not be verified; retain the receipt for cleanup."
                    .into(),
            );
        }
        receipt["state"] = json!("grant_verified");
    } else {
        receipt = serde_json::from_slice(
            &fs::read(&receipt_path)
                .map_err(|_| "An existing new-grant receipt is required for revocation.")?,
        )
        .map_err(|_| "Cannot parse the grant receipt.")?;
        let permission_id = receipt["permissionId"]
            .as_str()
            .filter(|s| id(s))
            .ok_or("The grant receipt has no valid permission ID.")?;
        if let Some(p) = current.iter().find(|p| p["id"] == permission_id) {
            if !removable(&receipt, p, &plan) {
                return Err("This permission no longer matches the new temporary writer grant; nothing was removed.".into());
            }
            auth.api(
                "DELETE",
                &format!(
                    "https://www.googleapis.com/drive/v3/files/{}/permissions/{permission_id}",
                    plan.folder_id
                ),
                None,
            )
            .await
            .map_err(|_| {
                "The revocation outcome is uncertain; inspect current permissions before recovery."
            })?;
        } else if receipt["folderId"] != plan.folder_id
            || receipt["memberEmail"] != plan.member_email
            || receipt["ownerSubject"] != plan.owner_subject
        {
            return Err("The grant receipt belongs to a different verification run.".into());
        }
        let after = permissions(&auth, &plan.folder_id).await?;
        if after.iter().any(|p| {
            p["id"] == permission_id
                || p["emailAddress"]
                    .as_str()
                    .is_some_and(|e| e.eq_ignore_ascii_case(&plan.member_email))
        }) {
            return Err("The member permission is still present; cleanup is not complete.".into());
        }
        receipt["state"] = json!("revocation_verified");
    }
    receipt["sourceSettingsUnchanged"] = json!(
        before
            == Sha256::digest(
                fs::read(&settings_path).map_err(|_| "Cannot recheck the owner settings.")?
            )
    );
    save(&receipt_path, &receipt)?;
    Ok(
        json!({"action":action,"state":receipt["state"],"notificationEmailSent":false,"sourceSettingsUnchanged":receipt["sourceSettingsUnchanged"],"onlySelectedTeamPermissionChanged":true,"analysisRequests":0}),
    )
}
#[tokio::main]
async fn main() {
    match run().await {
        Ok(v) => println!("{v}"),
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(1)
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn plan() -> Plan {
        Plan {
            folder_id: "team".into(),
            owner_subject: "owner-sub".into(),
            owner_permission_id: "owner".into(),
            member_email: "member@example.test".into(),
        }
    }
    #[test]
    fn existing_permissions_cannot_be_replaced_by_a_temporary_grant() {
        let p = plan();
        let owner = json!({"id":"owner","type":"user","role":"owner"});
        assert!(baseline(&[owner.clone()], &p));
        assert!(!baseline(
            &[
                owner,
                json!({"id":"existing","type":"user","role":"writer"})
            ],
            &p
        ));
    }
    #[test]
    fn only_a_new_matching_writer_permission_can_be_revoked() {
        let p = plan();
        let r = json!({"folderId":"team","ownerSubject":"owner-sub","ownerPermissionId":"owner","memberEmail":"member@example.test","permissionId":"member","baselinePermissionIds":["owner"]});
        let good = json!({"id":"member","type":"user","role":"writer","emailAddress":"member@example.test"});
        assert!(removable(&r, &good, &p));
        for (key, value) in [
            ("id", json!("owner")),
            ("role", json!("owner")),
            ("role", json!("reader")),
            ("type", json!("group")),
            ("emailAddress", json!("other@example.test")),
        ] {
            let mut invalid = good.clone();
            invalid[key] = value;
            assert!(!removable(&r, &invalid, &p));
        }
        let mut old = r.clone();
        old["baselinePermissionIds"] = json!(["owner", "member"]);
        assert!(!removable(&old, &good, &p));
        let mut other = r;
        other["folderId"] = json!("other-team");
        assert!(!removable(&other, &good, &p));
    }
}
