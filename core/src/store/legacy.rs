//! 이전 로컬 자료의 읽기 전용 목록과 선택적 내보내기. 네트워크·복호화·원본 갱신을 하지 않는다.
use crate::{err, Result};
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

const MAX_FILE: u64 = 32 * 1024 * 1024;
const MAX_TOTAL: u64 = 128 * 1024 * 1024;
const MAX_SOURCES: usize = 128;
pub const EXPORT_FOLDER: &str = "local-exports";

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub teams: usize,
    pub projects: usize,
    pub tasks: usize,
    pub files: usize,
    pub pc_archive_present: bool,
}

pub struct Archive {
    pub summary: Summary,
    document: Value,
}

struct Reader {
    root: PathBuf,
    total: u64,
}
impl Reader {
    fn json(&mut self, path: &Path) -> Result<Option<Value>> {
        let metadata = match fs::symlink_metadata(path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return err("이전 자료 파일을 읽지 못했어요. 원본은 그대로 유지돼요."),
        };
        let canonical = fs::canonicalize(path)
            .map_err(|_| "이전 자료 경로를 확인하지 못했어요. 원본은 유지돼요.")?;
        if !canonical.starts_with(&self.root) || !canonical.is_file() || metadata.len() > MAX_FILE {
            return err("이전 자료의 경로나 크기를 확인해 주세요. 원본은 유지돼요.");
        }
        let mut bytes = Vec::new();
        File::open(canonical)
            .and_then(|f| f.take(MAX_FILE + 1).read_to_end(&mut bytes))
            .map_err(|_| "이전 자료를 끝까지 읽지 못했어요. 원본은 유지돼요.")?;
        self.total += bytes.len() as u64;
        if bytes.len() as u64 > MAX_FILE || self.total > MAX_TOTAL {
            return err("이전 자료가 한 번에 내보낼 수 있는 크기를 넘었어요. 원본은 유지돼요.");
        }
        let v: Value = serde_json::from_slice(&bytes).map_err(|_| {
            "일부 이전 자료의 형식을 읽지 못했어요. 빈 자료로 바꾸지 않고 원본을 유지해요."
        })?;
        if !v.is_object() {
            return err("이전 자료가 지원하는 형식이 아니에요. 원본은 유지돼요.");
        }
        Ok(Some(v))
    }
}

fn safe_scope(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
}

// 허용한 필드도 객체·배열이면 그대로 복사하지 않는다. 연동·토큰·초대 코드·새 필드는 내보내지 않는다.
fn fields(v: &Value, names: &[&str]) -> Value {
    let mut result = Map::new();
    for key in names {
        if let Some(value) = v.get(*key) {
            if value.is_string() || value.is_number() || value.is_boolean() {
                result.insert((*key).into(), value.clone());
            }
        }
    }
    Value::Object(result)
}
fn rows<'a>(v: &'a Value, key: &str) -> Result<&'a [Value]> {
    match v.get(key) {
        None | Some(Value::Null) => Ok(&[]),
        Some(Value::Array(a)) => Ok(a),
        _ => err("일부 이전 목록의 형식을 읽지 못했어요. 원본은 유지돼요."),
    }
}
fn object<'a>(v: &'a Value, key: &str) -> Result<Option<&'a Map<String, Value>>> {
    match v.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Object(o)) => Ok(Some(o)),
        _ => err("일부 이전 자료의 형식을 읽지 못했어요. 원본은 유지돼요."),
    }
}
fn add_metadata(target: &mut BTreeMap<String, Value>, teams: &[Value]) -> Result<()> {
    for team in teams {
        let id = team["teamId"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or("이전 팀 식별자를 읽지 못했어요. 원본은 유지돼요.")?;
        target.insert(
            id.to_owned(),
            fields(team, &["teamId", "name", "displayName"]),
        );
    }
    Ok(())
}

impl Archive {
    pub fn read(dir: &Path) -> Result<Self> {
        let root = match fs::canonicalize(dir) {
            Ok(root) if root.is_dir() => root,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::empty()),
            _ => return err("자료 보관 폴더를 확인하지 못했어요."),
        };
        let mut reader = Reader {
            root: root.clone(),
            total: 0,
        };
        let settings = reader
            .json(&root.join("settings.json"))?
            .unwrap_or_else(|| json!({}));
        let current = match settings.get("teamServer") {
            None | Some(Value::Null) => "firebase",
            Some(v) => v["serverId"]
                .as_str()
                .ok_or("이전 서버 보관 정보의 형식을 읽지 못했어요. 원본은 유지돼요.")?,
        };
        if !safe_scope(current) {
            return err("이전 서버 보관 정보의 형식을 확인해 주세요. 원본은 유지돼요.");
        }
        let profiles = object(&settings, "teamServerProfiles")?;
        let mut scopes = BTreeSet::from(["firebase".to_owned(), current.to_owned()]);
        if let Some(profiles) = profiles {
            for scope in profiles.keys() {
                if !safe_scope(scope) {
                    return err("이전 보관 정보에 지원하지 않는 경로가 있어요. 원본은 유지돼요.");
                }
                scopes.insert(scope.clone());
            }
        }
        // 전환 전후 서버의 캐시도 각각 보관한다. 주소·포트·인증은 읽어서 연결하지 않는다.
        let servers = root.join("shared/servers");
        match fs::read_dir(&servers) {
            Ok(entries) => {
                if !fs::canonicalize(&servers)
                    .map_err(|_| "보관 경로를 확인하지 못했어요.")?
                    .starts_with(&root)
                {
                    return err("앱 폴더 밖의 이전 자료는 자동으로 읽지 않아요.");
                }
                for entry in entries {
                    let entry = entry.map_err(|_| "이전 보관 목록을 읽지 못했어요.")?;
                    if entry.path().is_dir() {
                        let scope = entry
                            .file_name()
                            .to_str()
                            .filter(|s| safe_scope(s))
                            .ok_or("이전 보관 폴더의 이름을 확인해 주세요.")?
                            .to_owned();
                        scopes.insert(scope);
                        if scopes.len() > MAX_SOURCES {
                            return err("이전 보관 목록이 너무 많아요. 원본은 유지돼요.");
                        }
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(_) => return err("이전 보관 목록을 읽지 못했어요. 원본은 유지돼요."),
        }
        if scopes.len() > MAX_SOURCES {
            return err("이전 보관 목록이 너무 많아요. 원본은 유지돼요.");
        }
        let mut summary = Summary {
            pc_archive_present: root.join("mesh/identity.sqlite3").exists(),
            ..Summary::default()
        };
        let mut sources = Vec::new();
        for scope in scopes {
            let folder = if scope == "firebase" {
                root.join("shared")
            } else {
                servers.join(&scope)
            };
            let cache = reader
                .json(&folder.join("team-cache.json"))?
                .unwrap_or_else(|| json!({}));
            let mut teams = BTreeMap::new();
            if scope == current {
                add_metadata(&mut teams, rows(&settings, "teams")?)?;
            } else if let Some(profile) = profiles.and_then(|p| p.get(&scope)) {
                add_metadata(&mut teams, rows(profile, "teams")?)?;
            }
            if let Some(caches) = object(&cache, "teams")? {
                for (id, cache) in caches {
                    if !cache.is_object() {
                        return err("이전 팀 캐시의 형식을 읽지 못했어요. 원본은 유지돼요.");
                    }
                    let team = teams
                        .entry(id.clone())
                        .or_insert_with(|| json!({"teamId":id}));
                    team["members"] = json!(rows(cache, "members")?
                        .iter()
                        .map(|m| fields(m, &["uid", "displayName"]))
                        .collect::<Vec<_>>());
                    let mut projects: BTreeMap<String, Value> = BTreeMap::new();
                    if let Some(collab) = cache.get("collab").filter(|v| !v.is_null()) {
                        if !collab.is_object() {
                            return err("이전 협업 자료의 형식을 읽지 못했어요. 원본은 유지돼요.");
                        }
                        for project in rows(collab, "projects")? {
                            let id = project["id"]
                                .as_str()
                                .ok_or("이전 프로젝트 식별자를 읽지 못했어요. 원본은 유지돼요.")?;
                            projects.insert(
                                id.to_owned(),
                                fields(
                                    project,
                                    &[
                                        "id",
                                        "title",
                                        "goal",
                                        "deliverable",
                                        "deadline",
                                        "createdAt",
                                    ],
                                ),
                            );
                        }
                        for (collection, allowed) in [
                            (
                                "tasks",
                                &[
                                    "id",
                                    "title",
                                    "status",
                                    "assignee",
                                    "deadline",
                                    "estimateHours",
                                    "difficulty",
                                    "doneCriteria",
                                    "reviewer",
                                    "accepted",
                                ][..],
                            ),
                            (
                                "files",
                                &[
                                    "id", "name", "url", "driveId", "mimeType", "size", "taskId",
                                    "access", "kind", "at",
                                ][..],
                            ),
                        ] {
                            if let Some(groups) = object(collab, collection)? {
                                for (id, group) in groups {
                                    let group = group.as_array().ok_or(
                                        "이전 협업 목록을 읽지 못했어요. 원본은 유지돼요.",
                                    )?;
                                    let project = projects
                                        .entry(id.clone())
                                        .or_insert_with(|| json!({"id":id,"cachedOnly":true}));
                                    project[collection] = json!(group
                                        .iter()
                                        .map(|row| fields(row, allowed))
                                        .collect::<Vec<_>>());
                                    if collection == "tasks" {
                                        summary.tasks += group.len();
                                    } else {
                                        summary.files += group.len();
                                    }
                                }
                            }
                        }
                    }
                    summary.projects += projects.len();
                    team["projects"] = json!(projects.into_values().collect::<Vec<_>>());
                }
            }
            summary.teams += teams.len();
            if !teams.is_empty() {
                sources
                    .push(json!({"source":scope,"teams":teams.into_values().collect::<Vec<_>>()}));
            }
        }
        Ok(Self {
            summary,
            document: Self::document(sources),
        })
    }
    fn document(sources: Vec<Value>) -> Value {
        json!({"format":"sai-legacy-team-export","version":1,"sources":sources,
            "notice":"이 기기에 남아 있는 팀·업무·파일 링크의 선택 사본입니다. 파일 원본·톡·개인 대화·인증 설정은 포함하지 않습니다. 새 팀으로 자동 복원하지 않습니다."})
    }
    fn empty() -> Self {
        Self {
            summary: Summary::default(),
            document: Self::document(vec![]),
        }
    }

    /// 내용은 사용자가 확인할 수 있는 JSON이다. 기존 파일을 덮어쓰지 않고, 생성 중인 파일은 제외한다.
    pub fn export(&self, dir: &Path) -> Result<PathBuf> {
        if self.summary.teams == 0 {
            return err("이 기기에 내보낼 이전 팀 자료가 없어요.");
        }
        let destination = export_folder(dir, true)?;
        let data = serde_json::to_vec_pretty(&self.document)
            .map_err(|_| "내보낼 자료를 만들지 못했어요.")?;
        let name = format!(
            "Sai-previous-teams-{}-{:016x}",
            crate::time::now_ms(),
            rand::random::<u64>()
        );
        let pending = destination.join(format!("{name}.partial"));
        let file = destination.join(format!("{name}.json"));
        let mut writer = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&pending)
            .map_err(|_| "보관 파일을 만들지 못했어요. 원본은 유지돼요.")?;
        // 완성 전 오류가 나면 .partial을 남겨 성공한 내보내기와 구분하고 원본은 건드리지 않는다.
        writer
            .write_all(&data)
            .and_then(|_| writer.sync_all())
            .map_err(|_| "보관 파일을 끝까지 저장하지 못했어요. 원본은 유지돼요.")?;
        drop(writer);
        // hard_link는 완성 이름이 이미 있으면 실패한다. rename의 덮어쓰기 동작을 사용하지 않는다.
        fs::hard_link(&pending, &file)
            .map_err(|_| "보관 파일을 완성하지 못했어요. 원본은 유지돼요.")?;
        let _ = fs::remove_file(pending);
        Ok(file)
    }
}

pub fn export_folder(dir: &Path, create: bool) -> Result<PathBuf> {
    let root = fs::canonicalize(dir).map_err(|_| "자료 보관 폴더를 찾지 못했어요.")?;
    let destination = root.join(EXPORT_FOLDER);
    if create {
        fs::create_dir_all(&destination).map_err(|_| "내보내기 폴더를 만들지 못했어요.")?;
    }
    let canonical = fs::canonicalize(destination).map_err(|_| "먼저 자료를 내보내 주세요.")?;
    if !canonical.starts_with(&root) || canonical == root || !canonical.is_dir() {
        return err("내보내기 폴더의 경로를 확인해 주세요.");
    }
    Ok(canonical)
}

#[cfg(test)]
mod tests;
