//! 로컬 원본 로그·발신 의무. 원본 본문은 DPAPI로 보관하고 전송은 transfer가 담당한다.
use super::keys::{digest, random_id, signed_bytes, unhex, verify};
use super::model::ApprovalPacket;
use super::session::local_member;
use super::vault::{db, load_packet, Vault};
use crate::{err, Result};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

pub(super) const DOMAIN: &str = "sai-mesh/local-event/1";
pub const MAX_TEXT: usize = 32768;
pub(super) const MAX_EVENTS: usize = 1024;

pub(super) fn migrate(conn: &Connection) -> Result<()> {
    db(conn.execute_batch("CREATE TABLE mesh_local_events(team TEXT NOT NULL REFERENCES mesh_teams(team_id),
        author TEXT NOT NULL, sequence INTEGER NOT NULL CHECK(sequence>0), id TEXT NOT NULL,
        request TEXT NOT NULL, event TEXT NOT NULL, sealed BLOB NOT NULL CHECK(length(sealed)<=131072),
        PRIMARY KEY(team,author,sequence), UNIQUE(team,id), UNIQUE(team,author,request));
        CREATE TABLE mesh_local_heads(team TEXT NOT NULL, author TEXT NOT NULL, sequence INTEGER NOT NULL,
        id TEXT NOT NULL, PRIMARY KEY(team,author));
        CREATE TABLE mesh_local_outbox(team TEXT NOT NULL, id TEXT NOT NULL, PRIMARY KEY(team,id),
        FOREIGN KEY(team,id) REFERENCES mesh_local_events(team,id)); PRAGMA user_version=2;"))
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Header {
    pub version: u8,
    pub team: String,
    pub author: String,
    pub sequence: u64,
    pub parent: String,
    pub request: String,
    pub revision: u64,
    pub membership_head: String,
    pub recipients: Vec<String>,
    pub content_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retract: Option<super::talk::Retraction>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Event {
    pub header: Header,
    pub signature: String,
}

impl Event {
    pub fn id(&self) -> Result<String> {
        Ok(digest(
            "sai-mesh/local-event-id/1",
            &signed_bytes(DOMAIN, self)?,
        ))
    }

    pub fn validate(&self, packet: &ApprovalPacket) -> Result<()> {
        let h = &self.header;
        let revision = usize::try_from(h.revision).map_err(|_| "승인 이력 범위가 달라요.")?;
        if revision == 0 || revision > packet.events.len() {
            return err("확인하지 않은 승인 이력이에요.");
        }
        let state = ApprovalPacket {
            version: 1,
            manifest: packet.manifest.clone(),
            events: packet.events[..revision].to_vec(),
        }
        .state()?;
        self.validate_state(packet, &state)
    }

    pub(super) fn validate_state(
        &self,
        packet: &ApprovalPacket,
        state: &super::model::MembershipState,
    ) -> Result<()> {
        let h = &self.header;
        let revision = usize::try_from(h.revision).map_err(|_| "승인 이력 범위가 달라요.")?;
        if ![1, 2].contains(&h.version)
            || h.team != packet.manifest.body.team_id
            || h.sequence == 0
            || h.sequence > MAX_EVENTS as u64
            || state.revision != h.revision
            || revision == 0
            || revision > packet.events.len()
        {
            return err("호환되는 톡 원본이 아니에요.");
        }
        unhex::<16>(&h.request)?;
        unhex::<32>(&h.parent)?;
        unhex::<32>(&h.content_hash)?;
        super::talk::validate_header(h)?;
        let member = state
            .devices
            .get(&h.author)
            .filter(|m| m.active)
            .ok_or("승인되지 않은 작성 장치예요.")?;
        let recipients: Vec<_> = state
            .devices
            .iter()
            .filter(|(_, m)| m.active)
            .map(|(id, _)| id.clone())
            .collect();
        if h.membership_head != state.head || h.recipients != recipients {
            return err("톡의 수신 장치와 승인 이력이 달라요.");
        }
        verify(&member.device.signing_key, DOMAIN, h, &self.signature)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Body {
    pub version: u8,
    pub event_id: String,
    pub salt: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<super::talk::Action>,
}

impl Body {
    pub fn validate(&self, event: &Event) -> Result<()> {
        match (&self.action, self.version) {
            (None, 1) => validate_text(&self.text)?,
            (Some(action), 2) => action.validate(&self.text, event.header.sequence)?,
            _ => return err("지원하지 않는 PC 톡 본문 형식이에요."),
        }
        let retract = match &self.action {
            Some(super::talk::Action::Delete { target, sequence }) => {
                Some(super::talk::Retraction {
                    target: target.clone(),
                    sequence: *sequence,
                })
            }
            _ => None,
        };
        if event.header.retract != retract {
            return err("삭제 요청 본문과 서명된 대상이 달라요.");
        }
        unhex::<16>(&self.salt)?;
        if self.version != event.header.version
            || self.event_id != event.id()?
            || body_hash(&self.salt, &self.text, &self.action)? != event.header.content_hash
        {
            return err("톡 본문과 서명된 원본 기록이 일치하지 않아요.");
        }
        Ok(())
    }
}

impl Drop for Body {
    fn drop(&mut self) {
        zeroize::Zeroize::zeroize(&mut self.text);
        zeroize::Zeroize::zeroize(&mut self.salt);
    }
}

fn content_hash(salt: &str, text: &str) -> String {
    let plain = Zeroizing::new(format!("{salt}\0{text}"));
    digest("sai-mesh/local-content/1", plain.as_bytes())
}
fn body_hash(salt: &str, text: &str, action: &Option<super::talk::Action>) -> Result<String> {
    if action.is_none() {
        return Ok(content_hash(salt, text));
    }
    let plain = Zeroizing::new(signed_bytes("sai-mesh/talk-body/2", &(salt, text, action))?);
    Ok(digest("sai-mesh/talk-content/2", &plain))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalMessage {
    pub event_id: String,
    pub sequence: u64,
    pub text: String,
    pub state: &'static str,
    pub recipient_count: usize,
    pub author: String,
    pub incoming: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalStatus {
    pub team_id: String,
    pub can_write: bool,
    pub stored: usize,
    pub pending: usize,
    pub received: usize,
    pub write_blocked: bool,
    pub conflicts: usize,
    pub messages: Vec<LocalMessage>,
}

pub(super) struct Stored {
    pub event: Event,
    pub id: String,
    pub sealed: Vec<u8>,
}

pub(super) fn origin(team: &str, author: &str) -> String {
    digest(
        "sai-mesh/local-origin/1",
        format!("{team}:{author}").as_bytes(),
    )
}

fn validate_text(text: &str) -> Result<()> {
    if text.trim().is_empty()
        || text.len() > MAX_TEXT
        || text
            .chars()
            .any(|c| c.is_control() && !['\n', '\r', '\t'].contains(&c))
    {
        return err("톡 내용을 32 KiB 이내로 입력해 주세요.");
    }
    Ok(())
}

pub(super) fn read_body_data(stored: &Stored) -> Result<Body> {
    let plain = Zeroizing::new(
        crate::secret::unprotect_device_secret(&stored.sealed).map_err(|_| {
            "이 Windows 사용자 계정에서 보관한 톡 본문을 열지 못했어요. 기존 원본을 유지했어요."
        })?,
    );
    let body: Body = serde_json::from_slice(&plain).map_err(|_| "보관한 톡 본문이 손상됐어요.")?;
    body.validate(&stored.event)?;
    Ok(body)
}

pub(super) fn read_chain(
    conn: &Connection,
    packet: &ApprovalPacket,
    author: &str,
) -> Result<Vec<Stored>> {
    let team = &packet.manifest.body.team_id;
    packet.state()?;
    let mut query = db(conn.prepare("SELECT sequence,id,request,event,sealed FROM mesh_local_events WHERE team=?1 AND author=?2 ORDER BY sequence LIMIT 1025"))?;
    let rows = db(query.query_map(params![team, author], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, Vec<u8>>(4)?,
        ))
    }))?;
    let mut records = Vec::new();
    let mut parent = origin(team, author);
    let mut history = std::collections::BTreeMap::new();
    for row in rows {
        let (sequence, id, request, encoded, sealed) = db(row)?;
        if encoded.len() > 65536 || sealed.len() > 131072 || records.len() >= MAX_EVENTS {
            return err("로컬 톡 원본의 크기 한도를 넘었어요.");
        }
        let event: Event =
            serde_json::from_str(&encoded).map_err(|_| "로컬 톡 서명 자료가 손상됐어요.")?;
        let h = &event.header;
        let revision =
            usize::try_from(h.revision).map_err(|_| "승인 이력 범위를 확인하지 못했어요.")?;
        if ![1, 2].contains(&h.version)
            || &h.team != team
            || h.author != author
            || h.sequence != records.len() as u64 + 1
            || u64::try_from(sequence).ok() != Some(h.sequence)
            || h.parent != parent
            || h.request != request
            || revision > packet.events.len()
            || revision == 0
            || id != event.id()?
        {
            return err("로컬 톡 원본의 순서·연결·서명을 확인하지 못했어요. 덮어쓰지 않았어요.");
        }
        unhex::<16>(&h.request)?;
        unhex::<32>(&h.content_hash)?;
        super::talk::validate_header(h)?;
        if !history.contains_key(&revision) {
            history.insert(
                revision,
                ApprovalPacket {
                    version: 1,
                    manifest: packet.manifest.clone(),
                    events: packet.events[..revision].to_vec(),
                }
                .state()?,
            );
        }
        let previous = &history[&revision];
        let member = previous
            .devices
            .get(author)
            .filter(|m| m.active)
            .ok_or("승인되지 않은 장치의 톡 원본이에요.")?;
        let recipients: Vec<_> = previous
            .devices
            .iter()
            .filter(|(_, m)| m.active)
            .map(|(id, _)| id.clone())
            .collect();
        if previous.head != h.membership_head || h.recipients != recipients {
            return err("톡 원본의 수신 장치와 승인 이력이 일치하지 않아요.");
        }
        verify(&member.device.signing_key, DOMAIN, h, &event.signature)?;
        parent = id.clone();
        super::transfer::send_enabled(conn, team, &id, &member.device)?;
        records.push(Stored { event, id, sealed });
    }
    let head: Option<(i64, String)> = db(conn
        .query_row(
            "SELECT sequence,id FROM mesh_local_heads WHERE team=?1 AND author=?2",
            params![team, author],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional())?;
    match (head, records.last()) {
        (None, None) => {}
        (Some((n, id)), Some(last))
            if u64::try_from(n).ok() == Some(last.event.header.sequence) && id == last.id => {}
        _ => {
            return err(
                "로컬 톡의 마지막 저장 기록이 달라요. 장치 순번을 자동 초기화하지 않았어요.",
            )
        }
    }
    let queued: i64 = db(conn.query_row("SELECT count(*) FROM mesh_local_outbox o JOIN mesh_local_events e ON e.team=o.team AND e.id=o.id WHERE e.team=?1 AND e.author=?2",params![team,author],|r|r.get(0)))?;
    if queued != records.len() as i64 {
        return err("톡 원본과 발신 대기함이 일치하지 않아요.");
    }
    // 과거 원본은 장치가 해제되어도 읽을 수 있다. 새 쓰기는 별도로 현재 승인을 확인한다.
    Ok(records)
}

impl Vault {
    pub fn local_draft_id() -> Result<String> {
        random_id()
    }

    pub fn append_local(&self, team: &str, request: &str, text: &str) -> Result<String> {
        self.append_action(team, request, text, None, None)
    }

    pub(super) fn append_action(
        &self,
        team: &str,
        request: &str,
        text: &str,
        action: Option<super::talk::Action>,
        scope: Option<&super::talk::TalkScope>,
    ) -> Result<String> {
        unhex::<16>(request)?;
        if action.is_none() {
            validate_text(text)?;
        }
        let mut conn = self.required()?;
        let tx = db(conn.transaction_with_behavior(TransactionBehavior::Immediate))?;
        let (key, packet) = local_member(self, &tx, team)?;
        let author = key.device()?.device_id;
        if let Some(scope) = scope {
            scope.check(&packet, &author)?;
        }
        if super::transfer::blocked(&tx, team, &author)? {
            return err(
                "장치 기록의 충돌이 확인되어 새 저장을 보류했어요. 기존 원본을 유지했어요.",
            );
        }
        let records = read_chain(&tx, &packet, &author)?;
        if let Some(existing) = records.iter().find(|r| r.event.header.request == request) {
            let body = read_body_data(existing)?;
            let same_action = body.action == action
                || matches!(
                    (&body.action, &action),
                    (
                        Some(super::talk::Action::Post { .. }),
                        Some(super::talk::Action::Post { .. })
                    )
                );
            if body.text != text || !same_action {
                return err("같은 저장 요청의 내용이 달라요. 기존 톡을 유지했어요.");
            }
            return Ok(existing.id.clone());
        }
        if records.len() >= MAX_EVENTS {
            return err("준비 버전의 로컬 톡 한도에 도달했어요.");
        }
        let state = packet.state()?;
        if let Some(action) = &action {
            action.validate(text, records.len() as u64 + 1)?;
            super::talk::validate_local_action(action, &records, &state)?;
        }
        let salt = random_id()?;
        let header = Header {
            version: if action.is_some() { 2 } else { 1 },
            team: team.into(),
            author: author.clone(),
            sequence: records.len() as u64 + 1,
            parent: records
                .last()
                .map(|r| r.id.clone())
                .unwrap_or_else(|| origin(team, &author)),
            request: request.into(),
            revision: state.revision,
            membership_head: state.head,
            recipients: state
                .devices
                .into_iter()
                .filter(|(_, m)| m.active)
                .map(|(id, _)| id)
                .collect(),
            content_hash: body_hash(&salt, text, &action)?,
            retract: match &action {
                Some(super::talk::Action::Delete { target, sequence }) => {
                    Some(super::talk::Retraction {
                        target: target.clone(),
                        sequence: *sequence,
                    })
                }
                _ => None,
            },
        };
        let event = Event {
            signature: key.sign(DOMAIN, &header)?,
            header,
        };
        let id = event.id()?;
        let body = Body {
            version: event.header.version,
            event_id: id.clone(),
            salt,
            text: text.into(),
            action,
        };
        let plain =
            Zeroizing::new(serde_json::to_vec(&body).map_err(|_| "톡을 저장하지 못했어요.")?);
        drop(body);
        let sealed = crate::secret::protect_device_secret(&plain)
            .map_err(|_| "Windows에서 톡 본문을 보호하지 못했어요. 저장하지 않았어요.")?;
        let encoded = serde_json::to_string(&event).map_err(|_| "톡 원본을 저장하지 못했어요.")?;
        let sequence =
            i64::try_from(event.header.sequence).map_err(|_| "장치 순번 한도를 넘었어요.")?;
        db(tx.execute("INSERT INTO mesh_local_events(team,author,sequence,id,request,event,sealed) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![team,author,sequence,id,request,encoded,sealed]))?;
        db(tx.execute("INSERT INTO mesh_local_heads(team,author,sequence,id) VALUES(?1,?2,?3,?4) ON CONFLICT(team,author) DO UPDATE SET sequence=excluded.sequence,id=excluded.id",params![team,author,sequence,id]))?;
        db(tx.execute(
            "INSERT INTO mesh_local_outbox(team,id) VALUES(?1,?2)",
            params![team, id],
        ))?;
        let public = serde_json::to_string(&key.device()?)
            .map_err(|_| "발신 장치 정보를 저장하지 못했어요.")?;
        let intent = key.sign("sai-mesh/send-intent/1", &(team, id.as_str()))?;
        db(tx.execute("INSERT INTO mesh_local_send_policy(team,id,enabled,public,proof) VALUES(?1,?2,1,?3,?4)",params![team,id,public,intent]))?;
        db(tx.commit())?;
        Ok(id)
    }

    pub fn local_status(&self, team: &str) -> Result<LocalStatus> {
        let conn = self.required()?;
        let tx = db(conn.unchecked_transaction())?;
        let packet = load_packet(&tx, team)?.ok_or("PC 팀을 먼저 선택해 주세요.")?;
        let device = self
            .key(&tx, "device")?
            .ok_or("먼저 내 장치를 만들어 주세요.")?
            .device()?;
        let records = read_chain(&tx, &packet, &device.device_id)?;
        let state = packet.state()?;
        let can_write = state
            .devices
            .get(&device.device_id)
            .is_some_and(|m| m.active);
        let write_blocked = super::transfer::blocked(&tx, team, &device.device_id)?;
        let conflicts: i64 = db(tx.query_row(
            "SELECT count(*) FROM mesh_transfer_blocks WHERE team=?1",
            [team],
            |r| r.get(0),
        ))?;
        let mut pending = 0;
        let mut states = Vec::new();
        for stored in &records {
            let status = super::transfer::outgoing_state(&tx, &packet, &stored.event, &state)?;
            if status == "local-pending" || status == "approval-held" || status == "legacy-held" {
                pending += 1;
            }
            states.push(status);
        }
        let rows = super::talk::entries(self, &tx, &packet, &device.device_id, &records, &states)?;
        let received = rows
            .iter()
            .filter(|r| r.incoming && r.body.is_some())
            .count();
        let projected = super::talk::projection(&rows, &packet, can_write && !write_blocked)?;
        let mut messages = Vec::new();
        for incoming in [false, true] {
            let selected: Vec<_> = projected
                .iter()
                .filter(|m| m.incoming == incoming)
                .rev()
                .take(50)
                .collect();
            for row in selected.into_iter().rev() {
                let original = rows
                    .iter()
                    .find(|r| r.id == row.event_id)
                    .ok_or("PC 톡의 원본을 확인하지 못했어요.")?;
                messages.push(LocalMessage {
                    event_id: row.event_id.clone(),
                    sequence: row.sequence,
                    text: if row.deleted {
                        "삭제 요청을 확인한 톡이에요.".into()
                    } else {
                        row.text.clone()
                    },
                    state: row.state,
                    recipient_count: original.event.header.recipients.len(),
                    author: row.author.clone(),
                    incoming,
                });
            }
        }
        Ok(LocalStatus {
            team_id: team.into(),
            can_write: can_write && !write_blocked,
            stored: records.len(),
            pending,
            received,
            write_blocked,
            conflicts: usize::try_from(conflicts).map_err(|_| "충돌 기록 수가 올바르지 않아요.")?,
            messages,
        })
    }
}
