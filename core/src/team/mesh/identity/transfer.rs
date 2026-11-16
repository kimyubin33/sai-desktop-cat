//! 직접 연결한 승인 장치 사이의 본문 암호화와 디스크 저장 확인.
use super::{
    crypt::{self, CipherBox},
    journal::{self, Body, Event},
    keys::{digest, signed_bytes, unhex, verify, Material},
    model::ApprovalPacket,
    session::local_member,
    vault::{db, Vault},
};
use crate::{err, Result};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use zeroize::Zeroizing;

const DELIVERY: &str = "sai-mesh/delivery/1";
const RECEIPT: &str = "sai-mesh/receipt/1";
const FRONTIER: &str = "sai-mesh/frontier/1";
pub(crate) const MAX_FRAME: usize = 131072;

pub(super) fn migrate(conn: &Connection) -> Result<()> {
    let schema: String = db(conn.query_row(
        "SELECT sql FROM sqlite_master WHERE name='mesh_local_events'",
        [],
        |r| r.get(0),
    ))?;
    if schema
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .contains("length(sealed)<=65536")
    {
        db(conn.execute_batch("CREATE TEMP TABLE mesh_v2_outbox AS SELECT team,id FROM mesh_local_outbox;
            CREATE TABLE mesh_events_v3(team TEXT NOT NULL REFERENCES mesh_teams(team_id),author TEXT NOT NULL,sequence INTEGER NOT NULL CHECK(sequence>0),
            id TEXT NOT NULL,request TEXT NOT NULL,event TEXT NOT NULL,sealed BLOB NOT NULL CHECK(length(sealed)<=131072),
            PRIMARY KEY(team,author,sequence),UNIQUE(team,id),UNIQUE(team,author,request));
            INSERT INTO mesh_events_v3 SELECT team,author,sequence,id,request,event,sealed FROM mesh_local_events;
            DROP TABLE mesh_local_outbox; DROP TABLE mesh_local_events; ALTER TABLE mesh_events_v3 RENAME TO mesh_local_events;
            CREATE TABLE mesh_local_outbox(team TEXT NOT NULL,id TEXT NOT NULL,PRIMARY KEY(team,id),FOREIGN KEY(team,id) REFERENCES mesh_local_events(team,id));
            INSERT INTO mesh_local_outbox SELECT team,id FROM mesh_v2_outbox; DROP TABLE mesh_v2_outbox;"))?;
    }
    db(conn.execute_batch("CREATE TABLE mesh_local_send_policy(team TEXT NOT NULL,id TEXT NOT NULL,enabled INTEGER NOT NULL CHECK(enabled IN (0,1)),public TEXT,proof TEXT,
        PRIMARY KEY(team,id),FOREIGN KEY(team,id) REFERENCES mesh_local_events(team,id));
        INSERT INTO mesh_local_send_policy(team,id,enabled) SELECT team,id,0 FROM mesh_local_events;
        CREATE TABLE mesh_received(team TEXT NOT NULL REFERENCES mesh_teams(team_id),author TEXT NOT NULL,
        sequence INTEGER NOT NULL CHECK(sequence>0),id TEXT NOT NULL,event TEXT NOT NULL,
        payload TEXT CHECK(length(payload)<=131072),receipt TEXT NOT NULL,
        PRIMARY KEY(team,author,sequence),UNIQUE(team,id));
        CREATE TABLE mesh_receive_heads(team TEXT NOT NULL,author TEXT NOT NULL,sequence INTEGER NOT NULL,id TEXT NOT NULL,PRIMARY KEY(team,author));
        CREATE TABLE mesh_sent_receipts(team TEXT NOT NULL,id TEXT NOT NULL,receiver TEXT NOT NULL,receipt TEXT NOT NULL,
        PRIMARY KEY(team,id,receiver),FOREIGN KEY(team,id) REFERENCES mesh_local_events(team,id));
        CREATE TABLE mesh_transfer_blocks(team TEXT NOT NULL REFERENCES mesh_teams(team_id),author TEXT NOT NULL,peer TEXT NOT NULL,
        expected TEXT NOT NULL,observed TEXT NOT NULL,PRIMARY KEY(team,author,peer)); PRAGMA user_version=3;"))
}

#[derive(Clone)]
pub(crate) struct Scope {
    pub team: String,
    pub own: String,
    pub peer: String,
    pub revision: u64,
    pub head: String,
}

impl Scope {
    pub(super) fn load(
        &self,
        vault: &Vault,
        conn: &Connection,
    ) -> Result<(Material, ApprovalPacket)> {
        let (key, packet) = local_member(vault, conn, &self.team)?;
        let state = packet.state()?;
        if key.device()?.device_id != self.own
            || self.own == self.peer
            || state.revision != self.revision
            || state.head != self.head
            || !state.devices.get(&self.peer).is_some_and(|m| m.active)
        {
            return err(
                "기록 교환의 장치 또는 승인 이력이 달라요. 최신 정보로 다시 시작해 주세요.",
            );
        }
        Ok((key, packet))
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DeliveryContext {
    version: u8,
    team: String,
    pub(super) author: String,
    event_id: String,
    pub(super) receiver: String,
    pub(super) revision: u64,
    pub(super) membership_head: String,
    body_allowed: bool,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Delivery {
    pub(super) event: Event,
    pub(super) context: DeliveryContext,
    pub(super) payload: Option<CipherBox>,
    signature: String,
}

impl Delivery {
    pub(super) fn hash(&self) -> Result<String> {
        Ok(digest(
            "sai-mesh/delivery-hash/1",
            &signed_bytes(DELIVERY, self)?,
        ))
    }
    pub(super) fn validate(&self, packet: &ApprovalPacket) -> Result<()> {
        self.validate_with(&mut Validation::new(packet))
    }
    pub(super) fn validate_with(&self, validation: &mut Validation<'_>) -> Result<()> {
        self.event.validate_state(
            validation.packet,
            validation.state(self.event.header.revision)?,
        )?;
        let c = &self.context;
        let h = &self.event.header;
        let state = validation.state(c.revision)?;
        let author = state
            .devices
            .get(&c.author)
            .filter(|m| m.active)
            .ok_or("승인되지 않은 전송 장치예요.")?;
        if c.version != 1
            || c.team != h.team
            || c.author != h.author
            || c.event_id != self.event.id()?
            || c.receiver == c.author
            || c.membership_head != state.head
            || !state.devices.get(&c.receiver).is_some_and(|m| m.active)
            || h.revision > c.revision
        {
            return err("암호화된 톡의 전송 대상과 승인 이력이 달라요.");
        }
        let should_have_body =
            c.body_allowed && h.revision == c.revision && h.recipients.contains(&c.receiver);
        if self.payload.is_some() != should_have_body {
            return err("톡 본문의 수신 범위가 맞지 않아요.");
        }
        verify(
            &author.device.signing_key,
            DELIVERY,
            &(&self.event, &self.context, &self.payload),
            &self.signature,
        )
    }
    pub(super) fn open(&self, key: &Material) -> Result<Body> {
        if key.device()?.device_id != self.context.receiver {
            return err("이 장치에 보내지 않은 톡이에요.");
        }
        let bytes = crypt::open(
            key,
            &signed_bytes("sai-mesh/body-aad/1", &self.context)?,
            self.payload.as_ref().ok_or("본문이 없는 기록이에요.")?,
        )?;
        let body: Body =
            serde_json::from_slice(&bytes).map_err(|_| "복호화한 톡 형식이 올바르지 않아요.")?;
        body.validate(&self.event)?;
        Ok(body)
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReceiptBody {
    version: u8,
    team: String,
    author: String,
    event_id: String,
    sequence: u64,
    pub(super) receiver: String,
    pub(super) delivery_hash: String,
    stored_body: bool,
    challenge: String,
    pub(super) revision: u64,
    pub(super) membership_head: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Receipt {
    pub(super) body: ReceiptBody,
    signature: String,
}

impl Receipt {
    pub(crate) fn has_body(&self) -> bool {
        self.body.stored_body
    }
    pub(super) fn confirms_event(&self, event: &Event) -> bool {
        self.has_body() || event.header.retract.is_some()
    }
    pub(super) fn validate(&self, packet: &ApprovalPacket, event: &Event) -> Result<()> {
        self.validate_with(&mut Validation::new(packet), event)
    }
    pub(super) fn validate_with(
        &self,
        validation: &mut Validation<'_>,
        event: &Event,
    ) -> Result<()> {
        let b = &self.body;
        let h = &event.header;
        unhex::<16>(&b.challenge)?;
        unhex::<32>(&b.delivery_hash)?;
        let state = validation.state(b.revision)?;
        let receiver = state
            .devices
            .get(&b.receiver)
            .filter(|m| m.active)
            .ok_or("저장 확인 장치의 승인을 확인하지 못했어요.")?;
        if b.version != 1
            || b.team != h.team
            || b.author != h.author
            || b.event_id != event.id()?
            || b.sequence != h.sequence
            || b.receiver == h.author
            || b.membership_head != state.head
            || b.revision < h.revision
            || (b.stored_body && (b.revision != h.revision || !h.recipients.contains(&b.receiver)))
        {
            return err("서명된 저장 확인이 이 톡의 수신 장치와 맞지 않아요.");
        }
        verify(&receiver.device.signing_key, RECEIPT, b, &self.signature)
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FrontierBody {
    version: u8,
    team: String,
    author: String,
    receiver: String,
    sequence: u64,
    event_id: String,
    challenge: String,
    revision: u64,
    membership_head: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Frontier {
    body: FrontierBody,
    signature: String,
    last: Option<Event>,
}

#[derive(Clone)]
pub(crate) struct Cursor {
    sequence: u64,
    id: String,
}

fn prefix(packet: &ApprovalPacket, revision: u64) -> Result<ApprovalPacket> {
    let n = usize::try_from(revision).map_err(|_| "승인 이력 범위가 달라요.")?;
    if n == 0 || n > packet.events.len() {
        return err("확인하지 않은 승인 이력이에요.");
    }
    Ok(ApprovalPacket {
        version: 1,
        manifest: packet.manifest.clone(),
        events: packet.events[..n].to_vec(),
    })
}

/// 같은 DB 스냅샷에서 검증한 승인 이력만 임시로 재사용한다.
pub(super) struct Validation<'a> {
    packet: &'a ApprovalPacket,
    states: BTreeMap<u64, super::model::MembershipState>,
}
impl<'a> Validation<'a> {
    pub(super) fn new(packet: &'a ApprovalPacket) -> Self {
        Self {
            packet,
            states: BTreeMap::new(),
        }
    }
    fn state(&mut self, revision: u64) -> Result<&super::model::MembershipState> {
        if !self.states.contains_key(&revision) {
            let state = prefix(self.packet, revision)?.state()?;
            self.states.insert(revision, state);
        }
        Ok(&self.states[&revision])
    }
}

pub(super) fn blocked(conn: &Connection, team: &str, author: &str) -> Result<bool> {
    let count: i64 = db(conn.query_row(
        "SELECT count(*) FROM mesh_transfer_blocks WHERE team=?1 AND author=?2",
        params![team, author],
        |r| r.get(0),
    ))?;
    Ok(count != 0)
}

pub(super) fn block(
    conn: &Connection,
    scope: &Scope,
    author: &str,
    expected: &str,
    observed: &str,
) -> Result<()> {
    db(conn.execute("INSERT INTO mesh_transfer_blocks(team,author,peer,expected,observed) VALUES(?1,?2,?3,?4,?5)
        ON CONFLICT(team,author,peer) DO UPDATE SET expected=excluded.expected,observed=excluded.observed",params![scope.team,author,scope.peer,expected,observed]))?;
    Ok(())
}

fn receipt_peers(
    conn: &Connection,
    packet: &ApprovalPacket,
    event: &Event,
) -> Result<BTreeSet<String>> {
    let mut query = db(
        conn.prepare("SELECT receiver,receipt FROM mesh_sent_receipts WHERE team=?1 AND id=?2")
    )?;
    let rows = db(
        query.query_map(params![event.header.team, event.id()?], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        }),
    )?;
    let mut peers = BTreeSet::new();
    for row in rows {
        let (peer, text) = db(row)?;
        if text.len() > 4096 {
            return err("저장 확인 자료가 너무 커요.");
        }
        let receipt: Receipt =
            serde_json::from_str(&text).map_err(|_| "보관한 저장 확인이 손상됐어요.")?;
        receipt.validate(packet, event)?;
        if !receipt.confirms_event(event) || receipt.body.receiver != peer {
            return err("본문 저장 확인 기록이 일치하지 않아요.");
        }
        peers.insert(peer);
    }
    Ok(peers)
}

pub(super) fn outgoing_state(
    conn: &Connection,
    packet: &ApprovalPacket,
    event: &Event,
    state: &super::model::MembershipState,
) -> Result<&'static str> {
    let public = &state
        .devices
        .get(&event.header.author)
        .ok_or("원본 작성 장치를 확인하지 못했어요.")?
        .device;
    if !send_enabled(conn, &event.header.team, &event.id()?, public)? {
        return Ok("legacy-held");
    }
    let targets: Vec<_> = event
        .header
        .recipients
        .iter()
        .filter(|id| *id != &event.header.author)
        .collect();
    if targets.is_empty() {
        return Ok("local-only");
    }
    let receipts = receipt_peers(conn, packet, event)?;
    if targets.iter().all(|id| receipts.contains(*id)) {
        return Ok(if event.header.retract.is_some() {
            "reference-stored"
        } else {
            "peer-stored"
        });
    }
    if event.header.revision != state.revision
        || targets
            .iter()
            .any(|id| !state.devices.get(*id).is_some_and(|m| m.active))
    {
        Ok("approval-held")
    } else {
        Ok("local-pending")
    }
}

pub(super) fn send_enabled(
    conn: &Connection,
    team: &str,
    id: &str,
    expected: &super::model::DevicePublic,
) -> Result<bool> {
    let record: Option<(i64, Option<String>, Option<String>)> = db(conn
        .query_row(
            "SELECT enabled,public,proof FROM mesh_local_send_policy WHERE team=?1 AND id=?2",
            params![team, id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional())?;
    match record {
        Some((0, _, _)) => Ok(false),
        Some((1, Some(public), Some(proof))) if public.len() <= 1024 => {
            let public: super::model::DevicePublic =
                serde_json::from_str(&public).map_err(|_| "발신 동의 장치 정보가 손상됐어요.")?;
            // expected는 같은 DB 스냅샷에서 관리자 서명·공개키 검증을 마친 장치다.
            if &public != expected {
                return err("발신 동의의 작성 장치가 달라요.");
            }
            verify(
                &public.signing_key,
                "sai-mesh/send-intent/1",
                &(team, id),
                &proof,
            )?;
            Ok(true)
        }
        _ => err("톡 원본의 전송 정책 기록이 없거나 손상됐어요."),
    }
}

fn received_chain(
    conn: &Connection,
    packet: &ApprovalPacket,
    author: &str,
    own: &str,
) -> Result<Vec<Event>> {
    let team = &packet.manifest.body.team_id;
    let mut q=db(conn.prepare("SELECT sequence,id,event,payload,receipt FROM mesh_received WHERE team=?1 AND author=?2 ORDER BY sequence LIMIT 1025"))?;
    let rows = db(q.query_map(params![team, author], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, Option<String>>(3)?,
            r.get::<_, String>(4)?,
        ))
    }))?;
    let mut events = Vec::new();
    let mut validation = Validation::new(packet);
    let mut parent = journal::origin(team, author);
    for row in rows {
        let (n, id, text, payload, proof) = db(row)?;
        if text.len() > 65536 || events.len() >= journal::MAX_EVENTS {
            return err("수신 원본 한도를 넘었어요.");
        }
        let event: Event =
            serde_json::from_str(&text).map_err(|_| "보관한 수신 원본이 손상됐어요.")?;
        event.validate_state(packet, validation.state(event.header.revision)?)?;
        if proof.len() > 4096 || payload.as_ref().is_some_and(|p| p.len() > MAX_FRAME) {
            return err("수신 저장 확인 자료의 한도를 넘었어요.");
        }
        let receipt: Receipt =
            serde_json::from_str(&proof).map_err(|_| "수신 저장 확인이 손상됐어요.")?;
        receipt.validate_with(&mut validation, &event)?;
        if receipt.body.receiver != own || receipt.body.stored_body != payload.is_some() {
            return err("수신 본문과 저장 확인이 일치하지 않아요.");
        }
        if let Some(payload) = payload {
            let delivery: Delivery =
                serde_json::from_str(&payload).map_err(|_| "수신 암호문이 손상됐어요.")?;
            if delivery.hash()? != receipt.body.delivery_hash || delivery.event.id()? != id {
                return err("보관한 암호문과 저장 확인의 해시가 달라요.");
            }
        }
        if event.header.author != author
            || event.header.parent != parent
            || event.header.sequence != events.len() as u64 + 1
            || u64::try_from(n).ok() != Some(event.header.sequence)
            || event.id() != Ok(id.clone())
        {
            return err("수신 원본의 순서 또는 내용이 달라요. 기존 원본을 유지했어요.");
        }
        parent = id;
        events.push(event);
    }
    let head: Option<(i64, String)> = db(conn
        .query_row(
            "SELECT sequence,id FROM mesh_receive_heads WHERE team=?1 AND author=?2",
            params![team, author],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional())?;
    match (head, events.last()) {
        (None, None) => {}
        (Some((n, id)), Some(last))
            if u64::try_from(n).ok() == Some(last.header.sequence)
                && last.id() == Ok(id.clone()) => {}
        _ => {
            return err(
                "수신 원본의 마지막 저장 기록이 달라요. 수신 경계를 자동 초기화하지 않았어요.",
            )
        }
    }
    Ok(events)
}

pub(super) fn received_entries(
    vault: &Vault,
    conn: &Connection,
    packet: &ApprovalPacket,
    own: &str,
) -> Result<Vec<super::talk::Entry>> {
    let team = &packet.manifest.body.team_id;
    let key = vault.key(conn, "device")?.ok_or("장치 키가 없어요.")?;
    let mut authors =
        db(conn.prepare("SELECT DISTINCT author FROM mesh_received WHERE team=?1 LIMIT 257"))?;
    let ids = db(authors.query_map([team], |r| r.get::<_, String>(0)))?
        .collect::<rusqlite::Result<Vec<_>>>();
    let ids = db(ids)?;
    if ids.len() > 256 {
        return err("수신 작성 장치 한도를 넘었어요.");
    }
    for id in ids {
        received_chain(conn, packet, &id, own)?;
    }
    let count: i64 = db(conn.query_row(
        "SELECT count(*) FROM mesh_received WHERE team=?1",
        [team],
        |r| r.get(0),
    ))?;
    if count < 0 || count > 1024 {
        return err("수신 본문 개수 한도를 넘었어요.");
    }
    let mut query = db(conn.prepare(
        "SELECT id,event,payload FROM mesh_received WHERE team=?1 ORDER BY rowid LIMIT 1025",
    ))?;
    let rows = db(query.query_map([team], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<String>>(2)?,
        ))
    }))?;
    let mut messages = Vec::new();
    for row in rows {
        let (id, header, payload) = db(row)?;
        if payload.is_none() {
            let event: Event =
                serde_json::from_str(&header).map_err(|_| "수신 원본이 손상됐어요.")?;
            // 위의 received_chain에서 서명·ID·연결과 저장 확인을 모두 검증했다.
            messages.push(super::talk::Entry {
                id,
                event,
                body: None,
                state: "header-only",
                incoming: true,
            });
            continue;
        }
        let encoded = payload.unwrap();
        if encoded.len() > MAX_FRAME {
            return err("보관한 수신 본문 한도를 넘었어요.");
        }
        let delivery: Delivery =
            serde_json::from_str(&encoded).map_err(|_| "수신 본문 자료가 손상됐어요.")?;
        delivery.validate(packet)?;
        if delivery.context.receiver != own || delivery.event.id() != Ok(id.clone()) {
            return err("수신 본문과 저장 위치가 달라요.");
        }
        let body = delivery.open(&key)?;
        messages.push(super::talk::Entry {
            id,
            event: delivery.event,
            body: Some(body),
            state: "received",
            incoming: true,
        });
    }
    Ok(messages)
}

pub(super) fn make_delivery(
    key: &Material,
    packet: &ApprovalPacket,
    stored: &journal::Stored,
    receiver: &str,
    body_allowed: bool,
) -> Result<Delivery> {
    let state = packet.state()?;
    let context = DeliveryContext {
        version: 1,
        team: stored.event.header.team.clone(),
        author: stored.event.header.author.clone(),
        event_id: stored.id.clone(),
        receiver: receiver.to_string(),
        revision: state.revision,
        membership_head: state.head.clone(),
        body_allowed: body_allowed,
    };
    let payload = if context.body_allowed
        && stored.event.header.revision == state.revision
        && stored
            .event
            .header
            .recipients
            .iter()
            .any(|id| id == receiver)
    {
        let body = journal::read_body_data(stored)?;
        let plain =
            Zeroizing::new(serde_json::to_vec(&body).map_err(|_| "톡 본문을 준비하지 못했어요.")?);
        Some(crypt::seal(
            &state.devices[receiver].device.encryption_key,
            &signed_bytes("sai-mesh/body-aad/1", &context)?,
            &plain,
        )?)
    } else {
        None
    };
    let event = stored.event.clone();
    let signature = key.sign(DELIVERY, &(&event, &context, &payload))?;
    let delivery = Delivery {
        event,
        context,
        payload,
        signature,
    };
    delivery.validate(packet)?;
    Ok(delivery)
}

impl Vault {
    pub(crate) fn receive_frontier(&self, scope: &Scope, challenge: &str) -> Result<Frontier> {
        unhex::<16>(challenge)?;
        let conn = self.required()?;
        let tx = db(conn.unchecked_transaction())?;
        let (key, packet) = scope.load(self, &tx)?;
        if blocked(&tx, &scope.team, &scope.peer)? {
            return err("이 작성 장치의 기록 교환은 충돌 확인이 필요해요.");
        }
        let events = received_chain(&tx, &packet, &scope.peer, &scope.own)?;
        let body = FrontierBody {
            version: 1,
            team: scope.team.clone(),
            author: scope.peer.clone(),
            receiver: scope.own.clone(),
            sequence: events.len() as u64,
            event_id: events
                .last()
                .map(Event::id)
                .transpose()?
                .unwrap_or_else(|| journal::origin(&scope.team, &scope.peer)),
            challenge: challenge.into(),
            revision: scope.revision,
            membership_head: scope.head.clone(),
        };
        Ok(Frontier {
            signature: key.sign(FRONTIER, &body)?,
            body,
            last: events.last().cloned(),
        })
    }

    pub(crate) fn begin_push(
        &self,
        scope: &Scope,
        frontier: &Frontier,
        challenge: &str,
    ) -> Result<Cursor> {
        let mut conn = self.required()?;
        let tx = db(conn.transaction_with_behavior(TransactionBehavior::Immediate))?;
        let (_, packet) = scope.load(self, &tx)?;
        let b = &frontier.body;
        unhex::<16>(challenge)?;
        unhex::<32>(&b.event_id)?;
        if b.version != 1
            || b.team != scope.team
            || b.author != scope.own
            || b.receiver != scope.peer
            || b.challenge != challenge
            || b.revision != scope.revision
            || b.membership_head != scope.head
        {
            return err("상대의 저장 경계가 현재 연결과 일치하지 않아요.");
        }
        verify(
            &packet.state()?.devices[&scope.peer].device.signing_key,
            FRONTIER,
            b,
            &frontier.signature,
        )?;
        // 상대의 주장만으로 내 원본을 잠그지 않는다. 내가 서명한 원본 증거가 필요하다.
        match (&frontier.last, b.sequence) {
            (None, 0) if b.event_id == journal::origin(&scope.team, &scope.own) => {}
            (Some(event), n) if n > 0 => {
                event.validate(&packet)?;
                if event.header.author != scope.own
                    || event.header.sequence != n
                    || event.id()? != b.event_id
                {
                    return err("상대 저장 경계의 작성자 서명 증거가 맞지 않아요.");
                }
            }
            _ => return err("상대 저장 경계에 작성자 서명 증거가 없어요."),
        }
        if blocked(&tx, &scope.team, &scope.own)? {
            return err("이 장치의 기록 충돌로 전송을 보류했어요.");
        }
        let records = journal::read_chain(&tx, &packet, &scope.own)?;
        let expected = if b.sequence == 0 {
            Some(journal::origin(&scope.team, &scope.own))
        } else {
            usize::try_from(b.sequence - 1)
                .ok()
                .and_then(|i| records.get(i))
                .map(|r| r.id.clone())
        };
        if expected.as_deref() != Some(b.event_id.as_str()) {
            block(
                &tx,
                scope,
                &scope.own,
                expected.as_deref().unwrap_or("missing"),
                &b.event_id,
            )?;
            db(tx.commit())?;
            return err("상대가 보관한 내 기록과 이 PC의 원본이 달라요. 순번 재사용을 막기 위해 새 저장과 전송을 보류했어요.");
        }
        Ok(Cursor {
            sequence: b.sequence,
            id: b.event_id.clone(),
        })
    }

    pub(crate) fn next_delivery(&self, scope: &Scope, cursor: &Cursor) -> Result<Option<Delivery>> {
        let conn = self.required()?;
        let tx = db(conn.unchecked_transaction())?;
        let (key, packet) = scope.load(self, &tx)?;
        let public = key.device()?;
        if blocked(&tx, &scope.team, &scope.own)? {
            return err("이 장치의 기록 충돌로 전송을 보류했어요.");
        }
        let records = journal::read_chain(&tx, &packet, &scope.own)?;
        let valid_cursor = if cursor.sequence == 0 {
            cursor.id == journal::origin(&scope.team, &scope.own)
        } else {
            usize::try_from(cursor.sequence - 1)
                .ok()
                .and_then(|i| records.get(i))
                .is_some_and(|r| r.id == cursor.id)
        };
        if !valid_cursor {
            return err("전송 중 로컬 원본의 경계가 바뀌었어요.");
        }
        let mut selected = None;
        for stored in records.iter().take(cursor.sequence as usize) {
            if send_enabled(&tx, &scope.team, &stored.id, &public)?
                && (stored.event.header.revision == scope.revision
                    || stored.event.header.retract.is_some())
                && stored.event.header.recipients.contains(&scope.peer)
                && !receipt_peers(&tx, &packet, &stored.event)?.contains(&scope.peer)
            {
                selected = Some(stored);
                break;
            }
        }
        let selected = selected.or_else(|| {
            usize::try_from(cursor.sequence)
                .ok()
                .and_then(|i| records.get(i))
        });
        let Some(stored) = selected else {
            return Ok(None);
        };
        Ok(Some(make_delivery(
            &key,
            &packet,
            stored,
            &scope.peer,
            send_enabled(&tx, &scope.team, &stored.id, &public)?,
        )?))
    }

    pub(crate) fn store_delivery(
        &self,
        scope: &Scope,
        delivery: &Delivery,
        challenge: &str,
    ) -> Result<Receipt> {
        unhex::<16>(challenge)?;
        let mut conn = self.required()?;
        let tx = db(conn.transaction_with_behavior(TransactionBehavior::Immediate))?;
        let (key, packet) = scope.load(self, &tx)?;
        if blocked(&tx, &scope.team, &scope.peer)? {
            return err("이 작성 장치의 기록 충돌로 수신을 보류했어요.");
        }
        delivery.validate(&packet)?;
        let c = &delivery.context;
        let event = &delivery.event;
        let id = event.id()?;
        if c.author != scope.peer
            || c.receiver != scope.own
            || c.revision != scope.revision
            || c.membership_head != scope.head
        {
            return err("이 연결의 상대가 작성한 기록이 아니에요.");
        }
        if delivery.payload.is_some() {
            delivery.open(&key)?;
        }
        let events = received_chain(&tx, &packet, &scope.peer, &scope.own)?;
        let index = usize::try_from(event.header.sequence - 1)
            .map_err(|_| "원본 순번이 올바르지 않아요.")?;
        if let Some(existing) = events.get(index) {
            if existing.id() != Ok(id.clone()) {
                block(&tx, scope, &scope.peer, &existing.id()?, &id)?;
                db(tx.commit())?;
                return err("같은 장치 순번의 다른 원본을 받았어요. 기존 원본을 보존하고 교환을 보류했어요.");
            }
        } else {
            let expected = events
                .last()
                .map(Event::id)
                .transpose()?
                .unwrap_or_else(|| journal::origin(&scope.team, &scope.peer));
            if index != events.len() || event.header.parent != expected {
                return err("먼저 받아야 할 원본이 빠져 있어요.");
            }
            let count: i64 = db(tx.query_row(
                "SELECT count(*) FROM mesh_received WHERE team=?1",
                [&scope.team],
                |r| r.get(0),
            ))?;
            if count >= 1024 {
                return err("준비 버전의 수신 원본 한도에 도달했어요.");
            }
        }
        let body = ReceiptBody {
            version: 1,
            team: scope.team.clone(),
            author: scope.peer.clone(),
            event_id: id.clone(),
            sequence: event.header.sequence,
            receiver: scope.own.clone(),
            delivery_hash: delivery.hash()?,
            stored_body: delivery.payload.is_some(),
            challenge: challenge.into(),
            revision: scope.revision,
            membership_head: scope.head.clone(),
        };
        let receipt = Receipt {
            signature: key.sign(RECEIPT, &body)?,
            body,
        };
        let encoded = serde_json::to_string(event).map_err(|_| "수신 원본을 저장하지 못했어요.")?;
        let payload = if delivery.payload.is_some() {
            Some(serde_json::to_string(delivery).map_err(|_| "수신 본문을 저장하지 못했어요.")?)
        } else {
            None
        };
        if payload.as_ref().is_some_and(|p| p.len() > MAX_FRAME) {
            return err("수신 본문 크기 한도를 넘었어요.");
        }
        let proof = serde_json::to_string(&receipt).map_err(|_| "저장 확인을 만들지 못했어요.")?;
        let sequence =
            i64::try_from(event.header.sequence).map_err(|_| "원본 순번이 너무 커요.")?;
        db(tx.execute("INSERT INTO mesh_received(team,author,sequence,id,event,payload,receipt) VALUES(?1,?2,?3,?4,?5,?6,?7)
            ON CONFLICT(team,author,sequence) DO UPDATE SET payload=coalesce(excluded.payload,mesh_received.payload),
            receipt=CASE WHEN excluded.payload IS NOT NULL OR mesh_received.payload IS NULL THEN excluded.receipt ELSE mesh_received.receipt END",params![scope.team,scope.peer,sequence,id,encoded,payload,proof]))?;
        let head = events.len().max(index + 1);
        let head_id = if head == index + 1 {
            id.clone()
        } else {
            events[head - 1].id()?
        };
        db(tx.execute(
            "INSERT INTO mesh_receive_heads(team,author,sequence,id) VALUES(?1,?2,?3,?4)
            ON CONFLICT(team,author) DO UPDATE SET sequence=excluded.sequence,id=excluded.id",
            params![scope.team, scope.peer, head as i64, head_id],
        ))?;
        db(tx.commit())?;
        Ok(receipt)
    }

    pub(crate) fn accept_receipt(
        &self,
        scope: &Scope,
        delivery: &Delivery,
        receipt: &Receipt,
        challenge: &str,
        cursor: &mut Cursor,
    ) -> Result<bool> {
        let mut conn = self.required()?;
        let tx = db(conn.transaction_with_behavior(TransactionBehavior::Immediate))?;
        let (_, packet) = scope.load(self, &tx)?;
        delivery.validate(&packet)?;
        receipt.validate(&packet, &delivery.event)?;
        let b = &receipt.body;
        let c = &delivery.context;
        if c.author != scope.own
            || c.receiver != scope.peer
            || c.revision != scope.revision
            || c.membership_head != scope.head
            || b.receiver != scope.peer
            || b.challenge != challenge
            || b.delivery_hash != delivery.hash()?
            || b.stored_body != delivery.payload.is_some()
            || b.revision != scope.revision
            || b.membership_head != scope.head
        {
            return err("이 전송 요청에 대한 상대의 저장 확인이 아니에요.");
        }
        let records = journal::read_chain(&tx, &packet, &scope.own)?;
        if !records
            .get((b.sequence - 1) as usize)
            .is_some_and(|r| r.id == b.event_id)
        {
            return err("확인할 로컬 원본이 없어요.");
        }
        if receipt.confirms_event(&delivery.event) {
            let text =
                serde_json::to_string(receipt).map_err(|_| "저장 확인을 보관하지 못했어요.")?;
            db(tx.execute(
                "INSERT INTO mesh_sent_receipts(team,id,receiver,receipt) VALUES(?1,?2,?3,?4)
                ON CONFLICT(team,id,receiver) DO UPDATE SET receipt=excluded.receipt",
                params![scope.team, b.event_id, scope.peer, text],
            ))?;
        }
        db(tx.commit())?;
        if b.sequence > cursor.sequence {
            cursor.sequence = b.sequence;
            cursor.id = b.event_id.clone();
        }
        Ok(b.stored_body)
    }
}

#[cfg(all(test, windows))]
#[path = "transfer_tests.rs"]
mod tests;
