//! 서버 전환은 새 프로세스가 시작될 때 적용한다. 실행 중인 동기화는 기존 서버와 기존 파일만 쓴다.
use super::{Store, Stores};
use crate::util::J;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub fn scope(target: &Value) -> &str {
    if target.is_null() { return "firebase"; }
    target["serverId"].as_str().filter(|id| crate::team::desktop::valid_id(id)).unwrap_or("invalid-desktop")
}

pub(super) fn folder(dir: &Path, target: &Value) -> PathBuf {
    let base = dir.join("shared");
    if target.is_null() { base } else { base.join("servers").join(scope(target)) }
}

impl Stores {
    /// 현재 서버의 목록·캐시는 보존한다. 처음 이전할 때만 도착 서버가 확인한 팀의 기록을 복사한다.
    pub fn apply_team_server(&self, dir: &Path) -> crate::Result<bool> {
        let settings = self.settings.get();
        let plan = &settings["pendingTeamServer"];
        if plan.is_null() { return Ok(false); }
        if plan["from"] != settings["teamServer"] || plan["sourceTeams"] != settings["teams"] {
            return crate::err("팀 목록이 변경됐어요. 설정에서 서버 연결을 다시 확인해 주세요.");
        }
        let to = &plan["to"];
        let before = scope(&settings["teamServer"]);
        let after = scope(to);
        if after == "invalid-desktop" { return crate::err("데스크톱 서버 정보를 다시 확인해 주세요."); }
        if before != after && !settings["teamServerProfiles"][after].is_object() {
            let teams = plan.arr("teams");
            let included = |id: &Value| teams.iter().any(|team| &team["teamId"] == id);
            let source = self.team_cache.get();
            let mut caches = json!({});
            for t in teams.iter().filter_map(|t| t["teamId"].as_str()) {
                if let Some(cache) = source["teams"].get(t) { caches[t] = cache.clone(); }
            }
            let items = self.shared_outbox.read(|o| o.arr("items").into_iter().filter(|row| included(&row["teamId"])).collect::<Vec<_>>());
            let path = folder(dir, to);
            for (name, contents) in [("outbox", json!({"items":items})), ("team-cache", json!({"teams":caches}))] {
                let file = Store::open(path.join(format!("{name}.json")), Value::Null);
                file.replace(contents);
                file.try_flush().map_err(|e| format!("기기에서 팀 데이터를 복사하지 못했어요: {e}"))?;
            }
        }
        self.settings.update(|s| {
            if before != after {
                s["teamServerProfiles"][before] = json!({"teams":s["teams"], "activeTeamId":s["activeTeamId"]});
            }
            s["teams"] = plan["teams"].clone();
            s["activeTeamId"] = plan["activeTeamId"].clone();
            s["teamServer"] = to.clone();
            s["pendingTeamServer"] = Value::Null;
            s["teamServerNotice"] = Value::Null;
        });
        if let Err(e) = self.settings.try_flush() {
            self.settings.replace(settings);
            return crate::err(format!("서버 설정을 저장하지 못했어요: {e}"));
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cutover_keeps_unmigrated_teams_and_separates_pending_items() {
        let dir = std::env::temp_dir().join(format!("sai-switch-{}", rand::random::<u64>()));
        let stores = Stores::open(&dir);
        let a = json!({"teamId":"a","name":"이전할 방"});
        let other = json!({"teamId":"b","name":"이전 서버에 남을 방"});
        let both = json!([a,other]);
        let target = json!({"url":"https://test.example","serverId":"5608a7b4-f9da-4ad7-a98f-646be56ac567"});
        stores.team_cache.update(|c| c["teams"] = json!({"a":{"x":1},"b":{"x":2}}));
        stores.shared_outbox.update(|o| o["items"] = json!([{"teamId":"a","id":"1"},{"teamId":"b","id":"2"}]));
        stores.settings.update(|s| {
            s["teams"] = both.clone();
            s["pendingTeamServer"] = json!({"from":null,"sourceTeams":both,"to":target,"teams":[a],"activeTeamId":"a"});
        });
        stores.flush_all();
        assert!(stores.apply_team_server(&dir).unwrap());
        let desktop = Stores::open(&dir);
        assert_eq!(desktop.settings.get()["teams"], json!([a]));
        assert_eq!(desktop.shared_outbox.read(|o| o.arr("items").len()), 1);
        assert!(desktop.team_cache.get()["teams"]["b"].is_null());
        desktop.settings.update(|s| s["pendingTeamServer"] = json!({"from":target,"sourceTeams":[a],"to":null,"teams":both,"activeTeamId":"b"}));
        assert!(desktop.apply_team_server(&dir).unwrap());
        let original = Stores::open(&dir);
        assert_eq!(original.settings.get()["teams"], both);
        assert_eq!(original.shared_outbox.read(|o| o.arr("items").len()), 2);
        assert_eq!(original.team_cache.get()["teams"]["b"]["x"], 2);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
