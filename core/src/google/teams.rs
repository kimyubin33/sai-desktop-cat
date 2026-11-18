//! Drive-backed teams. A remembered ID never substitutes for a live permission check.
use super::{
    oauth::{GoogleAuth, Lease},
    profile::{
        model::{id, TeamRef},
        Profiles,
    },
};
use crate::{err, Result};
use serde_json::{json, Value};
use std::collections::BTreeSet;

const FOLDER: &str = "application/vnd.google-apps.folder";
const FIELDS: &str = "id,name,mimeType,trashed,ownedByMe,writersCanShare,properties,owners(displayName,emailAddress),sharingUser(displayName,emailAddress),capabilities(canListChildren,canAddChildren,canShare)";
/// Roles the app hands out. Owner transfer and public (anyone/domain) sharing are never created here.
const ROLES: &[&str] = &["writer", "reader"];
#[derive(Default)]
pub(super) struct Teams {
    gate: tokio::sync::Mutex<()>,
}
fn message(e: (u16, String)) -> String {
    match e.0 {
        403 | 404 => {
            "이 계정에서 팀 폴더에 접근할 수 없어요. 소유자에게 공유 상태를 확인해 주세요.".into()
        }
        401 => "Google 연결을 다시 확인해 주세요.".into(),
        _ => e.1,
    }
}
fn folder(value: Value, fid: &str) -> Result<Value> {
    if value["id"] != fid
        || value["mimeType"] != FOLDER
        || value["trashed"] != false
        || value["properties"]["saiTeam"] != "1"
        || value["capabilities"]["canListChildren"] != true
    {
        return err("현재 접근할 수 있는 사이 팀 폴더를 선택해 주세요.");
    }
    let name = value["name"]
        .as_str()
        .filter(|s| {
            !s.trim().is_empty() && s.chars().count() <= 100 && !s.chars().any(char::is_control)
        })
        .ok_or("팀 폴더의 이름을 확인하지 못했어요.")?;
    let owned = value["ownedByMe"] == true;
    let can_share = value["capabilities"]["canShare"] == true;
    // Drive defaults writersCanShare to true. Teams created by this app start with it off,
    // so only the owner adds people unless the owner deliberately opens invites to members.
    let members_can_invite = value["writersCanShare"] == true;
    let owner = &value["owners"][0];
    Ok(json!({
        "folderId":fid,
        "name":name,
        "accessVerified":true,
        "canManage":owned && can_share,
        "canInvite":can_share && (owned || members_can_invite),
        "membersCanInvite":members_can_invite,
        "ownerEmail":text(&owner["emailAddress"]),
        "ownerName":text(&owner["displayName"]),
        "canUpload":value["capabilities"]["canAddChildren"] == true,
    }))
}
/// Remote display text (names, e-mail addresses) is untrusted: bounded and free of control characters.
fn text(value: &Value) -> String {
    value
        .as_str()
        .filter(|s| s.chars().count() <= 254 && !s.chars().any(char::is_control))
        .unwrap_or("")
        .trim()
        .to_owned()
}
async fn metadata(
    auth: &GoogleAuth,
    lease: &Lease,
    fid: &str,
) -> std::result::Result<Value, (u16, String)> {
    if !id(fid) {
        return Err((0, "팀 폴더 주소가 올바르지 않아요.".into()));
    }
    auth.profile_request(
        lease,
        "GET",
        &format!("/drive/v3/files/{fid}?fields={FIELDS}"),
        None,
    )
    .await
}
async fn checked(auth: &GoogleAuth, lease: &Lease, fid: &str) -> Result<Value> {
    folder(metadata(auth, lease, fid).await.map_err(message)?, fid)
}
async fn lease(auth: &GoogleAuth, epoch: u64) -> Result<Lease> {
    let lease = auth.file_lease().await?;
    if epoch != lease.epoch() {
        return err("계정이 바뀌었어요. 팀 목록을 다시 확인해 주세요.");
    }
    if !auth.has_file_scope() {
        return err("먼저 Drive 연결을 허용해 주세요.");
    }
    Ok(lease)
}
fn remembered(profiles: &Profiles, subject: &str, fid: &str) -> Result<()> {
    let state = profiles.team_state(subject)?;
    if !id(fid)
        || !state["teams"]
            .as_array()
            .is_some_and(|rows| rows.iter().any(|r| r["folderId"] == fid))
    {
        return err("현재 계정의 팀 목록에서 폴더를 선택해 주세요.");
    }
    Ok(())
}
impl Teams {
    pub async fn browse(
        &self,
        auth: &GoogleAuth,
        profiles: &Profiles,
        epoch: u64,
        page: &str,
    ) -> Result<Value> {
        let lease = auth.full_drive_lease(epoch).await?;
        if page.len() > 2048 {
            return err("다음 팀 목록 주소를 확인하지 못했어요.");
        }
        let state = profiles.team_state(lease.subject())?;
        let q="trashed = false and mimeType = 'application/vnd.google-apps.folder' and properties has { key='saiTeam' and value='1' }";
        let value=auth.profile_request(&lease,"GET",&format!("/drive/v3/files?spaces=drive&pageSize=25&fields=nextPageToken,incompleteSearch,files({FIELDS})&q={}&pageToken={}",urlencoding::encode(q),urlencoding::encode(page)),None).await.map_err(message)?;
        // A partial search is still useful: show what Drive returned and say the list may be incomplete.
        let incomplete = value["incompleteSearch"] == true;
        let mut entries = vec![];
        let mut skipped = 0;
        for meta in value["files"]
            .as_array()
            .filter(|v| v.len() <= 25)
            .ok_or("팀 목록의 형식을 확인하지 못했어요.")?
        {
            // One unusable folder (bad id, lost access, odd name) must not hide every other team.
            let Some(fid) = meta["id"].as_str().filter(|s| id(s)) else {
                skipped += 1;
                continue;
            };
            let Ok(team) = folder(meta.clone(), fid) else {
                skipped += 1;
                continue;
            };
            if state["teams"]
                .as_array()
                .is_some_and(|ts| ts.iter().any(|t| t["folderId"] == fid))
            {
                continue;
            }
            // Anyone can share a marked folder with anyone. Show who owns it and who shared it so the
            // person can recognise a stranger's folder before joining.
            entries.push(json!({
                "id":fid,
                "name":team["name"],
                "folder":true,
                "team":true,
                "ownerEmail":team["ownerEmail"],
                "ownerName":team["ownerName"],
                "sharedBy":text(&meta["sharingUser"]["emailAddress"]),
                "mine":team["canManage"],
            }));
        }
        let next = match value.get("nextPageToken") {
            None => String::new(),
            Some(Value::String(s)) if !s.is_empty() && s.len() <= 2048 && s != page => s.clone(),
            _ => return err("다음 팀 목록을 확인하지 못했어요."),
        };
        auth.with_lease(&lease, || {
            Ok(json!({"epoch":epoch,"entries":entries,"nextPage":next,"incomplete":incomplete,"skipped":skipped,"email":lease.email()}))
        })
        .await
    }
    pub async fn access(
        &self,
        auth: &GoogleAuth,
        profiles: &Profiles,
        epoch: u64,
        fid: &str,
    ) -> Result<(Lease, Value)> {
        let lease = lease(auth, epoch).await?;
        remembered(profiles, lease.subject(), fid)?;
        let team = checked(auth, &lease, fid).await?;
        Ok((lease, team))
    }
    pub async fn view(&self, auth: &GoogleAuth, profiles: &Profiles) -> Result<Value> {
        let lease = if auth.has_file_scope() {
            auth.file_lease().await?
        } else {
            auth.lease().await?
        };
        let state = profiles.team_state(lease.subject())?;
        let connected = auth.has_file_scope();
        let rows = state["teams"]
            .as_array()
            .ok_or("팀 목록을 읽지 못했어요.")?;
        if rows.len() > 32 {
            return err("현재는 팀 폴더를 32개까지 확인할 수 있어요.");
        }
        let mut teams = vec![];
        let mut connection_error = None;
        for row in rows {
            let fid = row["folderId"]
                .as_str()
                .ok_or("팀 폴더 정보를 확인하지 못했어요.")?;
            let mut team = if let Some(error) = &connection_error {
                json!({"folderId":fid,"name":row["name"],"accessVerified":false,"error":error})
            } else if connected {
                match metadata(auth, &lease, fid).await {
                    Ok(value) => match folder(value, fid) {
                        Ok(team) => team,
                        Err(error) => {
                            json!({"folderId":fid,"name":row["name"],"accessVerified":false,"error":error})
                        }
                    },
                    Err(error) => {
                        let stop =
                            error.0 == 0 || error.0 == 401 || error.0 == 429 || error.0 >= 500;
                        let error = message(error);
                        if stop {
                            connection_error = Some(error.clone());
                        }
                        json!({"folderId":fid,"name":row["name"],"accessVerified":false,"error":error})
                    }
                }
            } else {
                json!({"folderId":fid,"name":row["name"],"accessVerified":false,"error":"Drive 연결을 허용하면 접근 권한을 확인해요."})
            };
            team["epoch"] = json!(lease.epoch());
            teams.push(team);
        }
        let sync_error = profiles.sync_error(lease.subject());
        auth.with_lease(&lease,||Ok(json!({"epoch":lease.epoch(),"signedIn":true,"email":lease.email(),"driveConnected":connected,"driveFull":auth.has_full_drive(),"ready":state["ready"],"syncError":sync_error,"teams":teams,"draft":state["draft"],"pending":state["pending"]}))).await
    }
    pub async fn create(
        &self,
        auth: &GoogleAuth,
        profiles: &Profiles,
        epoch: u64,
        name: &str,
    ) -> Result<Value> {
        let lease = lease(auth, epoch).await?;
        let _gate = self.gate.lock().await;
        let mut draft = profiles.draft(auth, &lease, name.trim()).await?;
        let fid = match draft.file_id.take() {
            Some(id) => id,
            None => {
                let result = auth
                    .profile_request(
                        &lease,
                        "GET",
                        "/drive/v3/files/generateIds?count=1&space=drive&type=files",
                        None,
                    )
                    .await
                    .map_err(message)?;
                let fid = result["ids"]
                    .as_array()
                    .filter(|a| a.len() == 1)
                    .and_then(|a| a[0].as_str())
                    .filter(|s| id(s))
                    .ok_or("팀 폴더의 저장 주소를 준비하지 못했어요.")?
                    .to_owned();
                profiles
                    .draft_id(auth, &lease, &draft.operation, &fid)
                    .await?;
                fid
            }
        };
        let value = match metadata(auth, &lease, &fid).await {
            Ok(value) => value,
            Err((404, _)) => {
                let body = json!({"id":fid,"name":draft.name,"mimeType":FOLDER,"writersCanShare":false,"properties":{"saiTeam":"1","saiCreation":draft.operation}});
                match auth
                    .profile_request(
                        &lease,
                        "POST",
                        &format!("/drive/v3/files?fields={FIELDS}"),
                        Some(("application/json".into(), body.to_string().into_bytes())),
                    )
                    .await
                {
                    Ok(value) => value,
                    Err((409, _)) => metadata(auth, &lease, &fid).await.map_err(message)?,
                    Err(e) => return Err(message(e)),
                }
            }
            Err(e) => return Err(message(e)),
        };
        if value["ownedByMe"] != true || value["properties"]["saiCreation"] != draft.operation {
            return err("만들던 팀 폴더와 저장된 자료가 달라요. 덮어쓰지 않았어요.");
        }
        let team = folder(value, &fid)?;
        profiles
            .remember_team(
                auth,
                &lease,
                TeamRef {
                    folder_id: fid,
                    name: team["name"].as_str().unwrap().into(),
                    removed: false,
                },
                Some(&draft.operation),
            )
            .await?;
        Ok(team)
    }
    pub async fn join(
        &self,
        auth: &GoogleAuth,
        profiles: &Profiles,
        epoch: u64,
        fid: &str,
    ) -> Result<Value> {
        let lease = lease(auth, epoch).await?;
        let _gate = self.gate.lock().await;
        // Picker's returned ID and account are untrusted. Only the original verified app token is used here.
        let team = checked(auth, &lease, fid).await?;
        profiles
            .remember_team(
                auth,
                &lease,
                TeamRef {
                    folder_id: fid.into(),
                    name: team["name"].as_str().unwrap().into(),
                    removed: false,
                },
                None,
            )
            .await?;
        Ok(team)
    }
    pub async fn open(
        &self,
        auth: &GoogleAuth,
        profiles: &Profiles,
        epoch: u64,
        fid: &str,
    ) -> Result<String> {
        let lease = lease(auth, epoch).await?;
        remembered(profiles, lease.subject(), fid)?;
        checked(auth, &lease, fid).await?;
        auth.with_lease(&lease, || {
            Ok(format!("https://drive.google.com/drive/folders/{fid}"))
        })
        .await
    }
    pub async fn remove(
        &self,
        auth: &GoogleAuth,
        profiles: &Profiles,
        epoch: u64,
        fid: &str,
    ) -> Result<()> {
        let lease = auth.lease().await?;
        if lease.epoch() != epoch {
            return err("계정이 바뀌었어요. 다시 확인해 주세요.");
        }
        remembered(profiles, lease.subject(), fid)?;
        profiles
            .remember_team(
                auth,
                &lease,
                TeamRef {
                    folder_id: fid.into(),
                    name: String::new(),
                    removed: true,
                },
                None,
            )
            .await
    }
    pub async fn invite(
        &self,
        auth: &GoogleAuth,
        profiles: &Profiles,
        epoch: u64,
        fid: &str,
        email: &str,
        role: &str,
    ) -> Result<Value> {
        let email = email.trim();
        if !valid_email(email) {
            return err("초대할 팀원의 Google 이메일을 확인해 주세요.");
        }
        let role = role_of(role)?;
        let lease = lease(auth, epoch).await?;
        let _gate = self.gate.lock().await;
        remembered(profiles, lease.subject(), fid)?;
        let team = checked(auth, &lease, fid).await?;
        if team["canInvite"] != true {
            return err("이 팀은 소유자만 초대할 수 있어요.");
        }
        if email.eq_ignore_ascii_case(lease.email()) {
            return err("내 계정은 초대하지 않아도 돼요.");
        }
        let url = format!("https://drive.google.com/drive/folders/{fid}");
        let existing = permission(auth, &lease, fid, email).await?;
        let (method, path, body) = match existing {
            Some(p) if p["role"] == "owner" || p["role"] == role => {
                return Ok(json!({"alreadyShared":true,"role":p["role"],"url":url}))
            }
            Some(_) if team["canManage"] != true => {
                return err("이미 팀에 있는 사람의 권한은 소유자만 바꿀 수 있어요.")
            }
            Some(p) => {
                let pid = p["id"]
                    .as_str()
                    .filter(|s| id(s))
                    .ok_or("기존 공유 권한을 확인하지 못했어요.")?;
                (
                    "PATCH",
                    format!("/drive/v3/files/{fid}/permissions/{pid}?fields=id,role,type"),
                    json!({"role":role}),
                )
            }
            None => (
                "POST",
                format!("/drive/v3/files/{fid}/permissions?sendNotificationEmail=true&fields=id,role,type"),
                json!({"type":"user","role":role,"emailAddress":email}),
            ),
        };
        auth.profile_request(
            &lease,
            method,
            &path,
            Some(("application/json".into(), body.to_string().into_bytes())),
        )
        .await
        .map_err(|e| {
            format!(
                "{} 공유 후 응답이 끊겼다면 다시 확인할 때 기존 권한을 먼저 확인해요.",
                message(e)
            )
        })?;
        let confirmed = permission(auth, &lease, fid, email).await?;
        if !confirmed.is_some_and(|p| p["role"] == role || p["role"] == "owner") {
            return err("초대 결과를 아직 확인하지 못했어요. 공유 상태를 다시 확인해 주세요.");
        }
        Ok(json!({"alreadyShared":false,"role":role,"url":url}))
    }
    /// Owner-only member list, including any public (anyone/domain) access found on the folder.
    pub async fn members(
        &self,
        auth: &GoogleAuth,
        profiles: &Profiles,
        epoch: u64,
        fid: &str,
    ) -> Result<Value> {
        let lease = lease(auth, epoch).await?;
        owner(auth, profiles, &lease, fid).await?;
        let rows: Vec<_> = permissions(auth, &lease, fid)
            .await?
            .into_iter()
            .filter(|p| p["deleted"] != true && p["id"].as_str().is_some_and(id))
            .map(|p| {
                json!({
                    "id":p["id"],
                    "type":text(&p["type"]),
                    "role":text(&p["role"]),
                    "email":text(&p["emailAddress"]),
                    "name":text(&p["displayName"]),
                    "domain":text(&p["domain"]),
                    "public":p["type"] == "anyone" || p["type"] == "domain",
                    "me":p["emailAddress"].as_str().is_some_and(|e| e.eq_ignore_ascii_case(lease.email())),
                })
            })
            .collect();
        auth.with_lease(&lease, || {
            Ok(json!({"epoch":epoch,"folderId":fid,"members":rows}))
        })
        .await
    }
    /// Owner-only: change a member's role, or remove them with role "remove". The owner entry is never touched.
    pub async fn update_member(
        &self,
        auth: &GoogleAuth,
        profiles: &Profiles,
        epoch: u64,
        fid: &str,
        pid: &str,
        role: &str,
    ) -> Result<()> {
        let remove = role == "remove";
        let role = if remove { "" } else { role_of(role)? };
        if !id(pid) {
            return err("팀원 정보를 확인하지 못했어요.");
        }
        let lease = lease(auth, epoch).await?;
        let _gate = self.gate.lock().await;
        owner(auth, profiles, &lease, fid).await?;
        let target = permissions(auth, &lease, fid)
            .await?
            .into_iter()
            .find(|p| p["id"] == pid && p["deleted"] != true)
            .ok_or("이미 팀에서 빠진 사람이에요. 목록을 새로고침해 주세요.")?;
        if target["role"] == "owner" {
            return err("소유자의 권한은 바꾸거나 내보낼 수 없어요.");
        }
        let public = target["type"] == "anyone" || target["type"] == "domain";
        if public && !remove {
            return err("공개 링크 권한은 내보내기만 할 수 있어요.");
        }
        let path = format!("/drive/v3/files/{fid}/permissions/{pid}");
        let result = if remove {
            auth.profile_request(&lease, "DELETE", &path, None).await
        } else {
            auth.profile_request(
                &lease,
                "PATCH",
                &format!("{path}?fields=id,role"),
                Some((
                    "application/json".into(),
                    json!({"role":role}).to_string().into_bytes(),
                )),
            )
            .await
        };
        match result {
            // DELETE answers 204 with an empty body.
            Ok(_) | Err((204, _)) => Ok(()),
            Err(e) => Err(message(e)),
        }
    }
    /// Owner-only switch: may editors invite other people? Off by default for app-created teams.
    pub async fn set_policy(
        &self,
        auth: &GoogleAuth,
        profiles: &Profiles,
        epoch: u64,
        fid: &str,
        members_can_invite: bool,
    ) -> Result<Value> {
        let lease = lease(auth, epoch).await?;
        let _gate = self.gate.lock().await;
        owner(auth, profiles, &lease, fid).await?;
        let value = auth
            .profile_request(
                &lease,
                "PATCH",
                &format!("/drive/v3/files/{fid}?fields={FIELDS}"),
                Some((
                    "application/json".into(),
                    json!({"writersCanShare":members_can_invite})
                        .to_string()
                        .into_bytes(),
                )),
            )
            .await
            .map_err(message)?;
        let team = folder(value, fid)?;
        if team["membersCanInvite"] != members_can_invite {
            return err("초대 설정이 바뀌었는지 확인하지 못했어요. 다시 확인해 주세요.");
        }
        Ok(team)
    }
}
fn role_of(role: &str) -> Result<&'static str> {
    match role {
        "" => Ok("writer"),
        r => ROLES
            .iter()
            .copied()
            .find(|x| *x == r)
            .ok_or_else(|| "팀원 권한을 '편집' 또는 '보기'로 선택해 주세요.".into()),
    }
}
async fn owner(auth: &GoogleAuth, profiles: &Profiles, lease: &Lease, fid: &str) -> Result<()> {
    remembered(profiles, lease.subject(), fid)?;
    if checked(auth, lease, fid).await?["canManage"] != true {
        return err("팀 폴더의 소유자만 팀원을 관리할 수 있어요.");
    }
    Ok(())
}
fn valid_email(email: &str) -> bool {
    let parts: Vec<_> = email.split('@').collect();
    email.len() <= 254
        && email.is_ascii()
        && !email
            .bytes()
            .any(|b| b.is_ascii_whitespace() || b.is_ascii_control())
        && parts.len() == 2
        && !parts[0].is_empty()
        && parts[0].len() <= 64
        && parts[1].contains('.')
        && !parts[1].starts_with('.')
        && !parts[1].ends_with('.')
}
async fn permissions(auth: &GoogleAuth, lease: &Lease, fid: &str) -> Result<Vec<Value>> {
    let mut page = String::new();
    let mut pages = BTreeSet::new();
    let mut rows = vec![];
    loop {
        let value = auth.profile_request(lease,"GET",&format!("/drive/v3/files/{fid}/permissions?fields=nextPageToken,permissions(id,type,role,emailAddress,displayName,domain,deleted)&pageSize=100&pageToken={}",urlencoding::encode(&page)),None).await.map_err(message)?;
        for p in value["permissions"]
            .as_array()
            .ok_or("공유 권한 목록을 확인하지 못했어요.")?
        {
            if rows.len() >= 1024 {
                return err("공유 권한 목록이 너무 커서 확인을 중단했어요.");
            }
            rows.push(p.clone());
        }
        match value.get("nextPageToken") {
            None => return Ok(rows),
            Some(Value::String(token))
                if !token.is_empty() && token.len() <= 2048 && pages.insert(token.clone()) =>
            {
                page = token.clone()
            }
            _ => return err("공유 권한 목록의 다음 페이지를 확인하지 못했어요."),
        }
    }
}
async fn permission(
    auth: &GoogleAuth,
    lease: &Lease,
    fid: &str,
    email: &str,
) -> Result<Option<Value>> {
    Ok(permissions(auth, lease, fid).await?.into_iter().rfind(|p| {
        p["deleted"] != true
            && p["type"] == "user"
            && p["emailAddress"]
                .as_str()
                .is_some_and(|s| s.eq_ignore_ascii_case(email))
    }))
}
#[cfg(test)]
mod tests;
