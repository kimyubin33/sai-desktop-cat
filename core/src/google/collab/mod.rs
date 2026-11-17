//! Team Drive operation journal. No clock-based winner, server, or whole-board overwrite.
mod cloud;
pub(super) mod model;
use super::{
    oauth::{GoogleAuth, Lease},
    profile::{model::id, Profiles},
    teams::Teams,
};
use crate::{err, store::Store, Result};
use model::{Edit, Record};
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
struct Remote {
    file_id: String,
    revision: String,
    record: Record,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Pending {
    file_id: Option<String>,
    record: Record,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Board {
    version: u32,
    subject: String,
    folder_id: String,
    remote: Vec<Remote>,
    pending: Vec<Pending>,
    last_sync: i64,
}
impl Board {
    fn records(&self) -> Result<Vec<Record>> {
        let mut map: BTreeMap<String, Record> = BTreeMap::new();
        for r in self
            .remote
            .iter()
            .map(|r| &r.record)
            .chain(self.pending.iter().map(|p| &p.record))
        {
            if map
                .insert(r.op_id.clone(), r.clone())
                .is_some_and(|old| old != *r)
            {
                return err("같은 작업 번호의 협업 내용이 달라요.");
            }
        }
        let records = map.into_values().collect::<Vec<_>>();
        model::validate(&records, &self.folder_id)?;
        Ok(records)
    }
    fn valid(&self, subject: &str, folder: &str) -> bool {
        self.version == 1
            && self.subject == subject
            && self.folder_id == folder
            && id(folder)
            && id(subject)
            && self.pending.len() <= 32
            && self.remote.len() <= model::LIMIT
            && self.records().is_ok()
            && self.remote.iter().all(|r| {
                id(&r.file_id)
                    && !r.revision.is_empty()
                    && r.revision.len() <= 32
                    && r.revision.bytes().all(|b| b.is_ascii_digit())
            })
            && self
                .pending
                .iter()
                .all(|p| p.file_id.as_deref().is_none_or(id))
            && self
                .remote
                .iter()
                .map(|r| &r.file_id)
                .collect::<BTreeSet<_>>()
                .len()
                == self.remote.len()
            && self
                .remote
                .iter()
                .map(|r| &r.record.op_id)
                .collect::<BTreeSet<_>>()
                .len()
                == self.remote.len()
            && self
                .pending
                .iter()
                .map(|r| &r.record.op_id)
                .collect::<BTreeSet<_>>()
                .len()
                == self.pending.len()
    }
}
pub(super) struct Collaboration {
    root: PathBuf,
    stores: Mutex<BTreeMap<String, Store>>,
    gate: tokio::sync::Mutex<()>,
}
impl Collaboration {
    pub async fn calendar_task(
        &self,
        auth: &GoogleAuth,
        profiles: &Profiles,
        teams: &Teams,
        epoch: u64,
        folder: &str,
        task: &str,
        expected: Option<&[String]>,
    ) -> Result<Value> {
        if !id(task) {
            return err("추가할 할 일을 다시 선택해 주세요.");
        }
        let state = self
            .load(auth, profiles, teams, epoch, folder, false)
            .await?;
        let item = state["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["id"] == task)
            .ok_or("현재 팀에서 할 일을 찾지 못했어요.")?;
        if item["kind"] != "task" || item["conflict"] != false {
            return err("동시 변경을 정리한 할 일만 Calendar에 추가할 수 있어요.");
        }
        let heads: Vec<String> = serde_json::from_value(item["heads"].clone())
            .map_err(|_| "할 일의 변경 정보를 확인하지 못했어요.")?;
        if let Some(expected) = expected {
            let mut expected = expected.to_vec();
            expected.sort();
            let mut current = heads.clone();
            current.sort();
            if expected != current {
                return err("할 일이 변경됐어요. 최신 제목과 날짜를 다시 확인해 주세요.");
            }
        }
        let entity = &item["variants"][0]["entity"];
        Ok(json!({"epoch":epoch,"folderId":folder,"taskId":task,"heads":heads,"entity":entity}))
    }
    pub fn projects(&self, subject: &str, folder: &str) -> Result<Value> {
        self.board(subject, folder, false, |b| {
            Ok(json!(model::view(&b.records()?)
                .as_array()
                .unwrap()
                .iter()
                .filter(|i| i["kind"] == "project" && i["conflict"] == false)
                .map(|i| json!({"id":i["id"],"name":i["variants"][0]["entity"]["title"]}))
                .collect::<Vec<_>>()))
        })
    }
    pub async fn source_tasks(
        &self,
        auth: &GoogleAuth,
        profiles: &Profiles,
        teams: &Teams,
        epoch: u64,
        folder: &str,
        group: &str,
    ) -> Result<Value> {
        let lease = auth.full_drive_lease(epoch).await?;
        let _gate = self.gate.lock().await;
        teams.access(auth, profiles, epoch, folder).await?;
        self.pull(auth, &lease, folder).await?;
        let prefix = ai_prefix(group)?;
        auth.with_lease(&lease, || {
            self.board(lease.subject(), folder, false, |b| {
                Ok(json!(model::view(&b.records()?)
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|v| v["id"].as_str().is_some_and(|id| id.starts_with(&prefix)))
                    .cloned()
                    .collect::<Vec<_>>()))
            })
        })
        .await
    }
    pub async fn save_ai(
        &self,
        auth: &GoogleAuth,
        profiles: &Profiles,
        teams: &Teams,
        epoch: u64,
        folder: &str,
        group: &str,
        mut entities: Vec<model::Entity>,
    ) -> Result<Value> {
        let lease = auth.full_drive_lease(epoch).await?;
        let _gate = self.gate.lock().await;
        let (_, team) = teams.access(auth, profiles, epoch, folder).await?;
        if team["canUpload"] != true {
            return err("이 팀에는 할 일을 공유할 권한이 없어요.");
        }
        let prefix = ai_prefix(group)?;
        if entities.is_empty() || entities.len() > 12 {
            return err("검토한 할 일을 1~12개 선택해 주세요.");
        }
        for (slot, e) in entities.iter_mut().enumerate() {
            e.id = format!("{prefix}{slot}");
            if e.kind != model::Kind::Task || !e.valid() {
                return err("할 일의 제목·담당자·날짜를 확인해 주세요.");
            }
        }
        self.pull(auth, &lease, folder).await?;
        let added = auth
            .with_lease(&lease, || {
                self.board(lease.subject(), folder, true, |b| {
                    let records = b.records()?;
                    if records.iter().any(|r| r.entity.id.starts_with(&prefix)) {
                        return Ok(0);
                    }
                    if b.pending.len() + entities.len() > 32
                        || records.len() + entities.len() > model::LIMIT
                    {
                        return err("팀의 보관 한도나 전송 대기를 먼저 확인해 주세요.");
                    }
                    let count = entities.len();
                    for entity in entities {
                        b.pending.push(Pending {
                            file_id: None,
                            record: Record {
                                version: 1,
                                folder_id: folder.into(),
                                op_id: format!("work-{}", super::oauth::random_token()),
                                parents: vec![],
                                entity,
                            },
                        });
                    }
                    Ok(count)
                })
            })
            .await?;
        let error = match self.push(auth, profiles, teams, &lease, folder).await {
            Ok(()) => self.pull(auth, &lease, folder).await.err(),
            Err(e) => Some(e),
        };
        let state = self.view(auth, &lease, folder, team, error).await?;
        Ok(
            json!({"epoch":epoch,"folderId":folder,"added":added,"alreadyImported":added==0,"state":state}),
        )
    }
    pub async fn remove_team(
        &self,
        auth: &GoogleAuth,
        profiles: &Profiles,
        teams: &Teams,
        epoch: u64,
        folder: &str,
    ) -> Result<()> {
        let lease = auth.lease().await?;
        if lease.epoch() != epoch {
            return err("계정 연결이 바뀌었어요.");
        }
        let _gate = self.gate.lock().await;
        if self.board(lease.subject(), folder, false, |b| {
            Ok(!b.pending.is_empty())
        })? {
            return err("이 팀에 아직 공유하지 못한 협업 변경이 있어요. 협업 화면에서 먼저 동기화해 주세요. 권한이 취소됐다면 팀 소유자에게 접근 복구를 요청해 주세요.");
        }
        teams.remove(auth, profiles, epoch, folder).await
    }
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            stores: Mutex::new(BTreeMap::new()),
            gate: tokio::sync::Mutex::new(()),
        }
    }
    fn board<T>(
        &self,
        subject: &str,
        folder: &str,
        save: bool,
        f: impl FnOnce(&mut Board) -> Result<T>,
    ) -> Result<T> {
        if !id(subject) || !id(folder) {
            return err("협업 계정과 팀을 확인해 주세요.");
        }
        let key = format!(
            "{:x}",
            Sha256::digest(format!("{subject}\n{folder}").as_bytes())
        );
        let mut stores = self.stores.lock().unwrap();
        let store=stores.entry(key.clone()).or_insert_with(||{
            let path=self.root.join(format!("{key}.json"));let value=if path.exists(){Value::Null}else{json!({"version":1,"subject":subject,"folderId":folder,"remote":[],"pending":[],"lastSync":0})};Store::open(path,value)
        });
        if let Some(e) = store.load_error() {
            return Err(e);
        }
        let mut board: Board = serde_json::from_value(store.get())
            .map_err(|_| "협업 보관 자료를 읽지 못했어요. 기존 자료는 유지했어요.")?;
        if !board.valid(subject, folder) {
            return err("협업 보관 자료의 계정·팀·변경 기록을 확인하지 못했어요.");
        }
        let result = f(&mut board)?;
        if save {
            if !board.valid(subject, folder) {
                return err("저장할 협업 기록을 확인하지 못했어요.");
            }
            let value =
                serde_json::to_value(board).map_err(|_| "협업 보관 내용을 준비하지 못했어요.")?;
            store
                .commit(|v| *v = value)
                .map_err(|_| "협업 대기를 PC에 저장하지 못했어요.")?;
        }
        Ok(result)
    }
    async fn pull(&self, auth: &GoogleAuth, lease: &Lease, folder: &str) -> Result<()> {
        let cache = self.board(lease.subject(), folder, false, |b| Ok(b.remote.clone()))?;
        let remote = cloud::read(auth, lease, folder, &cache).await?;
        auth.with_lease(lease, || {
            self.board(lease.subject(), folder, true, |b| {
                for p in &b.pending {
                    if remote
                        .iter()
                        .any(|r| r.record.op_id == p.record.op_id && r.record != p.record)
                    {
                        return err("대기 내용과 Drive 협업 기록이 달라요.");
                    }
                }
                b.pending
                    .retain(|p| !remote.iter().any(|r| r.record.op_id == p.record.op_id));
                b.remote = remote;
                b.last_sync = crate::time::now_ms();
                Ok(())
            })
        })
        .await
    }
    async fn push(
        &self,
        auth: &GoogleAuth,
        profiles: &Profiles,
        teams: &Teams,
        lease: &Lease,
        folder: &str,
    ) -> Result<()> {
        loop {
            let next = self.board(lease.subject(), folder, false, |b| {
                Ok(b.pending.first().cloned())
            })?;
            let Some(mut next) = next else {
                return Ok(());
            };
            let (_, team) = teams.access(auth, profiles, lease.epoch(), folder).await?;
            if team["canUpload"] != true {
                return err("팀에 수정 권한이 없어요. 이 PC의 대기 내용은 유지했어요.");
            }
            if next.file_id.is_none() {
                let fid = cloud::generate(auth, lease).await?;
                next.file_id = Some(fid.clone());
                auth.with_lease(lease, || {
                    self.board(lease.subject(), folder, true, |b| {
                        b.pending
                            .iter_mut()
                            .find(|p| p.record.op_id == next.record.op_id)
                            .ok_or("협업 대기가 바뀌었어요.")?
                            .file_id = Some(fid);
                        Ok(())
                    })
                })
                .await?;
            }
            let written =
                cloud::write(auth, lease, next.file_id.as_deref().unwrap(), &next.record).await?;
            teams.access(auth, profiles, lease.epoch(), folder).await?;
            auth.with_lease(lease, || {
                self.board(lease.subject(), folder, true, |b| {
                    b.pending.retain(|p| p.record.op_id != written.record.op_id);
                    b.remote.push(written);
                    b.last_sync = crate::time::now_ms();
                    Ok(())
                })
            })
            .await?;
        }
    }
    async fn view(
        &self,
        auth: &GoogleAuth,
        lease: &Lease,
        folder: &str,
        team: Value,
        error: Option<String>,
    ) -> Result<Value> {
        auth.with_lease(lease,||self.board(lease.subject(),folder,false,|b|Ok(json!({"epoch":lease.epoch(),"folderId":folder,"team":team,"items":model::view(&b.records()?),"pending":b.pending.len(),"lastSync":b.last_sync,"error":error})))).await
    }
    pub async fn load(
        &self,
        auth: &GoogleAuth,
        profiles: &Profiles,
        teams: &Teams,
        epoch: u64,
        folder: &str,
        sync: bool,
    ) -> Result<Value> {
        let lease = auth.full_drive_lease(epoch).await?;
        let _gate = self.gate.lock().await;
        let (_, team) = teams.access(auth, profiles, epoch, folder).await?;
        self.pull(auth, &lease, folder).await?;
        let error = if sync {
            match self.push(auth, profiles, teams, &lease, folder).await {
                Ok(()) => self.pull(auth, &lease, folder).await.err(),
                Err(e) => Some(e),
            }
        } else {
            None
        };
        self.view(auth, &lease, folder, team, error).await
    }
    pub async fn save(
        &self,
        auth: &GoogleAuth,
        profiles: &Profiles,
        teams: &Teams,
        epoch: u64,
        folder: &str,
        value: Value,
    ) -> Result<Value> {
        let lease = auth.full_drive_lease(epoch).await?;
        let _gate = self.gate.lock().await;
        let (_, team) = teams.access(auth, profiles, epoch, folder).await?;
        if team["canUpload"] != true {
            return err("이 팀에는 협업 내용을 수정할 권한이 없어요.");
        }
        let mut edit: Edit =
            serde_json::from_value(value).map_err(|_| "저장할 프로젝트·할 일을 확인해 주세요.")?;
        if edit.entity.id.is_empty() {
            if !edit.heads.is_empty() {
                return err("새 항목의 변경 정보를 확인해 주세요.");
            }
            edit.entity.id = format!("item-{}", super::oauth::random_token());
        }
        if !edit.entity.valid()
            || edit.heads.len() > 16
            || edit.heads.iter().any(|s| !id(s))
            || edit.heads.iter().collect::<BTreeSet<_>>().len() != edit.heads.len()
        {
            return err("제목·프로젝트·담당자·마감일을 확인해 주세요.");
        }
        self.pull(auth, &lease, folder).await?;
        auth.with_lease(&lease,||self.board(lease.subject(),folder,true,|b|{
            let records=b.records()?;let mut heads=model::heads(&records,&edit.entity.id).iter().map(|r|r.op_id.clone()).collect::<Vec<_>>();heads.sort();edit.heads.sort();
            if heads!=edit.heads{return err("다른 변경이 먼저 도착했어요. 입력은 유지했어요. 새로고침 후 최신 항목을 확인해 주세요.");}
            if b.pending.len()>=32||records.len()>=model::LIMIT{return err("협업 보관 한도에 도달했어요. 대기를 먼저 동기화해 주세요.");}
            b.pending.push(Pending{file_id:None,record:Record{version:1,folder_id:folder.into(),op_id:format!("work-{}",super::oauth::random_token()),parents:edit.heads,entity:edit.entity}});Ok(())
        })).await?;
        let error = match self.push(auth, profiles, teams, &lease, folder).await {
            Ok(()) => self.pull(auth, &lease, folder).await.err(),
            Err(e) => Some(e),
        };
        self.view(auth, &lease, folder, team, error).await
    }
}
fn ai_prefix(group: &str) -> Result<String> {
    if group.len() != 64
        || !group
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return err("분석 자료의 고유 번호를 확인하지 못했어요.");
    }
    Ok(format!("ai-{group}-"))
}
#[cfg(test)]
mod tests;
