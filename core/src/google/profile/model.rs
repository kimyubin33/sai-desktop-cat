//! A bounded, immutable change journal. No device clock or whole-profile overwrite.
use crate::{err, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

pub const LIMIT: usize = 4096;
const CLOCK_LIMIT: u64 = 9_000_000_000_000_000;
pub fn id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
fn label(s: &str, max: usize) -> bool {
    s.chars().count() <= max && !s.chars().any(char::is_control)
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(
    tag = "field",
    content = "value",
    rename_all = "camelCase",
    deny_unknown_fields
)]
pub enum Change {
    DisplayName(String),
    CatName(String),
    Frequency(String),
    // A remembered reference is never proof of current Drive permission.
    Team(TeamRef),
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TeamRef {
    pub folder_id: String,
    pub name: String,
    pub removed: bool,
}
impl Change {
    fn key(&self) -> String {
        match self {
            Self::DisplayName(_) => "displayName".into(),
            Self::CatName(_) => "catName".into(),
            Self::Frequency(_) => "frequency".into(),
            Self::Team(t) => format!("team:{}", t.folder_id),
        }
    }
    fn valid(&self) -> bool {
        match self {
            Self::DisplayName(s) => label(s, 60),
            Self::CatName(s) => !s.trim().is_empty() && label(s, 30),
            Self::Frequency(s) => ["low", "normal", "high"].contains(&s.as_str()),
            Self::Team(t) => id(&t.folder_id) && label(&t.name, 100),
        }
    }
    fn removed(&self) -> bool {
        matches!(self, Self::Team(t) if t.removed)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Record {
    pub version: u32,
    pub subject: String,
    pub op_id: String,
    pub actor: String,
    pub clock: u64,
    pub changes: Vec<Change>,
}
impl Record {
    pub fn valid(&self, subject: &str) -> bool {
        let mut keys = BTreeSet::new();
        self.version == 1
            && self.subject == subject
            && id(&self.op_id)
            && id(&self.actor)
            && self.clock > 0
            && self.clock < CLOCK_LIMIT
            && !self.changes.is_empty()
            && self.changes.len() <= 32
            && self
                .changes
                .iter()
                .all(|c| c.valid() && keys.insert(c.key()))
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Pending {
    pub op_id: String,
    pub file_id: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Account {
    pub version: u32,
    pub subject: String,
    pub checked: bool,
    pub records: Vec<Record>,
    pub outbox: Vec<Pending>,
    pub last_sync: i64,
    #[serde(default)]
    pub folder_draft: Option<FolderDraft>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FolderDraft {
    pub name: String,
    pub file_id: Option<String>,
    pub operation: String,
}
impl Account {
    pub fn empty(subject: &str) -> Self {
        Self {
            version: 1,
            subject: subject.into(),
            checked: false,
            records: vec![],
            outbox: vec![],
            last_sync: 0,
            folder_draft: None,
        }
    }
    pub fn valid(&self, subject: &str) -> bool {
        let ids: BTreeSet<_> = self.records.iter().map(|r| &r.op_id).collect();
        let pending: BTreeSet<_> = self.outbox.iter().map(|p| &p.op_id).collect();
        self.version == 1
            && self.subject == subject
            && self.records.len() <= LIMIT
            && ids.len() == self.records.len()
            && pending.len() == self.outbox.len()
            && self.records.iter().all(|r| r.valid(subject))
            && self
                .outbox
                .iter()
                .all(|p| ids.contains(&p.op_id) && p.file_id.as_deref().is_none_or(id))
            && self.folder_draft.as_ref().is_none_or(|d| {
                !d.name.trim().is_empty()
                    && label(&d.name, 100)
                    && id(&d.operation)
                    && d.file_id.as_deref().is_none_or(id)
            })
    }
    pub fn merge(&mut self, incoming: Vec<Record>) -> Result<()> {
        let mut all: BTreeMap<_, _> = self
            .records
            .iter()
            .map(|r| (r.op_id.clone(), r.clone()))
            .collect();
        for r in incoming {
            if !r.valid(&self.subject) {
                return err("Google 설정 자료의 형식을 확인하지 못했어요. 기존 자료는 유지했어요.");
            }
            if all.get(&r.op_id).is_some_and(|old| old != &r) {
                return err("Google 설정 변경 내역이 일치하지 않아요. 기존 자료는 유지했어요.");
            }
            all.insert(r.op_id.clone(), r);
            if all.len() > LIMIT {
                return err("Google 설정 변경 내역의 보관 한도에 도달했어요.");
            }
        }
        self.records = all.into_values().collect();
        Ok(())
    }
    pub fn view(&self) -> Value {
        let mut chosen: BTreeMap<String, (&Record, &Change)> = BTreeMap::new();
        for r in &self.records {
            for c in &r.changes {
                let stamp = |r: &Record, c: &Change| {
                    (r.clock, c.removed(), r.actor.clone(), r.op_id.clone())
                };
                let replace = chosen
                    .get(&c.key())
                    .is_none_or(|(old, v)| stamp(r, c) > stamp(old, v));
                if replace {
                    chosen.insert(c.key(), (r, c));
                }
            }
        }
        let mut profile =
            json!({"displayName":"","catName":"사이","frequency":"normal","teams":[]});
        for (_, c) in chosen.values() {
            match c {
                Change::DisplayName(v) => profile["displayName"] = json!(v),
                Change::CatName(v) => profile["catName"] = json!(v),
                Change::Frequency(v) => profile["frequency"] = json!(v),
                Change::Team(t) if !t.removed => profile["teams"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!({"folderId":t.folder_id,"name":t.name,"accessVerified":false})),
                _ => (),
            }
        }
        profile
    }
    pub fn edit(&mut self, actor: &str, patch: Patch) -> Result<bool> {
        if !self.checked {
            return err("먼저 Google 계정의 기존 설정을 확인해 주세요.");
        }
        let view = self.view();
        let mut changes = vec![];
        for (field, value) in [
            ("displayName", patch.display_name),
            ("catName", patch.cat_name),
            ("frequency", patch.frequency),
        ] {
            if let Some(value) = value {
                let value = value.trim().to_owned();
                let c = match field {
                    "displayName" => Change::DisplayName(value.clone()),
                    "catName" => Change::CatName(value.clone()),
                    _ => Change::Frequency(value.clone()),
                };
                if !c.valid() {
                    return err("이름과 설정의 입력 범위를 확인해 주세요.");
                }
                if view[field] != value {
                    changes.push(c);
                }
            }
        }
        self.append(actor, changes)
    }
    pub fn team_edit(&mut self, actor: &str, team: TeamRef) -> Result<bool> {
        if !self.checked || !Change::Team(team.clone()).valid() {
            return err("먼저 계정 설정과 팀 폴더를 확인해 주세요.");
        }
        let view = self.view();
        let existing = view["teams"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["folderId"] == team.folder_id);
        if team.removed && existing.is_none()
            || !team.removed && existing.is_some_and(|t| t["name"] == team.name)
        {
            return Ok(false);
        }
        if !team.removed && existing.is_none() && view["teams"].as_array().unwrap().len() >= 32 {
            return err("현재는 팀 폴더를 32개까지 연결할 수 있어요.");
        }
        self.append(actor, vec![Change::Team(team)])
    }
    fn append(&mut self, actor: &str, changes: Vec<Change>) -> Result<bool> {
        if changes.is_empty() {
            return Ok(false);
        }
        if self.records.len() >= LIMIT {
            return err("설정 변경 내역의 보관 한도에 도달했어요.");
        }
        let clock = self.records.iter().map(|r| r.clock).max().unwrap_or(0) + 1;
        if clock >= CLOCK_LIMIT {
            return err("설정 변경 번호를 확인할 수 없어요.");
        }
        let op_id = super::super::oauth::random_token();
        self.records.push(Record {
            version: 1,
            subject: self.subject.clone(),
            op_id: op_id.clone(),
            actor: actor.into(),
            clock,
            changes,
        });
        self.outbox.push(Pending {
            op_id,
            file_id: None,
        });
        Ok(true)
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Patch {
    pub display_name: Option<String>,
    pub cat_name: Option<String>,
    pub frequency: Option<String>,
}
