//! 로컬 장치가 서명한 자동 복구 동의. 승인 이력과 Windows 네트워크에 묶는다.
use super::{
    automatic::AutoScope,
    keys::{unhex, verify},
    session::local_member,
    transfer,
    vault::{db, Vault},
};
use crate::{err, Result};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use std::net::Ipv4Addr;

const DOMAIN: &str = "sai-mesh/runtime-consent/1";
pub(super) fn migrate(conn: &Connection) -> Result<()> {
    db(conn.execute_batch("CREATE TABLE mesh_runtime_profile(slot INTEGER PRIMARY KEY CHECK(slot=1),data TEXT NOT NULL CHECK(length(CAST(data AS BLOB))<=8192)); PRAGMA user_version=5;"))
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct NetworkScope {
    pub adapter: String,
    pub network: String,
    pub subnet: Ipv4Addr,
    pub mask: Ipv4Addr,
    pub trusted: bool,
}
impl NetworkScope {
    pub fn validate(&self) -> Result<()> {
        unhex::<16>(&self.adapter)?;
        unhex::<16>(&self.network)?;
        let mask = u32::from(self.mask);
        let host = !mask;
        if mask == 0
            || host & host.wrapping_add(1) != 0
            || !self.subnet.is_private()
            || u32::from(self.subnet) & mask != u32::from(self.subnet)
        {
            return err("자동 복구 네트워크 범위가 올바르지 않아요.");
        }
        Ok(())
    }
    pub fn contains(&self, address: Ipv4Addr) -> bool {
        address.is_private() && u32::from(address) & u32::from(self.mask) == u32::from(self.subnet)
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Mode {
    Approved,
    Transfer,
    Automatic,
    Courier,
}
impl Mode {
    pub fn automatic(self) -> bool {
        matches!(self, Self::Automatic | Self::Courier)
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Body {
    version: u8,
    team: String,
    own: String,
    revision: u64,
    head: String,
    mode: Mode,
    network: NetworkScope,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Profile {
    body: Body,
    signature: String,
}
impl Profile {
    pub fn approval(&self) -> AutoScope {
        AutoScope {
            team: self.body.team.clone(),
            own: self.body.own.clone(),
            revision: self.body.revision,
            head: self.body.head.clone(),
        }
    }
    pub fn team(&self) -> &str {
        &self.body.team
    }
    pub fn mode(&self) -> Mode {
        self.body.mode
    }
    pub fn network(&self) -> &NetworkScope {
        &self.body.network
    }
}
impl Vault {
    pub(crate) fn remember_runtime(
        &self,
        approval: &AutoScope,
        mode: Mode,
        network: NetworkScope,
    ) -> Result<Profile> {
        network.validate()?;
        if !network.trusted || !mode.automatic() {
            return err("자동 복구는 개인·도메인 네트워크의 자동 연결에서만 허용해요.");
        }
        let mut conn = self.required()?;
        let tx = db(conn.transaction_with_behavior(TransactionBehavior::Immediate))?;
        let team = &approval.team;
        let (key, packet) = local_member(self, &tx, team)?;
        let state = packet.state()?;
        let own = key.device()?.device_id;
        if own != approval.own || state.revision != approval.revision || state.head != approval.head
        {
            return err("동의를 저장하는 동안 승인 이력이 바뀌었어요. 다시 확인해 주세요.");
        }
        if transfer::blocked(&tx, team, &own)? {
            return err("원본 충돌을 확인한 뒤 자동 복구를 설정해 주세요.");
        }
        let body = Body {
            version: 1,
            team: team.into(),
            own,
            revision: state.revision,
            head: state.head,
            mode,
            network,
        };
        let profile = Profile {
            signature: key.sign(DOMAIN, &body)?,
            body,
        };
        let text =
            serde_json::to_string(&profile).map_err(|_| "자동 복구 동의를 저장하지 못했어요.")?;
        db(tx.execute("INSERT INTO mesh_runtime_profile(slot,data) VALUES(1,?1) ON CONFLICT(slot) DO UPDATE SET data=excluded.data",[text]))?;
        db(tx.commit())?;
        Ok(profile)
    }
    pub(crate) fn runtime_profile(&self) -> Result<Option<Profile>> {
        let Some(conn) = self.connection(false)? else {
            return Ok(None);
        };
        let tx = db(conn.unchecked_transaction())?;
        let text: Option<String> = db(tx
            .query_row(
                "SELECT data FROM mesh_runtime_profile WHERE slot=1",
                [],
                |r| r.get(0),
            )
            .optional())?;
        let Some(text) = text else {
            return Ok(None);
        };
        if text.len() > 8192 {
            return err("자동 복구 동의가 너무 커요.");
        }
        let p: Profile = serde_json::from_str(&text)
            .map_err(|_| "자동 복구 동의가 손상됐어요. 다시 선택해 주세요.")?;
        let (key, packet) = local_member(self, &tx, &p.body.team)?;
        let state = packet.state()?;
        let own = key.device()?;
        if p.body.version != 1
            || p.body.own != own.device_id
            || p.body.revision != state.revision
            || p.body.head != state.head
            || !p.body.mode.automatic()
            || !p.body.network.trusted
            || transfer::blocked(&tx, &p.body.team, &p.body.own)?
        {
            return err("장치 또는 승인 이력이 달라 자동 복구를 보류했어요. 다시 동의해 주세요.");
        }
        p.body.network.validate()?;
        verify(&own.signing_key, DOMAIN, &p.body, &p.signature)?;
        Ok(Some(p))
    }
    pub(crate) fn forget_runtime(&self) -> Result<()> {
        let Some(mut conn) = self.connection(false)? else {
            return Ok(());
        };
        let tx = db(conn.transaction_with_behavior(TransactionBehavior::Immediate))?;
        db(tx.execute("DELETE FROM mesh_runtime_profile", []))?;
        db(tx.commit())
    }
    pub(crate) fn runtime_scope(&self, team: &str) -> Result<AutoScope> {
        let conn = self.required()?;
        let tx = db(conn.unchecked_transaction())?;
        let (key, packet) = local_member(self, &tx, team)?;
        let state = packet.state()?;
        Ok(AutoScope {
            team: team.into(),
            own: key.device()?.device_id,
            revision: state.revision,
            head: state.head,
        })
    }
}
