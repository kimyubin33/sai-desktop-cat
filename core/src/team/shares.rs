//! 팀 Drive 함: 텔레그램 대화방처럼 파일(Google Drive 링크)과 짧은 메시지를 주고받는다.
use super::Team;
use crate::util::{clean, J};
use crate::{err, Result};
use serde_json::{json, Value};
use std::collections::HashMap;

const KEEP: usize = 300;

impl Team {
    /// 서버에서 새로 읽어 이 기기에도 둔다 (오래된 순). 서버에 닿지 않으면 둔 것을 돌려준다
    pub async fn shares(&self) -> (Vec<Value>, Option<String>) {
        let fresh = async {
            let team_id = self.team_id()?;
            self.backend()?.list_shares(&team_id).await
        }
        .await;
        match fresh {
            Ok(mut list) => {
                list.sort_by_key(|x| x.i("at"));
                let start = list.len().saturating_sub(KEEP);
                let list: Vec<Value> = list.split_off(start);
                let keep = list.clone();
                self.cache_update(|c| c["shares"] = json!(keep));
                (list, None)
            }
            Err(e) => {
                let cached = self.cache_get().arr("shares");
                let msg = if e.contains("403") || e.contains("PERMISSION") { "팀 서버 규칙에 아직 Drive 함이 없어요. Firebase 콘솔에서 firestore.rules 를 게시해 주세요.".to_string() } else { e };
                (cached, Some(msg))
            }
        }
    }

    /// 화면용: 보낸 사람 이름, 내 것인지
    pub fn shares_view(&self, list: &[Value]) -> Vec<Value> {
        let me = self.my_uid();
        let names: HashMap<String, String> = self.members(true).iter().map(|m| (m.s("uid"), m.s("displayName"))).collect();
        list.iter()
            .map(|x| {
                let mut v = x.clone();
                let by = x.s("by");
                v["byName"] = json!(names.get(&by).cloned().unwrap_or_else(|| "나간 팀원".into()));
                v["mine"] = json!(by == me);
                v
            })
            .collect()
    }

    /// 보내기: 파일(이름·링크)이나 메시지, 또는 둘 다
    pub async fn add_share(&self, input: &Value) -> Result<Value> {
        let team_id = self.team_id()?;
        let text = clean(&input.s("text"), 1000);
        let url = input.s("url");
        if text.trim().is_empty() && url.is_empty() {
            return err("보낼 내용이 없어요.");
        }
        if !url.is_empty() && !url.starts_with("https://") {
            return err("https:// 로 시작하는 링크만 보낼 수 있어요.");
        }
        let mut data = json!({ "text": text, "by": self.my_uid(), "at": self.now() });
        if !url.is_empty() {
            data["name"] = json!(clean(&input.s("name"), 200));
            data["url"] = json!(clean(&url, 500));
            data["driveId"] = json!(clean(&input.s("driveId"), 100));
            data["mimeType"] = json!(clean(&input.s("mimeType"), 100));
            data["size"] = json!(input.i("size").max(0));
            data["access"] = json!(match input.s("access").as_str() {
                "edit" => "edit",
                "view" => "view",
                _ => "link",
            });
        }
        let id = self.backend()?.add_share(&team_id, data.clone()).await.map_err(|e| {
            if e.contains("403") || e.contains("PERMISSION") {
                "팀 서버 규칙에 아직 Drive 함이 없어요. Firebase 콘솔에서 firestore.rules 를 게시해 주세요.".to_string()
            } else {
                e
            }
        })?;
        data["id"] = json!(id);
        let d2 = data.clone();
        self.cache_update(|c| {
            let mut l = c.arr("shares");
            l.push(d2);
            c["shares"] = json!(l);
        });
        Ok(data)
    }

    pub async fn delete_share(&self, id: &str) -> Result<()> {
        let team_id = self.team_id()?;
        self.backend()?.delete_share(&team_id, id).await?;
        self.cache_update(|c| {
            let l: Vec<Value> = c.arr("shares").into_iter().filter(|x| x.s("id") != id).collect();
            c["shares"] = json!(l);
        });
        Ok(())
    }

    pub fn share(&self, id: &str) -> Option<Value> {
        self.cache_get().arr("shares").into_iter().find(|x| x.s("id") == id)
    }
}

#[cfg(test)]
mod tests {
    use crate::store::Stores;
    use crate::team::{Backend, Hub};
    use crate::util::J;
    use serde_json::json;
    use std::sync::Arc;

    fn hub(shared: Arc<std::sync::Mutex<serde_json::Value>>, uid: &str) -> Hub {
        let st = Stores::memory();
        let u = uid.to_string();
        Hub::new(st.settings, st.shared_outbox, st.team_cache, Box::new(move || Some(Backend::memory_shared(shared.clone(), &u))), Some(0), Arc::new(crate::time::now_ms))
    }

    #[tokio::test]
    async fn send_and_receive_in_team_box() {
        let shared = Backend::new_shared_memory();
        let a = hub(shared.clone(), "ua");
        let b = hub(shared.clone(), "ub");
        let t = a.create_team("팀", "유빈").await.unwrap();
        b.join_team(t["joinCode"].as_str().unwrap(), "민수").await.unwrap();
        let ta = a.active();
        ta.add_share(&json!({ "text": "기획안 올려요", "name": "기획안.pdf", "url": "https://drive.google.com/file/d/abcdefghijkl/view", "access": "edit" })).await.unwrap();
        ta.add_share(&json!({ "text": "확인 부탁해요" })).await.unwrap();
        assert!(ta.add_share(&json!({ "text": "" })).await.is_err());
        assert!(ta.add_share(&json!({ "url": "http://x" })).await.is_err());
        let tb = b.active();
        b.sync_team(&tb).await;
        let (list, e) = tb.shares().await;
        assert!(e.is_none());
        assert_eq!(list.len(), 2);
        let v = tb.shares_view(&list);
        assert_eq!(v[0].s("name"), "기획안.pdf");
        assert_eq!(v[0].s("byName"), "유빈");
        assert!(!v[0].b("mine"));
        assert!(v[1]["url"].is_null());
        ta.delete_share(&list[1].s("id")).await.unwrap();
        assert_eq!(tb.shares().await.0.len(), 1);
    }
}
