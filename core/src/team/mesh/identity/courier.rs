//! 작성자가 수신자별로 봉인한 전송 자료와 수신자의 디스크 저장 확인만 중계한다.
use super::{
    journal,
    keys::{digest, signed_bytes, unhex},
    model::ApprovalPacket,
    transfer::{self, Delivery, Receipt, Scope, Validation, MAX_FRAME},
    vault::{db, Vault},
};
use crate::{err, Result};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub(crate) const MAX_ITEMS: usize = 256;
const MAX_BYTES: usize = 32 * 1024 * 1024;
pub(crate) const BATCH: usize = 8;

pub(super) fn migrate(conn: &Connection) -> Result<()> {
    db(conn.execute_batch(
        "CREATE TABLE mesh_courier(
        team TEXT NOT NULL REFERENCES mesh_teams(team_id), key TEXT NOT NULL,
        author TEXT NOT NULL, sequence INTEGER NOT NULL CHECK(sequence>0),
        event_id TEXT NOT NULL, receiver TEXT NOT NULL, kind TEXT NOT NULL,
        data TEXT NOT NULL CHECK(length(CAST(data AS BLOB))<=131072),
        PRIMARY KEY(team,key)); PRAGMA user_version=4;",
    ))
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "value",
    deny_unknown_fields,
    rename_all = "camelCase"
)]
pub(crate) enum Parcel {
    Delivery(Delivery),
    Confirmation {
        delivery: Delivery,
        receipt: Receipt,
    },
}
impl Parcel {
    fn delivery(&self) -> &Delivery {
        match self {
            Self::Delivery(d) | Self::Confirmation { delivery: d, .. } => d,
        }
    }
    fn kind(&self) -> &'static str {
        match self {
            Self::Delivery(_) => "delivery",
            Self::Confirmation { .. } => "confirmation",
        }
    }
    pub(crate) fn key(&self) -> Result<String> {
        parcel_key(self.kind(), self.delivery())
    }
    fn validate(&self, packet: &ApprovalPacket) -> Result<()> {
        self.validate_with(&mut Validation::new(packet))
    }
    fn validate_with(&self, validation: &mut Validation<'_>) -> Result<()> {
        let d = self.delivery();
        d.validate_with(validation)?;
        if let Self::Confirmation { receipt, .. } = self {
            receipt.validate_with(validation, &d.event)?;
            if receipt.body.receiver != d.context.receiver
                || receipt.body.delivery_hash != d.hash()?
                || receipt.has_body() != d.payload.is_some()
                || receipt.body.revision != d.context.revision
                || receipt.body.membership_head != d.context.membership_head
            {
                return err("중계한 저장 확인과 원래 암호문이 달라요.");
            }
        }
        Ok(())
    }
    fn current(&self, scope: &Scope) -> bool {
        let c = &self.delivery().context;
        c.revision == scope.revision && c.membership_head == scope.head
    }
}

fn parcel_key(kind: &str, delivery: &Delivery) -> Result<String> {
    Ok(digest(
        "sai-mesh/courier-item/1",
        &signed_bytes("sai-mesh/courier-key/1", &(kind, delivery.hash()?))?,
    ))
}

fn load(conn: &Connection, packet: &ApprovalPacket) -> Result<Vec<(String, Parcel)>> {
    let team = &packet.manifest.body.team_id;
    let mut q = db(conn.prepare("SELECT key,author,sequence,event_id,receiver,kind,data FROM mesh_courier WHERE team=?1 ORDER BY kind,author,sequence,receiver,key LIMIT 257"))?;
    let rows = db(q.query_map([team], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, String>(5)?,
            r.get::<_, String>(6)?,
        ))
    }))?;
    let mut items = Vec::new();
    let mut validation = Validation::new(packet);
    let mut bytes = 0;
    for row in rows {
        let (key, author, sequence, id, receiver, kind, text) = db(row)?;
        bytes += text.len();
        if text.len() > MAX_FRAME || bytes > MAX_BYTES || items.len() >= MAX_ITEMS {
            return err("준비 버전의 중계 보관 한도를 넘었어요.");
        }
        let item: Parcel =
            serde_json::from_str(&text).map_err(|_| "중계 보관 자료가 손상됐어요.")?;
        item.validate_with(&mut validation)?;
        let d = item.delivery();
        if item.key()? != key
            || item.kind() != kind
            || d.context.author != author
            || d.context.receiver != receiver
            || i64::try_from(d.event.header.sequence).ok() != Some(sequence)
            || d.event.id()? != id
        {
            return err("중계 보관 위치와 서명된 자료가 달라요.");
        }
        items.push((key, item));
    }
    Ok(items)
}

fn insert(conn: &Connection, team: &str, item: &Parcel) -> Result<()> {
    let key = item.key()?;
    let exists: bool = db(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM mesh_courier WHERE team=?1 AND key=?2)",
        params![team, key],
        |r| r.get(0),
    ))?;
    if exists {
        return Ok(());
    }
    let text = serde_json::to_string(item).map_err(|_| "중계 자료를 만들지 못했어요.")?;
    let (count, bytes): (i64, i64) = db(conn.query_row("SELECT count(*),coalesce(sum(length(CAST(data AS BLOB))),0) FROM mesh_courier WHERE team=?1", [team], |r| Ok((r.get(0)?,r.get(1)?))))?;
    if text.len() > MAX_FRAME
        || count >= MAX_ITEMS as i64
        || bytes + text.len() as i64 > MAX_BYTES as i64
    {
        return err("중계 보관 한도에 도달했어요. 기존 자료를 유지했어요.");
    }
    let d = item.delivery();
    db(conn.execute("INSERT INTO mesh_courier(team,key,author,sequence,event_id,receiver,kind,data) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
        params![team,key,d.context.author,d.event.header.sequence as i64,d.event.id()?,d.context.receiver,item.kind(),text]))?;
    Ok(())
}

impl Vault {
    /// 이 호출 자체는 동의가 아니다. 서명된 양쪽 중계 동의를 검사한 연결에서만 호출한다.
    pub(crate) fn prepare_courier(&self, scope: &Scope, challenge: &str) -> Result<bool> {
        unhex::<16>(challenge)?;
        let mut conn = self.required()?;
        let tx = db(conn.transaction_with_behavior(TransactionBehavior::Immediate))?;
        let (key, packet) = scope.load(self, &tx)?;
        let state = packet.state()?;
        if transfer::blocked(&tx, &scope.team, &scope.own)? {
            return err("내 원본의 충돌로 중계를 보류했어요.");
        }
        if state.devices.values().filter(|m| m.active).count() > 9 {
            return err("준비 버전의 중계는 승인 장치 9대까지 지원해요.");
        }
        let items = load(&tx, &packet)?;
        let records = journal::read_chain(&tx, &packet, &scope.own)?;
        let public = key.device()?;
        let mut added = 0;
        let mut more = false;
        'records: for record in &records {
            for (receiver, member) in &state.devices {
                if !member.active || receiver == &scope.own {
                    continue;
                }
                if items.iter().any(|(_, p)| {
                    p.kind() == "delivery"
                        && p.current(scope)
                        && p.delivery().event.id() == Ok(record.id.clone())
                        && &p.delivery().context.receiver == receiver
                }) {
                    continue;
                }
                if added == BATCH {
                    more = true;
                    break 'records;
                }
                let allowed = transfer::send_enabled(&tx, &scope.team, &record.id, &public)?;
                let delivery = transfer::make_delivery(&key, &packet, record, receiver, allowed)?;
                insert(&tx, &scope.team, &Parcel::Delivery(delivery))?;
                added += 1;
            }
        }
        db(tx.commit())?;
        // 본문 저장은 기존 수신 트랜잭션을 사용한다. 중계 확인은 그 커밋 뒤에만 만든다.
        self.promote_courier(scope, challenge)?;
        Ok(more)
    }

    fn promote_courier(&self, scope: &Scope, challenge: &str) -> Result<()> {
        let conn = self.required()?;
        let (_, packet) = scope.load(self, &conn)?;
        let items = load(&conn, &packet)?;
        let mut visited = BTreeSet::new();
        for (_, item) in &items {
            let d = item.delivery();
            if !item.current(scope) || d.context.receiver != scope.own {
                continue;
            }
            if !visited.insert(d.hash()?) {
                continue;
            }
            let confirmation_key = parcel_key("confirmation", d)?;
            let stored: Option<(String, bool)> = db(conn.query_row("SELECT id,payload IS NOT NULL FROM mesh_received WHERE team=?1 AND author=?2 AND sequence=?3",
                params![scope.team,d.context.author,d.event.header.sequence as i64], |r| Ok((r.get(0)?,r.get(1)?))).optional())?;
            if items.iter().any(|(key, _)| key == &confirmation_key)
                && stored.is_some_and(|(id, body)| {
                    d.event.id() == Ok(id) && (body || d.payload.is_none())
                })
            {
                continue;
            }
            let head: i64 = db(conn.query_row("SELECT coalesce((SELECT sequence FROM mesh_receive_heads WHERE team=?1 AND author=?2),0)", params![scope.team,d.context.author], |r| r.get(0)))?;
            if d.event.header.sequence as i64 > head + 1 {
                continue;
            }
            let author_scope = Scope {
                peer: d.context.author.clone(),
                ..scope.clone()
            };
            let receipt = self.store_delivery(&author_scope, d, challenge)?;
            let mut c = self.required()?;
            let tx = db(c.transaction_with_behavior(TransactionBehavior::Immediate))?;
            scope.load(self, &tx)?;
            insert(
                &tx,
                &scope.team,
                &Parcel::Confirmation {
                    delivery: d.clone(),
                    receipt,
                },
            )?;
            db(tx.commit())?;
        }
        Ok(())
    }

    pub(crate) fn courier_inventory(&self, scope: &Scope) -> Result<Vec<String>> {
        let conn = self.required()?;
        let tx = db(conn.unchecked_transaction())?;
        let (_, packet) = scope.load(self, &tx)?;
        Ok(load(&tx, &packet)?
            .into_iter()
            .filter(|(_, p)| p.current(scope))
            .map(|(key, _)| key)
            .collect())
    }

    pub(crate) fn courier_batch(
        &self,
        scope: &Scope,
        inventory: &[String],
    ) -> Result<(Vec<Parcel>, bool)> {
        if inventory.len() > MAX_ITEMS {
            return err("상대의 중계 목록 한도를 넘었어요.");
        }
        let mut known = BTreeSet::new();
        for id in inventory {
            unhex::<32>(id)?;
            if !known.insert(id) {
                return err("중계 목록에 중복이 있어요.");
            }
        }
        let conn = self.required()?;
        let tx = db(conn.unchecked_transaction())?;
        let (_, packet) = scope.load(self, &tx)?;
        let mut pending = Vec::new();
        for (key, item) in load(&tx, &packet)? {
            if item.current(scope)
                && !known.contains(&key)
                && !transfer::blocked(&tx, &scope.team, &item.delivery().context.author)?
            {
                pending.push(item);
            }
        }
        let more = pending.len() > BATCH;
        pending.truncate(BATCH);
        Ok((pending, more))
    }

    pub(crate) fn store_courier(
        &self,
        scope: &Scope,
        item: &Parcel,
        challenge: &str,
    ) -> Result<()> {
        unhex::<16>(challenge)?;
        let mut conn = self.required()?;
        let tx = db(conn.transaction_with_behavior(TransactionBehavior::Immediate))?;
        let (_, packet) = scope.load(self, &tx)?;
        item.validate(&packet)?;
        if !item.current(scope) {
            return err("승인 이력이 바뀐 중계 자료는 보류해요.");
        }
        let d = item.delivery();
        let author = &d.context.author;
        if transfer::blocked(&tx, &scope.team, author)? {
            return err("원본 충돌로 중계 보관을 보류했어요.");
        }
        let id = d.event.id()?;
        let items = load(&tx, &packet)?;
        let existing = if author == &scope.own {
            journal::read_chain(&tx, &packet, author)?
                .get((d.event.header.sequence - 1) as usize)
                .map(|r| r.id.clone())
        } else {
            db(tx
                .query_row(
                    "SELECT id FROM mesh_received WHERE team=?1 AND author=?2 AND sequence=?3",
                    params![scope.team, author, d.event.header.sequence as i64],
                    |r| r.get::<_, String>(0),
                )
                .optional())?
        };
        let conflict = existing
            .as_ref()
            .filter(|old| *old != &id)
            .cloned()
            .or_else(|| {
                if author == &scope.own && existing.is_none() {
                    Some("missing".into())
                } else {
                    None
                }
            })
            .or_else(|| {
                items.iter().find_map(|(_, p)| {
                    let other = p.delivery();
                    (other.context.author == *author
                        && other.event.header.sequence == d.event.header.sequence
                        && other.event.id() != Ok(id.clone()))
                    .then(|| other.event.id().unwrap_or_default())
                })
            });
        if let Some(expected) = conflict {
            transfer::block(&tx, scope, author, &expected, &id)?;
            db(tx.commit())?;
            return err(
                "작성자가 서명한 중계 원본이 기존 기록과 달라요. 덮어쓰지 않고 보류했어요.",
            );
        }
        if let Parcel::Confirmation { receipt, .. } = item {
            if author == &scope.own {
                let delivery_key = Parcel::Delivery(d.clone()).key()?;
                if !items.iter().any(|(key, _)| key == &delivery_key) {
                    return err("이 PC에서 준비한 암호문의 저장 확인이 아니에요.");
                }
                if receipt.confirms_event(&d.event) {
                    let proof = serde_json::to_string(receipt)
                        .map_err(|_| "중계 저장 확인을 보관하지 못했어요.")?;
                    db(tx.execute("INSERT INTO mesh_sent_receipts(team,id,receiver,receipt) VALUES(?1,?2,?3,?4) ON CONFLICT(team,id,receiver) DO UPDATE SET receipt=excluded.receipt", params![scope.team,id,receipt.body.receiver,proof]))?;
                }
            }
        }
        insert(&tx, &scope.team, item)?;
        db(tx.commit())?;
        self.promote_courier(scope, challenge)
    }
}

#[cfg(all(test, windows))]
#[path = "courier_tests.rs"]
mod tests;
