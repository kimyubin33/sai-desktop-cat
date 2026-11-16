//! 장치 서명을 TLS 인증서·실험 ID·주소에 묶는다. 관리자 권한은 얻지 않는다.
use super::keys::{digest, unhex, verify, Material};
use super::model::ApprovalPacket;
use super::vault::{load_packet, Vault};
use crate::{err, Result};
use serde::{Deserialize, Serialize};

const DOMAIN: &str = "sai-mesh/session/1";

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionBody {
    pub version: u8,
    pub team_id: String,
    pub device_id: String,
    pub revision: u64,
    pub membership_head: String,
    pub binding: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionProof {
    pub body: SessionBody,
    pub signature: String,
}

pub(crate) fn binding(bytes: &[u8]) -> String {
    digest("sai-mesh/tls-card/1", bytes)
}

pub(super) fn local_member(
    vault: &Vault,
    conn: &rusqlite::Connection,
    team: &str,
) -> Result<(Material, ApprovalPacket)> {
    let packet = load_packet(conn, team)?.ok_or("먼저 PC 팀 승인 정보를 등록해 주세요.")?;
    let key = vault
        .key(conn, "device")?
        .ok_or("먼저 내 장치를 만들어 주세요.")?;
    let id = key.device()?.device_id;
    if !packet.state()?.devices.get(&id).is_some_and(|d| d.active) {
        return err("이 PC는 현재 승인된 장치가 아니에요.");
    }
    Ok((key, packet))
}

impl Vault {
    pub(crate) fn prove_session(&self, team: &str, binding: String) -> Result<SessionProof> {
        unhex::<32>(&binding)?;
        let conn = self.required()?;
        let tx = conn
            .unchecked_transaction()
            .map_err(|_| "장치 승인을 확인하지 못했어요.")?;
        let (key, packet) = local_member(self, &tx, team)?;
        let state = packet.state()?;
        let body = SessionBody {
            version: 1,
            team_id: team.into(),
            device_id: key.device()?.device_id,
            revision: state.revision,
            membership_head: state.head,
            binding,
        };
        Ok(SessionProof {
            signature: key.sign(DOMAIN, &body)?,
            body,
        })
    }

    pub(crate) fn verify_sessions(
        &self,
        team: &str,
        proofs: &[(&SessionProof, &str)],
    ) -> Result<()> {
        let conn = self.required()?;
        let tx = conn
            .unchecked_transaction()
            .map_err(|_| "장치 승인을 확인하지 못했어요.")?;
        let (_, packet) = local_member(self, &tx, team)?;
        let state = packet.state()?;
        for (proof, expected_binding) in proofs {
            let body = &proof.body;
            if body.version != 1
                || body.team_id != team
                || body.revision != state.revision
                || body.membership_head != state.head
                || body.binding != *expected_binding
            {
                return err("연결 정보와 현재 장치 승인 이력이 달라요. 최신 승인 정보를 등록하고 다시 시작해 주세요.");
            }
            let member = state
                .devices
                .get(&body.device_id)
                .filter(|m| m.active)
                .ok_or("승인되지 않았거나 해제된 상대 장치예요.")?;
            verify(&member.device.signing_key, DOMAIN, body, &proof.signature)?;
        }
        Ok(())
    }
}
