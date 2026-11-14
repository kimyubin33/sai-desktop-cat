//! 협업: 자기소개·프로젝트·작업을 팀 서버와 주고받고, 배분 제안·진행 확인을 계산한다.
mod brief;
pub mod docs;
pub mod planner;
pub mod ai_plan;
pub mod progress;

use crate::team::Team;
use crate::util::{arr_mut, clean, J};
use crate::{err, Result};
use serde_json::{json, Map, Value};
use std::collections::HashMap;

pub const STATUSES: &[&str] = &["todo", "doing", "blocked", "done"];
pub const OBJECTION_REASONS: &[&str] = &["가능 시간이 부족해요", "역량이 맞지 않아요", "다른 일과 겹쳐요", "기타"];

fn num(v: &Value, min: f64, max: f64, d: f64) -> f64 {
    match v.as_f64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok())) {
        Some(n) if n.is_finite() => n.clamp(min, max),
        _ => d,
    }
}

fn list(v: &Value, max_items: usize, max_len: usize) -> Vec<String> {
    let raw: Vec<String> = match v {
        Value::Array(a) => a.iter().map(|x| x.as_str().map(String::from).unwrap_or_else(|| x.to_string())).collect(),
        Value::String(s) => s.split(',').map(String::from).collect(),
        _ => vec![],
    };
    raw.iter().map(|x| clean(x, max_len)).filter(|x| !x.is_empty()).take(max_items).collect()
}

pub fn clean_profile(p: &Value) -> Value {
    let mut skills = Map::new();
    let entries: Vec<(String, Value)> = match p.get("skills") {
        Some(Value::Array(a)) => a.iter().map(|s| (s.s("name"), s.get("level").cloned().unwrap_or(Value::Null))).collect(),
        Some(Value::Object(o)) => o.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        _ => vec![],
    };
    for (name, level) in entries {
        let name = clean(&name, 30);
        if !name.is_empty() {
            skills.insert(name, json!(num(&level, 0.0, 3.0, 2.0)));
        }
    }
    json!({
        "pr": clean(&p.s("pr"), 500),
        "skills": skills,
        "learn": list(p.get("learn").unwrap_or(&Value::Null), 10, 30),
        "weeklyHours": num(p.get("weeklyHours").unwrap_or(&Value::Null), 0.0, 80.0, 0.0),
        "days": list(p.get("days").unwrap_or(&Value::Null), 7, 3),
    })
}

pub fn clean_task(t: &Value) -> Value {
    let title = clean(&t.s("title"), 80);
    let prep: Vec<Value> = match t.get("prep") {
        Some(Value::Array(a)) => a.iter().map(|x| json!({ "text": clean(&x.s("text"), 60), "done": x.b("done") })).collect(),
        Some(v) => list(v, 10, 60).into_iter().map(|text| json!({ "text": text, "done": false })).collect(),
        None => vec![],
    };
    json!({
        "title": if title.is_empty() { "이름 없는 작업".to_string() } else { title },
        "estimateHours": num(t.get("estimateHours").unwrap_or(&Value::Null), 0.0, 200.0, 1.0),
        "difficulty": num(t.get("difficulty").unwrap_or(&Value::Null), 1.0, 3.0, 2.0),
        "skills": list(t.get("skills").unwrap_or(&Value::Null), 10, 30),
        "dependsOn": t.strs("dependsOn").iter().map(|x| clean(x, 40)).filter(|x| !x.is_empty()).take(10).collect::<Vec<_>>(),
        "doneCriteria": clean(&t.s("doneCriteria"), 200),
        "prep": prep.into_iter().filter(|x| !x.s("text").is_empty()).take(10).collect::<Vec<_>>(),
        "reviewer": t.get("reviewer").cloned().unwrap_or(Value::Null),
    })
}

impl Team {
    // ---------- 저장 ----------
    pub fn collab_data(&self) -> Value {
        let c = self.cache_get();
        match c.get("collab") {
            Some(v) if v.is_object() => v.clone(),
            _ => json!({ "profiles": [], "projects": [], "tasks": {}, "activeProjectId": null, "nudges": {} }),
        }
    }

    pub fn collab_update<R>(&self, f: impl FnOnce(&mut Value) -> R) -> R {
        self.cache_update(|c| {
            if !c["collab"].is_object() {
                c["collab"] = json!({ "profiles": [], "projects": [], "tasks": {}, "activeProjectId": null, "nudges": {} });
            }
            f(&mut c["collab"])
        })
    }

    pub async fn collab_sync(&self) -> Result<()> {
        if !self.is_connected() {
            return Ok(());
        }
        let team_id = self.team_id()?;
        let b = self.backend()?;
        let profiles = b.list_profiles(&team_id).await?;
        let projects = b.list_projects(&team_id).await?;
        let mut tasks = Map::new();
        let mut files = Map::new();
        for p in projects.iter().take(10) {
            tasks.insert(p.s("id"), json!(b.list_tasks(&team_id, &p.s("id")).await?));
            // 팀 자료: 서버 규칙을 아직 게시하지 않았으면 권한 오류가 나므로 조용히 비워 둔다
            let f = b.list_files(&team_id, &p.s("id")).await.unwrap_or_default();
            files.insert(p.s("id"), json!(f));
        }
        self.collab_update(|d| {
            d["profiles"] = json!(profiles);
            d["projects"] = json!(projects);
            d["tasks"] = Value::Object(tasks);
            d["files"] = Value::Object(files);
            let active = d.s("activeProjectId");
            if !projects.iter().any(|p| p.s("id") == active) {
                d["activeProjectId"] = projects.first().map(|p| p["id"].clone()).unwrap_or(Value::Null);
            }
        });
        Ok(())
    }

    // ---------- 자기소개 ----------
    pub fn my_profile(&self) -> Option<Value> {
        let me = self.my_uid();
        self.collab_data().arr("profiles").into_iter().find(|p| p.s("uid") == me)
    }

    pub async fn save_profile(&self, input: &Value) -> Result<Value> {
        let team_id = self.team_id()?;
        let p = clean_profile(input);
        self.backend()?.set_profile(&team_id, p.clone()).await?;
        let me = self.my_uid();
        self.collab_update(|d| {
            let ps = arr_mut(d, "profiles");
            ps.retain(|x| x.s("uid") != me);
            let mut np = p.clone();
            np["uid"] = json!(me);
            ps.push(np);
        });
        Ok(p)
    }

    // ---------- 프로젝트 ----------
    pub async fn create_project(&self, input: &Value) -> Result<String> {
        let team_id = self.team_id()?;
        let deadline = input.s("deadline");
        let title = clean(&input.s("title"), 80);
        let p = json!({
            "title": if title.is_empty() { "새 프로젝트".into() } else { title },
            "goal": clean(&input.s("goal"), 300),
            "deliverable": clean(&input.s("deliverable"), 200),
            "deadline": if crate::time::parse_deadline(&deadline).is_some() { deadline } else { String::new() },
        });
        let id = self.backend()?.create_project(&team_id, p.clone()).await?;
        self.collab_update(|d| {
            let mut np = p.clone();
            np["id"] = json!(id);
            arr_mut(d, "projects").push(np);
            d["tasks"][&id] = json!([]);
            d["activeProjectId"] = json!(id);
        });
        Ok(id)
    }

    pub async fn update_project(&self, id: &str, input: &Value) -> Result<()> {
        let team_id = self.team_id()?;
        let mut patch = Map::new();
        for k in ["title", "goal", "deliverable"] {
            if input.get(k).is_some() {
                patch.insert(k.into(), json!(clean(&input.s(k), if k == "title" { 80 } else { 300 })));
            }
        }
        if input.get("deadline").is_some() {
            let dl = input.s("deadline");
            patch.insert("deadline".into(), json!(if crate::time::parse_deadline(&dl).is_some() { dl } else { String::new() }));
        }
        self.backend()?.update_project(&team_id, id, Value::Object(patch.clone())).await?;
        self.collab_update(|d| {
            for p in arr_mut(d, "projects") {
                if p.s("id") == id {
                    for (k, v) in &patch {
                        p[k] = v.clone();
                    }
                }
            }
        });
        Ok(())
    }

    pub fn set_active_project(&self, id: &str) {
        self.collab_update(|d| d["activeProjectId"] = json!(id));
    }

    pub fn active_project(&self) -> Option<Value> {
        let d = self.collab_data();
        let a = d.s("activeProjectId");
        d.arr("projects").into_iter().find(|p| p.s("id") == a)
    }

    pub fn project(&self, id: &str) -> Option<Value> {
        self.collab_data().arr("projects").into_iter().find(|p| p.s("id") == id)
    }

    pub fn tasks_of(&self, project_id: &str) -> Vec<Value> {
        self.collab_data()["tasks"].get(project_id).and_then(|v| v.as_array().cloned()).unwrap_or_default()
    }

    // ---------- 팀 자료 ----------
    pub fn files_of(&self, project_id: &str) -> Vec<Value> {
        let mut v = self.collab_data()["files"].get(project_id).and_then(|v| v.as_array().cloned()).unwrap_or_default();
        v.sort_by_key(|f| -f.i("at"));
        v
    }

    /// Drive 파일 하나를 프로젝트 자료로 올린다 (task_id 가 있으면 그 작업의 자료)
    pub async fn add_file(&self, project_id: &str, file: &Value) -> Result<()> {
        let team_id = self.team_id()?;
        let url = file.s("url");
        if !url.starts_with("https://") {
            return crate::err("https:// 로 시작하는 링크만 올릴 수 있어요.");
        }
        let data = json!({
            "name": non_empty_name(&clean(&file.s("name"), 200)),
            "url": clean(&url, 500),
            "driveId": clean(&file.s("driveId"), 100),
            "mimeType": clean(&file.s("mimeType"), 100),
            "size": file.i("size").max(0),
            "taskId": clean(&file.s("taskId"), 60),
            // edit: 링크가 있는 팀원이 고칠 수 있음, view: 보기만, link: 붙여 넣은 링크라 Drive 의 권한 그대로
            "access": match file.s("access").as_str() { "edit" => "edit", "view" => "view", _ => "link" },
            // file: 올린 파일, link: 붙여 넣은 링크, note: 작업 노트, report: 현황 기록
            "kind": match file.s("kind").as_str() { "note" => "note", "report" => "report", "link" => "link", _ => "file" },
            "by": self.my_uid(),
            "at": self.now(),
        });
        let b = self.backend()?;
        let id = b.add_file(&team_id, project_id, data.clone()).await.map_err(|e| {
            if e.contains("403") || e.contains("PERMISSION") || e.contains("권한") {
                "팀 서버 규칙에 아직 자료 공유가 없어요. Firebase 콘솔에서 firestore.rules 를 게시해 주세요.".to_string()
            } else {
                e
            }
        })?;
        self.collab_update(|d| {
            if !d["files"].is_object() {
                d["files"] = json!({});
            }
            let mut f = data;
            f["id"] = json!(id);
            let list = d["files"].as_object_mut().unwrap().entry(project_id.to_string()).or_insert_with(|| json!([]));
            if let Some(a) = list.as_array_mut() {
                a.push(f);
            }
        });
        Ok(())
    }

    pub fn file(&self, project_id: &str, file_id: &str) -> Option<Value> {
        self.files_of(project_id).into_iter().find(|f| f.s("id") == file_id)
    }

    pub async fn set_file_access(&self, project_id: &str, file_id: &str, edit: bool) -> Result<()> {
        let team_id = self.team_id()?;
        let access = if edit { "edit" } else { "view" };
        self.backend()?.update_file(&team_id, project_id, file_id, json!({ "access": access })).await?;
        self.collab_update(|d| {
            if let Some(a) = d["files"].get_mut(project_id).and_then(|v| v.as_array_mut()) {
                for f in a.iter_mut().filter(|f| f.s("id") == file_id) {
                    f["access"] = json!(access);
                }
            }
        });
        Ok(())
    }

    pub async fn delete_file(&self, project_id: &str, file_id: &str) -> Result<()> {
        let team_id = self.team_id()?;
        self.backend()?.delete_file(&team_id, project_id, file_id).await?;
        self.collab_update(|d| {
            if let Some(a) = d["files"].get_mut(project_id).and_then(|v| v.as_array_mut()) {
                a.retain(|f| f.s("id") != file_id);
            }
        });
        Ok(())
    }

    // ---------- 작업 ----------
    pub async fn add_task(&self, project_id: &str, input: &Value) -> Result<String> {
        let team_id = self.team_id()?;
        let mut t = clean_task(input);
        t["status"] = json!("todo");
        t["assignee"] = Value::Null;
        t["accepted"] = json!(false);
        t["objections"] = json!({});
        let id = self.backend()?.create_task(&team_id, project_id, t.clone()).await?;
        self.collab_update(|d| {
            t["id"] = json!(id);
            if !d["tasks"].is_object() {
                d["tasks"] = json!({});
            }
            let list = d["tasks"].as_object_mut().unwrap().entry(project_id.to_string()).or_insert_with(|| json!([]));
            if let Some(a) = list.as_array_mut() {
                a.push(t);
            }
        });
        Ok(id)
    }

    pub async fn edit_task(&self, project_id: &str, task_id: &str, input: &Value) -> Result<()> {
        self.patch_task(project_id, task_id, clean_task(input)).await
    }

    pub async fn patch_task(&self, project_id: &str, task_id: &str, patch: Value) -> Result<()> {
        let team_id = self.team_id()?;
        self.backend()?.update_task(&team_id, project_id, task_id, patch.clone()).await?;
        self.collab_update(|d| {
            if let Some(a) = d["tasks"].get_mut(project_id).and_then(|v| v.as_array_mut()) {
                if let Some(t) = a.iter_mut().find(|t| t.s("id") == task_id) {
                    if let Some(o) = patch.as_object() {
                        for (k, v) in o {
                            t[k] = v.clone();
                        }
                    }
                }
            }
        });
        Ok(())
    }

    pub async fn delete_task(&self, project_id: &str, task_id: &str) -> Result<()> {
        let team_id = self.team_id()?;
        self.backend()?.delete_task(&team_id, project_id, task_id).await?;
        self.collab_update(|d| {
            if let Some(a) = d["tasks"].get_mut(project_id).and_then(|v| v.as_array_mut()) {
                a.retain(|t| t.s("id") != task_id);
                for t in a.iter_mut() {
                    let deps: Vec<String> = t.strs("dependsOn").into_iter().filter(|x| x != task_id).collect();
                    t["dependsOn"] = json!(deps);
                }
            }
        });
        Ok(())
    }

    /// (projectId, task)
    pub fn find_task(&self, task_id: &str) -> Option<(String, Value)> {
        let d = self.collab_data();
        for (pid, ts) in d.obj("tasks") {
            if let Some(t) = ts.as_array().and_then(|a| a.iter().find(|x| x.s("id") == task_id)) {
                return Some((pid, t.clone()));
            }
        }
        None
    }

    pub async fn set_status(&self, task_id: &str, status: &str) -> Result<()> {
        if !STATUSES.contains(&status) {
            return err("알 수 없는 상태예요.");
        }
        let Some((pid, task)) = self.find_task(task_id) else { return Ok(()) };
        let mut patch = json!({ "status": status });
        if status == "doing" && task.i("startedAt") == 0 {
            patch["startedAt"] = json!(self.now());
        }
        if status == "done" {
            patch["doneAt"] = json!(self.now());
        }
        self.patch_task(&pid, task_id, patch).await?;
        if status == "blocked" && task.s("status") != "blocked" {
            let project = self.project(&pid).map(|p| p.s("title")).unwrap_or_default();
            let me = self.my_uid();
            let who = self.members(true).iter().find(|m| m.s("uid") == me).map(|m| m.s("displayName")).unwrap_or_else(|| "팀원".into());
            let _ = self.tg_blocked(&task.s("title"), &project, &who).await;
        }
        Ok(())
    }

    pub async fn accept(&self, task_id: &str) -> Result<()> {
        let me = self.my_uid();
        match self.find_task(task_id) {
            Some((pid, t)) if t.s("assignee") == me => self.patch_task(&pid, task_id, json!({ "accepted": true })).await,
            _ => err("나에게 배정된 작업만 수락할 수 있어요."),
        }
    }

    /// 이의 제기: 배정을 풀고, 다음 배분 제안에서 이 사람은 이 작업에서 빠진다
    pub async fn object(&self, task_id: &str, reason: &str) -> Result<()> {
        let Some((pid, t)) = self.find_task(task_id) else { return Ok(()) };
        let r = if OBJECTION_REASONS.contains(&reason) { reason } else { "기타" };
        let me = self.my_uid();
        let mut objections = t.obj("objections");
        objections.insert(me.clone(), json!(r));
        let mut patch = json!({ "objections": objections });
        if t.s("assignee") == me {
            patch["assignee"] = Value::Null;
            patch["accepted"] = json!(false);
        }
        self.patch_task(&pid, task_id, patch).await
    }

    pub async fn toggle_prep(&self, task_id: &str, index: usize) -> Result<()> {
        let Some((pid, t)) = self.find_task(task_id) else { return Ok(()) };
        let prep: Vec<Value> = t
            .arr("prep")
            .into_iter()
            .enumerate()
            .map(|(i, mut p)| {
                if i == index {
                    p["done"] = json!(!p.b("done"));
                }
                p
            })
            .collect();
        self.patch_task(&pid, task_id, json!({ "prep": prep })).await
    }

    // ---------- 배분 ----------
    /// [{ uid, name, weeklyHours, skills, learn, pr, hasProfile }]
    pub fn collab_members(&self) -> Vec<Value> {
        let profiles: HashMap<String, Value> = self.collab_data().arr("profiles").into_iter().map(|p| (p.s("uid"), p)).collect();
        self.members(true)
            .iter()
            .map(|m| {
                let p = profiles.get(&m.s("uid"));
                let e = json!({});
                let pp = p.unwrap_or(&e);
                json!({
                    "uid": m.s("uid"), "name": m.s("displayName"), "weeklyHours": pp.n("weeklyHours"),
                    "skills": pp.get("skills").cloned().unwrap_or(json!({})), "learn": pp.arr("learn"), "pr": pp.s("pr"),
                    "hasProfile": p.is_some(),
                })
            })
            .collect()
    }

    pub fn plan(&self, project_id: &str) -> Option<Value> {
        let project = self.project(project_id)?;
        Some(planner::plan(&self.collab_members(), &self.tasks_of(project_id), &project.s("deadline"), self.now()))
    }

    /// 제안을 실제 배정으로 옮긴다. 수락된 배정은 건드리지 않는다. 돌려주는 값: 새로 배정한 수
    pub async fn apply_plan(&self, project_id: &str, assignments: &[Value]) -> Result<usize> {
        let mut applied = vec![];
        for a in assignments {
            let uid = a.s("uid");
            if a.b("fixed") || uid.is_empty() {
                continue;
            }
            let Some(t) = self.tasks_of(project_id).into_iter().find(|x| x.s("id") == a.s("taskId")) else { continue };
            if t.b("accepted") || t.s("status") == "done" || t.s("assignee") == uid {
                continue;
            }
            self.patch_task(project_id, &t.s("id"), json!({ "assignee": uid, "accepted": false })).await?;
            let hours = if a.n("effectiveHours") > 0.0 { a.n("effectiveHours") } else { t.n("estimateHours") };
            applied.push((uid, t.s("title"), hours));
        }
        if !applied.is_empty() {
            let project = self.project(project_id).map(|p| p.s("title")).unwrap_or_default();
            let _ = self.tg_assigned(&applied, &project).await;
        }
        Ok(applied.len())
    }

    // ---------- 진행 확인 ----------
    pub fn nudge(&self) -> Option<Value> {
        if !self.is_connected() {
            return None;
        }
        let project = self.active_project()?;
        let tasks = self.tasks_of(&project.s("id"));
        if tasks.is_empty() {
            return None;
        }
        let p = self.plan(&project.s("id"));
        let effective: HashMap<String, f64> = p.as_ref().map(|p| p.arr("assignments").iter().map(|a| (a.s("taskId"), a.n("effectiveHours"))).collect()).unwrap_or_default();
        let warnings = p.as_ref().map(|p| p.arr("warnings")).unwrap_or_default();
        let profile = self.my_profile();
        let recent = self.collab_data().get("nudges").cloned().unwrap_or(json!({}));
        progress::next_nudge(&self.my_uid(), &project, &tasks, profile.as_ref(), &warnings, &effective, self.now(), &recent)
    }

    pub fn mark_nudged(&self, key: &str) {
        let now = self.now();
        self.collab_update(|d| {
            if !d["nudges"].is_object() {
                d["nudges"] = json!({});
            }
            d["nudges"][key] = json!(now);
        });
    }
}

#[cfg(test)]
mod tests {
    use crate::store::Stores;
    use crate::team::{Backend, Hub};
    use serde_json::json;
    use std::sync::Arc;

    fn hub(shared: Arc<std::sync::Mutex<serde_json::Value>>, uid: &str) -> Hub {
        let st = Stores::memory();
        let u = uid.to_string();
        Hub::new(st.settings, st.shared_outbox, st.team_cache, Box::new(move || Some(Backend::memory_shared(shared.clone(), &u))), Some(0), Arc::new(crate::time::now_ms))
    }

    #[tokio::test]
    async fn plan_apply_accept_between_two_members() {
        let shared = Backend::new_shared_memory();
        let a = hub(shared.clone(), "ua");
        let b = hub(shared.clone(), "ub");
        let t = a.create_team("홍보팀", "유빈").await.unwrap();
        b.join_team(&t["joinCode"].as_str().unwrap(), "민수").await.unwrap();
        let ta = a.active();
        a.sync_team(&ta).await;
        ta.save_profile(&json!({ "skills": [{ "name": "영상편집", "level": 3 }], "weeklyHours": 10 })).await.unwrap();
        let tb = b.active();
        b.sync_team(&tb).await;
        tb.save_profile(&json!({ "skills": [{ "name": "디자인", "level": 3 }], "weeklyHours": 6 })).await.unwrap();
        a.sync_team(&ta).await;
        let pid = ta.create_project(&json!({ "title": "홍보 영상", "deadline": "2099-12-01" })).await.unwrap();
        ta.add_task(&pid, &json!({ "title": "컷 편집", "estimateHours": 5, "skills": "영상편집" })).await.unwrap();
        let poster = ta.add_task(&pid, &json!({ "title": "포스터", "estimateHours": 4, "skills": "디자인" })).await.unwrap();
        let plan = ta.plan(&pid).unwrap();
        assert_eq!(ta.apply_plan(&pid, &plan["assignments"].as_array().unwrap()).await.unwrap(), 2);
        b.sync_team(&tb).await;
        let (_, task) = tb.find_task(&poster).unwrap();
        assert_eq!(task["assignee"], "ub");
        tb.accept(&poster).await.unwrap();
        assert!(tb.accept(&tb.tasks_of(&pid).iter().find(|t| t["title"] == "컷 편집").unwrap()["id"].as_str().unwrap().to_string()).await.is_err());
        let n = tb.nudge().unwrap();
        assert!(n["key"].as_str().unwrap().starts_with("start:") || n["key"].as_str().unwrap().starts_with("prep:"));
        tb.set_status(&poster, "done").await.unwrap();
        a.sync_team(&ta).await;
        assert_eq!(ta.find_task(&poster).unwrap().1["status"], "done");
    }

    #[tokio::test]
    async fn team_files_notes_and_report() {
        let shared = Backend::new_shared_memory();
        let a = hub(shared.clone(), "ua");
        let b = hub(shared.clone(), "ub");
        let t = a.create_team("홍보팀", "유빈").await.unwrap();
        b.join_team(&t["joinCode"].as_str().unwrap(), "민수").await.unwrap();
        let ta = a.active();
        let pid = ta.create_project(&json!({ "title": "홍보 영상", "goal": "축제 홍보", "deadline": "2099-12-01" })).await.unwrap();
        let task = ta.add_task(&pid, &json!({ "title": "컷 <편집>", "estimateHours": 5, "doneCriteria": "3분 이내" })).await.unwrap();
        ta.add_file(&pid, &json!({ "name": "기획안.pdf", "url": "https://drive.google.com/file/d/abcdefghijkl/view", "access": "edit", "kind": "file", "taskId": task })).await.unwrap();
        assert!(ta.add_file(&pid, &json!({ "name": "x", "url": "http://bad" })).await.is_err());
        // 다른 팀원에게도 보인다
        let tb = b.active();
        b.sync_team(&tb).await;
        let files = tb.files_of(&pid);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0]["access"], "edit");
        assert_eq!(files[0]["by"], "ua");
        // 올린 사람이 보기만으로 바꾼다
        ta.set_file_access(&pid, files[0]["id"].as_str().unwrap(), false).await.unwrap();
        assert_eq!(ta.files_of(&pid)[0]["access"], "view");
        // 작업 노트와 현황 기록 내용
        let (title, html) = ta.task_note_html(&pid, &task).unwrap();
        assert!(title.contains("컷 <편집>") && title.contains("작업 노트"));
        assert!(html.contains("컷 &lt;편집&gt;") && html.contains("3분 이내"));
        a.sync_team(&ta).await;
        let (rt, rh) = ta.project_report_html(&pid).unwrap();
        assert!(rt.starts_with("[홍보 영상] 현황"));
        assert!(rh.contains("기획안.pdf") && rh.contains("축제 홍보") && rh.contains("민수"));
    }
}

fn non_empty_name(s: &str) -> String {
    if s.trim().is_empty() {
        "이름 없는 파일".into()
    } else {
        s.to_string()
    }
}
