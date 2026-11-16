//! 서버에서 새로 확인한 공개 톡을 선택해 로컬 보관한다. 전환·재발송·삭제 전파는 하지 않는다.
pub use super::identity::archive::{ArchivePage, ImportReport};
use super::identity::{archive::Scope, Vault};
use crate::{err, team::Backend, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::{Duration, Instant},
};
pub(crate) const MAX_ROWS: usize = 1000;
pub(crate) const MAX_BYTES: usize = 4 * 1024 * 1024;
const MAX_SELECTION: usize = 200;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct Source {
    pub scope: String,
    pub kind: String,
    pub team_id: String,
    pub team_name: String,
    pub reader_uid: String,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct Row {
    pub id: String,
    pub by: String,
    pub at: i64,
    pub text: String,
}
impl Drop for Row {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.text.zeroize();
    }
}
impl Row {
    pub(crate) fn validate(&self) -> Result<()> {
        let segment = |s: &str| {
            !s.is_empty()
                && s.len() <= 128
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        };
        if !segment(&self.id)
            || !segment(&self.by)
            || self.at <= 0
            || self.at > 9_000_000_000_000_000
            || self.text.trim().is_empty()
            || self.text.chars().count() > 1000
            || self
                .text
                .chars()
                .any(|c| c.is_control() && !['\n', '\r', '\t'].contains(&c))
        {
            return err("기존 톡의 형식 또는 길이를 확인해 주세요.");
        }
        Ok(())
    }
}
pub(crate) struct Snapshot {
    pub source: Source,
    pub rows: Vec<Row>,
    pub excluded: BTreeMap<String, usize>,
    pub scanned: usize,
    pub read_at: i64,
}
impl Snapshot {
    pub fn from_rows(source: Source, rows: Vec<Value>, read_at: i64) -> Result<Self> {
        if rows.len() > MAX_ROWS {
            return err("이전 조회 개수 한도를 넘었어요.");
        }
        let scanned = rows.len();
        let mut seen = BTreeSet::new();
        let mut included = Vec::new();
        let mut excluded = BTreeMap::new();
        let mut bytes = 0usize;
        for value in rows {
            bytes += value.to_string().len();
            if bytes > MAX_BYTES {
                return err("이전 조회 크기 한도를 넘었어요.");
            }
            let reason = if !value.is_object() {
                Some("invalid")
            } else if value
                .get("url")
                .and_then(Value::as_str)
                .is_some_and(|s| !s.is_empty())
            {
                Some("file")
            } else if value
                .as_object()
                .unwrap()
                .keys()
                .any(|k| !["id", "by", "at", "text"].contains(&k.as_str()))
            {
                Some("unknown-fields")
            } else if value["id"]
                .as_str()
                .is_some_and(|id| id.starts_with("aip-"))
                || value["text"]
                    .as_str()
                    .is_some_and(|text| text.starts_with("[팀 약속 제안] "))
            {
                Some("linked-proposal")
            } else {
                None
            };
            if let Some(id) = value["id"].as_str() {
                if !seen.insert(id.to_owned()) {
                    return err("기존 목록에 같은 문서 ID가 반복돼요. 다시 읽어 주세요.");
                }
            }
            if let Some(reason) = reason {
                *excluded.entry(reason.into()).or_insert(0) += 1;
                continue;
            }
            let row = serde_json::from_value::<Row>(value)
                .ok()
                .filter(|r| r.validate().is_ok());
            if let Some(row) = row {
                included.push(row);
            } else {
                *excluded.entry("invalid".into()).or_insert(0) += 1;
            }
        }
        included.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(Self {
            source,
            rows: included,
            excluded,
            scanned,
            read_at,
        })
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub id: String,
    pub claimed_by: String,
    pub claimed_at: i64,
    pub text: String,
    pub state: &'static str,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    pub token: String,
    pub target_team: String,
    pub source_team: String,
    pub source_name: String,
    pub source_kind: String,
    pub read_at: i64,
    pub expires_at: i64,
    pub scanned: usize,
    pub excluded: BTreeMap<String, usize>,
    pub candidates: Vec<Candidate>,
    pub max_selection: usize,
}
struct Plan {
    token: String,
    scope: Scope,
    snapshot: Snapshot,
    backend: Arc<Backend>,
    expires: Instant,
    completed: Option<(Vec<String>, ImportReport)>,
}
pub struct Importer {
    vault: Vault,
    plan: tokio::sync::Mutex<Option<Plan>>,
}
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|_| "이전 기록을 처리하지 못했어요.")?
}
async fn fresh(backend: &Backend, team: &str) -> Result<Snapshot> {
    tokio::time::timeout(Duration::from_secs(45), backend.migration_snapshot(team))
        .await
        .map_err(|_| "기존 서버 조회 시간이 지났어요. 캐시로 대신 이전하지 않았어요.")?
}
impl Importer {
    pub fn new(vault: Vault) -> Self {
        Self {
            vault,
            plan: Default::default(),
        }
    }
    pub async fn preview(
        &self,
        backend: Arc<Backend>,
        source_team: &str,
        target_team: &str,
    ) -> Result<Preview> {
        let mut slot = self.plan.lock().await;
        *slot = None;
        let (v, target) = (self.vault.clone(), target_team.to_owned());
        let scope = blocking(move || v.archive_scope(&target)).await?;
        let snapshot = fresh(&backend, source_team).await?;
        let (v, s, source, rows) = (
            self.vault.clone(),
            scope.clone(),
            snapshot.source.clone(),
            snapshot.rows.clone(),
        );
        let states = blocking(move || v.archive_evaluate(&s, &source, &rows)).await?;
        let token = Vault::local_draft_id()?;
        let expires_at = crate::time::now_ms() + 5 * 60 * 1000;
        let result = Preview {
            token: token.clone(),
            target_team: target_team.into(),
            source_team: source_team.into(),
            source_name: snapshot.source.team_name.clone(),
            source_kind: snapshot.source.kind.clone(),
            read_at: snapshot.read_at,
            expires_at,
            scanned: snapshot.scanned,
            excluded: snapshot.excluded.clone(),
            candidates: snapshot
                .rows
                .iter()
                .zip(states)
                .map(|(r, state)| Candidate {
                    id: r.id.clone(),
                    claimed_by: r.by.clone(),
                    claimed_at: r.at,
                    text: r.text.clone(),
                    state,
                })
                .collect(),
            max_selection: MAX_SELECTION,
        };
        *slot = Some(Plan {
            token,
            scope,
            snapshot,
            backend,
            expires: Instant::now() + Duration::from_secs(5 * 60),
            completed: None,
        });
        Ok(result)
    }
    pub async fn commit(
        &self,
        backend: Arc<Backend>,
        source_team: &str,
        target_team: &str,
        token: &str,
        mut ids: Vec<String>,
        consent: bool,
    ) -> Result<ImportReport> {
        if !consent {
            return err("선택한 톡을 이 PC에만 보관하는 데 동의해 주세요.");
        }
        if ids.is_empty() || ids.len() > MAX_SELECTION || ids.iter().any(|id| id.len() > 128) {
            return err("이전할 톡을 1~200개 선택해 주세요.");
        }
        ids.sort();
        if ids.windows(2).any(|w| w[0] == w[1]) {
            return err("이전 선택에 중복 ID가 있어요.");
        }
        let mut slot = self.plan.lock().await;
        let plan = slot
            .as_mut()
            .ok_or("먼저 기존 팀 톡을 미리 확인해 주세요.")?;
        if plan.expires <= Instant::now()
            || plan.token != token
            || plan.scope.team != target_team
            || plan.snapshot.source.team_id != source_team
            || !Arc::ptr_eq(&plan.backend, &backend)
        {
            return err("미리보기의 시간·팀·서버가 달라요. 다시 조회해 주세요.");
        }
        if let Some((selection, report)) = &plan.completed {
            if selection == &ids {
                return Ok(report.clone());
            }
            return err("이미 처리한 미리보기예요. 다른 선택은 새로 조회해 주세요.");
        }
        let selected: Vec<_> = ids
            .iter()
            .map(|id| {
                plan.snapshot
                    .rows
                    .iter()
                    .find(|r| &r.id == id)
                    .cloned()
                    .ok_or_else(|| "미리보기에 없는 톡을 선택했어요.".to_string())
            })
            .collect::<Result<_>>()?;
        let current = fresh(&backend, source_team).await?;
        if current.source != plan.snapshot.source
            || selected
                .iter()
                .any(|row| !current.rows.iter().any(|r| r == row))
        {
            return err("선택한 톡 또는 기존 팀 정보가 변경·삭제됐어요. 다시 조회해 주세요.");
        }
        if plan.expires <= Instant::now() {
            return err("미리보기 시간이 끝났어요. 다시 조회해 주세요.");
        }
        let (v, scope, source, at) = (
            self.vault.clone(),
            plan.scope.clone(),
            plan.snapshot.source.clone(),
            current.read_at,
        );
        let report = blocking(move || v.archive_import(&scope, &source, &selected, at)).await?;
        plan.completed = Some((ids, report.clone()));
        Ok(report)
    }
    pub async fn page(&self, team: &str, offset: usize) -> Result<ArchivePage> {
        let (v, team) = (self.vault.clone(), team.to_owned());
        blocking(move || v.archive_page(&team, offset)).await
    }
}

#[cfg(test)]
mod tests;
