//! 팀 서버. Firebase(익명 로그인 + Firestore REST) 또는 시험용 메모리/파일 저장소.
//!
//! 데이터 구조 (자세한 권한은 firestore.rules):
//!   joinCodes/{code}              { teamId, teamName }
//!   teams/{teamId}                { name, joinCode, createdBy }
//!   teams/{teamId}/members/{uid}  { displayName, joinCode }
//!   teams/{teamId}/signals/{id}   { topic, toMember, week, held }      — 보낸 사람 정보 없음
//!   teams/{teamId}/proposals/{id} { topic, week, responses: {uid: 'agree'|'later'} }
//!   teams/{teamId}/profiles/{uid}, projects/{id}, projects/{id}/tasks/{id}
//!   teams/{teamId}/integrations/telegram, teams/{teamId}/telegram/{uid}
//! 문안(text)은 서버에 올리지 않는다. 받는 쪽이 자기 앱에 있는 주제별 일반 문안으로 말한다.
use super::codec::{decode_doc, encode_fields};
use crate::http::{client, json_body, short};
use crate::util::{to_base36, J};
use crate::{err, Result};
use rand::Rng;
use serde_json::{json, Map, Value};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

const CODE_CHARS: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

pub fn random_code(len: usize) -> String {
    let mut r = rand::thread_rng();
    (0..len).map(|_| CODE_CHARS[r.gen_range(0..CODE_CHARS.len())] as char).collect()
}

/// Firebase 콘솔에서 복사한 설정(JS 객체 또는 JSON)에서 apiKey, projectId 를 뽑는다
pub fn parse_firebase_config(text: &str) -> Option<Value> {
    let get = |k: &str| {
        let re = regex::Regex::new(&format!(r#"["']?{k}["']?\s*:\s*["']([^"']+)["']"#)).ok()?;
        re.captures(text).map(|c| c[1].trim().to_string())
    };
    Some(json!({ "apiKey": get("apiKey")?, "projectId": get("projectId")? }))
}

pub type SaveSession = Box<dyn Fn(&Value) + Send + Sync>;

pub struct Firebase {
    api_key: String,
    auth_base: String,
    token_url: String,
    docs_base: String,
    session: Mutex<Option<(String, String)>>, // (uid, refreshToken)
    id_token: Mutex<Option<(String, i64)>>,
    save_session: SaveSession,
}

pub struct Memory {
    file: Option<PathBuf>,
    data: Arc<Mutex<Value>>,
    uid: String,
}

pub enum Backend {
    Firebase(Firebase),
    Memory(Memory),
}

impl Backend {
    /// config: { apiKey, projectId, emulator?: { auth, firestore } }, session: { uid, refreshToken }
    pub fn firebase(config: &Value, session: Option<Value>, save_session: SaveSession) -> Result<Backend> {
        let api_key = config.s("apiKey");
        let project = config.s("projectId");
        if api_key.is_empty() || project.is_empty() {
            return err("Firebase 설정(apiKey, projectId)이 필요해요.");
        }
        let emu = config.get("emulator").cloned().unwrap_or(Value::Null);
        let auth_emu = emu.os("auth");
        let fs_emu = emu.os("firestore");
        let sess = session.and_then(|s| {
            let (u, r) = (s.s("uid"), s.s("refreshToken"));
            (!u.is_empty() && !r.is_empty()).then_some((u, r))
        });
        Ok(Backend::Firebase(Firebase {
            auth_base: match &auth_emu {
                Some(a) => format!("{a}/identitytoolkit.googleapis.com/v1"),
                None => "https://identitytoolkit.googleapis.com/v1".into(),
            },
            token_url: match &auth_emu {
                Some(a) => format!("{a}/securetoken.googleapis.com/v1/token"),
                None => "https://securetoken.googleapis.com/v1/token".into(),
            },
            docs_base: format!("{}/v1/projects/{}/databases/(default)/documents", fs_emu.unwrap_or_else(|| "https://firestore.googleapis.com".into()), project),
            api_key,
            session: Mutex::new(sess),
            id_token: Mutex::new(None),
            save_session,
        }))
    }

    /// 시험용: 파일을 주면 한 컴퓨터에서 앱 두 개를 띄워 팀처럼 시험할 수 있다
    pub fn memory(file: Option<PathBuf>, uid: &str) -> Backend {
        Backend::Memory(Memory { file, data: Arc::new(Mutex::new(json!({ "docs": {}, "seq": 0 }))), uid: uid.into() })
    }

    /// 시험용: 여러 사용자가 같은 메모리 저장소를 쓴다
    pub fn memory_shared(data: Arc<Mutex<Value>>, uid: &str) -> Backend {
        Backend::Memory(Memory { file: None, data, uid: uid.into() })
    }

    pub fn new_shared_memory() -> Arc<Mutex<Value>> {
        Arc::new(Mutex::new(json!({ "docs": {}, "seq": 0 })))
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Backend::Firebase(_) => "firebase",
            Backend::Memory(m) => {
                if m.file.is_some() {
                    "local-file"
                } else {
                    "memory"
                }
            }
        }
    }

    // ---------- 인증 ----------
    /// 지금 로그인 정보 { uid, refreshToken } (다른 컴퓨터로 옮길 때)
    pub fn session(&self) -> Option<Value> {
        match self {
            Backend::Firebase(f) => f.session.lock().unwrap().as_ref().map(|(u, r)| json!({ "uid": u, "refreshToken": r })),
            Backend::Memory(m) => Some(json!({ "uid": m.uid, "refreshToken": "" })),
        }
    }

    pub async fn sign_in(&self) -> Result<String> {
        match self {
            Backend::Memory(m) => Ok(m.uid.clone()),
            Backend::Firebase(f) => {
                f.token().await?;
                Ok(f.session.lock().unwrap().as_ref().map(|s| s.0.clone()).unwrap_or_default())
            }
        }
    }

    // ---------- 문서 다루기 ----------
    async fn get_doc(&self, path: &str) -> Result<Option<Value>> {
        match self {
            Backend::Firebase(f) => Ok(f.req("GET", &format!("/{path}"), None, "").await?.and_then(|d| decode_doc(&d))),
            Backend::Memory(m) => Ok(m.tx(|d| d["docs"].get(path).cloned()).map(|mut v| {
                v["id"] = json!(path.rsplit('/').next().unwrap_or(""));
                v
            })),
        }
    }

    /// 새 문서 → id
    async fn create_doc(&self, coll: &str, id: Option<&str>, data: Value) -> Result<String> {
        match self {
            Backend::Firebase(f) => {
                let q = id.map(|i| format!("?documentId={}", urlencoding::encode(i))).unwrap_or_default();
                let body = json!({ "fields": encode_fields(data.as_object().unwrap_or(&Map::new())) });
                let d = f.req("POST", &format!("/{coll}"), Some(body), &q).await?.unwrap_or(Value::Null);
                decode_doc(&d).map(|v| v.s("id")).ok_or_else(|| "문서를 만들지 못했어요.".to_string())
            }
            Backend::Memory(m) => m.tx(|d| {
                let id = match id {
                    Some(i) => i.to_string(),
                    None => {
                        let n = d["seq"].as_u64().unwrap_or(0) + 1;
                        d["seq"] = json!(n);
                        format!("d{}{}", to_base36(n), random_code(4).to_lowercase())
                    }
                };
                let path = format!("{coll}/{id}");
                if d["docs"].get(&path).is_some() {
                    return Err("이미 있는 문서예요.".to_string());
                }
                d["docs"][&path] = data;
                Ok(id)
            }),
        }
    }

    /// field_paths 의 필드만 바꾼다 ("responses.`uid`" 같은 중첩 경로 가능). 문서가 없으면 실패
    async fn patch_doc(&self, path: &str, data: Value, field_paths: &[String]) -> Result<()> {
        match self {
            Backend::Firebase(f) => {
                let mask = field_paths.iter().map(|p| format!("updateMask.fieldPaths={}", urlencoding::encode(p))).collect::<Vec<_>>().join("&");
                let q = format!("?{mask}&currentDocument.exists=true");
                let body = json!({ "fields": encode_fields(data.as_object().unwrap_or(&Map::new())) });
                f.req("PATCH", &format!("/{path}"), Some(body), &q).await?;
                Ok(())
            }
            Backend::Memory(m) => m.tx(|d| {
                let Some(doc) = d["docs"].get_mut(path) else { return Err("문서가 없어요.".to_string()) };
                for fp in field_paths {
                    let segs: Vec<String> = fp.split('.').map(|s| s.trim_matches('`').to_string()).collect();
                    let mut src = &data;
                    for s in &segs {
                        src = src.get(s).unwrap_or(&Value::Null);
                    }
                    let mut dst = &mut *doc;
                    for s in &segs[..segs.len() - 1] {
                        if !dst.get(s).map(|x| x.is_object()).unwrap_or(false) {
                            dst[s] = json!({});
                        }
                        dst = dst.get_mut(s).unwrap();
                    }
                    dst[segs.last().unwrap()] = src.clone();
                }
                Ok(())
            }),
        }
    }

    async fn delete_doc(&self, path: &str) -> Result<()> {
        match self {
            Backend::Firebase(f) => {
                f.req("DELETE", &format!("/{path}"), None, "").await?;
                Ok(())
            }
            Backend::Memory(m) => m.tx(|d| {
                if let Some(o) = d["docs"].as_object_mut() {
                    o.remove(path);
                    // 하위 문서도 지운다
                    let prefix = format!("{path}/");
                    o.retain(|k, _| !k.starts_with(&prefix));
                }
                Ok(())
            }),
        }
    }

    async fn list_docs(&self, coll: &str) -> Result<Vec<Value>> {
        match self {
            Backend::Firebase(f) => {
                let mut out = vec![];
                let mut page = String::new();
                loop {
                    let q = format!("?pageSize=100{}", if page.is_empty() { String::new() } else { format!("&pageToken={}", urlencoding::encode(&page)) });
                    let d = f.req("GET", &format!("/{coll}"), None, &q).await?.unwrap_or(Value::Null);
                    for doc in d.arr("documents") {
                        if let Some(v) = decode_doc(&doc) {
                            out.push(v);
                        }
                    }
                    page = d.s("nextPageToken");
                    if page.is_empty() {
                        break;
                    }
                }
                Ok(out)
            }
            Backend::Memory(m) => Ok(m.tx(|d| {
                let prefix = format!("{coll}/");
                let mut v: Vec<(String, Value)> = d["docs"]
                    .as_object()
                    .map(|o| {
                        o.iter()
                            .filter(|(k, _)| k.starts_with(&prefix) && !k[prefix.len()..].contains('/'))
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect()
                    })
                    .unwrap_or_default();
                v.sort_by(|a, b| a.0.cmp(&b.0));
                v.into_iter()
                    .map(|(k, mut doc)| {
                        doc["id"] = json!(k[prefix.len()..]);
                        doc
                    })
                    .collect()
            })),
        }
    }

    async fn query_eq(&self, parent: &str, coll: &str, field: &str, value: &str) -> Result<Vec<Value>> {
        match self {
            Backend::Firebase(f) => {
                let body = json!({ "structuredQuery": {
                    "from": [{ "collectionId": coll }],
                    "where": { "fieldFilter": { "field": { "fieldPath": field }, "op": "EQUAL", "value": { "stringValue": value } } },
                    "limit": 200
                }});
                let rows = f.req("POST", &format!("/{parent}:runQuery"), Some(body), "").await?.unwrap_or(json!([]));
                Ok(rows.as_array().cloned().unwrap_or_default().iter().filter_map(|r| r.get("document").and_then(decode_doc)).collect())
            }
            Backend::Memory(_) => Ok(self.list_docs(&format!("{parent}/{coll}")).await?.into_iter().filter(|d| d.s(field) == value).collect()),
        }
    }

    async fn upsert(&self, coll: &str, id: &str, data: Value) -> Result<()> {
        let path = format!("{coll}/{id}");
        if self.get_doc(&path).await.ok().flatten().is_some() {
            let keys: Vec<String> = data.as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default();
            self.patch_doc(&path, data, &keys).await
        } else {
            self.create_doc(coll, Some(id), data).await.map(|_| ())
        }
    }

    /// 여러 문서를 한 번에 (모두 되거나 모두 안 되거나).
    /// writes: { op: "create" | "update" | "delete", path, data?, fields? }
    async fn commit(&self, writes: Vec<Value>) -> Result<()> {
        match self {
            Backend::Firebase(f) => {
                // 문서 이름은 projects/…/documents/<path> 꼴이어야 한다
                let root = f.docs_base.split("/v1/").nth(1).unwrap_or("").to_string();
                let ws: Vec<Value> = writes
                    .iter()
                    .map(|w| {
                        let name = format!("{root}/{}", w.s("path"));
                        let fields = encode_fields(w["data"].as_object().unwrap_or(&Map::new()));
                        match w.s("op").as_str() {
                            "delete" => json!({ "delete": name }),
                            "update" => json!({ "update": { "name": name, "fields": fields }, "updateMask": { "fieldPaths": w.arr("fields") }, "currentDocument": { "exists": true } }),
                            _ => json!({ "update": { "name": name, "fields": fields }, "currentDocument": { "exists": false } }),
                        }
                    })
                    .collect();
                f.req("POST", ":commit", Some(json!({ "writes": ws })), "").await?;
                Ok(())
            }
            Backend::Memory(m) => m.tx(|d| {
                for w in &writes {
                    let exists = d["docs"].get(w.s("path")).is_some();
                    match w.s("op").as_str() {
                        "create" if exists => return Err("이미 있는 문서예요.".to_string()),
                        "update" if !exists => return Err("문서가 없어요.".to_string()),
                        _ => {}
                    }
                }
                for w in &writes {
                    let path = w.s("path");
                    match w.s("op").as_str() {
                        "delete" => {
                            if let Some(o) = d["docs"].as_object_mut() {
                                o.remove(&path);
                            }
                        }
                        "update" => {
                            for k in w.arr("fields") {
                                let k = k.as_str().unwrap_or("").to_string();
                                d["docs"][&path][&k] = w["data"][&k].clone();
                            }
                        }
                        _ => d["docs"][&path] = w["data"].clone(),
                    }
                }
                Ok(())
            }),
        }
    }

    // ---------- 팀 ----------
    /// → { teamId, joinCode, name }
    pub async fn create_team(&self, name: &str, display_name: &str) -> Result<Value> {
        let uid = self.sign_in().await?;
        for _ in 0..5 {
            let code = random_code(6);
            if self.get_doc(&format!("joinCodes/{code}")).await.ok().flatten().is_some() {
                continue;
            }
            let team_id = self.create_doc("teams", None, json!({ "name": name, "joinCode": code, "createdBy": uid })).await?;
            self.create_doc("joinCodes", Some(&code), json!({ "teamId": team_id, "teamName": name })).await?;
            self.enter(&team_id, &uid, display_name, &code, false).await?;
            return Ok(json!({ "teamId": team_id, "joinCode": code, "name": name }));
        }
        err("참여 코드를 만들지 못했어요. 다시 시도해 주세요.")
    }

    pub async fn join_team(&self, join_code: &str, display_name: &str) -> Result<Value> {
        let uid = self.sign_in().await?;
        let code = join_code.trim().to_uppercase();
        let Some(r) = self.get_doc(&format!("joinCodes/{code}")).await? else { return err("참여 코드를 찾을 수 없어요.") };
        let team_id = r.s("teamId");
        let mpath = format!("teams/{team_id}/members/{uid}");
        let already = self.get_doc(&mpath).await.ok().flatten();
        let key = name_key(display_name);
        if let Some(me) = &already {
            // 이미 이 팀의 팀원: 이름만 바꾸는 경우. 팀원이라 목록을 볼 수 있으니 먼저 확인한다
            if name_key(&me.s("displayName")) != key || me.s("nameKey") != key {
                if self.list_members(&team_id).await?.iter().any(|m| m.s("uid") != uid && name_key(&m.s("displayName")) == key) {
                    return err(same_name(display_name));
                }
                self.enter(&team_id, &uid, display_name, &code, true).await?;
                let old = me.s("nameKey");
                if !old.is_empty() && old != key {
                    let _ = self.delete_doc(&format!("teams/{team_id}/names/{old}")).await;
                }
            }
        } else {
            self.enter(&team_id, &uid, display_name, &code, false).await?;
            // 예전 버전으로 들어와 이름을 잡아 두지 않은 팀원과 겹치는지: 들어온 뒤에야 목록을 볼 수 있다
            let clash = self.list_members(&team_id).await.map(|l| l.iter().any(|m| m.s("uid") != uid && name_key(&m.s("displayName")) == key)).unwrap_or(false);
            if clash {
                let _ = self.delete_doc(&format!("teams/{team_id}/names/{key}")).await;
                let _ = self.delete_doc(&mpath).await;
                return err(same_name(display_name));
            }
        }
        Ok(json!({ "teamId": team_id, "joinCode": code, "name": r.s("teamName") }))
    }

    /// 이름을 잡아 두고(names/<이름>) 팀원으로 등록한다. 같은 이름을 다른 사람이 잡고 있으면 실패
    async fn enter(&self, team_id: &str, uid: &str, display_name: &str, code: &str, member_exists: bool) -> Result<()> {
        let key = name_key(display_name);
        let npath = format!("teams/{team_id}/names/{key}");
        let mpath = format!("teams/{team_id}/members/{uid}");
        let member_write = |with_key: bool| {
            let mut data = json!({ "displayName": display_name });
            if with_key {
                data["nameKey"] = json!(key);
            }
            if member_exists {
                let fields: Vec<&str> = if with_key { vec!["displayName", "nameKey"] } else { vec!["displayName"] };
                json!({ "op": "update", "path": mpath, "data": data, "fields": fields })
            } else {
                data["joinCode"] = json!(code);
                json!({ "op": "create", "path": mpath, "data": data })
            }
        };
        match self.get_doc(&npath).await {
            Ok(Some(n)) if n.s("uid") != uid => err(same_name(display_name)),
            Ok(Some(_)) => self.commit(vec![member_write(true)]).await,
            Ok(None) => self.commit(vec![json!({ "op": "create", "path": npath, "data": { "uid": uid } }), member_write(true)]).await.map_err(|e| if denied(&e) { same_name(display_name) } else { e }),
            // 서버 규칙이 예전 것이면 이름 잡아 두기를 쓸 수 없다 → 예전 방식으로 들어간다
            Err(e) if denied(&e) => self.commit(vec![member_write(false)]).await,
            Err(e) => Err(e),
        }
    }

    pub async fn leave_team(&self, team_id: &str) -> Result<()> {
        let uid = self.sign_in().await?;
        let mpath = format!("teams/{team_id}/members/{uid}");
        if let Ok(Some(me)) = self.get_doc(&mpath).await {
            let key = if me.s("nameKey").is_empty() { name_key(&me.s("displayName")) } else { me.s("nameKey") };
            if let Ok(Some(n)) = self.get_doc(&format!("teams/{team_id}/names/{key}")).await {
                if n.s("uid") == uid {
                    let _ = self.delete_doc(&format!("teams/{team_id}/names/{key}")).await;
                }
            }
            let _ = self.delete_doc(&format!("teams/{team_id}/admins/{uid}")).await;
        }
        self.delete_doc(&mpath).await
    }

    /// [{ uid, displayName }]
    pub async fn list_members(&self, team_id: &str) -> Result<Vec<Value>> {
        Ok(self.list_docs(&format!("teams/{team_id}/members")).await?.iter().map(|d| json!({ "uid": d.s("id"), "displayName": d.s("displayName"), "nameKey": d.s("nameKey") })).collect())
    }

    /// 이름 잡아 두기 정리: 내 이름이 안 잡혀 있으면 잡고, 나간 사람이 잡아 둔 이름은 푼다 (규칙이 예전 것이면 아무것도 안 함)
    pub async fn tidy_names(&self, team_id: &str, members: &[Value]) {
        let Ok(uid) = self.sign_in().await else { return };
        let Ok(names) = self.list_docs(&format!("teams/{team_id}/names")).await else { return };
        for n in &names {
            if !members.iter().any(|m| m.s("uid") == n.s("uid")) {
                let _ = self.delete_doc(&format!("teams/{team_id}/names/{}", n.s("id"))).await;
            }
        }
        let Some(me) = members.iter().find(|m| m.s("uid") == uid) else { return };
        let key = name_key(&me.s("displayName"));
        if me.s("nameKey") != key && !names.iter().any(|n| n.s("id") == key && members.iter().any(|m| m.s("uid") == n.s("uid"))) {
            let _ = self.enter(team_id, &uid, &me.s("displayName"), "", true).await;
        }
    }

    // ---------- 관리자 (팀마다 아이디·비밀번호) ----------
    /// → { supported, exists, adminId, me }
    pub async fn admin_state(&self, team_id: &str) -> Value {
        let Ok(uid) = self.sign_in().await else { return json!({ "supported": false }) };
        match self.get_doc(&format!("teams/{team_id}/admin/config")).await {
            Err(_) => json!({ "supported": false }),
            Ok(cfg) => {
                let me = self.get_doc(&format!("teams/{team_id}/admins/{uid}")).await.ok().flatten().is_some();
                json!({ "supported": true, "exists": cfg.is_some(), "adminId": cfg.map(|c| c.s("adminId")), "me": me })
            }
        }
    }

    /// 처음 한 번: 관리자 아이디·비밀번호를 정한다 (정한 사람이 관리자가 된다)
    pub async fn admin_setup(&self, team_id: &str, admin_id: &str, password: &str) -> Result<()> {
        let uid = self.sign_in().await?;
        check_admin_input(admin_id, password)?;
        if self.get_doc(&format!("teams/{team_id}/admin/config")).await.map_err(|e| if denied(&e) { rules_old() } else { e })?.is_some() {
            return err("이 팀에는 이미 관리자가 있어요. 관리자 아이디와 비밀번호로 로그인해 주세요.");
        }
        let salt = random_code(16);
        let key = admin_key(&salt, admin_id, password);
        self.commit(vec![
            json!({ "op": "create", "path": format!("teams/{team_id}/adminKeys/{key}"), "data": { "v": 1 } }),
            json!({ "op": "create", "path": format!("teams/{team_id}/admin/config"), "data": { "adminId": admin_id.trim(), "salt": salt, "by": uid } }),
            json!({ "op": "create", "path": format!("teams/{team_id}/admins/{uid}"), "data": { "key": key } }),
        ])
        .await
        .map_err(|e| if denied(&e) { rules_old() } else { e })
    }

    /// 다른 컴퓨터에서도: 아이디·비밀번호가 맞으면 이 기기가 관리자가 된다
    pub async fn admin_login(&self, team_id: &str, admin_id: &str, password: &str) -> Result<()> {
        let uid = self.sign_in().await?;
        let Some(cfg) = self.get_doc(&format!("teams/{team_id}/admin/config")).await.map_err(|e| if denied(&e) { rules_old() } else { e })? else {
            return err("이 팀에는 아직 관리자가 없어요.");
        };
        let wrong = || "관리자 아이디나 비밀번호가 달라요.".to_string();
        if cfg.s("adminId") != admin_id.trim() {
            return Err(wrong());
        }
        let key = admin_key(&cfg.s("salt"), admin_id, password);
        if self.get_doc(&format!("teams/{team_id}/adminKeys/{key}")).await.ok().flatten().is_none() {
            return Err(wrong());
        }
        let path = format!("teams/{team_id}/admins/{uid}");
        if self.get_doc(&path).await.ok().flatten().is_none() {
            self.create_doc(&format!("teams/{team_id}/admins"), Some(&uid), json!({ "key": key })).await.map_err(|e| if denied(&e) { wrong() } else { e })?;
        }
        Ok(())
    }

    pub async fn admin_logout(&self, team_id: &str) -> Result<()> {
        let uid = self.sign_in().await?;
        self.delete_doc(&format!("teams/{team_id}/admins/{uid}")).await
    }

    /// 관리자만: 팀원 내보내기 (그 사람이 잡아 둔 이름도 푼다)
    pub async fn remove_member(&self, team_id: &str, member: &str) -> Result<()> {
        let uid = self.sign_in().await?;
        if member == uid {
            return err("나 자신은 내보낼 수 없어요. \"보고 있는 팀 나가기\"를 써 주세요.");
        }
        if self.get_doc(&format!("teams/{team_id}/admins/{uid}")).await.ok().flatten().is_none() {
            return err("관리자만 팀원을 내보낼 수 있어요.");
        }
        let mpath = format!("teams/{team_id}/members/{member}");
        if let Ok(Some(m)) = self.get_doc(&mpath).await {
            let key = if m.s("nameKey").is_empty() { name_key(&m.s("displayName")) } else { m.s("nameKey") };
            if let Ok(Some(n)) = self.get_doc(&format!("teams/{team_id}/names/{key}")).await {
                if n.s("uid") == member {
                    let _ = self.delete_doc(&format!("teams/{team_id}/names/{key}")).await;
                }
            }
        }
        let _ = self.delete_doc(&format!("teams/{team_id}/admins/{member}")).await;
        let _ = self.delete_doc(&format!("teams/{team_id}/profiles/{member}")).await;
        self.delete_doc(&mpath).await.map_err(|e| if denied(&e) { "팀원을 내보내지 못했어요. 관리자 로그인이 풀렸거나 서버 규칙이 예전 것이에요.".to_string() } else { e })
    }

    // ---------- 조언 주제 신호 ----------
    pub async fn create_signal(&self, team_id: &str, topic: &str, to_member: &str, week: &str) -> Result<String> {
        self.create_doc(&format!("teams/{team_id}/signals"), None, json!({ "topic": topic, "toMember": to_member, "week": week, "held": false })).await
    }

    pub async fn get_signal(&self, team_id: &str, id: &str) -> Result<Option<Value>> {
        self.get_doc(&format!("teams/{team_id}/signals/{id}")).await
    }

    pub async fn list_signals_for_me(&self, team_id: &str) -> Result<Vec<Value>> {
        let uid = self.sign_in().await?;
        self.query_eq(&format!("teams/{team_id}"), "signals", "toMember", &uid).await
    }

    pub async fn hold_signal(&self, team_id: &str, id: &str) -> Result<()> {
        self.patch_doc(&format!("teams/{team_id}/signals/{id}"), json!({ "held": true }), &["held".into()]).await
    }

    // ---------- 팀 약속 제안 ----------
    pub async fn create_proposal(&self, team_id: &str, topic: &str, week: &str) -> Result<String> {
        self.create_doc(&format!("teams/{team_id}/proposals"), None, json!({ "topic": topic, "week": week, "responses": {} })).await
    }

    pub async fn list_proposals(&self, team_id: &str) -> Result<Vec<Value>> {
        self.list_docs(&format!("teams/{team_id}/proposals")).await
    }

    pub async fn respond_proposal(&self, team_id: &str, id: &str, response: &str) -> Result<()> {
        let uid = self.sign_in().await?;
        let mut resp = Map::new();
        resp.insert(uid.clone(), json!(response));
        self.patch_doc(&format!("teams/{team_id}/proposals/{id}"), json!({ "responses": resp }), &[format!("responses.`{uid}`")]).await
    }

    // ---------- 협업: 자기소개·프로젝트·작업 ----------
    pub async fn set_profile(&self, team_id: &str, data: Value) -> Result<()> {
        let uid = self.sign_in().await?;
        self.upsert(&format!("teams/{team_id}/profiles"), &uid, data).await
    }

    /// [{ uid, ...자기소개 }]
    pub async fn list_profiles(&self, team_id: &str) -> Result<Vec<Value>> {
        Ok(self
            .list_docs(&format!("teams/{team_id}/profiles"))
            .await?
            .into_iter()
            .map(|mut d| {
                let id = d["id"].take();
                let o = d.as_object_mut().unwrap();
                o.remove("id");
                o.insert("uid".into(), id);
                d
            })
            .collect())
    }

    pub async fn create_project(&self, team_id: &str, data: Value) -> Result<String> {
        self.create_doc(&format!("teams/{team_id}/projects"), None, data).await
    }

    pub async fn list_projects(&self, team_id: &str) -> Result<Vec<Value>> {
        self.list_docs(&format!("teams/{team_id}/projects")).await
    }

    pub async fn update_project(&self, team_id: &str, project_id: &str, patch: Value) -> Result<()> {
        let keys: Vec<String> = patch.as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default();
        self.patch_doc(&format!("teams/{team_id}/projects/{project_id}"), patch, &keys).await
    }

    pub async fn create_task(&self, team_id: &str, project_id: &str, data: Value) -> Result<String> {
        self.create_doc(&format!("teams/{team_id}/projects/{project_id}/tasks"), None, data).await
    }

    pub async fn list_tasks(&self, team_id: &str, project_id: &str) -> Result<Vec<Value>> {
        self.list_docs(&format!("teams/{team_id}/projects/{project_id}/tasks")).await
    }

    pub async fn update_task(&self, team_id: &str, project_id: &str, task_id: &str, patch: Value) -> Result<()> {
        let keys: Vec<String> = patch.as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default();
        self.patch_doc(&format!("teams/{team_id}/projects/{project_id}/tasks/{task_id}"), patch, &keys).await
    }

    pub async fn delete_task(&self, team_id: &str, project_id: &str, task_id: &str) -> Result<()> {
        self.delete_doc(&format!("teams/{team_id}/projects/{project_id}/tasks/{task_id}")).await
    }

    // ---------- 외부 연결 ----------
    /// 팀 공용 연결 정보 (예: telegram 봇 토큰) — 팀원만 읽는다
    // ---------- 팀 Drive 함 (파일·메시지 주고받기) ----------
    pub async fn add_share(&self, team_id: &str, data: Value) -> Result<String> {
        self.create_doc(&format!("teams/{team_id}/shares"), None, data).await
    }

    pub async fn list_shares(&self, team_id: &str) -> Result<Vec<Value>> {
        self.list_docs(&format!("teams/{team_id}/shares")).await
    }

    pub async fn delete_share(&self, team_id: &str, id: &str) -> Result<()> {
        self.delete_doc(&format!("teams/{team_id}/shares/{id}")).await
    }

    // ---------- 팀 자료 (Drive 링크) ----------
    pub async fn add_file(&self, team_id: &str, project_id: &str, data: Value) -> Result<String> {
        self.create_doc(&format!("teams/{team_id}/projects/{project_id}/files"), None, data).await
    }

    pub async fn list_files(&self, team_id: &str, project_id: &str) -> Result<Vec<Value>> {
        self.list_docs(&format!("teams/{team_id}/projects/{project_id}/files")).await
    }

    pub async fn update_file(&self, team_id: &str, project_id: &str, file_id: &str, patch: Value) -> Result<()> {
        let keys: Vec<String> = patch.as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default();
        self.patch_doc(&format!("teams/{team_id}/projects/{project_id}/files/{file_id}"), patch, &keys).await
    }

    pub async fn delete_file(&self, team_id: &str, project_id: &str, file_id: &str) -> Result<()> {
        self.delete_doc(&format!("teams/{team_id}/projects/{project_id}/files/{file_id}")).await
    }

    pub async fn set_integration(&self, team_id: &str, name: &str, data: Value) -> Result<()> {
        self.upsert(&format!("teams/{team_id}/integrations"), name, data).await
    }

    pub async fn get_integration(&self, team_id: &str, name: &str) -> Result<Option<Value>> {
        Ok(self.get_doc(&format!("teams/{team_id}/integrations/{name}")).await?.map(|mut d| {
            d.as_object_mut().unwrap().remove("id");
            d
        }))
    }

    pub async fn delete_integration(&self, team_id: &str, name: &str) -> Result<()> {
        self.delete_doc(&format!("teams/{team_id}/integrations/{name}")).await
    }

    /// 내 Telegram 채팅 연결 — 본인만 쓰고, 팀원은 알림을 보내기 위해 읽는다
    pub async fn set_telegram_link(&self, team_id: &str, data: Value) -> Result<()> {
        let uid = self.sign_in().await?;
        self.upsert(&format!("teams/{team_id}/telegram"), &uid, data).await
    }

    pub async fn remove_telegram_link(&self, team_id: &str) -> Result<()> {
        let uid = self.sign_in().await?;
        self.delete_doc(&format!("teams/{team_id}/telegram/{uid}")).await
    }

    pub async fn list_telegram_links(&self, team_id: &str) -> Result<Vec<Value>> {
        Ok(self
            .list_docs(&format!("teams/{team_id}/telegram"))
            .await?
            .into_iter()
            .map(|mut d| {
                let id = d["id"].take();
                let o = d.as_object_mut().unwrap();
                o.remove("id");
                o.insert("uid".into(), id);
                d
            })
            .collect())
    }
}

impl Firebase {
    async fn token(&self) -> Result<String> {
        if let Some((t, exp)) = self.id_token.lock().unwrap().clone() {
            if crate::time::now_ms() < exp - 60_000 {
                return Ok(t);
            }
        }
        let refresh = self.session.lock().unwrap().as_ref().map(|s| s.1.clone());
        if let Some(rt) = refresh {
            let res = client()
                .post(format!("{}?key={}", self.token_url, self.api_key))
                .header("content-type", "application/x-www-form-urlencoded")
                .body(crate::http::form(&[("grant_type", "refresh_token"), ("refresh_token", &rt)]))
                .send()
                .await
                .map_err(|e| format!("팀 서버에 연결하지 못했어요: {e}"))?;
            if res.status().is_success() {
                let (_, d) = json_body(res).await;
                self.set_token(&d.s("id_token"), d.n("expires_in"), &d.s("user_id"), &d.s("refresh_token"));
                return Ok(d.s("id_token"));
            }
            // refresh token 이 무효하면 새 익명 계정을 만든다 (팀에는 다시 참여해야 함)
            *self.session.lock().unwrap() = None;
        }
        let res = client()
            .post(format!("{}/accounts:signUp?key={}", self.auth_base, self.api_key))
            .json(&json!({ "returnSecureToken": true }))
            .send()
            .await
            .map_err(|e| format!("팀 서버에 연결하지 못했어요: {e}"))?;
        let (status, d) = json_body(res).await;
        if !(200..300).contains(&status) {
            return err(format!("익명 로그인 실패 ({status}): {}", short(&d)));
        }
        self.set_token(&d.s("idToken"), d.n("expiresIn"), &d.s("localId"), &d.s("refreshToken"));
        Ok(d.s("idToken"))
    }

    fn set_token(&self, id_token: &str, expires_in: f64, uid: &str, refresh: &str) {
        let exp = crate::time::now_ms() + (if expires_in > 0.0 { expires_in } else { 3600.0 } * 1000.0) as i64;
        *self.id_token.lock().unwrap() = Some((id_token.to_string(), exp));
        let changed = {
            let mut s = self.session.lock().unwrap();
            let changed = s.as_ref().map(|(u, r)| u != uid || r != refresh).unwrap_or(true);
            *s = Some((uid.to_string(), refresh.to_string()));
            changed
        };
        if changed {
            (self.save_session)(&json!({ "uid": uid, "refreshToken": refresh }));
        }
    }

    /// 404 는 None
    async fn req(&self, method: &str, path: &str, body: Option<Value>, query: &str) -> Result<Option<Value>> {
        let token = self.token().await?;
        let url = format!("{}{}{}", self.docs_base, path, query);
        let m = reqwest::Method::from_bytes(method.as_bytes()).unwrap();
        let mut rb = client().request(m, &url).bearer_auth(token);
        if let Some(b) = body {
            rb = rb.json(&b);
        }
        let res = rb.send().await.map_err(|e| format!("팀 서버에 연결하지 못했어요: {e}"))?;
        let (status, d) = json_body(res).await;
        if status == 404 {
            return Ok(None);
        }
        if !(200..300).contains(&status) {
            return err(format!("Firestore {method} {path} 실패 ({status}): {}", short(&d)));
        }
        Ok(Some(d))
    }
}

impl Memory {
    fn tx<R>(&self, f: impl FnOnce(&mut Value) -> R) -> R {
        let mut g = self.data.lock().unwrap();
        if let Some(file) = &self.file {
            if let Ok(raw) = std::fs::read_to_string(file) {
                if let Ok(v) = serde_json::from_str::<Value>(&raw) {
                    *g = v;
                }
            }
        }
        if !g["docs"].is_object() {
            g["docs"] = json!({});
        }
        let r = f(&mut g);
        if let Some(file) = &self.file {
            if let Some(dir) = file.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let tmp = file.with_extension(format!("{}.tmp", std::process::id()));
            if std::fs::write(&tmp, serde_json::to_string_pretty(&*g).unwrap_or_default()).is_ok() {
                let _ = std::fs::rename(&tmp, file);
            }
        }
        r
    }
}

/// 이름 비교용 (공백·대소문자 무시). Firestore 문서 id 로도 쓴다
pub fn name_key(name: &str) -> String {
    let s: String = name.trim().to_lowercase().chars().filter(|c| !c.is_whitespace()).map(|c| if c == '/' { '_' } else { c }).collect();
    format!("n-{s}")
}

fn same_name(name: &str) -> String {
    format!("이 팀에는 이미 \"{}\"(이)라는 이름의 팀원이 있어요. 다른 이름으로 들어와 주세요. 예전에 쓰던 내 이름이라면 관리자에게 그 이름을 내보내 달라고 해 주세요.", name.trim())
}

fn denied(e: &str) -> bool {
    e.contains("(403)") || e.contains("PERMISSION_DENIED")
}

fn rules_old() -> String {
    "팀 서버 규칙에 아직 관리자 기능이 없어요. Firebase 콘솔에서 firestore.rules 를 게시해 주세요.".to_string()
}

fn check_admin_input(admin_id: &str, password: &str) -> Result<()> {
    let id = admin_id.trim();
    if id.chars().count() < 3 || id.chars().count() > 30 {
        return err("관리자 아이디는 3~30자로 정해 주세요.");
    }
    if password.chars().count() < 6 || password.chars().count() > 100 {
        return err("관리자 비밀번호는 6자 이상으로 정해 주세요.");
    }
    Ok(())
}

/// 비밀번호 자체는 어디에도 저장하지 않는다. 아이디·비밀번호로 만든 이 값을 아는 것이 관리자라는 증거가 된다
fn admin_key(salt: &str, admin_id: &str, password: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::digest(format!("sai-admin:{salt}:{}:{password}", admin_id.trim()).as_bytes());
    for _ in 0..100_000 {
        h = Sha256::digest(h);
    }
    format!("{h:x}")
}
