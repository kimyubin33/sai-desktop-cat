//! Explicit preview -> one paid attempt -> reviewed, idempotent task import.
//! Source and results remain in memory. Disk contains attempt IDs/fingerprints only.
use super::{
    collab::{
        model::{Entity, Kind, Status},
        Collaboration,
    },
    files::{
        text::{validate_text, TextSource},
        Files,
    },
    oauth::{GoogleAuth, Lease},
    profile::{model::id, Profiles},
    team_ai::{Attempt, Client},
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
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub(super) enum Input {
    Text {
        text: String,
    },
    Drive {
        #[serde(rename = "fileId")]
        file_id: String,
    },
}
#[derive(Clone, PartialEq)]
enum Phase {
    Preview,
    Started,
    Ready,
    Failed,
    Done,
}
#[derive(Clone)]
struct Session {
    subject: String,
    epoch: u64,
    folder: String,
    created: i64,
    source: Option<TextSource>,
    text: String,
    group: String,
    request_id: String,
    phase: Phase,
    result: Option<Value>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Trace {
    request_id: String,
    at: i64,
    finished: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    version: u32,
    subject: String,
    attempts: BTreeMap<String, Trace>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Choice {
    index: usize,
    title: String,
    assignee: String,
    due_date: String,
    project_id: String,
}
pub(super) struct Reviews {
    root: PathBuf,
    stores: Mutex<BTreeMap<String, Store>>,
    sessions: Mutex<BTreeMap<String, Session>>,
    gate: tokio::sync::Mutex<()>,
}
fn digest(s: &str) -> String {
    format!("{:x}", Sha256::digest(s.as_bytes()))
}
fn fingerprint(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}
impl Reviews {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            stores: Mutex::new(BTreeMap::new()),
            sessions: Mutex::new(BTreeMap::new()),
            gate: tokio::sync::Mutex::new(()),
        }
    }
    pub fn clear(&self) {
        self.sessions.lock().unwrap().clear();
    }
    pub fn forget(&self, epoch: u64, folder: &str, review: &str) {
        self.sessions
            .lock()
            .unwrap()
            .retain(|id, s| !(id == review && s.epoch == epoch && s.folder == folder));
    }
    fn journal<T>(
        &self,
        subject: &str,
        save: bool,
        f: impl FnOnce(&mut Journal) -> Result<T>,
    ) -> Result<T> {
        let mut stores = self.stores.lock().unwrap();
        let store = stores.entry(subject.into()).or_insert_with(|| {
            let path = self.root.join(format!("{}.json", digest(subject)));
            let defaults = if path.exists() {
                Value::Null
            } else {
                json!({"version":1,"subject":subject,"attempts":{}})
            };
            Store::open(path, defaults)
        });
        if let Some(e) = store.load_error() {
            return Err(e);
        }
        let mut journal: Journal = serde_json::from_value(store.get())
            .map_err(|_| "분석 시도 기록을 읽지 못했어요. 기존 기록은 유지했어요.")?;
        if journal.version != 1
            || journal.subject != subject
            || !id(subject)
            || journal.attempts.len() > 512
            || journal
                .attempts
                .iter()
                .any(|(group, t)| !fingerprint(group) || !id(&t.request_id) || t.at <= 0)
        {
            return err("분석 시도 기록의 계정과 형식을 확인하지 못했어요.");
        }
        let result = f(&mut journal)?;
        if save {
            while journal.attempts.len() > 512 {
                let oldest = journal
                    .attempts
                    .iter()
                    .min_by_key(|(_, v)| v.at)
                    .map(|(k, _)| k.clone())
                    .unwrap();
                journal.attempts.remove(&oldest);
            }
            let value =
                serde_json::to_value(journal).map_err(|_| "분석 시도 기록을 준비하지 못했어요.")?;
            store
                .commit(|v| *v = value)
                .map_err(|_| "분석 시도 기록을 PC에 보관하지 못했어요.")?;
        }
        Ok(result)
    }
    fn session(&self, lease: &Lease, folder: &str, review: &str) -> Result<Session> {
        self.sessions
            .lock()
            .unwrap()
            .get(review)
            .filter(|s| {
                s.subject == lease.subject()
                    && s.epoch == lease.epoch()
                    && s.folder == folder
                    && crate::time::now_ms() - s.created < 30 * 60 * 1000
            })
            .cloned()
            .ok_or(
                "현재 계정의 분석 미리보기가 없거나 만료됐어요. 자료를 다시 선택해 주세요.".into(),
            )
    }
    async fn check(
        &self,
        files: &Files,
        auth: &GoogleAuth,
        profiles: &Profiles,
        teams: &Teams,
        s: &Session,
    ) -> Result<()> {
        if let Some(source) = &s.source {
            files
                .check_text_source(auth, profiles, teams, s.epoch, &s.folder, source)
                .await?;
        } else {
            teams.access(auth, profiles, s.epoch, &s.folder).await?;
        }
        Ok(())
    }
    pub async fn prepare(
        &self,
        auth: &GoogleAuth,
        profiles: &Profiles,
        teams: &Teams,
        files: &Files,
        collab: &Collaboration,
        epoch: u64,
        folder: &str,
        input: Input,
    ) -> Result<Value> {
        let lease = auth.full_drive_lease(epoch).await?;
        let _gate = self
            .gate
            .try_lock()
            .map_err(|_| "다른 분석 작업을 마친 뒤 다시 선택해 주세요.")?;
        let (_, team) = teams.access(auth, profiles, epoch, folder).await?;
        let (source, text, key) = match input {
            Input::Text { text } => {
                validate_text(&text)?;
                let key = format!(
                    "text\n{}",
                    text.split_whitespace().collect::<Vec<_>>().join(" ")
                );
                (None, text, key)
            }
            Input::Drive { file_id } => {
                let source = files
                    .text_source(auth, profiles, teams, epoch, folder, &file_id)
                    .await?;
                let key = format!("drive\n{}", source.file_id);
                let text = source.text.clone();
                (Some(source), text, key)
            }
        };
        let group = digest(&format!("{folder}\n{key}"));
        let existing = collab
            .source_tasks(auth, profiles, teams, epoch, folder, &group)
            .await?;
        let prior = self.journal(lease.subject(), false, |j| {
            Ok(j.attempts.contains_key(&group))
        })?;
        let review = super::oauth::random_token();
        let name = source
            .as_ref()
            .map(|s| s.name.clone())
            .unwrap_or("직접 입력한 텍스트".into());
        let session = Session {
            subject: lease.subject().into(),
            epoch,
            folder: folder.into(),
            created: crate::time::now_ms(),
            source,
            text: text.clone(),
            group,
            request_id: super::oauth::random_token(),
            phase: Phase::Preview,
            result: None,
        };
        auth.with_lease(&lease,||{
            let mut sessions=self.sessions.lock().unwrap();sessions.retain(|_,s|s.subject==lease.subject()&&s.epoch==epoch&&crate::time::now_ms()-s.created<30*60*1000);
            if sessions.len()>=4{let oldest=sessions.iter().min_by_key(|(_,s)|s.created).map(|(k,_)|k.clone()).unwrap();sessions.remove(&oldest);}
            sessions.insert(review.clone(),session);Ok(json!({"epoch":epoch,"folderId":folder,"reviewId":review,"name":name,"text":text,"bytes":text.len(),"priorAttempt":prior,"existing":existing,"canShare":team["canUpload"],"projects":collab.projects(lease.subject(),folder)?}))
        }).await
    }
    pub async fn run(
        &self,
        auth: &GoogleAuth,
        profiles: &Profiles,
        teams: &Teams,
        files: &Files,
        collab: &Collaboration,
        epoch: u64,
        folder: &str,
        review: &str,
        confirmed: bool,
        endpoint: Option<&str>,
    ) -> Result<Value> {
        if !confirmed {
            return err("전송할 내용과 처리 서비스를 먼저 확인해 주세요.");
        }
        let lease = auth.full_drive_lease(epoch).await?;
        let _gate = self
            .gate
            .try_lock()
            .map_err(|_| "분석이 진행 중이에요. 같은 요청을 다시 보내지 않았어요.")?;
        let s = self.session(&lease, folder, review)?;
        self.check(files, auth, profiles, teams, &s).await?;
        if s.phase == Phase::Ready || s.phase == Phase::Done {
            return auth
                .with_lease(&lease, || {
                    Ok(json!({"epoch":epoch,"folderId":folder,"reviewId":review,"result":s.result}))
                })
                .await;
        }
        if s.phase != Phase::Preview {
            return err("이미 시도한 분석이에요. 자동 재호출하지 않았어요. 결과를 잃었다면 자료를 새로 선택해 새 비용 가능성을 확인해 주세요.");
        }
        if !collab
            .source_tasks(auth, profiles, teams, epoch, folder, &s.group)
            .await?
            .as_array()
            .unwrap()
            .is_empty()
        {
            return err(
                "이미 이 자료에서 등록한 할 일이 있어요. 협업에서 기존 할 일을 확인해 주세요.",
            );
        }
        let client = Client::new(endpoint)?;
        let status = client.status(auth, profiles, teams, epoch, folder).await?;
        if status["ready"] != true {
            return err(
                "팀 Claude 연결이 아직 준비되지 않았어요. 연결 상태와 권한 안내를 확인해 주세요.",
            );
        }
        let now = crate::time::now_ms();
        auth.with_lease(&lease, || {
            self.journal(lease.subject(), true, |j| {
                j.attempts.insert(
                    s.group.clone(),
                    Trace {
                        request_id: s.request_id.clone(),
                        at: now,
                        finished: false,
                    },
                );
                Ok(())
            })?;
            self.sessions
                .lock()
                .unwrap()
                .get_mut(review)
                .ok_or("미리보기가 닫혔어요.")?
                .phase = Phase::Started;
            Ok(())
        })
        .await?;
        let result = async {
            let result = client
                .analyze(
                    auth,
                    profiles,
                    teams,
                    epoch,
                    folder,
                    &Attempt {
                        request_id: s.request_id.clone(),
                        created_at: now,
                        text: s.text.clone(),
                    },
                )
                .await?;
            self.check(files, auth, profiles, teams, &s).await?;
            Ok(result)
        }
        .await;
        auth.with_lease(&lease,||{
            let mut sessions=self.sessions.lock().unwrap();let current=sessions.get_mut(review).ok_or("분석 창이 닫혔어요. 요청 결과를 자동으로 다시 받지 않았어요.")?;
            match result {
                Ok(value)=>{let value=value["result"].clone();let warning=self.journal(lease.subject(),true,|j|{if let Some(t)=j.attempts.get_mut(&s.group){t.finished=true;}Ok(())}).err();current.phase=Phase::Ready;current.created=crate::time::now_ms();current.result=Some(value.clone());Ok(json!({"epoch":epoch,"folderId":folder,"reviewId":review,"result":value,"warning":warning}))},
                Err(e)=>{current.phase=Phase::Failed;Err(e)},
            }
        }).await
    }
    pub async fn commit(
        &self,
        auth: &GoogleAuth,
        profiles: &Profiles,
        teams: &Teams,
        files: &Files,
        collab: &Collaboration,
        epoch: u64,
        folder: &str,
        review: &str,
        choices: Vec<Choice>,
        confirmed: bool,
    ) -> Result<Value> {
        if !confirmed {
            return err("선택한 할 일을 팀에 공유할지 먼저 확인해 주세요.");
        }
        let lease = auth.full_drive_lease(epoch).await?;
        let _gate = self
            .gate
            .try_lock()
            .map_err(|_| "진행 중인 분석 작업을 먼저 마쳐 주세요.")?;
        let s = self.session(&lease, folder, review)?;
        if ![Phase::Ready, Phase::Done].contains(&s.phase) {
            return err("검토할 AI 결과가 없어요.");
        }
        self.check(files, auth, profiles, teams, &s).await?;
        let candidates = s
            .result
            .as_ref()
            .and_then(|v| v["candidates"].as_array())
            .ok_or("AI 결과를 확인하지 못했어요.")?;
        let mut choices = choices;
        choices.sort_by_key(|c| c.index);
        let unique = choices.iter().map(|c| c.index).collect::<BTreeSet<_>>();
        if choices.is_empty()
            || choices.len() > 12
            || unique.len() != choices.len()
            || choices.iter().any(|c| c.index >= candidates.len())
        {
            return err("검토한 할 일을 서로 다르게 선택해 주세요.");
        }
        let entities = choices
            .into_iter()
            .map(|c| Entity {
                id: String::new(),
                kind: Kind::Task,
                title: c.title.trim().into(),
                project_id: c.project_id,
                assignee: c.assignee.trim().into(),
                due_date: c.due_date,
                status: Status::Todo,
            })
            .collect();
        let saved = collab
            .save_ai(auth, profiles, teams, epoch, folder, &s.group, entities)
            .await?;
        auth.with_lease(&lease, || {
            if let Some(current) = self.sessions.lock().unwrap().get_mut(review) {
                current.phase = Phase::Done;
            }
            Ok(saved)
        })
        .await
    }
}
