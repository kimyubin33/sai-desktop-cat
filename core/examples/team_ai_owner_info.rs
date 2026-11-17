//! Operator-only metadata read using the app's protected Google session. No source writes or token output.
use sai_core::{
    google::oauth::{parse_google_client, GoogleAuth},
    store::{default_settings, Store},
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf};

fn id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}
fn checked(folder: &Value, about: &Value, expected: &str) -> Result<Value, &'static str> {
    let owners = folder["owners"]
        .as_array()
        .ok_or("Folder owner metadata is missing.")?;
    let owner = about["user"]["permissionId"]
        .as_str()
        .filter(|v| id(v))
        .ok_or("The current account permission ID is missing.")?;
    if folder["id"] != expected
        || folder["mimeType"] != "application/vnd.google-apps.folder"
        || folder["trashed"] != false
        || folder["ownedByMe"] != true
        || folder["properties"]["saiTeam"] != "1"
        || folder["capabilities"]["canListChildren"] != true
        || !folder["driveId"].is_null()
        || owners.len() != 1
        || owners[0]["permissionId"] != owner
        || owners[0]["me"] != true
    {
        return Err("The current account is not the verified sole owner of this Sai team.");
    }
    let name = folder["name"]
        .as_str()
        .filter(|n| {
            !n.trim().is_empty() && n.chars().count() <= 100 && !n.chars().any(char::is_control)
        })
        .ok_or("The team name is invalid.")?;
    Ok(
        json!({"folderId":expected,"folderName":name,"ownerPermissionId":owner,
        "ownerPermissionIdVerified":true,"currentAccountIsSoleOwner":true}),
    )
}
async fn run() -> Result<(), String> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 4 {
        return Err("Pass app data directory, desktop client file, folder ID and expected subject explicitly.".into());
    }
    let root = fs::canonicalize(PathBuf::from(&args[0]))
        .map_err(|_| "Cannot open the selected app data directory.")?;
    let client_path = fs::canonicalize(PathBuf::from(&args[1]))
        .map_err(|_| "Cannot open the desktop client configuration.")?;
    let fid = args[2]
        .to_str()
        .filter(|v| id(v))
        .ok_or("Invalid folder ID.")?;
    let subject = args[3]
        .to_str()
        .filter(|v| id(v))
        .ok_or("Invalid expected account subject.")?;
    let settings_path = root.join("settings.json");
    let before = Sha256::digest(
        fs::read(&settings_path).map_err(|_| "Cannot read the selected app settings.")?,
    );
    let source = Store::open(&settings_path, default_settings());
    if source.load_error().is_some() {
        return Err("The selected app settings could not be parsed; nothing was changed.".into());
    }
    // Keep refreshes in memory so a running app can never have its settings overwritten by this reader.
    let session = Store::memory(source.read(|s| json!({"googleTokensEnc":s["googleTokensEnc"]})));
    let raw_client = fs::read_to_string(client_path)
        .map_err(|_| "Cannot read the desktop client configuration.")?;
    let client = parse_google_client(&raw_client).ok_or("Invalid desktop client configuration.")?;
    let auth = GoogleAuth::new(&client, session);
    if !auth.has_session() {
        return Err(
            "An existing protected Google session is required; no login was started.".into(),
        );
    }
    let identity = auth.verified_identity().await?;
    if identity.subject != subject {
        return Err("The protected Google session belongs to a different account.".into());
    }
    let about = auth
        .api(
            "GET",
            "https://www.googleapis.com/drive/v3/about?fields=user(permissionId)",
            None,
        )
        .await
        .map_err(|_| "Cannot verify the current Drive account.")?;
    let url = format!("https://www.googleapis.com/drive/v3/files/{fid}?fields=id,name,mimeType,trashed,ownedByMe,driveId,properties,owners(permissionId,me),capabilities(canListChildren)");
    let folder = auth
        .api("GET", &url, None)
        .await
        .map_err(|_| "Cannot verify the selected team folder.")?;
    let mut result = checked(&folder, &about, fid).map_err(str::to_string)?;
    // Reproduce the gateway's read-only authorization queries without sending a token to a third party.
    let userinfo = auth
        .api(
            "GET",
            "https://openidconnect.googleapis.com/v1/userinfo",
            None,
        )
        .await;
    result["gatewayUserInfoStatus"] = json!(userinfo.as_ref().map(|_| 200).unwrap_or_else(|e| e.0));
    result["gatewayUserInfoVerified"] = json!(userinfo
        .as_ref()
        .is_ok_and(|v| v["sub"] == subject && v["email_verified"] == true));
    let permissions_url = format!("https://www.googleapis.com/drive/v3/files/{fid}/permissions?pageSize=100&fields=nextPageToken,permissions(id,type,role,deleted)");
    for (label, url) in [
        ("withoutEmptyPageToken", permissions_url.clone()),
        (
            "withEmptyPageToken",
            format!("{permissions_url}&pageToken="),
        ),
    ] {
        let permissions = auth.api("GET", &url, None).await;
        let owner = &about["user"]["permissionId"];
        result[label] = json!({
            "status": permissions.as_ref().map(|_| 200).unwrap_or_else(|e| e.0),
            "explicitOwnerFound": permissions.as_ref().is_ok_and(|v| v["permissions"].as_array().is_some_and(|p| p.iter().any(|p| &p["id"] == owner && p["type"] == "user" && p["role"] == "owner" && p["deleted"] != true)))
        });
    }
    let after = Sha256::digest(
        fs::read(&settings_path).map_err(|_| "Cannot recheck the selected app settings.")?,
    );
    if before != after {
        return Err(
            "App settings changed during this read; no operator result was emitted.".into(),
        );
    }
    result["sourceSettingsUnchanged"] = json!(true);
    result["accountSubjectMatched"] = json!(true);
    println!(
        "{}",
        serde_json::to_string(&result).map_err(|_| "Cannot serialize operator metadata.")?
    );
    Ok(())
}
#[tokio::main]
async fn main() {
    if let Err(message) = run().await {
        eprintln!("{message}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (Value, Value) {
        (
            json!({"id":"team-only","name":"Verification team","mimeType":"application/vnd.google-apps.folder",
            "trashed":false,"ownedByMe":true,"properties":{"saiTeam":"1"},
            "owners":[{"permissionId":"owner-only","me":true}],"capabilities":{"canListChildren":true}}),
            json!({"user":{"permissionId":"owner-only"}}),
        )
    }
    #[test]
    fn exports_only_verified_operator_metadata() {
        let (folder, about) = fixture();
        let result = checked(&folder, &about, "team-only").unwrap();
        assert_eq!(result["ownerPermissionId"], "owner-only");
        assert_eq!(result.as_object().unwrap().len(), 5);
    }
    #[test]
    fn rejects_wrong_owner_unmarked_shared_drive_and_nonfolders() {
        for change in 0..8 {
            let (mut folder, mut about) = fixture();
            match change {
                0 => folder["owners"][0]["permissionId"] = json!("another-owner"),
                1 => folder["owners"] = json!([]),
                2 => folder["ownedByMe"] = json!(false),
                3 => folder["properties"] = json!({}),
                4 => folder["driveId"] = json!("shared-drive"),
                5 => folder["mimeType"] = json!("text/plain"),
                6 => about["user"] = json!({}),
                _ => folder["owners"][0]["me"] = json!(false),
            }
            assert!(checked(&folder, &about, "team-only").is_err());
        }
    }
}
