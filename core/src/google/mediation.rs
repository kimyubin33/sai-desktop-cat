//! 팀원 사이 중재 전달의 Drive 쪽. 서버 없이 Drive 개별 공유로 주고받는다.
//!
//! - 보내는 사람: 자기 Drive의 "사이 메시지" 폴더에 메시지 파일을 만들고, 받는 사람 한 명에게만 보기 권한을 준다.
//!   알림 메일은 보내지 않는다. 받는 사람은 파일 소유자(보낸 사람)를 볼 수 있다. 익명이 아니다.
//! - 받는 사람: 나와 공유된 사이 메시지 파일 중 같은 팀 명단에 있는 사람이 보낸 것만 받는다.
//! - 팀 명단: 팀 폴더 소유자의 앱이 폴더 안에 만들어 갱신하는 파일. 소유자가 아니면 Drive 공유 목록을
//!   볼 수 없어서, 명단 파일의 소유자가 폴더 소유자와 같을 때만 믿는다.
use super::{
    oauth::{random_token, GoogleAuth, Lease},
    profile::{model::id, Profiles},
    teams::{self, Teams},
};
use crate::mediation::{reaction_label, valid_text};
use crate::time::now_ms;
use crate::util::J;
use crate::{err, Result};
use serde_json::{json, Value};
use std::collections::HashMap;

const FOLDER: &str = "application/vnd.google-apps.folder";
const ROSTER: &str = "sai-roster-v1.json";
const OUTBOX: &str = "사이 메시지";
const MESSAGE: &str = "사이가 전한 말.json";
const META: &str = "id,name,mimeType,trashed,ownedByMe,properties,createdTime,owners(displayName,emailAddress)";
const MAX_MEMBERS: usize = 1024;

fn message(e: (u16, String)) -> String {
    match e.0 {
        401 => "Google 연결을 다시 확인해 주세요.".into(),
        403 | 404 => "Drive에서 이 파일이나 폴더에 접근할 수 없어요.".into(),
        _ => e.1,
    }
}

fn q(query: &str) -> String {
    urlencoding::encode(query).into_owned()
}

fn same(a: &str, b: &str) -> bool {
    !a.is_empty() && a.eq_ignore_ascii_case(b)
}

async fn create_json(auth: &GoogleAuth, lease: &Lease, metadata: Value, content: &Value) -> Result<String> {
    let boundary = format!("sai_{}", random_token());
    let body = format!("--{boundary}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n{metadata}\r\n--{boundary}\r\nContent-Type: application/json\r\n\r\n{content}\r\n--{boundary}--\r\n");
    let value = auth
        .profile_request(
            lease,
            "POST",
            "/upload/drive/v3/files?uploadType=multipart&fields=id",
            Some((format!("multipart/related; boundary={boundary}"), body.into_bytes())),
        )
        .await
        .map_err(message)?;
    value["id"].as_str().filter(|s| id(s)).map(str::to_owned).ok_or_else(|| "Drive에 만든 파일을 확인하지 못했어요.".into())
}

async fn read_json(auth: &GoogleAuth, lease: &Lease, fid: &str) -> std::result::Result<Value, (u16, String)> {
    auth.profile_request(lease, "GET", &format!("/drive/v3/files/{fid}?alt=media"), None).await
}

async fn delete(auth: &GoogleAuth, lease: &Lease, fid: &str) {
    let _ = auth.profile_request(lease, "DELETE", &format!("/drive/v3/files/{fid}"), None).await;
}

// ---------- 팀 명단 ----------

fn member(email: &str, name: &str, role: &str) -> Value {
    json!({"email":email.to_ascii_lowercase(),"name":name,"role":role})
}

/// Drive 공유 목록에서 사람 계정만 남긴다. 공개 링크·도메인 권한은 명단에 넣지 않는다.
fn from_permissions(perms: &[Value]) -> Vec<Value> {
    let mut out: Vec<Value> = perms
        .iter()
        .filter(|p| p["type"] == "user" && p["deleted"] != true)
        .filter_map(|p| {
            let email = teams::text(&p["emailAddress"]);
            teams::valid_email(&email).then(|| member(&email, &teams::text(&p["displayName"]), &teams::text(&p["role"])))
        })
        .collect();
    out.sort_by(|a, b| a.s("email").cmp(&b.s("email")));
    out.dedup_by(|a, b| a["email"] == b["email"]);
    out
}

fn valid_roster(v: &Value, fid: &str) -> Option<Vec<Value>> {
    if v["v"] != 1 || v["team"] != fid {
        return None;
    }
    let rows = v["members"].as_array().filter(|r| r.len() <= MAX_MEMBERS)?;
    let mut out = vec![];
    for r in rows {
        let email = teams::text(&r["email"]);
        if !teams::valid_email(&email) {
            return None;
        }
        out.push(member(&email, &teams::text(&r["name"]), &teams::text(&r["role"])));
    }
    Some(out)
}

/// 폴더 안에서 `owner`가 만들고 마지막으로 고친 명단 파일을 찾는다.
/// 편집 권한이 있는 팀원은 소유자의 파일 내용도 고칠 수 있어서, 마지막으로 고친 사람까지 확인한다.
async fn find_roster(auth: &GoogleAuth, lease: &Lease, fid: &str, owner: &str) -> Result<Option<(String, Vec<Value>)>> {
    let query = format!("'{fid}' in parents and trashed = false and properties has {{ key='saiRoster' and value='1' }}");
    let list = auth
        .profile_request(
            lease,
            "GET",
            &format!("/drive/v3/files?spaces=drive&pageSize=20&fields=files(id,name,owners(emailAddress),lastModifyingUser(emailAddress))&q={}", q(&query)),
            None,
        )
        .await
        .map_err(message)?;
    for f in list["files"].as_array().ok_or("팀원 명단을 확인하지 못했어요.")? {
        let owners = f["owners"].as_array().map(Vec::as_slice).unwrap_or(&[]);
        let Some(rid) = f["id"].as_str().filter(|s| id(s)) else { continue };
        if f["name"] != ROSTER
            || owners.len() != 1
            || !same(owners[0].s("emailAddress").as_str(), owner)
            || !same(&f["lastModifyingUser"].s("emailAddress"), owner)
        {
            continue;
        }
        let content = read_json(auth, lease, rid).await.map_err(message)?;
        if let Some(rows) = valid_roster(&content, fid) {
            return Ok(Some((rid.to_owned(), rows)));
        }
    }
    Ok(None)
}

/// 소유자만: Drive 공유 목록으로 명단 파일을 만들거나 고친다.
async fn publish(auth: &GoogleAuth, lease: &Lease, fid: &str) -> Result<Vec<Value>> {
    let members = from_permissions(&teams::permissions(auth, lease, fid).await?);
    let content = json!({"v":1,"team":fid,"members":members,"updatedAt":now_ms()});
    match find_roster(auth, lease, fid, lease.email()).await? {
        Some((_, old)) if old == members => {}
        Some((rid, _)) => {
            auth.profile_request(
                lease,
                "PATCH",
                &format!("/upload/drive/v3/files/{rid}?uploadType=media&fields=id"),
                Some(("application/json".into(), content.to_string().into_bytes())),
            )
            .await
            .map_err(message)?;
        }
        None => {
            let meta = json!({"name":ROSTER,"mimeType":"application/json","parents":[fid],"properties":{"saiRoster":"1"},"description":"사이가 팀원끼리 말을 전할 때 쓰는 팀원 명단이에요. 팀 소유자의 사이 앱이 고쳐요."});
            create_json(auth, lease, meta, &content).await?;
        }
    }
    Ok(members)
}

/// 이 팀에서 말을 주고받을 수 있는 사람. 명단 파일이 아직 없으면 None.
async fn roster(auth: &GoogleAuth, lease: &Lease, team: &Value) -> Result<Option<Vec<Value>>> {
    let fid = team.s("folderId");
    let owner = team.s("ownerEmail");
    let mut rows = if team["canManage"] == true {
        publish(auth, lease, &fid).await?
    } else {
        match find_roster(auth, lease, &fid, &owner).await? {
            Some((_, rows)) => rows,
            None => return Ok(None),
        }
    };
    if teams::valid_email(&owner) && !rows.iter().any(|r| same(&r.s("email"), &owner)) {
        rows.push(member(&owner, &team.s("ownerName"), "owner"));
    }
    Ok(Some(rows))
}

/// 소유자가 팀원을 초대하거나 내보낸 뒤 명단 파일을 맞춘다. 실패해도 원래 작업은 이미 끝났다.
pub(super) async fn refresh_roster(auth: &GoogleAuth, profiles: &Profiles, teams: &Teams, epoch: u64, fid: &str) -> Result<()> {
    let (lease, team) = teams.access(auth, profiles, epoch, fid).await?;
    if team["canManage"] == true {
        publish(auth, &lease, fid).await?;
    }
    Ok(())
}

// ---------- 보내기 ----------

/// 받는 사람으로 고를 수 있는 팀원 목록(나 제외).
pub(super) async fn targets(auth: &GoogleAuth, profiles: &Profiles, teams: &Teams, epoch: u64) -> Result<Value> {
    let view = teams.view(auth, profiles).await?;
    if view["epoch"] != epoch {
        return err("계정 연결이 바뀌었어요. 다시 열어 주세요.");
    }
    let me = view.s("email");
    let mut out = vec![];
    for team in view["teams"].as_array().cloned().unwrap_or_default() {
        if team["accessVerified"] != true {
            continue;
        }
        let fid = team.s("folderId");
        let mut row = json!({"folderId":fid,"name":team["name"],"members":[],"rosterMissing":false,"error":null});
        match teams.access(auth, profiles, epoch, &fid).await {
            Ok((lease, team)) => match roster(auth, &lease, &team).await {
                Ok(Some(rows)) => {
                    row["members"] = json!(rows.into_iter().filter(|r| !same(&r.s("email"), &me)).collect::<Vec<_>>())
                }
                Ok(None) => row["rosterMissing"] = json!(true),
                Err(e) => row["error"] = json!(e),
            },
            Err(e) => row["error"] = json!(e),
        }
        out.push(row);
    }
    Ok(json!({"epoch":epoch,"email":me,"teams":out}))
}

/// 받는 사람이 지금 이 팀 명단에 있는지 확인하고 이름을 돌려준다.
pub(super) async fn recipient(auth: &GoogleAuth, profiles: &Profiles, teams: &Teams, epoch: u64, fid: &str, to: &str) -> Result<Value> {
    let (lease, team) = teams.access(auth, profiles, epoch, fid).await?;
    if !teams::valid_email(to) || same(to, lease.email()) {
        return err("받을 팀원을 다시 골라 주세요.");
    }
    let rows = roster(auth, &lease, &team)
        .await?
        .ok_or("팀 소유자가 사이에서 팀원 목록을 한 번 열어야 말을 전할 수 있어요.")?;
    let found = rows.iter().find(|r| same(&r.s("email"), to)).cloned().ok_or("지금 이 팀에 있는 사람에게만 전할 수 있어요.")?;
    // 문안에 팀원 이름이나 이메일이 들어가지 않았는지 검사할 때 쓴다.
    let mut names: Vec<String> = rows.iter().flat_map(|r| [r.s("name"), r.s("email")]).collect();
    names.push(lease.email().to_owned());
    names.extend(rows.iter().map(|r| r.s("email").split('@').next().unwrap_or("").to_owned()));
    names.retain(|n| n.chars().count() >= 2);
    Ok(json!({"email":found["email"],"name":found["name"],"teamName":team["name"],"names":names}))
}

/// 내 Drive의 "사이 메시지" 폴더. 메시지 파일이 내 Drive 최상위에 흩어지지 않게 한다.
async fn outbox(auth: &GoogleAuth, lease: &Lease) -> Result<String> {
    let query = format!("trashed = false and 'me' in owners and mimeType = '{FOLDER}' and properties has {{ key='saiOutbox' and value='1' }}");
    let list = auth
        .profile_request(lease, "GET", &format!("/drive/v3/files?spaces=drive&pageSize=1&fields=files(id)&q={}", q(&query)), None)
        .await
        .map_err(message)?;
    if let Some(fid) = list["files"][0]["id"].as_str().filter(|s| id(s)) {
        return Ok(fid.to_owned());
    }
    let body = json!({"name":OUTBOX,"mimeType":FOLDER,"properties":{"saiOutbox":"1"},"description":"사이 앱에서 팀원에게 전한 말이 보관돼요. 각 파일은 받는 사람 한 명에게만 공유돼요."});
    let value = auth
        .profile_request(
            lease,
            "POST",
            "/drive/v3/files?fields=id",
            Some(("application/json".into(), body.to_string().into_bytes())),
        )
        .await
        .map_err(message)?;
    value["id"].as_str().filter(|s| id(s)).map(str::to_owned).ok_or_else(|| "메시지 폴더를 만들지 못했어요.".into())
}

/// 메시지 파일을 만들고 받는 사람 한 명에게만 보기 권한을 준다. 공유에 실패하면 파일을 지운다.
async fn deliver(auth: &GoogleAuth, lease: &Lease, to: &str, content: &Value) -> Result<String> {
    let folder = outbox(auth, lease).await?;
    let meta = json!({"name":MESSAGE,"mimeType":"application/json","parents":[folder],"properties":{"saiMessage":"1"},"description":"사이 앱에서 팀원에게 전한 말이에요."});
    let fid = create_json(auth, lease, meta, content).await?;
    let share = json!({"type":"user","role":"reader","emailAddress":to});
    let shared = auth
        .profile_request(
            lease,
            "POST",
            &format!("/drive/v3/files/{fid}/permissions?sendNotificationEmail=false&fields=id,role,type"),
            Some(("application/json".into(), share.to_string().into_bytes())),
        )
        .await;
    if let Err(e) = shared {
        delete(auth, lease, &fid).await;
        return Err(format!("{} 받는 사람에게 공유하지 못해 보내지 않았어요.", message(e)));
    }
    Ok(fid)
}

pub(super) async fn send(auth: &GoogleAuth, profiles: &Profiles, teams: &Teams, epoch: u64, fid: &str, to: &str, text: &str) -> Result<Value> {
    if !valid_text(text) {
        return err("전할 말은 1~400자로 써 주세요.");
    }
    let person = recipient(auth, profiles, teams, epoch, fid, to).await?;
    let (lease, team) = teams.access(auth, profiles, epoch, fid).await?;
    let mid = random_token();
    let now = now_ms();
    let content = json!({"v":1,"type":"request","id":mid,"team":fid,"teamName":team["name"],"text":text,"createdAt":now});
    let file = deliver(auth, &lease, person["email"].as_str().unwrap_or(to), &content).await?;
    Ok(json!({"id":mid,"fileId":file,"to":person["email"],"toName":person["name"],"team":fid,"teamName":team["name"],"text":text,"at":now,"reaction":null}))
}

pub(super) async fn react(auth: &GoogleAuth, profiles: &Profiles, teams: &Teams, epoch: u64, fid: &str, to: &str, re: &str, reaction: &str) -> Result<()> {
    if reaction_label(reaction).is_none() || !id(re) || !teams::valid_email(to) {
        return err("반응을 다시 골라 주세요.");
    }
    let (lease, _) = teams.access(auth, profiles, epoch, fid).await?;
    let content = json!({"v":1,"type":"reaction","id":random_token(),"re":re,"team":fid,"reaction":reaction,"createdAt":now_ms()});
    deliver(auth, &lease, to, &content).await?;
    Ok(())
}

// ---------- 받기 ----------

/// 공유받은 파일 하나를 검사한다. 보낸 사람은 파일 내용이 아니라 Drive가 알려 준 소유자로 정한다.
fn incoming(meta: &Value, content: &Value) -> Option<Value> {
    let owners = meta["owners"].as_array().filter(|o| o.len() == 1)?;
    let from = teams::text(&owners[0]["emailAddress"]);
    if !teams::valid_email(&from)
        || meta["ownedByMe"] == true
        || meta["trashed"] == true
        || meta["mimeType"] != "application/json"
        || meta["properties"]["saiMessage"] != "1"
        || content["v"] != 1
    {
        return None;
    }
    let mid = content["id"].as_str().filter(|s| id(s) && s.len() <= 64)?;
    let team = content["team"].as_str().filter(|s| id(s))?;
    let mut item = json!({
        "fileId":meta["id"],"id":mid,"from":from.to_ascii_lowercase(),"fromName":teams::text(&owners[0]["displayName"]),
        "team":team,"createdAt":content["createdAt"].as_i64().unwrap_or(0),
    });
    match content["type"].as_str()? {
        "request" => {
            let text = content["text"].as_str().filter(|t| valid_text(t))?;
            item["type"] = json!("request");
            item["text"] = json!(text);
        }
        "reaction" => {
            let re = content["re"].as_str().filter(|s| id(s) && s.len() <= 64)?;
            let reaction = content["reaction"].as_str().filter(|r| reaction_label(r).is_some())?;
            item["type"] = json!("reaction");
            item["re"] = json!(re);
            item["reaction"] = json!(reaction);
        }
        _ => return None,
    }
    Some(item)
}

/// 나와 공유된 사이 메시지 중 아직 보지 않은 것을 확인한다.
/// 반환: items(받아도 되는 것), rejected(다시 확인하지 않을 파일). 일시적인 오류는 다음에 다시 본다.
pub(super) async fn fetch(auth: &GoogleAuth, profiles: &Profiles, teams: &Teams, epoch: u64, seen: &[String]) -> Result<Value> {
    let lease = auth.full_drive_lease(epoch).await?;
    let query = "sharedWithMe = true and trashed = false and mimeType = 'application/json' and properties has { key='saiMessage' and value='1' }";
    let list = auth
        .profile_request(
            &lease,
            "GET",
            &format!("/drive/v3/files?pageSize=50&orderBy=createdTime%20desc&fields=files({META})&q={}", q(query)),
            None,
        )
        .await
        .map_err(message)?;
    let files = list["files"].as_array().filter(|f| f.len() <= 50).ok_or("받은 말 목록을 확인하지 못했어요.")?;
    let fresh: Vec<&Value> = files
        .iter()
        .filter(|f| f["id"].as_str().is_some_and(|s| id(s) && !seen.iter().any(|x| x == s)))
        .collect();
    if fresh.is_empty() {
        return Ok(json!({"items":[],"rejected":[]}));
    }
    let view = teams.view(auth, profiles).await?;
    let my_teams: HashMap<String, Value> = view["teams"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|t| t["accessVerified"] == true)
        .map(|t| (t.s("folderId"), t.clone()))
        .collect();
    let mut rosters: HashMap<String, Option<Vec<Value>>> = HashMap::new();
    let (mut items, mut rejected) = (vec![], vec![]);
    for meta in fresh {
        let fid = meta.s("id");
        let Ok(content) = read_json(auth, &lease, &fid).await else { continue };
        let Some(mut item) = incoming(meta, &content) else {
            rejected.push(fid);
            continue;
        };
        let team_id = item.s("team");
        let Some(team) = my_teams.get(&team_id) else {
            rejected.push(fid);
            continue;
        };
        if item["type"] == "request" {
            if !rosters.contains_key(&team_id) {
                let found = roster(auth, &lease, team).await.ok().flatten();
                rosters.insert(team_id.clone(), found);
            }
            match &rosters[&team_id] {
                // 명단이 아직 없으면 판단을 미루고 다음에 다시 확인한다.
                None => continue,
                Some(rows) if !rows.iter().any(|r| same(&r.s("email"), &item.s("from"))) => {
                    rejected.push(fid);
                    continue;
                }
                Some(_) => {}
            }
        }
        item["teamName"] = team["name"].clone();
        items.push(item);
    }
    auth.with_lease(&lease, || Ok(json!({"items":items,"rejected":rejected}))).await
}

#[cfg(test)]
mod tests;
