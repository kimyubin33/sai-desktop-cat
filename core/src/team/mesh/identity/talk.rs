//! PC 톡의 확인된 작성 장치와 서명된 삭제 요청을 화면용으로 투영한다.
use super::{
    journal::{self, Body, Event, Stored},
    keys::unhex,
    model::{ApprovalPacket, MembershipState},
    session::local_member,
    vault::{db, load_packet, Vault},
};
use crate::{err, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub(super) enum Action {
    Post { created_at: i64 },
    Delete { target: String, sequence: u64 },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Retraction {
    pub target: String,
    pub sequence: u64,
}
pub(super) fn validate_header(h: &journal::Header) -> Result<()> {
    if let Some(r) = &h.retract {
        unhex::<32>(&r.target)?;
        if h.version != 2 || r.sequence == 0 || r.sequence >= h.sequence {
            return err("서명된 삭제 대상의 형식이 올바르지 않아요.");
        }
    }
    Ok(())
}
impl Action {
    pub(super) fn validate(&self, text: &str, sequence: u64) -> Result<()> {
        match self {
            Self::Post { created_at } => {
                if *created_at <= 0
                    || *created_at > 9_000_000_000_000_000
                    || text.trim().is_empty()
                    || text.chars().count() > 1000
                    || text
                        .chars()
                        .any(|c| c.is_control() && !['\n', '\r', '\t'].contains(&c))
                {
                    return err("PC 톡 내용을 1~1,000자로 입력해 주세요.");
                }
            }
            Self::Delete {
                target,
                sequence: n,
            } => {
                unhex::<32>(target)?;
                if !text.is_empty() || *n == 0 || *n >= sequence {
                    return err("삭제 대상의 원본 순번이 올바르지 않아요.");
                }
            }
        }
        Ok(())
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TalkScope {
    pub team_id: String,
    pub device_id: String,
    pub revision: u64,
    pub membership_head: String,
}
impl TalkScope {
    pub(super) fn check(&self, packet: &ApprovalPacket, own: &str) -> Result<()> {
        let state = packet.state()?;
        if self.team_id != packet.manifest.body.team_id
            || self.device_id != own
            || self.revision != state.revision
            || self.membership_head != state.head
        {
            return err("PC 톡의 팀·장치·승인 범위가 바뀌었어요. 새로 확인해 주세요.");
        }
        Ok(())
    }
}
pub(super) fn validate_local_action(
    action: &Action,
    records: &[Stored],
    _state: &MembershipState,
) -> Result<()> {
    if let Action::Delete { target, sequence } = action {
        let original = records
            .iter()
            .find(|r| &r.id == target && r.event.header.sequence == *sequence)
            .ok_or("이 장치가 작성한 PC 톡만 삭제 요청할 수 있어요.")?;
        if !matches!(
            journal::read_body_data(original)?.action,
            Some(Action::Post { .. })
        ) {
            return err("PC 톡 원문만 삭제 요청할 수 있어요.");
        }
        for r in records {
            if matches!(journal::read_body_data(r)?.action,Some(Action::Delete{target:ref id,..}) if id==target)
            {
                return err("이미 삭제 요청한 톡이에요. 목록을 새로 읽어 주세요.");
            }
        }
    }
    Ok(())
}
pub(super) struct Entry {
    pub event: Event,
    pub id: String,
    pub body: Option<Body>,
    pub state: &'static str,
    pub incoming: bool,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TalkMessage {
    pub event_id: String,
    pub author: String,
    pub member_id: String,
    pub sequence: u64,
    pub text: String,
    pub created_at: Option<i64>,
    pub state: &'static str,
    pub incoming: bool,
    pub deleted: bool,
    pub delete_state: Option<&'static str>,
    pub can_delete: bool,
    pub prepared: bool,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TalkFeed {
    pub team_id: String,
    pub team_name: String,
    pub scope: TalkScope,
    pub can_write: bool,
    pub pending: usize,
    pub stored: usize,
    pub received: usize,
    pub conflicts: usize,
    pub active_devices: Vec<String>,
    pub total: usize,
    pub offset: usize,
    pub next_offset: Option<usize>,
    pub messages: Vec<TalkMessage>,
}

pub(super) fn entries(
    vault: &Vault,
    conn: &rusqlite::Connection,
    packet: &ApprovalPacket,
    own: &str,
    records: &[Stored],
    states: &[&'static str],
) -> Result<Vec<Entry>> {
    let mut rows = Vec::new();
    for (r, state) in records.iter().zip(states) {
        rows.push(Entry {
            event: r.event.clone(),
            id: r.id.clone(),
            body: Some(journal::read_body_data(r)?),
            state,
            incoming: false,
        });
    }
    rows.extend(super::transfer::received_entries(vault, conn, packet, own)?);
    Ok(rows)
}
pub(super) fn projection(
    rows: &[Entry],
    packet: &ApprovalPacket,
    can_write: bool,
) -> Result<Vec<TalkMessage>> {
    let current = packet.state()?;
    let mut deletes = BTreeMap::new();
    let mut posts = BTreeMap::new();
    for row in rows {
        if row
            .body
            .as_ref()
            .is_some_and(|b| !matches!(b.action, Some(Action::Delete { .. })))
        {
            posts.insert(row.id.as_str(), row);
        }
    }
    for row in rows {
        if let Some(Retraction { target, sequence }) = &row.event.header.retract {
            // 대상 ID만으로 다른 작성 장치의 톡을 숨길 수 없다.
            if let Some(original) = posts.get(target.as_str()) {
                let a = &original.event.header;
                let b = &row.event.header;
                if original
                    .body
                    .as_ref()
                    .is_some_and(|b| matches!(b.action, Some(Action::Post { .. })))
                    && a.author == b.author
                    && a.sequence == *sequence
                    && a.sequence < b.sequence
                    && a.revision <= b.revision
                {
                    deletes.entry(target.as_str()).or_insert(row.state);
                }
            }
        }
    }
    let mut result = Vec::new();
    for row in rows {
        let Some(body) = &row.body else {
            continue;
        };
        if matches!(body.action, Some(Action::Delete { .. })) {
            continue;
        }
        let h = &row.event.header;
        let deleted = deletes.get(row.id.as_str()).copied();
        let prepared = body.action.is_none();
        let created_at = match &body.action {
            Some(Action::Post { created_at }) => Some(*created_at),
            _ => None,
        };
        result.push(TalkMessage {
            event_id: row.id.clone(),
            author: h.author.clone(),
            member_id: current
                .devices
                .get(&h.author)
                .ok_or("작성 장치의 승인 정보를 확인하지 못했어요.")?
                .member_id
                .clone(),
            sequence: h.sequence,
            text: if deleted.is_some() {
                String::new()
            } else {
                body.text.clone()
            },
            created_at,
            state: row.state,
            incoming: row.incoming,
            deleted: deleted.is_some(),
            delete_state: deleted,
            can_delete: can_write && !row.incoming && !prepared && deleted.is_none(),
            prepared,
        });
    }
    result.sort_by(|a, b| {
        (a.created_at.unwrap_or(0), &a.author, a.sequence).cmp(&(
            b.created_at.unwrap_or(0),
            &b.author,
            b.sequence,
        ))
    });
    Ok(result)
}
impl Vault {
    pub fn post_talk(&self, scope: &TalkScope, request: &str, text: &str) -> Result<String> {
        self.append_action(
            &scope.team_id,
            request,
            text,
            Some(Action::Post {
                created_at: crate::time::now_ms(),
            }),
            Some(scope),
        )
    }
    pub fn delete_talk(
        &self,
        scope: &TalkScope,
        request: &str,
        target: &str,
        sequence: u64,
    ) -> Result<String> {
        self.append_action(
            &scope.team_id,
            request,
            "",
            Some(Action::Delete {
                target: target.into(),
                sequence,
            }),
            Some(scope),
        )
    }
    pub fn talk_feed(&self, team: &str, offset: usize) -> Result<TalkFeed> {
        if offset > 2048 {
            return err("PC 톡 목록 위치를 확인해 주세요.");
        }
        let conn = self.required()?;
        let tx = db(conn.unchecked_transaction())?;
        let packet = load_packet(&tx, team)?.ok_or("PC 팀을 선택해 주세요.")?;
        let key = self.key(&tx, "device")?.ok_or("장치 키가 없어요.")?;
        let own = key.device()?.device_id;
        let current = packet.state()?;
        let records = journal::read_chain(&tx, &packet, &own)?;
        let can_write = records.len() < journal::MAX_EVENTS
            && local_member(self, &tx, team).is_ok()
            && !super::transfer::blocked(&tx, team, &own)?;
        let states = records
            .iter()
            .map(|r| super::transfer::outgoing_state(&tx, &packet, &r.event, &current))
            .collect::<Result<Vec<_>>>()?;
        let pending = states
            .iter()
            .filter(|s| ["local-pending", "approval-held", "legacy-held"].contains(s))
            .count();
        let rows = entries(self, &tx, &packet, &own, &records, &states)?;
        let received = rows
            .iter()
            .filter(|r| r.incoming && r.body.is_some())
            .count();
        let mut messages = projection(&rows, &packet, can_write)?;
        let total = messages.len();
        let end = total.saturating_sub(offset);
        let start = end.saturating_sub(50);
        messages = messages.drain(start..end).collect();
        let conflicts: i64 = db(tx.query_row(
            "SELECT count(*) FROM mesh_transfer_blocks WHERE team=?1",
            [team],
            |r| r.get(0),
        ))?;
        let active_devices = current
            .devices
            .iter()
            .filter(|(_, m)| m.active)
            .map(|(id, _)| id.clone())
            .collect();
        Ok(TalkFeed {
            team_id: team.into(),
            team_name: packet.manifest.body.name.clone(),
            scope: TalkScope {
                team_id: team.into(),
                device_id: own,
                revision: current.revision,
                membership_head: current.head,
            },
            can_write,
            pending,
            stored: records.len(),
            received,
            conflicts: conflicts as usize,
            active_devices,
            total,
            offset,
            next_offset: (start > 0).then_some(offset + 50),
            messages,
        })
    }
}

#[cfg(test)]
mod tests;
