//! Server-side copies. Original files and their sharing settings are never modified.
use super::{api_error, checksum, file_view, metadata, size, Progress, FIELDS};
use crate::google::{
    oauth::{GoogleAuth, Lease},
    profile::{model::id, Profiles},
    teams::Teams,
};
use crate::{err, store::Store, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::Mutex,
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CopyJob {
    operation: String,
    folder_id: String,
    source_id: String,
    name: String,
    mime: String,
    version: String,
    size: Option<u64>,
    checksum: Option<String>,
    file_id: Option<String>,
    attempted: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Ledger {
    version: u32,
    subject: String,
    jobs: Vec<CopyJob>,
}
pub(super) struct Imports {
    root: PathBuf,
    stores: Mutex<BTreeMap<String, Store>>,
    gate: tokio::sync::Mutex<()>,
}
impl Imports {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            stores: Mutex::new(BTreeMap::new()),
            gate: tokio::sync::Mutex::new(()),
        }
    }
    fn ledger<T>(
        &self,
        subject: &str,
        save: bool,
        f: impl FnOnce(&mut Ledger) -> Result<T>,
    ) -> Result<T> {
        let mut stores = self.stores.lock().unwrap();
        let store = stores.entry(subject.into()).or_insert_with(|| {
            let path = self
                .root
                .join(format!("{:x}.json", Sha256::digest(subject.as_bytes())));
            let default = if path.exists() {
                Value::Null
            } else {
                json!({"version":1,"subject":subject,"jobs":[]})
            };
            Store::open(path, default)
        });
        if let Some(e) = store.load_error() {
            return Err(e);
        }
        let mut ledger: Ledger = serde_json::from_value(store.get())
            .map_err(|_| "Drive 가져오기 대기를 읽지 못했어요. 원본은 유지했어요.")?;
        let unique: BTreeSet<_> = ledger.jobs.iter().map(|j| &j.operation).collect();
        if ledger.version != 1
            || ledger.subject != subject
            || ledger.jobs.len() > 20
            || unique.len() != ledger.jobs.len()
            || !ledger.jobs.iter().all(|j| {
                id(&j.operation)
                    && id(&j.folder_id)
                    && id(&j.source_id)
                    && j.file_id.as_deref().is_none_or(id)
                    && !j.name.is_empty()
                    && j.name.chars().count() <= 1000
                    && j.mime.len() <= 128
                    && valid_version(&j.version)
                    && j.checksum.as_deref().is_none_or(super::store::hash)
            })
        {
            return err("Drive 가져오기 대기 정보가 올바르지 않아요. 기존 자료는 유지했어요.");
        }
        let result = f(&mut ledger)?;
        if save {
            let next = serde_json::to_value(ledger)
                .map_err(|_| "Drive 가져오기 정보를 준비하지 못했어요.")?;
            store
                .commit(|v| *v = next)
                .map_err(|_| "Drive 가져오기 대기를 저장하지 못했어요.")?;
        }
        Ok(result)
    }
    pub fn pending(&self, subject: &str, folder: Option<&str>) -> Result<Vec<Value>> {
        self.ledger(subject,false,|l|Ok(l.jobs.iter().filter(|j|folder.is_none_or(|f|j.folder_id==f)).map(|j|json!({"origin":"drive","operation":j.operation,"folderId":j.folder_id,"name":j.name,"size":j.size,"uncertain":j.attempted&&j.file_id.is_none()})).collect()))
    }
    fn job(&self, subject: &str, op: &str, folder: &str) -> Result<CopyJob> {
        self.ledger(subject, false, |l| {
            l.jobs
                .iter()
                .find(|j| j.operation == op && j.folder_id == folder)
                .cloned()
                .ok_or("현재 계정과 팀에 해당하는 가져오기 대기를 찾지 못했어요.".into())
        })
    }
    pub async fn discard(
        &self,
        auth: &GoogleAuth,
        epoch: u64,
        folder: &str,
        op: &str,
    ) -> Result<()> {
        let lease = auth.lease().await?;
        if lease.epoch() != epoch {
            return err("계정 연결이 바뀌었어요.");
        }
        let _gate = self.gate.lock().await;
        self.job(lease.subject(), op, folder)?;
        auth.with_lease(&lease, || {
            self.ledger(lease.subject(), true, |l| {
                l.jobs.retain(|j| j.operation != op);
                Ok(())
            })
        })
        .await
    }
    pub async fn batch(
        &self,
        auth: &GoogleAuth,
        profiles: &Profiles,
        teams: &Teams,
        epoch: u64,
        folder: &str,
        ids: &[String],
        progress: Progress,
    ) -> Result<Value> {
        let lease = auth.full_drive_lease(epoch).await?;
        let _gate = self.gate.lock().await;
        let (_, team) = teams.access(auth, profiles, epoch, folder).await?;
        if team["canUpload"] != true {
            return err("이 팀 폴더에 사본을 추가할 권한이 없어요.");
        }
        let unique: BTreeSet<_> = ids.iter().collect();
        if ids.is_empty()
            || ids.len() > 10
            || unique.len() != ids.len()
            || !ids.iter().all(|s| id(s))
        {
            return err("서로 다른 파일을 1~10개 선택해 주세요.");
        }
        let mut jobs = vec![];
        let mut existing = 0;
        for source in ids {
            let meta = metadata(auth, &lease, source).await.map_err(api_error)?;
            if meta["parents"]
                .as_array()
                .is_some_and(|ps| ps.iter().any(|p| p == folder))
            {
                file_view(&meta, folder, Some(source))?;
                existing += 1;
                continue;
            }
            jobs.push(source_job(&meta, source, folder)?);
        }
        let jobs=auth.with_lease(&lease,||self.ledger(lease.subject(),true,|l|{
            let mut selected=vec![];
            for job in jobs{
                if let Some(prior)=l.jobs.iter().find(|j|j.folder_id==folder&&j.source_id==job.source_id){selected.push(prior.clone());}
                else {if l.jobs.len()>=20{return err("가져오기 대기는 20개까지 보관할 수 있어요. 기존 항목을 먼저 확인해 주세요.");}l.jobs.push(job.clone());selected.push(job);}
            }Ok(selected)
        })).await?;
        let mut completed = 0;
        for job in &jobs {
            progress("copying", completed, jobs.len() as u64);
            if let Err(e) = self.run(auth, profiles, teams, &lease, job.clone()).await {
                return err(format!("사본 {completed}개 추가, 기존 파일 {existing}개 확인. {e} 남은 항목은 전송 대기에서 확인할 수 있어요."));
            }
            completed += 1;
        }
        progress("verifying", completed, jobs.len() as u64);
        Ok(
            json!({"epoch":epoch,"added":completed,"existing":existing,"message":format!("팀 폴더에 사본 {completed}개를 추가했어요. 이미 있던 파일은 {existing}개예요. 원본은 그대로 유지했어요.")}),
        )
    }
    pub async fn retry(
        &self,
        auth: &GoogleAuth,
        profiles: &Profiles,
        teams: &Teams,
        epoch: u64,
        folder: &str,
        op: &str,
        progress: Progress,
    ) -> Result<Value> {
        let lease = auth.full_drive_lease(epoch).await?;
        let _gate = self.gate.lock().await;
        let job = self.job(lease.subject(), op, folder)?;
        progress("copying", 0, 1);
        let file = self.run(auth, profiles, teams, &lease, job).await?;
        Ok(
            json!({"epoch":epoch,"file":file,"message":"팀 폴더의 사본과 저장 결과를 확인했어요. 원본은 유지했어요."}),
        )
    }
    async fn run(
        &self,
        auth: &GoogleAuth,
        profiles: &Profiles,
        teams: &Teams,
        lease: &Lease,
        mut job: CopyJob,
    ) -> Result<Value> {
        let (_, team) = teams
            .access(auth, profiles, lease.epoch(), &job.folder_id)
            .await?;
        if team["canUpload"] != true {
            return err("팀 폴더에 사본을 추가할 권한이 없어요.");
        }
        if let Some(found) = find_copy(auth, lease, &job).await? {
            return self.finish(auth, lease, &job, found).await;
        }
        if let Some(fid) = &job.file_id {
            match metadata(auth, lease, fid).await {
                Ok(meta) => return self.finish(auth, lease, &job, meta).await,
                Err((404, _)) => (),
                Err(e) => return Err(api_error(e)),
            }
        } else if job.attempted {
            return err("Google 문서 복사 요청의 결과를 아직 확인하지 못했어요. 중복 생성을 막기 위해 자동으로 다시 복사하지 않았어요. 잠시 뒤 ‘결과 확인’을 눌러 주세요.");
        }
        let source = metadata(auth, lease, &job.source_id)
            .await
            .map_err(api_error)?;
        let current = source_job(&source, &job.source_id, &job.folder_id)?;
        if current.version != job.version {
            return err(
                "선택한 뒤 원본이 바뀌었어요. 대기 기록을 정리하고 현재 파일을 다시 선택해 주세요.",
            );
        }
        if job.file_id.is_none() && !job.mime.starts_with("application/vnd.google-apps.") {
            let generated = auth
                .profile_request(
                    lease,
                    "GET",
                    "/drive/v3/files/generateIds?count=1&space=drive&type=files",
                    None,
                )
                .await
                .map_err(api_error)?;
            let fid = generated["ids"]
                .as_array()
                .filter(|v| v.len() == 1)
                .and_then(|v| v[0].as_str())
                .filter(|s| id(s))
                .ok_or("사본의 저장 주소를 준비하지 못했어요.")?;
            job.file_id = Some(fid.into());
        }
        job.attempted = true;
        auth.with_lease(lease, || {
            self.ledger(lease.subject(), true, |l| {
                let saved = l
                    .jobs
                    .iter_mut()
                    .find(|j| j.operation == job.operation)
                    .ok_or("가져오기 대기가 바뀌었어요.")?;
                *saved = job.clone();
                Ok(())
            })
        })
        .await?;
        let mut body = json!({"name":job.name,"parents":[job.folder_id],"appProperties":{"saiImport":job.operation,"saiSource":source_tag(&job.source_id)}});
        if let Some(fid) = &job.file_id {
            body["id"] = json!(fid);
        }
        let result = auth
            .profile_request(
                lease,
                "POST",
                &format!(
                    "/drive/v3/files/{}/copy?supportsAllDrives=true&fields={FIELDS}",
                    job.source_id
                ),
                Some(("application/json".into(), body.to_string().into_bytes())),
            )
            .await;
        let meta = match result {
            Ok(meta) => meta,
            Err((409, _)) if job.file_id.is_some() => {
                metadata(auth, lease, job.file_id.as_deref().unwrap())
                    .await
                    .map_err(api_error)?
            }
            Err(e) => {
                if [400, 401, 403, 404].contains(&e.0) {
                    let _ = auth
                        .with_lease(lease, || {
                            self.ledger(lease.subject(), true, |l| {
                                if let Some(j) =
                                    l.jobs.iter_mut().find(|j| j.operation == job.operation)
                                {
                                    j.attempted = false;
                                }
                                Ok(())
                            })
                        })
                        .await;
                }
                return Err(api_error(e));
            }
        };
        teams
            .access(auth, profiles, lease.epoch(), &job.folder_id)
            .await?;
        self.finish(auth, lease, &job, meta).await
    }
    async fn finish(
        &self,
        auth: &GoogleAuth,
        lease: &Lease,
        job: &CopyJob,
        meta: Value,
    ) -> Result<Value> {
        let view = file_view(&meta, &job.folder_id, job.file_id.as_deref())?;
        if meta["appProperties"]["saiImport"] != job.operation
            || meta["appProperties"]["saiSource"] != source_tag(&job.source_id)
            || meta["mimeType"] != job.mime
        {
            return err("사본의 저장 정보가 일치하지 않아 대기를 유지했어요.");
        }
        if let Some(expected) = job.checksum.as_deref() {
            if checksum(&meta)?
                .as_deref()
                .is_some_and(|got| got != expected)
            {
                return err("원본과 사본 내용이 달라요. 원본 변경 여부를 확인해 주세요.");
            }
        }
        if let Some(expected) = job.size {
            if size(&meta)? != expected {
                return err("사본 크기가 달라 대기 정보를 유지했어요.");
            }
        }
        auth.with_lease(lease, || {
            self.ledger(lease.subject(), true, |l| {
                l.jobs.retain(|j| j.operation != job.operation);
                Ok(view)
            })
        })
        .await
    }
}
fn valid_version(s: &str) -> bool {
    !s.is_empty() && s.len() <= 32 && s.bytes().all(|b| b.is_ascii_digit())
}
fn source_tag(id: &str) -> String {
    format!("{:x}", Sha256::digest(id.as_bytes()))
}
fn source_job(meta: &Value, source: &str, folder: &str) -> Result<CopyJob> {
    let mime = meta["mimeType"]
        .as_str()
        .ok_or("파일 형식을 확인하지 못했어요.")?;
    if meta["id"] != source
        || meta["properties"]["saiCollab"] == "1"
        || meta["trashed"] != false
        || meta["capabilities"]["canCopy"] != true
        || [
            "application/vnd.google-apps.folder",
            "application/vnd.google-apps.shortcut",
        ]
        .contains(&mime)
    {
        return err(
            "복사 가능한 파일만 선택해 주세요. 폴더·바로가기·복사 제한 파일은 추가하지 않았어요.",
        );
    }
    let name = meta["name"]
        .as_str()
        .filter(|s| !s.is_empty() && s.chars().count() <= 1000)
        .ok_or("파일 이름을 확인하지 못했어요.")?;
    let version = meta["version"]
        .as_str()
        .filter(|s| valid_version(s))
        .ok_or("파일 변경 번호를 확인하지 못했어요.")?;
    Ok(CopyJob {
        operation: format!("copy-{}", crate::google::oauth::random_token()),
        folder_id: folder.into(),
        source_id: source.into(),
        name: name.into(),
        mime: mime.into(),
        version: version.into(),
        size: if mime.starts_with("application/vnd.google-apps.") {
            None
        } else {
            Some(size(meta)?)
        },
        checksum: checksum(meta)?,
        file_id: None,
        attempted: false,
    })
}
async fn find_copy(auth: &GoogleAuth, lease: &Lease, job: &CopyJob) -> Result<Option<Value>> {
    let q=format!("'{}' in parents and trashed = false and appProperties has {{ key='saiImport' and value='{}' }}",job.folder_id,job.operation);
    let value=auth.profile_request(lease,"GET",&format!("/drive/v3/files?spaces=drive&pageSize=2&fields=incompleteSearch,nextPageToken,files({FIELDS})&q={}",urlencoding::encode(&q)),None).await.map_err(api_error)?;
    let rows = value["files"]
        .as_array()
        .ok_or("사본의 저장 결과 목록을 읽지 못했어요.")?;
    if value["incompleteSearch"] == true || value.get("nextPageToken").is_some() || rows.len() > 1 {
        return err("사본 저장 결과를 하나로 확인하지 못했어요. 중복 복사를 진행하지 않았어요.");
    }
    Ok(rows.first().cloned())
}
