//! 기존 공개 톡의 로컬 확인 사본. 장치 이벤트·발신 정책·중계 큐와 물리적으로 분리한다.
use super::{
    keys::{digest, random_id, signed_bytes, unhex, verify, Material},
    model::ApprovalPacket,
    session::local_member,
    vault::{db, Vault},
};
use crate::{
    err,
    team::mesh::migration::{Row, Source},
    Result,
};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use zeroize::Zeroizing;

const DOMAIN: &str = "sai-mesh/legacy-archive/1";
const LIMIT: usize = 1024;
pub(super) fn migrate(conn: &Connection) -> Result<()> {
    db(conn.execute_batch("CREATE TABLE mesh_legacy_archive(team TEXT NOT NULL REFERENCES mesh_teams(team_id),key TEXT NOT NULL,header TEXT NOT NULL CHECK(length(CAST(header AS BLOB))<=8192),sealed BLOB NOT NULL CHECK(length(sealed)<=65536),PRIMARY KEY(team,key));PRAGMA user_version=6;"))
}
#[derive(Clone)]
pub(crate) struct Scope {
    pub team: String,
    own: String,
    revision: u64,
    head: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct HeaderBody {
    version: u8,
    team: String,
    key: String,
    commitment: String,
    importer: String,
    revision: u64,
    head: String,
    imported_at: i64,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    body: HeaderBody,
    signature: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Body {
    version: u8,
    source: Source,
    row: Row,
    read_at: i64,
    salt: String,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    pub target_team: String,
    pub selected: usize,
    pub imported: usize,
    pub existing: usize,
    pub total: usize,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveMessage {
    pub key: String,
    pub source_team: String,
    pub source_name: String,
    pub source_kind: String,
    pub legacy_id: String,
    pub claimed_by: String,
    pub claimed_at: i64,
    pub text: String,
    pub imported_at: i64,
    pub read_at: i64,
    pub importer_device: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchivePage {
    pub team_id: String,
    pub total: usize,
    pub offset: usize,
    pub next_offset: Option<usize>,
    pub messages: Vec<ArchiveMessage>,
}
fn index(source: &Source, row: &Row) -> Result<String> {
    Ok(digest(
        "sai-mesh/archive-index/1",
        &signed_bytes(
            "sai-mesh/archive-source/1",
            &(&source.scope, &source.team_id, &row.id),
        )?,
    ))
}
fn commitment(body: &Body) -> Result<String> {
    let bytes = Zeroizing::new(signed_bytes("sai-mesh/archive-body/1", body)?);
    Ok(digest("sai-mesh/archive-commitment/1", &bytes))
}
fn admin(
    vault: &Vault,
    conn: &Connection,
    team: &str,
) -> Result<(Material, Material, ApprovalPacket)> {
    let (device, packet) = local_member(vault, conn, team)?;
    let authority = vault
        .key(conn, "authority")?
        .ok_or("이 PC에 대상 PC 팀의 운영 관리자 키가 필요해요.")?;
    if authority.signing_public()? != packet.manifest.body.authority_key {
        return err("대상 PC 팀의 운영 관리자 키가 아니에요.");
    }
    if super::transfer::blocked(conn, team, &device.device()?.device_id)? {
        return err("장치 원본의 충돌을 확인한 뒤 이전해 주세요.");
    }
    Ok((device, authority, packet))
}
impl Scope {
    fn load(
        &self,
        vault: &Vault,
        conn: &Connection,
    ) -> Result<(Material, Material, ApprovalPacket)> {
        let (device, authority, packet) = admin(vault, conn, &self.team)?;
        let state = packet.state()?;
        if device.device()?.device_id != self.own
            || state.revision != self.revision
            || state.head != self.head
        {
            return err("이전 준비 뒤 PC 장치·승인 이력이 바뀌었어요. 다시 조회해 주세요.");
        }
        Ok((device, authority, packet))
    }
}
struct Reader<'a> {
    packet: &'a ApprovalPacket,
    own: &'a str,
    heads: BTreeMap<u64, String>,
}
impl Reader<'_> {
    fn open(
        &mut self,
        team: &str,
        key: &str,
        header: &str,
        sealed: &[u8],
    ) -> Result<(Header, Body)> {
        if header.len() > 8192 || sealed.len() > 65536 {
            return err("이전 보관 자료의 한도를 넘었어요.");
        }
        let h: Header = serde_json::from_str(header).map_err(|_| "이전 보관 서명이 손상됐어요.")?;
        let b = &h.body;
        unhex::<32>(&b.key)?;
        unhex::<32>(&b.commitment)?;
        let revision =
            usize::try_from(b.revision).map_err(|_| "보관 승인 이력을 확인하지 못했어요.")?;
        if revision == 0 || revision > self.packet.events.len() {
            return err("보관 시점의 승인 이력이 없어요.");
        }
        if !self.heads.contains_key(&b.revision) {
            let state = ApprovalPacket {
                version: 1,
                manifest: self.packet.manifest.clone(),
                events: self.packet.events[..revision].to_vec(),
            }
            .state()?;
            if !state.devices.get(self.own).is_some_and(|m| m.active) {
                return err("보관 장치가 당시 승인되지 않았어요.");
            }
            self.heads.insert(b.revision, state.head);
        }
        if b.version != 1
            || b.team != team
            || b.key != key
            || b.importer != self.own
            || self.heads[&b.revision] != b.head
        {
            return err("이전 보관 자료의 팀·장치·승인 이력이 달라요.");
        }
        verify(
            &self.packet.manifest.body.authority_key,
            DOMAIN,
            b,
            &h.signature,
        )?;
        let plain = Zeroizing::new(
            crate::secret::unprotect_device_secret(sealed)
                .map_err(|_| "이 Windows 계정에서 이전 보관 기록을 열지 못했어요.")?,
        );
        let body: Body =
            serde_json::from_slice(&plain).map_err(|_| "이전 보관 본문이 손상됐어요.")?;
        body.row.validate()?;
        unhex::<16>(&body.salt)?;
        unhex::<32>(&body.source.scope)?;
        if body.version != 1
            || index(&body.source, &body.row)? != key
            || commitment(&body)? != b.commitment
        {
            return err("이전 보관 본문과 관리자 확인 서명이 달라요.");
        }
        Ok((h, body))
    }
}
fn stored(conn: &Connection, team: &str, key: &str) -> Result<Option<(String, Vec<u8>)>> {
    db(conn
        .query_row(
            "SELECT header,sealed FROM mesh_legacy_archive WHERE team=?1 AND key=?2",
            params![team, key],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional())
}
impl Vault {
    pub(crate) fn archive_scope(&self, team: &str) -> Result<Scope> {
        let conn = self.required()?;
        let tx = db(conn.unchecked_transaction())?;
        let (key, _, packet) = admin(self, &tx, team)?;
        let state = packet.state()?;
        Ok(Scope {
            team: team.into(),
            own: key.device()?.device_id,
            revision: state.revision,
            head: state.head,
        })
    }
    pub(crate) fn archive_evaluate(
        &self,
        scope: &Scope,
        source: &Source,
        rows: &[Row],
    ) -> Result<Vec<&'static str>> {
        let conn = self.required()?;
        let tx = db(conn.unchecked_transaction())?;
        let (_, _, packet) = scope.load(self, &tx)?;
        let mut reader = Reader {
            packet: &packet,
            own: &scope.own,
            heads: Default::default(),
        };
        rows.iter()
            .map(|row| {
                let key = index(source, row)?;
                match stored(&tx, &scope.team, &key)? {
                    None => Ok("new"),
                    Some((header, sealed)) => {
                        let (_, body) = reader.open(&scope.team, &key, &header, &sealed)?;
                        Ok(if &body.row == row {
                            "existing"
                        } else {
                            "conflict"
                        })
                    }
                }
            })
            .collect()
    }
    pub(crate) fn archive_import(
        &self,
        scope: &Scope,
        source: &Source,
        rows: &[Row],
        read_at: i64,
    ) -> Result<ImportReport> {
        if rows.is_empty() || rows.len() > 200 {
            return err("한 번에 1~200개를 보관해 주세요.");
        }
        let mut conn = self.required()?;
        let tx = db(conn.transaction_with_behavior(TransactionBehavior::Immediate))?;
        let (_, authority, packet) = scope.load(self, &tx)?;
        let mut reader = Reader {
            packet: &packet,
            own: &scope.own,
            heads: Default::default(),
        };
        let mut seen = BTreeSet::new();
        let mut imported = 0;
        let mut existing = 0;
        let count: i64 = db(tx.query_row(
            "SELECT count(*) FROM mesh_legacy_archive WHERE team=?1",
            [&scope.team],
            |r| r.get(0),
        ))?;
        if count < 0 || count as usize > LIMIT {
            return err("이전 보관 기록 한도를 넘었어요.");
        }
        for row in rows {
            row.validate()?;
            let key = index(source, row)?;
            if !seen.insert(key.clone()) {
                return err("이전 선택이 중복됐어요.");
            }
            if let Some((header, sealed)) = stored(&tx, &scope.team, &key)? {
                let (_, body) = reader.open(&scope.team, &key, &header, &sealed)?;
                if &body.row != row {
                    return err("이미 보관한 같은 원본 ID의 내용이 달라요. 덮어쓰지 않았어요.");
                }
                existing += 1;
                continue;
            }
            if count as usize + imported >= LIMIT {
                return err("준비 버전의 이전 보관 한도 1,024개에 도달했어요.");
            }
            let body = Body {
                version: 1,
                source: source.clone(),
                row: row.clone(),
                read_at,
                salt: random_id()?,
            };
            let header = HeaderBody {
                version: 1,
                team: scope.team.clone(),
                key: key.clone(),
                commitment: commitment(&body)?,
                importer: scope.own.clone(),
                revision: scope.revision,
                head: scope.head.clone(),
                imported_at: crate::time::now_ms(),
            };
            let signed = Header {
                signature: authority.sign(DOMAIN, &header)?,
                body: header,
            };
            let plain = Zeroizing::new(
                serde_json::to_vec(&body).map_err(|_| "이전 본문을 준비하지 못했어요.")?,
            );
            let sealed = crate::secret::protect_device_secret(&plain)
                .map_err(|_| "Windows에서 이전 본문을 보호하지 못했어요. 보관하지 않았어요.")?;
            let header = serde_json::to_string(&signed)
                .map_err(|_| "이전 확인 서명을 준비하지 못했어요.")?;
            db(tx.execute(
                "INSERT INTO mesh_legacy_archive(team,key,header,sealed) VALUES(?1,?2,?3,?4)",
                params![scope.team, key, header, sealed],
            ))?;
            imported += 1;
        }
        db(tx.commit())?;
        Ok(ImportReport {
            target_team: scope.team.clone(),
            selected: rows.len(),
            imported,
            existing,
            total: count as usize + imported,
        })
    }
    pub(crate) fn archive_page(&self, team: &str, offset: usize) -> Result<ArchivePage> {
        if offset > LIMIT {
            return err("보관 목록의 위치가 올바르지 않아요.");
        }
        let conn = self.required()?;
        let tx = db(conn.unchecked_transaction())?;
        let (key, packet) = local_member(self, &tx, team)?;
        let own = key.device()?.device_id;
        let count: i64 = db(tx.query_row(
            "SELECT count(*) FROM mesh_legacy_archive WHERE team=?1",
            [team],
            |r| r.get(0),
        ))?;
        if count < 0 || count as usize > LIMIT {
            return err("이전 보관 목록의 한도를 넘었어요.");
        }
        let mut q=db(tx.prepare("SELECT key,header,sealed FROM mesh_legacy_archive WHERE team=?1 ORDER BY rowid DESC LIMIT 50 OFFSET ?2"))?;
        let rows = db(q.query_map(params![team, offset as i64], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Vec<u8>>(2)?,
            ))
        }))?;
        let mut reader = Reader {
            packet: &packet,
            own: &own,
            heads: Default::default(),
        };
        let mut messages = Vec::new();
        for row in rows {
            let (id, header, sealed) = db(row)?;
            let (header, body) = reader.open(team, &id, &header, &sealed)?;
            messages.push(ArchiveMessage {
                key: id,
                source_team: body.source.team_id.clone(),
                source_name: body.source.team_name.clone(),
                source_kind: body.source.kind.clone(),
                legacy_id: body.row.id.clone(),
                claimed_by: body.row.by.clone(),
                claimed_at: body.row.at,
                text: body.row.text.clone(),
                imported_at: header.body.imported_at,
                read_at: body.read_at,
                importer_device: header.body.importer,
            });
        }
        let next = offset + messages.len();
        Ok(ArchivePage {
            team_id: team.into(),
            total: count as usize,
            offset,
            next_offset: (next < count as usize).then_some(next),
            messages,
        })
    }
}
