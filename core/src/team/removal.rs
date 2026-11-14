//! 내 기기의 정리함 기록 삭제와 작성자의 팀 전체 철회를 구분한다.
use super::{Hub, Team};
use crate::util::{arr_mut, J};
use crate::{err, Result};
use serde_json::{json, Value};

fn delivered(row: &Value) -> bool {
    matches!(row.s("status").as_str(), "sent" | "held")
}

impl Hub {
    /// 네트워크가 복구되어 발송하는 순간과 취소 버튼을 누르는 순간이 겹쳐도 결과를 구분한다.
    /// 팀에서 나갔거나 팀 연결 전에 저장한 항목도 개인 대기함에서 지울 수 있다.
    pub async fn remove_outbox(&self, team_id: Option<&str>, id: &str, was_delivered: bool) -> Result<()> {
        let lock = self.lock_for(team_id.unwrap_or("_none"));
        let _guard = lock.lock().await;
        self.0.outbox.update(|d| {
            let rows = arr_mut(d, "items");
            let pos = rows.iter().position(|row| row["id"] == id && row["teamId"].as_str() == team_id)
                .ok_or("공유 대기함 항목을 찾을 수 없어요. 다시 열어 주세요.")?;
            if delivered(&rows[pos]) != was_delivered {
                return err("이미 전송되어 대기 취소 시간이 지났어요. 정리함을 다시 열어 상태를 확인해 주세요.");
            }
            rows.remove(pos);
            Ok(())
        })
    }
}

impl Team {
    /// 동기화로 받은 목록을 새로 써도 이 기기에서 지운 항목은 다시 나타나지 않는다.
    pub fn visible_advice(&self) -> Vec<Value> {
        let c = self.cache_get();
        let uid = c.s("myUid");
        c.arr("inbox").into_iter().filter(|r| !c["hiddenAdvice"][&uid][r.s("id")].as_bool().unwrap_or(false)).collect()
    }

    pub fn hide_received(&self, id: &str, is_advice: bool) -> Result<()> {
        let uid = self.my_uid();
        let visible = if is_advice { self.visible_advice() } else { self.proposals() };
        if uid.is_empty() || !visible.iter().any(|r| r["id"] == id) {
            return err("이 팀에서 삭제할 항목을 찾을 수 없어요.");
        }
        let key = if is_advice { "hiddenAdvice" } else { "hiddenProposals" };
        self.cache_update(|c| c[key][&uid][id] = json!(true));
        Ok(())
    }

    pub async fn withdraw_promise(&self, id: &str) -> Result<()> {
        let tid = self.team_id()?;
        let lock = self.hub.lock_for(&tid);
        let _guard = lock.lock().await;
        let current = self.proposals().into_iter().find(|p| p["id"] == id).ok_or("이 팀에서 해당 약속을 찾을 수 없어요.")?;
        if !current.b("custom") || current["by"] != self.my_uid() {
            return err("직접 만든 팀 약속만 팀 전체에서 삭제할 수 있어요.");
        }
        self.backend()?.delete_owned_promise(&tid, id).await?;
        self.cache_update(|c| {
            for key in ["proposals", "shares"] {
                arr_mut(c, key).retain(|r| r["id"] != id);
            }
        });
        self.outbox_update(|rows| rows.retain(|r| !(r["remoteId"] == id && r["target"] == "team")));
        Ok(())
    }

    pub async fn withdraw_advice(&self, item_id: &str) -> Result<()> {
        let tid = self.team_id()?;
        let lock = self.hub.lock_for(&tid);
        let _guard = lock.lock().await;
        let row = self.outbox_items().into_iter()
            .find(|r| r["id"] == item_id && r.b("custom") && r["target"] == "person" && delivered(r))
            .ok_or("이 팀에서 철회할 개인 전달 문안을 찾을 수 없어요.")?;
        let remote = row["remoteId"].as_str().filter(|id| !id.is_empty()).ok_or("전달 정보가 없어요.")?;
        self.backend()?.delete_personal_delivery(&tid, remote).await?;
        self.outbox_update(|rows| rows.retain(|r| r["id"] != item_id));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::team::{tests::hub_for, Backend};

    #[tokio::test]
    async fn hiding_legacy_entries_stays_local_after_sync() {
        let data = Backend::new_shared_memory();
        let (a, _) = hub_for(data.clone(), "one");
        let (b, _) = hub_for(data, "two");
        let team = a.create_team("시험", "가").await.unwrap();
        b.join_team(&team.s("joinCode"), "나").await.unwrap();
        let first = a.active();
        let second = b.active();
        first.propose_team("workload").unwrap();
        first.sync().await;
        let db = first.backend().unwrap();
        db.create_signal(&team.s("teamId"), "time_promise", "two", &crate::time::iso_week(a.now())).await.unwrap();
        second.sync().await;
        let promise = second.proposals()[0].s("id");
        let message = second.visible_advice()[0].s("id");
        second.hide_received(&promise, false).unwrap();
        second.hide_received(&message, true).unwrap();
        second.sync().await;
        assert!(second.proposals().is_empty());
        assert!(second.visible_advice().is_empty());
        assert!(second.boosts().is_empty());
        assert!(second.pending_proposals_for_me().is_empty());
        first.sync().await;
        assert_eq!(first.proposals()[0]["id"], promise);
        assert!(second.hide_received("elsewhere", false).is_err());
    }

    #[tokio::test]
    async fn only_sender_can_withdraw_approved_text_after_delivery() {
        let data = Backend::new_shared_memory();
        let (author, _) = hub_for(data.clone(), "author");
        let (peer, _) = hub_for(data, "peer");
        let info = author.create_team("팀", "작성자").await.unwrap();
        peer.join_team(&info.s("joinCode"), "팀원").await.unwrap();
        let own = author.active();
        let other = peer.active();
        own.sync().await;
        let mut draft = json!({"kind":"draft", "title":"자료 점검", "text":"매주 수요일에 각자 자료를 확인해요.", "target":"team", "topic":"workload"});
        let proposal = own.queue_ai_delivery("norm", &draft, None).unwrap();
        draft["target"] = json!("person");
        let message = own.queue_ai_delivery("private", &draft, Some("peer")).unwrap();
        own.sync().await;
        own.sync().await;
        other.sync().await;
        assert_eq!(other.proposals()[0]["id"], proposal);
        assert_eq!(other.visible_advice()[0]["id"], message);
        assert!(other.withdraw_promise(&proposal).await.is_err());
        assert!(other.withdraw_advice(&message).await.is_err());
        assert!(other.backend().unwrap().delete_owned_promise(&info.s("teamId"), &proposal).await.is_err());
        assert!(other.backend().unwrap().delete_personal_delivery(&info.s("teamId"), &message).await.is_err());
        own.withdraw_promise(&proposal).await.unwrap();
        own.withdraw_advice(&message).await.unwrap();
        other.sync().await;
        assert!(other.proposals().is_empty());
        assert!(other.visible_advice().is_empty());
        assert!(own.outbox_items().is_empty());
    }

    #[tokio::test]
    async fn closed_team_is_removed_on_peer_sync_without_touching_personal_sessions() {
        let data = Backend::new_shared_memory();
        let (owner, own_store) = hub_for(data.clone(), "creator");
        let (member, peer_store) = hub_for(data, "peer");
        own_store.private_conv.update(|s| s["sessions"] = json!([{"id":"own"}]));
        peer_store.private_conv.update(|s| s["sessions"] = json!([{"id":"peer"}]));
        let t = owner.create_team("종료", "개설자").await.unwrap();
        member.join_team(&t.s("joinCode"), "팀원").await.unwrap();
        let other_team = member.create_team("활동 팀", "팀원").await.unwrap();
        let id = t.s("teamId");
        let member_closed = member.team(Some(&id)).unwrap();
        assert!(owner.dissolve_team(&id, "다른 팀").await.is_err());
        assert!(member_closed.backend().unwrap().dissolve_team(&id, &t.s("joinCode")).await.is_err());
        owner.dissolve_team(&id, "종료").await.unwrap();
        assert!(owner.team(Some(&id)).is_none());
        assert!(!member_closed.sync().await.b("ok"));
        assert!(member.team(Some(&id)).is_none());
        assert_eq!(member.active().id.as_deref(), Some(other_team.s("teamId").as_str()));
        assert_eq!(own_store.private_conv.get()["sessions"][0]["id"], "own");
        assert_eq!(peer_store.private_conv.get()["sessions"][0]["id"], "peer");
    }

    #[tokio::test]
    async fn cancel_and_history_removal_check_team_and_delivery_state() {
        let data = Backend::new_shared_memory();
        let (hub, _) = hub_for(data, "sender");
        let team = hub.create_team("A", "가").await.unwrap();
        let a = hub.active();
        let id = a.propose_team("workload").unwrap();
        assert!(hub.remove_outbox(Some("B"), &id, false).await.is_err());
        let tid = team.s("teamId");
        a.sync().await;
        assert!(hub.remove_outbox(Some(&tid), &id, false).await.is_err());
        assert_eq!(a.outbox_items().len(), 1);
        hub.remove_outbox(Some(&tid), &id, true).await.unwrap();
        assert!(a.outbox_items().is_empty());
        let id = a.propose_team("workload").unwrap();
        hub.remove_outbox(Some(&tid), &id, false).await.unwrap();
        a.sync().await;
        assert!(a.outbox_items().is_empty());
    }
}
