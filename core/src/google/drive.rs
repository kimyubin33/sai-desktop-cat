//! Google Drive: 계정 백업(사이 전용 숨김 폴더) · 팀 자료 올리기 · 공유 링크 읽기.
//! 권한은 두 가지뿐이다.
//!  - drive.appdata: 사이만 보는 숨김 폴더. 내 Drive 의 다른 파일은 볼 수 없다
//!  - drive.file: 사이로 올리거나 사이에서 연 파일만. 내 Drive 전체는 볼 수 없다
use super::oauth::GoogleAuth;
use crate::http::{client, json_body};
use crate::util::J;
use crate::{err, Result};
use serde_json::{json, Value};

const API: &str = "https://www.googleapis.com/drive/v3";
const UPLOAD: &str = "https://www.googleapis.com/upload/drive/v3";
pub const MAX_UPLOAD: usize = 25 * 1024 * 1024;

fn msg(status: u16, d: &Value) -> String {
    let m = d.get("error").and_then(|e| e.os("message")).unwrap_or_else(|| status.to_string());
    if m.contains("has not been used") || m.contains("is disabled") {
        return "Google Cloud 프로젝트에서 Google Drive API 가 꺼져 있어요. console.cloud.google.com → API 및 서비스 → 라이브러리에서 \"Google Drive API\"를 사용 설정해 주세요.".into();
    }
    if status == 403 && (m.contains("insufficient") || m.contains("scope")) {
        return "Drive 권한이 없어요. 설정 → 외부 연결에서 Google 로그아웃 후 다시 로그인해 주세요.".into();
    }
    format!("Google Drive 실패 ({status}): {m}")
}

async fn send(a: &GoogleAuth, rb: reqwest::RequestBuilder) -> Result<Value> {
    let token = a.access_token().await?;
    let res = rb.bearer_auth(token).send().await.map_err(|e| format!("Google Drive 에 연결하지 못했어요: {e}"))?;
    let (status, d) = json_body(res).await;
    if !(200..300).contains(&status) {
        return Err(msg(status, &d));
    }
    Ok(d)
}

// ---------- 숨김 폴더 (계정 백업) ----------
async fn appdata_id(a: &GoogleAuth, name: &str) -> Result<Option<String>> {
    let q = format!("name = '{name}' and trashed = false");
    let d = send(a, client().get(format!("{API}/files")).query(&[("spaces", "appDataFolder"), ("q", q.as_str()), ("fields", "files(id,modifiedTime)"), ("orderBy", "modifiedTime desc")])).await?;
    Ok(d["files"].get(0).map(|f| f.s("id")))
}

pub async fn appdata_read(a: &GoogleAuth, name: &str) -> Result<Option<Value>> {
    let Some(id) = appdata_id(a, name).await? else { return Ok(None) };
    let d = send(a, client().get(format!("{API}/files/{id}")).query(&[("alt", "media")])).await?;
    Ok(Some(d).filter(|v| v.is_object()))
}

pub async fn appdata_write(a: &GoogleAuth, name: &str, data: &Value) -> Result<()> {
    let body = serde_json::to_vec(data).map_err(|e| e.to_string())?;
    match appdata_id(a, name).await? {
        Some(id) => {
            send(a, client().patch(format!("{UPLOAD}/files/{id}")).query(&[("uploadType", "media")]).header("Content-Type", "application/json").body(body)).await?;
        }
        None => {
            let meta = json!({ "name": name, "parents": ["appDataFolder"], "mimeType": "application/json" });
            let id = create_resumable(a, &meta, "application/json", body.len()).await?;
            put_bytes(a, &id, "application/json", body).await?;
        }
    }
    Ok(())
}

// ---------- 올리기 ----------
async fn create_resumable(a: &GoogleAuth, meta: &Value, mime: &str, len: usize) -> Result<String> {
    let token = a.access_token().await?;
    let res = client()
        .post(format!("{UPLOAD}/files"))
        .query(&[("uploadType", "resumable")])
        .bearer_auth(token)
        .header("X-Upload-Content-Type", mime)
        .header("X-Upload-Content-Length", len.to_string())
        .json(meta)
        .send()
        .await
        .map_err(|e| format!("Google Drive 에 연결하지 못했어요: {e}"))?;
    let loc = res.headers().get("location").and_then(|v| v.to_str().ok()).map(String::from);
    let (status, d) = json_body(res).await;
    if !(200..300).contains(&status) {
        return Err(msg(status, &d));
    }
    loc.ok_or_else(|| "Google Drive 가 올릴 주소를 주지 않았어요.".to_string())
}

async fn put_bytes(a: &GoogleAuth, location: &str, mime: &str, body: Vec<u8>) -> Result<Value> {
    send(a, client().put(location).header("Content-Type", mime).timeout(std::time::Duration::from_secs(600)).body(body)).await
}

/// 사이가 만든 "사이 - 팀 자료" 폴더 (drive.file 권한으로 보이는 건 사이가 만든 폴더뿐)
async fn folder(a: &GoogleAuth, name: &str) -> Result<String> {
    let safe = name.replace('\'', " ");
    let q = format!("name = '{safe}' and mimeType = 'application/vnd.google-apps.folder' and trashed = false");
    let d = send(a, client().get(format!("{API}/files")).query(&[("q", q.as_str()), ("fields", "files(id)"), ("spaces", "drive")])).await?;
    if let Some(f) = d["files"].get(0) {
        return Ok(f.s("id"));
    }
    let d = send(a, client().post(format!("{API}/files")).query(&[("fields", "id")]).json(&json!({ "name": safe, "mimeType": "application/vnd.google-apps.folder" }))).await?;
    Ok(d.s("id"))
}

const FIELDS: &str = "id,name,mimeType,size,webViewLink";

/// 파일을 내 Drive 의 팀 폴더에 올리고, 링크가 있는 사람은 볼 수 있게 한다
pub async fn upload(a: &GoogleAuth, folder_name: &str, name: &str, mime: &str, bytes: Vec<u8>) -> Result<Value> {
    if bytes.is_empty() {
        return err("빈 파일이에요.");
    }
    if bytes.len() > MAX_UPLOAD {
        return err(format!("{}MB 보다 큰 파일은 Drive 에 직접 올린 뒤 링크를 붙여 넣어 주세요.", MAX_UPLOAD / 1024 / 1024));
    }
    let mime = if mime.is_empty() { "application/octet-stream" } else { mime };
    let parent = folder(a, folder_name).await?;
    let loc = create_resumable(a, &json!({ "name": name, "parents": [parent] }), mime, bytes.len()).await?;
    let f = put_bytes(a, &format!("{loc}"), mime, bytes).await?;
    let id = f.s("id");
    share_link(a, &id).await?;
    let d = send(a, client().get(format!("{API}/files/{id}")).query(&[("fields", FIELDS)])).await?;
    Ok(d)
}

/// 링크가 있는 사람은 볼 수 있게 (팀원이 Google 계정을 몰라도 열리도록)
async fn share_link(a: &GoogleAuth, id: &str) -> Result<()> {
    send(a, client().post(format!("{API}/files/{id}/permissions")).json(&json!({ "role": "reader", "type": "anyone" }))).await?;
    Ok(())
}

/// Drive 링크에서 파일 id 찾기
pub fn drive_id(url: &str) -> Option<String> {
    let u = url::Url::parse(url.trim()).ok()?;
    let host = u.host_str()?;
    if !(host.ends_with("drive.google.com") || host.ends_with("docs.google.com")) {
        return None;
    }
    if let Some((_, id)) = u.query_pairs().find(|(k, _)| k == "id") {
        return Some(id.to_string());
    }
    let parts: Vec<&str> = u.path_segments()?.collect();
    // /file/d/<id>/view, /document/d/<id>/edit, /drive/folders/<id>
    parts.iter().position(|p| *p == "d" || *p == "folders").and_then(|i| parts.get(i + 1)).map(|s| s.to_string()).filter(|s| s.len() > 10)
}

/// 붙여 넣은 링크의 이름 (사이가 볼 수 없는 파일이면 None)
pub async fn meta(a: &GoogleAuth, id: &str) -> Option<Value> {
    send(a, client().get(format!("{API}/files/{id}")).query(&[("fields", FIELDS), ("supportsAllDrives", "true")])).await.ok()
}

#[cfg(test)]
mod tests {
    use super::drive_id;

    #[test]
    fn finds_ids() {
        assert_eq!(drive_id("https://drive.google.com/file/d/1AbCdEfGhIjKlMnOp/view?usp=sharing").as_deref(), Some("1AbCdEfGhIjKlMnOp"));
        assert_eq!(drive_id("https://docs.google.com/presentation/d/1XyZ0123456789ab/edit#slide=id.p").as_deref(), Some("1XyZ0123456789ab"));
        assert_eq!(drive_id("https://drive.google.com/drive/folders/1Folder123456789").as_deref(), Some("1Folder123456789"));
        assert_eq!(drive_id("https://drive.google.com/open?id=1OpenId123456789").as_deref(), Some("1OpenId123456789"));
        assert_eq!(drive_id("https://example.com/file/d/1AbCdEfGhIjKlMnOp/view"), None);
    }
}
