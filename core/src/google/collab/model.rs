use super::super::profile::model::id;
use crate::{err, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

pub const LIMIT: usize = 512;
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    Project,
    Task,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    Todo,
    Doing,
    Done,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Entity {
    pub id: String,
    pub kind: Kind,
    pub title: String,
    pub project_id: String,
    pub assignee: String,
    pub due_date: String,
    pub status: Status,
}
fn text(s: &str, max: usize) -> bool {
    s.chars().count() <= max && !s.chars().any(char::is_control)
}
impl Entity {
    pub fn valid(&self) -> bool {
        id(&self.id)
            && !self.title.trim().is_empty()
            && text(&self.title, 160)
            && text(&self.assignee, 120)
            && (self.project_id.is_empty() || id(&self.project_id))
            && (self.due_date.is_empty()
                || self.due_date.len() == 10
                    && chrono::NaiveDate::parse_from_str(&self.due_date, "%Y-%m-%d").is_ok())
            && (self.kind == Kind::Task
                || self.project_id.is_empty()
                    && self.assignee.is_empty()
                    && self.due_date.is_empty())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Record {
    pub version: u32,
    pub folder_id: String,
    pub op_id: String,
    pub parents: Vec<String>,
    pub entity: Entity,
}
impl Record {
    pub fn valid(&self, folder: &str) -> bool {
        self.version == 1
            && self.folder_id == folder
            && id(folder)
            && id(&self.op_id)
            && self.entity.valid()
            && self.parents.len() <= 16
            && self.parents.iter().all(|p| id(p) && p != &self.op_id)
            && self.parents.iter().collect::<BTreeSet<_>>().len() == self.parents.len()
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Edit {
    pub entity: Entity,
    pub heads: Vec<String>,
}

/// Explicit causal heads, independent of clocks. Concurrent snapshots remain visible.
pub fn validate(records: &[Record], folder: &str) -> Result<()> {
    if records.len() > LIMIT || records.iter().any(|r| !r.valid(folder)) {
        return err("협업 기록의 형식이나 보관 한도를 확인해 주세요.");
    }
    let map: BTreeMap<_, _> = records.iter().map(|r| (r.op_id.as_str(), r)).collect();
    if map.len() != records.len() {
        return err("중복된 협업 변경 기록이 있어요.");
    }
    let mut entities = BTreeMap::new();
    for r in records {
        if entities
            .insert(&r.entity.id, &r.entity.kind)
            .is_some_and(|kind| kind != &r.entity.kind)
        {
            return err("협업 항목의 종류가 바뀌었어요.");
        }
        for parent in &r.parents {
            if !map
                .get(parent.as_str())
                .is_some_and(|p| p.entity.id == r.entity.id)
            {
                return err("협업 변경의 앞선 기록이 없어요. 다시 동기화해 주세요.");
            }
        }
    }
    if entities.len() > 128 {
        return err("이 미리보기는 팀당 프로젝트·할 일을 128개까지 보관해요.");
    }
    for r in records {
        if !r.entity.project_id.is_empty()
            && entities.get(&r.entity.project_id) != Some(&&Kind::Project)
        {
            return err("할 일에 연결된 프로젝트를 확인하지 못했어요.");
        }
    }
    let mut visited = BTreeSet::new();
    while visited.len() < map.len() {
        let before = visited.len();
        for r in records {
            if r.parents.iter().all(|p| visited.contains(p)) {
                visited.insert(r.op_id.clone());
            }
        }
        if before == visited.len() {
            return err("협업 변경 기록이 순환해 정리를 중단했어요.");
        }
    }
    Ok(())
}
fn parent_ids(records: &[Record]) -> BTreeSet<&str> {
    records
        .iter()
        .flat_map(|r| r.parents.iter().map(String::as_str))
        .collect()
}

pub fn heads<'a>(records: &'a [Record], entity: &str) -> Vec<&'a Record> {
    let parents = parent_ids(records);
    records
        .iter()
        .filter(|r| r.entity.id == entity && !parents.contains(r.op_id.as_str()))
        .collect()
}

pub fn view(records: &[Record]) -> Value {
    let parents = parent_ids(records);
    let mut entities: BTreeMap<&str, Vec<&Record>> = BTreeMap::new();
    for record in records {
        if !parents.contains(record.op_id.as_str()) {
            entities.entry(&record.entity.id).or_default().push(record);
        }
    }
    Value::Array(
        entities
            .into_iter()
            .map(|(id, mut variants)| {
                variants.sort_by(|a, b| a.op_id.cmp(&b.op_id));
                json!({
                    "id": id,
                    "kind": variants[0].entity.kind,
                    "heads": variants.iter().map(|r| &r.op_id).collect::<Vec<_>>(),
                    "conflict": variants.len() > 1,
                    "variants": variants
                        .iter()
                        .map(|r| json!({"operation": r.op_id, "entity": r.entity}))
                        .collect::<Vec<_>>(),
                })
            })
            .collect(),
    )
}
