//! Personal appData journal, isolated from all legacy Firebase backup formats.
mod cloud;
pub(super) mod model;
use super::oauth::{GoogleAuth, Lease};
use crate::{err, store::Store, Result};
use model::{Account, Patch};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::PathBuf, sync::Mutex};

pub(super) struct Profiles {
    root: PathBuf,
    stores: Mutex<BTreeMap<String, Store>>,
    errors: Mutex<BTreeMap<String, String>>,
    gate: tokio::sync::Mutex<()>,
}
impl Profiles {
    /// Last settings-sync failure for this account, so the team screen can say why joining is paused.
    pub fn sync_error(&self, subject: &str) -> Option<String> {
        self.errors.lock().unwrap().get(subject).cloned()
    }
    pub fn team_state(&self, subject: &str) -> Result<Value> {
        self.account(subject, |a| Ok(json!({"ready":a.checked,"teams":a.view()["teams"],"draft":a.folder_draft,"pending":a.outbox.len()})), false)
    }
    pub async fn draft(
        &self,
        auth: &GoogleAuth,
        lease: &Lease,
        name: &str,
    ) -> Result<model::FolderDraft> {
        auth.with_lease(lease, || {
            self.account(
                lease.subject(),
                |a| {
                    if !a.checked
                        || a.records.len() >= model::LIMIT
                        || a.view()["teams"].as_array().unwrap().len() >= 32
                    {
                        return err("계정 동기화 상태와 팀 보관 한도를 먼저 확인해 주세요.");
                    }
                    if let Some(draft) = &a.folder_draft {
                        if !name.is_empty() && draft.name != name {
                            return err(
                                "만들던 팀 폴더가 있어요. 먼저 이어 만들기를 완료해 주세요.",
                            );
                        }
                        return Ok(draft.clone());
                    }
                    if name.trim().is_empty()
                        || name.chars().count() > 100
                        || name.chars().any(char::is_control)
                    {
                        return err("팀 이름을 1~100자로 입력해 주세요.");
                    }
                    let draft = model::FolderDraft {
                        name: name.into(),
                        file_id: None,
                        operation: super::oauth::random_token(),
                    };
                    a.folder_draft = Some(draft.clone());
                    Ok(draft)
                },
                true,
            )
        })
        .await
    }
    pub async fn draft_id(
        &self,
        auth: &GoogleAuth,
        lease: &Lease,
        operation: &str,
        id: &str,
    ) -> Result<()> {
        auth.with_lease(lease, || {
            self.account(
                lease.subject(),
                |a| {
                    let draft = a
                        .folder_draft
                        .as_mut()
                        .filter(|d| d.operation == operation)
                        .ok_or("만들던 폴더 정보가 바뀌었어요.")?;
                    if !model::id(id) {
                        return err("폴더 주소를 확인하지 못했어요.");
                    }
                    draft.file_id = Some(id.into());
                    Ok(())
                },
                true,
            )
        })
        .await
    }
    pub async fn remember_team(
        &self,
        auth: &GoogleAuth,
        lease: &Lease,
        team: model::TeamRef,
        finish: Option<&str>,
    ) -> Result<()> {
        auth.with_lease(lease, || {
            let actor = self.actor()?;
            self.account(
                lease.subject(),
                |a| {
                    a.team_edit(&actor, team)?;
                    if let Some(operation) = finish {
                        if !a
                            .folder_draft
                            .as_ref()
                            .is_some_and(|d| d.operation == operation)
                        {
                            return err("만들던 폴더 정보가 바뀌었어요.");
                        }
                        a.folder_draft = None;
                    }
                    Ok(())
                },
                true,
            )
        })
        .await
    }
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            stores: Mutex::new(BTreeMap::new()),
            errors: Mutex::new(BTreeMap::new()),
            gate: tokio::sync::Mutex::new(()),
        }
    }
    fn account<T>(
        &self,
        subject: &str,
        edit: impl FnOnce(&mut Account) -> Result<T>,
        save: bool,
    ) -> Result<T> {
        // This lock also serializes read-modify-commit against local edits during network I/O.
        let mut stores = self.stores.lock().unwrap();
        let store = stores.entry(subject.into()).or_insert_with(|| {
            let filename = format!("{:x}.json", Sha256::digest(subject.as_bytes()));
            let path = self.root.join("accounts").join(filename);
            let defaults = if path.exists() {
                Value::Null
            } else {
                serde_json::to_value(Account::empty(subject)).unwrap()
            };
            Store::open(path, defaults)
        });
        if let Some(error) = store.load_error() {
            return Err(error);
        }
        let mut account: Account = serde_json::from_value(store.get())
            .map_err(|_| "이 기기의 계정 자료를 읽지 못했어요. 원본은 유지했어요.")?;
        if !account.valid(subject) {
            return err("이 기기의 계정 자료를 확인하지 못했어요. 원본은 유지했어요.");
        }
        let result = edit(&mut account)?;
        if save {
            let next =
                serde_json::to_value(account).map_err(|_| "계정 설정을 준비하지 못했어요.")?;
            store.commit(|v| *v = next).map_err(|_| {
                "이 기기에 계정 설정을 저장하지 못했어요. 저장 공간을 확인해 주세요."
            })?;
        }
        Ok(result)
    }
    fn actor(&self) -> Result<String> {
        // app_local_data_dir, deliberately not the roaming settings directory.
        let path = self.root.join("installation.json");
        let defaults = if path.exists() {
            Value::Null
        } else {
            json!({"version":1,"actor":super::oauth::random_token()})
        };
        let store = Store::open(path, defaults);
        if let Some(error) = store.load_error() {
            return Err(error);
        }
        let v = store.get();
        let actor = v["actor"].as_str().filter(|s| model::id(s));
        if v["version"] != 1 || actor.is_none() {
            return err("이 기기의 설정 동기화 정보를 확인하지 못했어요.");
        }
        store
            .commit(|_| ())
            .map_err(|_| "이 기기의 동기화 정보를 저장하지 못했어요.")?;
        Ok(actor.unwrap().into())
    }
    pub fn status(&self, subject: &str, epoch: u64) -> Value {
        match self.account(subject, |a| Ok(json!({"ready":a.checked,"profile":a.view(),"pending":a.outbox.len(),"lastSync":a.last_sync})), false) {
            Ok(mut v) => {
                v["epoch"] = json!(epoch);
                v["error"] = json!(self.errors.lock().unwrap().get(subject));
                v
            }
            Err(error) => json!({"epoch":epoch,"ready":false,"error":error}),
        }
    }
    fn apply(&self, settings: &Store, subject: &str) -> Result<()> {
        self.account(
            subject,
            |a| {
                if a.checked {
                    let view = a.view();
                    settings.set_overlay(Some(
                        json!({"catName":view["catName"],"frequency":view["frequency"]}),
                    ));
                }
                Ok(())
            },
            false,
        )
    }
    pub async fn update(
        &self,
        auth: &GoogleAuth,
        settings: &Store,
        epoch: u64,
        patch: Value,
    ) -> Result<()> {
        let patch: Patch = serde_json::from_value(patch)
            .map_err(|_| "Google에 저장할 이름과 설정을 확인해 주세요.")?;
        let lease = auth.lease().await?;
        if epoch != lease.epoch() {
            return err("계정 연결이 바뀌었어요. 설정을 다시 열어 주세요.");
        }
        auth.with_lease(&lease, || {
            let actor = self.actor()?;
            self.account(lease.subject(), |a| a.edit(&actor, patch), true)?;
            self.apply(settings, lease.subject())
        })
        .await
    }
    pub async fn sync(&self, auth: &GoogleAuth, settings: &Store) -> Result<()> {
        // Capture before waiting: a queued sync must never silently switch accounts.
        let lease = auth.lease().await?;
        let _gate = self.gate.lock().await;
        auth.with_lease(&lease, || self.apply(settings, lease.subject()))
            .await?;
        let result = self.sync_inner(auth, settings, &lease).await;
        if auth.lease_current(&lease) {
            let mut errors = self.errors.lock().unwrap();
            match &result {
                Ok(_) => {
                    errors.remove(lease.subject());
                }
                Err(e) => {
                    errors.insert(lease.subject().into(), e.clone());
                }
            }
        }
        result
    }
    async fn sync_inner(&self, auth: &GoogleAuth, settings: &Store, lease: &Lease) -> Result<()> {
        let incoming = cloud::read(auth, lease).await?;
        auth.with_lease(lease, || {
            self.account(
                lease.subject(),
                |a| {
                    a.merge(incoming)?;
                    a.checked = true;
                    Ok(())
                },
                true,
            )?;
            self.apply(settings, lease.subject())
        })
        .await?;
        let pending = self.account(lease.subject(), |a| Ok(a.outbox.clone()), false)?;
        for p in pending {
            let fid = match p.file_id {
                Some(id) => id,
                None => {
                    let id = cloud::generate_id(auth, lease).await?;
                    auth.with_lease(lease, || {
                        self.account(
                            lease.subject(),
                            |a| {
                                let p = a
                                    .outbox
                                    .iter_mut()
                                    .find(|o| o.op_id == p.op_id)
                                    .ok_or("설정 대기 내역이 바뀌었어요.")?;
                                p.file_id = Some(id.clone());
                                Ok(())
                            },
                            true,
                        )
                    })
                    .await?;
                    id
                }
            };
            let record = self.account(
                lease.subject(),
                |a| {
                    a.records
                        .iter()
                        .find(|r| r.op_id == p.op_id)
                        .cloned()
                        .ok_or("대기 중인 설정 내용을 찾지 못했어요.".into())
                },
                false,
            )?;
            cloud::write(auth, lease, &fid, &record).await?;
            auth.with_lease(lease, || {
                self.account(
                    lease.subject(),
                    |a| {
                        a.outbox.retain(|o| o.op_id != p.op_id);
                        Ok(())
                    },
                    true,
                )
            })
            .await?;
        }
        auth.with_lease(lease, || {
            self.account(
                lease.subject(),
                |a| {
                    a.last_sync = crate::time::now_ms();
                    Ok(())
                },
                true,
            )
        })
        .await
    }
}
#[cfg(test)]
mod tests;
