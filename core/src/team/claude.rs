//! 팀에 Claude API 키 공유하기.
//! 한 사람이 자기 키를 팀에 올리면 키가 없는 팀원도 AI 기능(자유 대화, 말로 일정 고치기 등)을 쓸 수 있다.
//!  - 요금은 키 주인 계정에 나간다. 팀원은 누구든 키를 꺼내 볼 수 있다는 점을 화면에서 알린다.
//!  - 공유한 사람만 바꾸거나 끌 수 있다 (firestore.rules).
//!  - 받은 키는 실행 중에만 기억하고 파일에 저장하지 않는다. 화면 캐시에는 누가 공유했는지만 둔다.
use super::{Hub, Team};
use crate::util::J;
use crate::{err, Result};
use serde_json::{json, Value};

impl Team {
    /// 이 팀이 공유한 키만 반환한다. 다른 팀의 프로젝트에 과금되지 않게 한다.
    pub(crate) fn project_claude_key(&self) -> Option<String> {
        let team_id = self.id.as_ref()?;
        self.hub.0.claude_keys.lock().unwrap().get(team_id).cloned()
    }

    pub async fn claude_share(&self, api_key: &str) -> Result<()> {
        let key = api_key.trim();
        if !key.starts_with("sk-ant-") {
            return err("Claude API 키(sk-ant-…)가 아니에요.");
        }
        let team_id = self.team_id()?;
        let b = self.backend()?;
        let me = b.sign_in().await?;
        if let Some(cur) = b.get_integration(&team_id, "claude").await? {
            if cur.s("sharedBy") != me {
                return err("이미 다른 팀원이 키를 공유하고 있어요. 그 팀원이 공유를 끈 뒤에 공유할 수 있어요.");
            }
        }
        b.set_integration(&team_id, "claude", json!({ "apiKey": key, "sharedBy": me })).await?;
        self.claude_sync().await
    }

    pub async fn claude_unshare(&self) -> Result<()> {
        let team_id = self.team_id()?;
        let b = self.backend()?;
        let me = b.sign_in().await?;
        match b.get_integration(&team_id, "claude").await? {
            Some(cur) if cur.s("sharedBy") != me => return err("다른 팀원이 공유한 키는 그 팀원만 끌 수 있어요."),
            Some(_) => b.delete_integration(&team_id, "claude").await?,
            None => {}
        }
        self.claude_sync().await
    }

    pub async fn claude_sync(&self) -> Result<()> {
        if !self.is_connected() {
            return Ok(());
        }
        let team_id = self.team_id()?;
        let doc = self.backend()?.get_integration(&team_id, "claude").await?;
        let id = self.id.clone().unwrap_or_default();
        {
            let mut keys = self.hub.0.claude_keys.lock().unwrap();
            match doc.as_ref().map(|d| d.s("apiKey")).filter(|k| !k.is_empty()) {
                Some(k) => keys.insert(id, k),
                None => keys.remove(&id),
            };
        }
        let by = doc.map(|d| d.s("sharedBy"));
        self.cache_update(|c| c["claudeShare"] = by.map(|u| json!({ "sharedBy": u })).unwrap_or(Value::Null));
        Ok(())
    }

    /// 화면용: { shared, mine, byName }
    pub fn claude_status(&self) -> Value {
        let c = self.cache_get();
        let by = c["claudeShare"].s("sharedBy");
        if by.is_empty() {
            return json!({ "shared": false, "mine": false, "byName": null });
        }
        let name = self.members(true).iter().find(|m| m.s("uid") == by).map(|m| m.s("displayName")).unwrap_or_else(|| "나간 팀원".into());
        json!({ "shared": true, "mine": by == c.s("myUid"), "byName": name })
    }
}

impl Hub {
    /// 쓸 수 있는 팀 공유 키 (보고 있는 팀을 먼저)
    pub fn shared_claude_key(&self) -> Option<String> {
        let keys = self.0.claude_keys.lock().unwrap();
        let active = self.active().id;
        active.as_ref().and_then(|a| keys.get(a).cloned()).or_else(|| keys.values().next().cloned())
    }
}
