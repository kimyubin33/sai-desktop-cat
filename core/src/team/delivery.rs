//! 승인한 AI 문안의 보관·전달. 외부 AI 연결은 생성 단계에서만 사용한다.
use super::{Backend, Team};
use crate::time::iso_week;
use crate::topics::topic;
use crate::util::J;
use crate::{err, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub(super) fn is_delivery_id(id: &str) -> bool {
    id.strip_prefix("aip-").is_some_and(|part| part.len() == 64 && part.bytes().all(|c| c.is_ascii_hexdigit()))
}

impl Team {
    /// draft_id는 승인 화면과 함께 저장한 고유 번호다. 같은 초안의 재시도는 한 번만 등록한다.
    pub fn queue_ai_delivery(&self, draft_id: &str, draft: &Value, recipient: Option<&str>) -> Result<String> {
        let team = self.team_id()?;
        let author = self.my_uid();
        if author.is_empty() { return err("팀 계정을 동기화한 뒤 다시 시도해 주세요."); }
        if draft_id.is_empty() || draft_id.len() > 96 || !draft_id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_') {
            return err("AI 초안 번호를 확인할 수 없어요.");
        }
        let title = draft.s("title");
        let text = draft.s("text");
        let target = draft.s("target");
        let subject = draft.s("topic");
        if draft["kind"] != "draft" || topic(&subject).is_none() || title.trim().is_empty() || title.contains(['\r', '\n'])
            || title.chars().count() > 80 || text.trim().is_empty() || text.chars().count() > 400 {
            return err("제목과 내용이 있는 AI 초안을 선택해 주세요.");
        }
        let to = match target.as_str() {
            "person" => {
                let to = recipient.ok_or("받을 팀원을 선택해 주세요.")?;
                if !self.members(false).iter().any(|m| m["uid"] == to) { return err("현재 팀에서 받을 팀원을 선택해 주세요."); }
                Some(to)
            }
            "team" => None,
            _ => return err("전달할 대상을 선택해 주세요."),
        };
        let now = self.now();
        let id = format!("aip-{:x}", Sha256::digest(format!("{team}/{author}/{draft_id}")));
        let after = if to.is_some() { self.send_after() } else { now };
        self.outbox_update(|rows| {
            if let Some(old) = rows.iter().find(|r| r["id"] == id) {
                if old["toMember"] != json!(to) || old["target"] != target || old["text"] != text || old["title"] != title {
                    return err("이미 다른 내용이나 대상으로 저장한 초안이에요. 새로 작성해 주세요.");
                }
                return Ok(id);
            }
            rows.push(json!({"id":id,"custom":true,"author":author,"target":target,"scope":"review","topic":subject,
                "title":title,"text":text,"label":title,"advice":text,"teamProposal":text,"toMember":to,
                "createdAt":now,"week":iso_week(now),"status":"queued","sendAfter":after}));
            Ok(id)
        })
    }

    pub(super) async fn send_custom(&self, backend: &Backend, team: &str, row: &Value) -> Result<String> {
        let id = row.s("id");
        if !is_delivery_id(&id) || row["author"] != backend.sign_in().await? { return err("이 문안을 승인한 계정으로 팀에 연결해 주세요."); }
        match row.s("target").as_str() {
            "team" => backend.deliver_promise(team, &id, row).await?,
            "person" => backend.deliver_private(team, &id, row).await?,
            _ => return err("받는 대상이 올바르지 않아요."),
        }
        Ok(id)
    }
}

/// 정리함과 말풍선은 일반 팀 톡에도 표시되는 같은 문안을 읽는다. 철회된 제안은 표시하지 않는다.
pub(super) async fn resolve_proposals(b: &Backend, team: &str, raw: Vec<Value>) -> Result<Vec<Value>> {
    let mut out = Vec::with_capacity(raw.len());
    for mut row in raw {
        if row["topic"] == "ai_promise" {
            let id = row.s("id");
            if !is_delivery_id(&id) { continue; }
            let Some(post) = b.get_share(team, &id).await? else { continue };
            let message = post.s("text");
            let Some((title, text)) = message.strip_prefix("[팀 약속 제안] ").and_then(|s| s.split_once('\n')) else { continue };
            row["custom"] = json!(true);
            row["label"] = json!(title);
            row["text"] = json!(text);
            row["by"] = post["by"].clone();
        }
        out.push(row);
    }
    Ok(out)
}

pub(super) async fn resolve_inbox(b: &Backend, team: &str, raw: Vec<Value>) -> Result<Vec<Value>> {
    let mut out = Vec::with_capacity(raw.len());
    for row in raw {
        let id = row.s("id");
        let mut item = json!({"id":id,"topic":row["topic"],"week":row["week"],"held":row["held"]});
        if is_delivery_id(&id) {
            let Some(body) = b.get_private_body(team, &id).await? else { continue };
            item["title"] = body["title"].clone();
            item["text"] = body["text"].clone();
            item["custom"] = json!(true);
        }
        out.push(item);
    }
    Ok(out)
}
