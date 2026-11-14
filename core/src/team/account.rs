//! 다른 컴퓨터로 옮기기: 팀 서버의 내 계정(익명 로그인)과 참여한 팀 목록을 묶어 Google 계정에 맡기고 되찾는다.
//! 같은 계정을 쓰므로 맡은 작업·자기소개·동의한 약속이 그대로 이어진다.
use super::Hub;
use crate::util::J;
use crate::Result;
use serde_json::{json, Value};

pub const FILE: &str = "sai-account.json";

impl Hub {
    pub fn firebase_session(&self) -> Option<Value> {
        self.0.settings.read(|s| crate::secret::decrypt_json_field(s, "firebaseSessionEnc")).filter(|v| !v.s("uid").is_empty() && !v.s("refreshToken").is_empty())
    }

    /// 맡길 내용. 팀 서버 계정이 없거나(아직 팀을 써 본 적 없음) 로컬 시험 모드면 None
    pub fn account_snapshot(&self) -> Option<Value> {
        if self.backend_kind() == Some("memory") {
            return None;
        }
        let session = self.firebase_session()?;
        let teams: Vec<Value> = self.teams().iter().map(|t| json!({ "teamId": t["teamId"], "name": t["name"], "joinCode": t["joinCode"], "displayName": t["displayName"] })).collect();
        Some(json!({ "version": 1, "uid": session.s("uid"), "refreshToken": session.s("refreshToken"), "teams": teams, "savedAt": self.now() }))
    }

    /// 맡겨 둔 계정으로 이어 쓰기. 돌려주는 값: { restored: [팀 이름], rejoined: [팀 이름], same: bool }
    pub async fn restore_account(&self, snap: &Value) -> Result<Value> {
        let (uid, refresh) = (snap.s("uid"), snap.s("refreshToken"));
        if uid.is_empty() || refresh.is_empty() {
            return Ok(json!({ "restored": [], "rejoined": [], "same": false }));
        }
        let local = self.teams();
        let remote = snap.arr("teams");
        let mine = self.firebase_session().map(|s| s.s("uid"));
        let same = mine.as_deref() == Some(uid.as_str());
        let has = |list: &[Value], id: &str| list.iter().any(|t| t.s("teamId") == id);
        let local_only: Vec<Value> = local.iter().filter(|t| !has(&remote, &t.s("teamId"))).cloned().collect();

        let before = self.0.settings.read(|s| s["firebaseSessionEnc"].clone());
        if !same {
            // 이 컴퓨터의 계정 대신 맡겨 둔 계정을 쓴다
            let enc = crate::secret::encrypt_json(&json!({ "uid": uid, "refreshToken": refresh }));
            self.0.settings.update(|s| s["firebaseSessionEnc"] = json!(enc));
            self.reset_backend();
        }
        let b = self.backend()?;
        let signed = b.sign_in().await;
        if let Err(e) = &signed {
            self.0.settings.update(|s| s["firebaseSessionEnc"] = before.clone());
            self.reset_backend();
            return crate::err(format!("팀 서버에 연결하지 못했어요: {e}"));
        }
        if signed.unwrap_or_default() != uid {
            // 되찾지 못했으면 이 컴퓨터의 원래 계정으로 되돌린다
            self.0.settings.update(|s| s["firebaseSessionEnc"] = before);
            self.reset_backend();
            return crate::err("Google 계정에 맡겨 둔 팀 로그인 정보가 만료됐어요. 참여 코드로 다시 들어가 주세요.");
        }

        let mut restored = vec![];
        for t in &remote {
            if !has(&local, &t.s("teamId")) && !t.s("teamId").is_empty() {
                self.add_team(t.clone());
                restored.push(t.s("name"));
            }
        }
        // 이 컴퓨터에서만 들어가 있던 팀은 이어 쓰는 계정으로 다시 참여한다 (참여 코드 사용)
        let mut rejoined = vec![];
        if !same {
            for t in &local_only {
                match b.join_team(&t.s("joinCode"), &t.s("displayName")).await {
                    Ok(_) => rejoined.push(t.s("name")),
                    Err(_) => self.remove_team(&t.s("teamId")),
                }
            }
        }
        if let Some(first) = remote.first().filter(|_| local.is_empty()) {
            let _ = self.set_active(&first.s("teamId"));
        }
        self.sync_all().await;
        Ok(json!({ "restored": restored, "rejoined": rejoined, "same": same }))
    }
}
