//! Team file access and a local, account-bound upload outbox. No overwrite/share APIs.
mod imports;
mod store;
pub(super) mod text;
use super::{
    oauth::{media::silent, GoogleAuth, Lease},
    profile::Profiles,
    teams::Teams,
};
use crate::{err, Result};
use serde_json::{json, Value};
use std::{path::PathBuf, sync::Arc};
pub use store::safe_name;
use store::{digest, hash, Outbox, Upload};

pub const MAX_BYTES: usize = 25 * 1024 * 1024;
const MAX_PENDING: usize = 8;
const FIELDS:&str="id,name,mimeType,size,parents,trashed,version,sha256Checksum,appProperties,properties,capabilities(canDownload,canCopy)";
pub type Progress = Arc<dyn Fn(&str, u64, u64) + Send + Sync>;
pub struct Download {
    pub name: String,
    pub bytes: Vec<u8>,
}
pub(super) struct Files {
    outbox: Outbox,
    gate: tokio::sync::Mutex<()>,
    imports: imports::Imports,
}
impl Files {
    pub async fn check_pending_target(
        &self,
        auth: &GoogleAuth,
        epoch: u64,
        folder: &str,
        operation: &str,
    ) -> Result<()> {
        let lease = auth.lease().await?;
        if lease.epoch() != epoch {
            return err("계정 연결이 바뀌었어요.");
        }
        let upload = self.outbox.get(lease.subject(), operation)?;
        if upload.folder_id != folder {
            return err("선택한 팀의 대기 항목만 처리할 수 있어요.");
        }
        Ok(())
    }
    pub fn new(root: PathBuf) -> Self {
        Self {
            imports: imports::Imports::new(root.join("drive-imports")),
            outbox: Outbox::new(root),
            gate: tokio::sync::Mutex::new(()),
        }
    }
    pub fn pending(&self, subject: &str, folder: Option<&str>) -> Result<Value> {
        let mut rows=self.outbox.pending(subject,folder)?.iter().map(|u|json!({"origin":"pc","operation":u.operation,"folderId":u.folder_id,"name":u.name,"size":u.size})).collect::<Vec<_>>();
        rows.extend(self.imports.pending(subject, folder)?);
        Ok(json!(rows))
    }
    pub fn has_pending(&self, subject: &str, folder: &str) -> Result<bool> {
        Ok(!self.outbox.pending(subject, Some(folder))?.is_empty()
            || !self.imports.pending(subject, Some(folder))?.is_empty())
    }
    pub async fn import(
        &self,
        auth: &GoogleAuth,
        profiles: &Profiles,
        teams: &Teams,
        epoch: u64,
        folder: &str,
        ids: &[String],
        progress: Progress,
    ) -> Result<Value> {
        self.imports
            .batch(auth, profiles, teams, epoch, folder, ids, progress)
            .await
    }
    pub async fn retry_import(
        &self,
        auth: &GoogleAuth,
        profiles: &Profiles,
        teams: &Teams,
        epoch: u64,
        folder: &str,
        op: &str,
        progress: Progress,
    ) -> Result<Value> {
        self.imports
            .retry(auth, profiles, teams, epoch, folder, op, progress)
            .await
    }
    pub async fn discard_import(
        &self,
        auth: &GoogleAuth,
        epoch: u64,
        folder: &str,
        op: &str,
    ) -> Result<()> {
        self.imports.discard(auth, epoch, folder, op).await
    }
    pub async fn browse(
        &self,
        auth: &GoogleAuth,
        epoch: u64,
        mode: &str,
        parent: &str,
        query: &str,
        page: &str,
    ) -> Result<Value> {
        let lease = auth.full_drive_lease(epoch).await?;
        if !["folder", "shared", "search"].contains(&mode)
            || page.len() > 2048
            || query.chars().count() > 80
            || query.chars().any(char::is_control)
        {
            return err("Drive 탐색 조건을 확인해 주세요.");
        }
        let q = match mode {
            "shared" => "sharedWithMe = true and trashed = false".into(),
            "search" => {
                if query.trim().is_empty() {
                    return err("찾을 파일 이름을 입력해 주세요.");
                }
                format!(
                    "name contains '{}' and trashed = false",
                    query.replace('\\', "\\\\").replace('\'', "\\'")
                )
            }
            _ => {
                let p = if parent.is_empty() { "root" } else { parent };
                if !super::profile::model::id(p) {
                    return err("Drive 폴더 주소가 올바르지 않아요.");
                }
                format!("'{p}' in parents and trashed = false")
            }
        };
        let response=auth.profile_request(&lease,"GET",&format!("/drive/v3/files?spaces=drive&pageSize=25&supportsAllDrives=true&includeItemsFromAllDrives=true&fields=nextPageToken,incompleteSearch,files({FIELDS})&q={}&pageToken={}&orderBy=folder,name",urlencoding::encode(&q),urlencoding::encode(page)),None).await.map_err(api_error)?;
        if response["incompleteSearch"] == true {
            return err("Drive 파일 목록을 끝까지 확인하지 못했어요.");
        }
        let mut entries = vec![];
        for meta in response["files"]
            .as_array()
            .filter(|v| v.len() <= 25)
            .ok_or("Drive 목록의 형식을 확인하지 못했어요.")?
        {
            if meta["properties"]["saiCollab"] == "1" {
                continue;
            }
            let id = meta["id"]
                .as_str()
                .filter(|s| super::profile::model::id(s))
                .ok_or("파일 주소가 올바르지 않아요.")?;
            let name = meta["name"]
                .as_str()
                .filter(|s| !s.is_empty() && s.chars().count() <= 1000)
                .ok_or("파일 이름을 확인하지 못했어요.")?;
            let folder = meta["mimeType"] == "application/vnd.google-apps.folder";
            entries.push(json!({"id":id,"name":name,"folder":folder,"size":meta["size"].as_str().and_then(|s|s.parse::<u64>().ok()),"canCopy":!folder&&meta["mimeType"]!="application/vnd.google-apps.shortcut"&&meta["capabilities"]["canCopy"]==true}));
        }
        let next = match response.get("nextPageToken") {
            None => String::new(),
            Some(Value::String(s)) if !s.is_empty() && s.len() <= 2048 && s != page => s.clone(),
            _ => return err("다음 Drive 목록을 확인하지 못했어요."),
        };
        auth.with_lease(&lease, || {
            Ok(json!({"epoch":epoch,"entries":entries,"nextPage":next}))
        })
        .await
    }
    pub async fn list(
        &self,
        auth: &GoogleAuth,
        profiles: &Profiles,
        teams: &Teams,
        epoch: u64,
        folder: &str,
        page: &str,
    ) -> Result<Value> {
        let base = auth.lease().await?;
        if base.epoch() != epoch {
            return err("계정 연결이 바뀌었어요.");
        }
        let pending = self.pending(base.subject(), Some(folder))?;
        let result=async {
            let (lease,team)=teams.access(auth,profiles,epoch,folder).await?;
            if page.len()>2048 {return err("다음 파일 목록의 주소를 확인하지 못했어요.");}
            let q=format!("'{folder}' in parents and trashed = false and mimeType != 'application/vnd.google-apps.folder' and mimeType != 'application/vnd.google-apps.shortcut' and not properties has {{ key='saiCollab' and value='1' }}");
            let path=format!("/drive/v3/files?spaces=drive&pageSize=25&fields=nextPageToken,incompleteSearch,files({FIELDS})&q={}&pageToken={}",urlencoding::encode(&q),urlencoding::encode(page));
            let response=auth.profile_request(&lease,"GET",&path,None).await.map_err(api_error)?;
            if response["incompleteSearch"]==true {return err("파일 목록을 끝까지 확인하지 못했어요. 다시 시도해 주세요.");}
            let next=match response.get("nextPageToken") {None=>String::new(),Some(Value::String(s)) if !s.is_empty()&&s.len()<=2048&&s!=page=>s.clone(),_=>return err("다음 파일 목록을 확인하지 못했어요.")};
            let rows=response["files"].as_array().ok_or("파일 목록의 형식이 올바르지 않아요.")?;
            if rows.len()>25 {return err("파일 목록의 크기가 올바르지 않아요.");}
            let files=rows.iter().map(|v|file_view(v,folder,None)).collect::<Result<Vec<_>>>()?;
            auth.with_lease(&lease,||Ok(json!({"epoch":epoch,"folderId":folder,"team":team,"files":files,"nextPage":next,"pending":pending}))).await
        }.await;
        match result {
            Ok(v)=>Ok(v),
            Err(error)=>auth.with_lease(&base,||Ok(json!({"epoch":epoch,"folderId":folder,"files":[],"nextPage":"","pending":pending,"error":error}))).await,
        }
    }
    pub async fn info(
        &self,
        auth: &GoogleAuth,
        profiles: &Profiles,
        teams: &Teams,
        epoch: u64,
        folder: &str,
        fid: &str,
    ) -> Result<Value> {
        let (lease, _) = teams.access(auth, profiles, epoch, folder).await?;
        let meta = metadata(auth, &lease, fid).await.map_err(api_error)?;
        let view = file_view(&meta, folder, Some(fid))?;
        auth.with_lease(&lease, || Ok(view)).await
    }
    pub async fn selected(
        &self,
        auth: &GoogleAuth,
        profiles: &Profiles,
        teams: &Teams,
        epoch: u64,
        folder: &str,
        ids: &[String],
    ) -> Result<Value> {
        let (lease, _) = teams.access(auth, profiles, epoch, folder).await?;
        if ids.is_empty() || ids.len() > 10 {
            return err("팀 폴더 안의 파일을 10개까지 선택해 주세요.");
        }
        let mut rows = vec![];
        for id in ids {
            rows.push(file_view(
                &metadata(auth, &lease, id).await.map_err(api_error)?,
                folder,
                Some(id),
            )?);
        }
        auth.with_lease(&lease, || Ok(json!({"epoch":epoch,"files":rows})))
            .await
    }
    pub async fn stage(
        &self,
        auth: &GoogleAuth,
        profiles: &Profiles,
        teams: &Teams,
        epoch: u64,
        folder: &str,
        name: &str,
        bytes: &[u8],
    ) -> Result<String> {
        let (lease, team) = teams.access(auth, profiles, epoch, folder).await?;
        if team["canUpload"] != true {
            return err("이 팀 폴더에는 파일을 올릴 권한이 없어요.");
        }
        auth.with_lease(&lease, || {
            Ok(self
                .outbox
                .stage(lease.subject(), folder, name, bytes)?
                .operation)
        })
        .await
    }
    pub async fn retry(
        &self,
        auth: &GoogleAuth,
        profiles: &Profiles,
        teams: &Teams,
        epoch: u64,
        operation: &str,
        progress: Progress,
    ) -> Result<Value> {
        let base = auth.lease().await?;
        if base.epoch() != epoch {
            return err("계정 연결이 바뀌었어요.");
        }
        let _gate = self.gate.lock().await;
        let mut upload = self.outbox.get(base.subject(), operation)?;
        let (lease, team) = teams
            .access(auth, profiles, epoch, &upload.folder_id)
            .await?;
        if team["canUpload"] != true {
            return err("이 팀 폴더에는 파일을 올릴 권한이 없어요. 대기 사본은 유지했어요.");
        }
        progress("preparing", 0, upload.size);
        let fid = match &upload.file_id {
            Some(fid) => fid.clone(),
            None => {
                let value = auth
                    .profile_request(
                        &lease,
                        "GET",
                        "/drive/v3/files/generateIds?count=1&space=drive&type=files",
                        None,
                    )
                    .await
                    .map_err(api_error)?;
                let fid = value["ids"]
                    .as_array()
                    .filter(|a| a.len() == 1)
                    .and_then(|a| a[0].as_str())
                    .filter(|s| super::profile::model::id(s))
                    .ok_or("파일 저장 주소를 준비하지 못했어요.")?
                    .to_owned();
                auth.with_lease(&lease, || {
                    self.outbox.file_id(lease.subject(), operation, &fid)
                })
                .await?;
                upload.file_id = Some(fid.clone());
                fid
            }
        };
        let meta = match metadata(auth, &lease, &fid).await {
            Ok(meta) => meta,
            Err((404, _)) => {
                let bytes = self.outbox.bytes(lease.subject(), &upload)?;
                let boundary = format!("sai_{}", super::oauth::random_token());
                let meta = json!({"id":fid,"name":upload.name,"parents":[upload.folder_id],"mimeType":mime(&upload.name),"appProperties":{"saiUpload":upload.operation}});
                let mut body=format!("--{boundary}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n{meta}\r\n--{boundary}\r\nContent-Type: {}\r\n\r\n",mime(&upload.name)).into_bytes();
                body.extend_from_slice(&bytes);
                body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
                match auth
                    .upload_media(&lease, &boundary, body, progress.clone())
                    .await
                {
                    Ok(meta) => meta,
                    Err((409, _)) => metadata(auth, &lease, &fid).await.map_err(api_error)?,
                    Err(e) => return Err(api_error(e)),
                }
            }
            Err(e) => return Err(api_error(e)),
        };
        progress("verifying", upload.size, upload.size);
        verify_upload(auth, &lease, &upload, &fid, &meta).await?;
        // Re-check the parent after transfer; an old folder check cannot authorize success after revocation.
        teams
            .access(auth, profiles, epoch, &upload.folder_id)
            .await?;
        let view = file_view(&meta, &upload.folder_id, Some(&fid))?;
        auth.with_lease(&lease, || {
            self.outbox.remove(lease.subject(), operation)?;
            Ok(json!({"epoch":epoch,"file":view}))
        })
        .await
    }
    pub async fn discard(&self, auth: &GoogleAuth, epoch: u64, operation: &str) -> Result<()> {
        let lease = auth.lease().await?;
        if lease.epoch() != epoch {
            return err("계정 연결이 바뀌었어요.");
        }
        let _gate = self.gate.lock().await;
        auth.with_lease(&lease, || self.outbox.remove(lease.subject(), operation))
            .await
    }
    pub async fn download(
        &self,
        auth: &GoogleAuth,
        profiles: &Profiles,
        teams: &Teams,
        epoch: u64,
        folder: &str,
        fid: &str,
        progress: Progress,
    ) -> Result<Download> {
        let (lease, _) = teams.access(auth, profiles, epoch, folder).await?;
        let before = metadata(auth, &lease, fid).await.map_err(api_error)?;
        let view = file_view(&before, folder, Some(fid))?;
        if view["canDownload"] != true {
            return err(
                "이 파일은 Drive에서 열어 주세요. 내려받기 권한과 파일 형식을 확인할 수 있어요.",
            );
        }
        let bytes = auth
            .download_media(&lease, fid, progress)
            .await
            .map_err(api_error)?;
        let after = metadata(auth, &lease, fid).await.map_err(api_error)?;
        if file_view(&after, folder, Some(fid))?["canDownload"] != true {
            return err("파일의 내려받기 권한이 바뀌어 저장하지 않았어요.");
        }
        if before["version"] != after["version"] || bytes.len() as u64 != size(&before)? {
            return err("받는 중에 파일이 바뀌었어요. 저장하지 않았으니 다시 내려받아 주세요.");
        }
        if let Some(checksum) = checksum(&before)? {
            if digest(&bytes) != checksum {
                return err("받은 파일의 내용이 일치하지 않아 저장하지 않았어요.");
            }
        }
        teams.access(auth, profiles, epoch, folder).await?;
        auth.with_lease(&lease, || {
            Ok(Download {
                name: safe_name(before["name"].as_str().unwrap_or("download.bin")),
                bytes,
            })
        })
        .await
    }
}
fn api_error(e: (u16, String)) -> String {
    match e.0 {401=>"Google 계정 연결을 다시 확인해 주세요.".into(),403|404=>"이 계정에서 파일에 접근할 수 없어요. 팀 공유 상태를 확인하거나 Google에서 파일을 선택해 연결해 주세요.".into(),_=>e.1}
}
async fn metadata(
    auth: &GoogleAuth,
    lease: &Lease,
    fid: &str,
) -> std::result::Result<Value, (u16, String)> {
    if !super::profile::model::id(fid) {
        return Err((0, "파일 주소가 올바르지 않아요.".into()));
    }
    auth.profile_request(
        lease,
        "GET",
        &format!("/drive/v3/files/{fid}?supportsAllDrives=true&fields={FIELDS}"),
        None,
    )
    .await
}
fn size(meta: &Value) -> Result<u64> {
    match &meta["size"] {
        Value::String(s) => s
            .parse::<u64>()
            .map_err(|_| "파일 크기를 확인하지 못했어요.".into()),
        Value::Number(n) => n.as_u64().ok_or("파일 크기가 올바르지 않아요.".into()),
        _ => err("파일 크기를 확인하지 못했어요."),
    }
}
fn checksum(meta: &Value) -> Result<Option<String>> {
    match meta.get("sha256Checksum") {
        None => Ok(None),
        Some(Value::String(s)) if hash(s) => Ok(Some(s.to_ascii_lowercase())),
        _ => err("파일 검사 정보를 확인하지 못했어요."),
    }
}
fn file_view(meta: &Value, folder: &str, expected: Option<&str>) -> Result<Value> {
    let fid = meta["id"]
        .as_str()
        .filter(|s| super::profile::model::id(s))
        .ok_or("파일 주소가 올바르지 않아요.")?;
    let name = meta["name"]
        .as_str()
        .filter(|s| !s.is_empty() && s.chars().count() <= 1000)
        .ok_or("파일 이름을 확인하지 못했어요.")?;
    let kind = meta["mimeType"]
        .as_str()
        .ok_or("파일 형식을 확인하지 못했어요.")?;
    if meta["properties"]["saiCollab"] == "1"
        || expected.is_some_and(|e| e != fid)
        || meta["trashed"] != false
        || !meta["parents"]
            .as_array()
            .is_some_and(|p| p.iter().any(|id| id == folder))
        || [
            "application/vnd.google-apps.folder",
            "application/vnd.google-apps.shortcut",
        ]
        .contains(&kind)
    {
        return err("선택한 팀 폴더 바로 안의 파일만 열 수 있어요. 다른 자료는 이동하거나 공유하지 않았어요.");
    }
    if !meta["version"]
        .as_str()
        .is_some_and(|s| !s.is_empty() && s.len() <= 32 && s.bytes().all(|b| b.is_ascii_digit()))
    {
        return err("파일의 변경 번호를 확인하지 못했어요.");
    }
    checksum(meta)?;
    let native = kind.starts_with("application/vnd.google-apps.");
    let length = if native { None } else { Some(size(meta)?) };
    Ok(
        json!({"id":fid,"name":name,"mimeType":kind,"size":length,"canDownload":!native&&length.is_some_and(|n|n<=MAX_BYTES as u64)&&meta["capabilities"]["canDownload"]==true,"googleDocument":native,"canAnalyze":text::supported(meta)&&meta["capabilities"]["canDownload"]==true&&(native||length.is_some_and(|n|n<=24*1024))}),
    )
}
async fn verify_upload(
    auth: &GoogleAuth,
    lease: &Lease,
    upload: &Upload,
    fid: &str,
    meta: &Value,
) -> Result<()> {
    file_view(meta, &upload.folder_id, Some(fid))?;
    if meta["appProperties"]["saiUpload"] != upload.operation || size(meta)? != upload.size {
        return err("같은 저장 주소에 다른 파일 정보가 있어 덮어쓰지 않았어요.");
    }
    let actual = match checksum(meta)? {
        Some(hash) => hash,
        None => digest(
            &auth
                .download_media(lease, fid, silent())
                .await
                .map_err(api_error)?,
        ),
    };
    if actual != upload.sha256 {
        return err("Drive 파일과 전송 사본이 달라요. 대기 정보를 유지했어요.");
    }
    Ok(())
}
fn mime(name: &str) -> &'static str {
    match name
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "pdf" => "application/pdf",
        "txt" | "md" | "csv" => "text/plain",
        "json" => "application/json",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "zip" => "application/zip",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        _ => "application/octet-stream",
    }
}
#[cfg(test)]
mod tests;
