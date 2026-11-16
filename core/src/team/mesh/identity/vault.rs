use rusqlite::{params, Connection, OpenFlags, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use zeroize::Zeroizing;

use super::keys::{random_id, Material};
use super::model::{
    ApprovalPacket, DevicePublic, EnrollmentRequest, MemberDevice, MembershipAction, TeamManifest,
};
use crate::{err, Result};

const PREFIX: &[u8] = b"sai-mesh/dpapi/1\0";
const KEY_LIMIT: usize = 16384;

pub(super) trait Protector: Send + Sync {
    fn seal(&self, plain: &[u8]) -> Result<Vec<u8>>;
    fn open(&self, sealed: &[u8]) -> Result<Vec<u8>>;
}

struct NativeProtector;
impl Protector for NativeProtector {
    fn seal(&self, plain: &[u8]) -> Result<Vec<u8>> {
        crate::secret::protect_device_secret(plain)
    }
    fn open(&self, sealed: &[u8]) -> Result<Vec<u8>> {
        crate::secret::unprotect_device_secret(sealed)
    }
}

pub(super) fn db<T>(result: rusqlite::Result<T>) -> Result<T> {
    result.map_err(|_| {
        "장치 등록 저장소를 읽거나 저장하지 못했어요. 기존 자료를 자동 교체하지 않았어요.".into()
    })
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct KeyEnvelope {
    version: u8,
    slot: String,
    material: Material,
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub device: Option<DevicePublic>,
    pub teams: Vec<TeamStatus>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamStatus {
    pub team_id: String,
    pub name: String,
    pub authority_fingerprint: String,
    pub revision: u64,
    pub admin_here: bool,
    pub devices: Vec<MemberDevice>,
}

#[derive(Clone)]
pub struct Vault {
    path: PathBuf,
    protector: Arc<dyn Protector>,
}

impl Vault {
    #[cfg(test)]
    pub(super) fn with_protector(path: PathBuf, protector: Arc<dyn Protector>) -> Self {
        Self { path, protector }
    }

    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
            protector: Arc::new(NativeProtector),
        }
    }

    pub(super) fn connection(&self, create: bool) -> Result<Option<Connection>> {
        let exists = self
            .path
            .try_exists()
            .map_err(|_| "장치 등록 저장소의 접근 권한을 확인해 주세요.")?;
        if !exists && !create {
            return Ok(None);
        }
        if !exists {
            if let Some(parent) = self.path.parent() {
                std::fs::create_dir_all(parent).map_err(|_| "장치 등록 폴더를 만들지 못했어요.")?;
            }
        }
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
            | if create {
                OpenFlags::SQLITE_OPEN_CREATE
            } else {
                OpenFlags::empty()
            };
        let mut connection = db(Connection::open_with_flags(&self.path, flags))?;
        db(connection.busy_timeout(Duration::from_secs(5)))?;
        let version: i64 =
            db(connection.pragma_query_value(None, "user_version", |row| row.get(0)))?;
        if version == 0 && create {
            let tx = db(connection.transaction_with_behavior(TransactionBehavior::Immediate))?;
            let version: i64 = db(tx.pragma_query_value(None, "user_version", |row| row.get(0)))?;
            if version == 0 {
                let count: i64 = db(tx.query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type='table'",
                    [],
                    |r| r.get(0),
                ))?;
                if count != 0 {
                    return err("등록 저장소 형식이 달라요. 덮어쓰지 않았어요.");
                }
                db(tx.execute_batch("CREATE TABLE mesh_keys(slot TEXT PRIMARY KEY, sealed BLOB NOT NULL CHECK(length(sealed)<=16384));
                    CREATE TABLE mesh_key_checkpoints(slot TEXT PRIMARY KEY, public TEXT NOT NULL);
                    CREATE TABLE mesh_teams(team_id TEXT PRIMARY KEY, manifest TEXT NOT NULL);
                    CREATE TABLE mesh_membership(team_id TEXT NOT NULL REFERENCES mesh_teams(team_id), revision INTEGER NOT NULL,
                        hash TEXT NOT NULL, event TEXT NOT NULL, PRIMARY KEY(team_id,revision));
                    PRAGMA user_version=1;"))?;
            } else if !(1..=6).contains(&version) {
                return err("지원하지 않는 장치 저장소 버전이에요.");
            }
            db(tx.commit())?;
        } else if !(1..=6).contains(&version) {
            return err("지원하지 않는 장치 저장소 버전이에요. 기존 자료를 유지했어요.");
        }
        // v1의 키·승인 이력을 유지한 채 로컬 로그 테이블만 원자적으로 추가한다.
        let version: i64 =
            db(connection.pragma_query_value(None, "user_version", |row| row.get(0)))?;
        if version == 1 {
            let tx = db(connection.transaction_with_behavior(TransactionBehavior::Immediate))?;
            let version: i64 = db(tx.pragma_query_value(None, "user_version", |row| row.get(0)))?;
            if version == 1 {
                super::journal::migrate(&tx)?;
            } else if version != 2 && version != 3 && version != 4 && version != 5 && version != 6 {
                return err("지원하지 않는 장치 저장소 버전이에요.");
            }
            db(tx.commit())?;
        }
        let version: i64 =
            db(connection.pragma_query_value(None, "user_version", |row| row.get(0)))?;
        if version == 2 {
            let tx = db(connection.transaction_with_behavior(TransactionBehavior::Immediate))?;
            let version: i64 = db(tx.pragma_query_value(None, "user_version", |row| row.get(0)))?;
            if version == 2 {
                super::transfer::migrate(&tx)?;
            } else if version != 3 && version != 4 && version != 5 && version != 6 {
                return err("지원하지 않는 기록 저장소 버전이에요.");
            }
            db(tx.commit())?;
        }
        let version: i64 = db(connection.pragma_query_value(None, "user_version", |r| r.get(0)))?;
        if version == 3 {
            let tx = db(connection.transaction_with_behavior(TransactionBehavior::Immediate))?;
            let version: i64 = db(tx.pragma_query_value(None, "user_version", |r| r.get(0)))?;
            if version == 3 {
                super::courier::migrate(&tx)?;
            } else if version != 4 && version != 5 && version != 6 {
                return err("지원하지 않는 중계 저장소 버전이에요.");
            }
            db(tx.commit())?;
        }
        let version: i64 = db(connection.pragma_query_value(None, "user_version", |r| r.get(0)))?;
        if version == 4 {
            let tx = db(connection.transaction_with_behavior(TransactionBehavior::Immediate))?;
            let version: i64 = db(tx.pragma_query_value(None, "user_version", |r| r.get(0)))?;
            if version == 4 {
                super::consent::migrate(&tx)?;
            } else if version != 5 && version != 6 {
                return err("지원하지 않는 실행 동의 저장소 버전이에요.");
            }
            db(tx.commit())?;
        }
        let version: i64 = db(connection.pragma_query_value(None, "user_version", |r| r.get(0)))?;
        if version == 5 {
            let tx = db(connection.transaction_with_behavior(TransactionBehavior::Immediate))?;
            let version: i64 = db(tx.pragma_query_value(None, "user_version", |r| r.get(0)))?;
            if version == 5 {
                super::archive::migrate(&tx)?;
            } else if version != 6 {
                return err("지원하지 않는 이전 보관 저장소 버전이에요.");
            }
            db(tx.commit())?;
        }
        db(connection.execute_batch(
            "PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;",
        ))?;
        Ok(Some(connection))
    }

    pub(super) fn required(&self) -> Result<Connection> {
        self.connection(false)?
            .ok_or_else(|| "먼저 내 장치를 만들어 주세요.".into())
    }

    pub(super) fn key(&self, conn: &Connection, slot: &str) -> Result<Option<Material>> {
        let sealed: Option<Vec<u8>> = db(conn
            .query_row(
                "SELECT sealed FROM mesh_keys WHERE slot=?1",
                [slot],
                |row| row.get(0),
            )
            .optional())?;
        let checkpoint: Option<String> = db(conn
            .query_row(
                "SELECT public FROM mesh_key_checkpoints WHERE slot=?1",
                [slot],
                |row| row.get(0),
            )
            .optional())?;
        let Some(sealed) = sealed else {
            if checkpoint.is_some() {
                return err("이전에 만든 개인키를 찾을 수 없어요. 새 키로 자동 교체하지 않았어요.");
            }
            return Ok(None);
        };
        if sealed.len() > KEY_LIMIT || !sealed.starts_with(PREFIX) {
            return err("지원하지 않는 개인키 보관 형식이에요. 자동 교체하지 않았어요.");
        }
        let plain = Zeroizing::new(self.protector.open(&sealed[PREFIX.len()..])?);
        let envelope: KeyEnvelope = serde_json::from_slice(&plain)
            .map_err(|_| "보관된 개인키가 손상됐어요. 자동 교체하지 않았어요.")?;
        if envelope.version != 1
            || envelope.slot != slot
            || envelope.material.is_device() != (slot == "device")
        {
            return err("개인키의 용도와 보관 위치가 맞지 않아요.");
        }
        envelope.material.signing_public()?;
        let expected = key_checkpoint(&envelope.material, slot)?;
        if checkpoint.as_deref() != Some(expected.as_str()) {
            return err("보관된 개인키와 장치 신원 기록이 맞지 않아요.");
        }
        Ok(Some(envelope.material))
    }

    fn ensure_key(&self, conn: &Connection, slot: &str) -> Result<Material> {
        if let Some(key) = self.key(conn, slot)? {
            return Ok(key);
        }
        let envelope = KeyEnvelope {
            version: 1,
            slot: slot.into(),
            material: Material::generate(slot == "device")?,
        };
        let plain = Zeroizing::new(
            serde_json::to_vec(&envelope).map_err(|_| "개인키를 준비하지 못했어요.")?,
        );
        let encrypted = self.protector.seal(&plain)?;
        let mut sealed = PREFIX.to_vec();
        sealed.extend(encrypted);
        if sealed.len() > KEY_LIMIT {
            return err("개인키 보관 자료가 너무 커요.");
        }
        db(conn.execute(
            "INSERT INTO mesh_keys(slot,sealed) VALUES(?1,?2)",
            params![slot, sealed],
        ))?;
        db(conn.execute(
            "INSERT INTO mesh_key_checkpoints(slot,public) VALUES(?1,?2)",
            params![slot, key_checkpoint(&envelope.material, slot)?],
        ))?;
        Ok(envelope.material)
    }

    pub fn create_device(&self) -> Result<DevicePublic> {
        let mut conn = self.connection(true)?.unwrap();
        let tx = db(conn.transaction_with_behavior(TransactionBehavior::Immediate))?;
        let public = self.ensure_key(&tx, "device")?.device()?;
        db(tx.commit())?;
        Ok(public)
    }

    pub fn create_team(&self, name: &str) -> Result<ApprovalPacket> {
        let mut conn = self.required()?;
        let tx = db(conn.transaction_with_behavior(TransactionBehavior::Immediate))?;
        let device = self
            .key(&tx, "device")?
            .ok_or("먼저 내 장치를 만들어 주세요.")?;
        let authority = self.ensure_key(&tx, "authority")?;
        let manifest = TeamManifest::create(&authority, name)?;
        let request = EnrollmentRequest::create(&device, &manifest)?;
        let mut packet = ApprovalPacket {
            version: 1,
            manifest,
            events: Vec::new(),
        };
        packet.append(
            &authority,
            MembershipAction::Approve {
                request,
                member_id: random_id()?,
            },
        )?;
        save_packet(&tx, &packet)?;
        db(tx.commit())?;
        Ok(packet)
    }

    pub fn request(&self, team: &str) -> Result<EnrollmentRequest> {
        let conn = self.required()?;
        let packet =
            load_packet(&conn, team)?.ok_or("먼저 관리자의 PC 팀 승인 정보를 등록해 주세요.")?;
        let device = self
            .key(&conn, "device")?
            .ok_or("먼저 내 장치를 만들어 주세요.")?;
        EnrollmentRequest::create(&device, &packet.manifest)
    }

    pub fn export(&self, team: &str) -> Result<ApprovalPacket> {
        load_packet(&self.required()?, team)?.ok_or_else(|| "PC 팀 승인 정보가 없어요.".into())
    }

    pub fn import(&self, text: &str, expected_fingerprint: &str) -> Result<String> {
        let incoming = ApprovalPacket::parse(text)?;
        incoming.state()?;
        let mut conn = self.connection(true)?.unwrap();
        let tx = db(conn.transaction_with_behavior(TransactionBehavior::Immediate))?;
        let known = load_packet(&tx, &incoming.manifest.body.team_id)?;
        let fingerprint = incoming.manifest.fingerprint()?;
        if known.is_none() && expected_fingerprint.trim() != fingerprint {
            return err("처음 등록할 때는 관리자 화면에서 직접 확인한 지문이 필요해요.");
        }
        if !expected_fingerprint.trim().is_empty() && expected_fingerprint.trim() != fingerprint {
            return err("확인한 관리자 지문과 자료가 일치하지 않아요.");
        }
        save_packet(&tx, &incoming)?;
        db(tx.commit())?;
        Ok(incoming.manifest.body.team_id)
    }

    pub fn approve(&self, team: &str, request_text: &str) -> Result<ApprovalPacket> {
        let request = EnrollmentRequest::parse(request_text)?;
        let mut conn = self.required()?;
        let tx = db(conn.transaction_with_behavior(TransactionBehavior::Immediate))?;
        let mut packet = load_packet(&tx, team)?.ok_or("PC 팀 정보가 없어요.")?;
        let authority = self.authority(&tx, &packet)?;
        request.validate(&packet.manifest)?;
        let state = packet.state()?;
        let old = state.devices.get(&request.body.device.device_id);
        if old.is_some_and(|device| device.active) {
            return Ok(packet);
        }
        let member_id = match old {
            Some(old) => old.member_id.clone(),
            None => random_id()?,
        };
        packet.append(&authority, MembershipAction::Approve { request, member_id })?;
        save_packet(&tx, &packet)?;
        db(tx.commit())?;
        Ok(packet)
    }

    pub fn revoke(&self, team: &str, device_id: &str) -> Result<ApprovalPacket> {
        let mut conn = self.required()?;
        let tx = db(conn.transaction_with_behavior(TransactionBehavior::Immediate))?;
        let mut packet = load_packet(&tx, team)?.ok_or("PC 팀 정보가 없어요.")?;
        let authority = self.authority(&tx, &packet)?;
        let state = packet.state()?;
        let device = state
            .devices
            .get(device_id)
            .ok_or("이 팀에 등록한 장치가 아니에요.")?;
        if !device.active {
            return Ok(packet);
        }
        packet.append(
            &authority,
            MembershipAction::Revoke {
                device_id: device_id.into(),
            },
        )?;
        save_packet(&tx, &packet)?;
        db(tx.commit())?;
        Ok(packet)
    }

    fn authority(&self, conn: &Connection, packet: &ApprovalPacket) -> Result<Material> {
        let key = self
            .key(conn, "authority")?
            .ok_or("이 PC에는 운영 관리자 키가 없어요.")?;
        if key.signing_public()? != packet.manifest.body.authority_key {
            return err("이 PC 팀의 운영 관리자 장치가 아니에요.");
        }
        Ok(key)
    }

    pub fn status(&self) -> Result<Status> {
        let Some(conn) = self.connection(false)? else {
            return Ok(Status::default());
        };
        // 같은 스냅샷에서 장치 키·팀 목록·승인 이력을 읽는다.
        let tx = db(conn.unchecked_transaction())?;
        let device = self
            .key(&tx, "device")?
            .map(|key| key.device())
            .transpose()?;
        let authority = self
            .key(&tx, "authority")?
            .map(|key| key.signing_public())
            .transpose()?;
        let mut query = db(tx.prepare("SELECT team_id FROM mesh_teams ORDER BY rowid"))?;
        let ids = db(query.query_map([], |row| row.get::<_, String>(0)))?
            .collect::<rusqlite::Result<Vec<_>>>();
        let mut teams = Vec::new();
        for id in db(ids)? {
            let packet = load_packet(&tx, &id)?.ok_or("PC 팀 정보를 읽지 못했어요.")?;
            let state = packet.state()?;
            teams.push(TeamStatus {
                team_id: id,
                name: packet.manifest.body.name.clone(),
                authority_fingerprint: packet.manifest.fingerprint()?,
                revision: state.revision,
                admin_here: authority.as_deref()
                    == Some(packet.manifest.body.authority_key.as_str()),
                devices: state.devices.into_values().collect(),
            });
        }
        Ok(Status { device, teams })
    }
}

pub(super) fn load_packet(conn: &Connection, team: &str) -> Result<Option<ApprovalPacket>> {
    let manifest: Option<String> = db(conn
        .query_row(
            "SELECT manifest FROM mesh_teams WHERE team_id=?1",
            [team],
            |row| row.get(0),
        )
        .optional())?;
    let Some(manifest) = manifest else {
        return Ok(None);
    };
    let manifest: TeamManifest =
        serde_json::from_str(&manifest).map_err(|_| "보관된 PC 팀 정보가 손상됐어요.")?;
    if manifest.body.team_id != team {
        return err("PC 팀의 저장 위치와 신뢰 기준이 맞지 않아요.");
    }
    let mut query = db(conn.prepare(
        "SELECT revision,hash,event FROM mesh_membership WHERE team_id=?1 ORDER BY revision",
    ))?;
    let rows = db(query.query_map([team], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    }))?;
    let mut events = Vec::new();
    for row in rows {
        let (revision, hash, event) = db(row)?;
        let event: super::model::MembershipEvent =
            serde_json::from_str(&event).map_err(|_| "보관된 승인 이력이 손상됐어요.")?;
        if u64::try_from(revision).ok() != Some(event.body.revision) || event.hash()? != hash {
            return err("보관된 승인 이력의 무결성을 확인하지 못했어요.");
        }
        events.push(event);
        if events.len() > super::model::MAX_EVENTS {
            return err("지원하는 승인 이력 개수를 넘었어요.");
        }
    }
    let packet = ApprovalPacket {
        version: 1,
        manifest,
        events,
    };
    packet.state()?;
    Ok(Some(packet))
}

fn key_checkpoint(material: &Material, slot: &str) -> Result<String> {
    if slot == "device" {
        serde_json::to_string(&material.device()?)
            .map_err(|_| "공개 장치 기록을 만들지 못했어요.".into())
    } else {
        material.signing_public()
    }
}

fn save_packet(conn: &Connection, incoming: &ApprovalPacket) -> Result<()> {
    incoming.state()?;
    let team = &incoming.manifest.body.team_id;
    let known = load_packet(conn, team)?;
    if let Some(known) = &known {
        if known.manifest != incoming.manifest {
            return err("이미 확인한 PC 팀의 신뢰 기준과 달라요.");
        }
        for (old, new) in known.events.iter().zip(&incoming.events) {
            if old.hash()? != new.hash()? {
                return err(
                    "같은 순번에 서로 다른 관리자 승인 이력이 있어요. 기존 이력을 유지했어요.",
                );
            }
        }
    } else {
        let first: Option<String> = db(conn
            .query_row(
                "SELECT team_id FROM mesh_teams ORDER BY rowid LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional())?;
        if let Some(first) = first {
            let trusted =
                load_packet(conn, &first)?.ok_or("운영 관리자 신뢰 기준을 읽지 못했어요.")?;
            if trusted.manifest.body.authority_key != incoming.manifest.body.authority_key {
                return err("이 저장소에서 이미 확인한 운영 관리자와 달라요. 새 관리자 권한을 만들거나 교체하지 않았어요.");
            }
        }
        let count: i64 =
            db(conn.query_row("SELECT count(*) FROM mesh_teams", [], |row| row.get(0)))?;
        if count >= 32 {
            return err("실험용 PC 팀 등록 한도에 도달했어요.");
        }
        let manifest = serde_json::to_string(&incoming.manifest)
            .map_err(|_| "PC 팀 정보를 저장하지 못했어요.")?;
        db(conn.execute(
            "INSERT INTO mesh_teams(team_id,manifest) VALUES(?1,?2)",
            params![team, manifest],
        ))?;
    }
    let known_len = known.as_ref().map_or(0, |packet| packet.events.len());
    // 오래된 정상 자료는 현재 승인·해제 상태를 되돌리지 않는다.
    for event in incoming.events.iter().skip(known_len) {
        let encoded = serde_json::to_string(event).map_err(|_| "승인 이력을 저장하지 못했어요.")?;
        let revision = i64::try_from(event.body.revision).map_err(|_| "승인 순번이 너무 커요.")?;
        db(conn.execute(
            "INSERT INTO mesh_membership(team_id,revision,hash,event) VALUES(?1,?2,?3,?4)",
            params![team, revision, event.hash()?, encoded],
        ))?;
    }
    Ok(())
}
