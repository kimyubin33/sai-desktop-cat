//! Google 할 일: 내가 수락한 사이 작업을 "사이 · 프로젝트" 목록에 올리고, 완료 상태를 양쪽으로 맞춘다.
//!
//! 동기화 기준
//!   - 대상: 나에게 배정되어 내가 수락한 작업만 (다른 사람 작업, 미수락 작업은 올리지 않음)
//!   - 완료: 어느 쪽에서든 완료하면 다른 쪽도 완료 (사이 쪽은 팀 전체에 보이는 상태)
//!   - 제목: 사이 쪽이 기준. Google 에서 제목을 바꿔도 다음 동기화 때 사이 제목으로 돌아간다
//!   - Google 에서 지운 할 일은 다시 만들지 않는다 (사용자가 지운 뜻을 존중)
//!   - 중복 방지: 사이 작업 id ↔ Google 할 일 id 연결표를 이 기기에 보관
use super::oauth::GoogleAuth;
use crate::util::J;
use crate::Result;
use serde_json::{json, Map, Value};
use std::collections::HashMap;

const TASKS: &str = "https://tasks.googleapis.com/tasks/v1";

pub async fn ensure_list(auth: &GoogleAuth, title: &str) -> Result<String> {
    let lists = auth.api("GET", &format!("{TASKS}/users/@me/lists?maxResults=100"), None).await.map_err(|e| e.1)?;
    if let Some(l) = lists.arr("items").iter().find(|l| l.s("title") == title) {
        return Ok(l.s("id"));
    }
    Ok(auth.api("POST", &format!("{TASKS}/users/@me/lists"), Some(json!({ "title": title }))).await.map_err(|e| e.1)?.s("id"))
}

async fn list_all(auth: &GoogleAuth, list_id: &str) -> Result<Vec<Value>> {
    let mut out = vec![];
    let mut page = String::new();
    loop {
        let url = format!(
            "{TASKS}/lists/{list_id}/tasks?showCompleted=true&showHidden=true&maxResults=100{}",
            if page.is_empty() { String::new() } else { format!("&pageToken={page}") }
        );
        let d = auth.api("GET", &url, None).await.map_err(|e| e.1)?;
        out.extend(d.arr("items"));
        page = d.s("nextPageToken");
        if page.is_empty() {
            break;
        }
    }
    Ok(out)
}

pub struct SyncResult {
    pub links: Map<String, Value>,
    pub mark_done: Vec<String>,
    pub created: usize,
    pub completed_in_google: usize,
}

/// mine: 나에게 배정·수락된 사이 작업 [{ id, title, doneCriteria, status }]
/// links: { [사이 작업 id]: Google 할 일 id | 'deleted' }
pub async fn sync_tasks(auth: &GoogleAuth, list_id: &str, mine: &[Value], links: &Map<String, Value>, deadline: &str, project_title: &str) -> Result<SyncResult> {
    let remote: HashMap<String, Value> = list_all(auth, list_id).await?.into_iter().map(|t| (t.s("id"), t)).collect();
    let mut res = SyncResult { links: links.clone(), mark_done: vec![], created: 0, completed_in_google: 0 };
    // Google 할 일의 due 는 날짜만 의미가 있다
    let due = crate::time::parse_deadline(deadline).map(|_| format!("{deadline}T00:00:00.000Z"));
    for t in mine {
        let id = t.s("id");
        let gid = res.links.get(&id).and_then(|v| v.as_str()).map(String::from);
        if gid.as_deref() == Some("deleted") {
            continue;
        }
        let r = gid.as_ref().and_then(|g| remote.get(g));
        if gid.is_some() && r.map(|x| x.b("deleted")).unwrap_or(true) {
            res.links.insert(id, json!("deleted"));
            continue;
        }
        let Some(r) = r else {
            if t.s("status") == "done" {
                continue;
            }
            let notes = [if t.s("doneCriteria").is_empty() { None } else { Some(format!("완료 기준: {}", t.s("doneCriteria"))) }, Some(format!("사이 · {project_title}"))]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join("\n");
            let mut body = json!({ "title": t.s("title"), "notes": notes });
            if let Some(d) = &due {
                body["due"] = json!(d);
            }
            let created = auth.api("POST", &format!("{TASKS}/lists/{list_id}/tasks"), Some(body)).await.map_err(|e| e.1)?;
            res.links.insert(id, json!(created.s("id")));
            res.created += 1;
            continue;
        };
        let gid = gid.unwrap();
        if r.s("status") == "completed" && t.s("status") != "done" {
            res.mark_done.push(id.clone());
        } else if t.s("status") == "done" && r.s("status") != "completed" {
            auth.api("PATCH", &format!("{TASKS}/lists/{list_id}/tasks/{gid}"), Some(json!({ "status": "completed" }))).await.map_err(|e| e.1)?;
            res.completed_in_google += 1;
        }
        if r.s("title") != t.s("title") {
            auth.api("PATCH", &format!("{TASKS}/lists/{list_id}/tasks/{gid}"), Some(json!({ "title": t.s("title") }))).await.map_err(|e| e.1)?;
        }
    }
    Ok(res)
}
