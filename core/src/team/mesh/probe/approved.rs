use super::*;
use crate::team::mesh::identity::{
    session::{binding, SessionProof},
    Vault,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovedCard {
    pub card: PeerCard,
    pub proof: SessionProof,
    #[serde(default)]
    pub exchange: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub automatic: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub courier: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub host: bool,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub record_version: u8,
}
fn is_zero(value: &u8) -> bool {
    *value == 0
}

fn is_false(value: &bool) -> bool {
    !value
}

fn card_binding(
    card: &PeerCard,
    exchange: bool,
    automatic: bool,
    courier: bool,
    host: bool,
    record_version: u8,
) -> Result<String> {
    if record_version != 0 {
        if record_version != 2 || !exchange {
            return err("호환되는 PC 톡 형식이 아니에요. 양쪽 사이 앱을 업데이트해 주세요.");
        }
        let base = card_binding(card, exchange, automatic, courier, host, 0)?;
        return Ok(binding(
            &serde_json::to_vec(&("sai-mesh/records", base, record_version))
                .map_err(|_| "PC 톡 연결 정보를 만들지 못했어요.")?,
        ));
    }
    if host {
        if !courier || !exchange || !automatic {
            return err("대표 후보는 자동 연결과 중계 동의가 필요해요.");
        }
        return Ok(binding(
            &serde_json::to_vec(&(card, true, true, true, true))
                .map_err(|_| "대표 후보 연결 정보를 만들지 못했어요.")?,
        ));
    }
    if courier {
        if !exchange || !automatic {
            return err("중계에는 자동 연결과 기록 교환 동의가 필요해요.");
        }
        return Ok(binding(
            &serde_json::to_vec(&(card, true, true, true))
                .map_err(|_| "중계 연결 정보를 만들지 못했어요.")?,
        ));
    }
    if automatic {
        if !exchange {
            return err("자동 연결에는 기록 교환 동의가 필요해요.");
        }
        return Ok(binding(
            &serde_json::to_vec(&(card, true, true))
                .map_err(|_| "자동 연결 정보를 만들지 못했어요.")?,
        ));
    }
    if exchange {
        return Ok(binding(
            &serde_json::to_vec(&(card, true))
                .map_err(|_| "기록 교환 연결 정보를 만들지 못했어요.")?,
        ));
    }
    Ok(binding(
        &serde_json::to_vec(card).map_err(|_| "연결 정보를 만들지 못했어요.")?,
    ))
}

#[derive(Clone)]
pub(super) struct Context {
    pub(super) vault: Vault,
    pub team: String,
    pub own: ApprovedCard,
}

impl Context {
    pub async fn new(
        vault: Vault,
        team: String,
        card: PeerCard,
        exchange: bool,
        automatic: bool,
        courier: bool,
    ) -> Result<Self> {
        let worker = vault.clone();
        let id = team.clone();
        let record_version = if exchange { 2 } else { 0 };
        let digest = card_binding(&card, exchange, automatic, courier, courier, record_version)?;
        let proof = tokio::task::spawn_blocking(move || worker.prove_session(&id, digest))
            .await
            .map_err(|_| "장치 신원을 확인하지 못했어요.")??;
        Ok(Self {
            vault,
            team,
            own: ApprovedCard {
                card,
                proof,
                exchange,
                automatic,
                courier,
                host: courier,
                record_version,
            },
        })
    }

    fn validate(&self, peer: &ApprovedCard) -> Result<()> {
        if peer.proof.body.device_id == self.own.proof.body.device_id {
            return err("내 장치의 다른 연결 정보예요.");
        }
        self.vault.verify_sessions(
            &self.team,
            &[
                (
                    &self.own.proof,
                    &card_binding(
                        &self.own.card,
                        self.own.exchange,
                        self.own.automatic,
                        self.own.courier,
                        self.own.host,
                        self.own.record_version,
                    )?,
                ),
                (
                    &peer.proof,
                    &card_binding(
                        &peer.card,
                        peer.exchange,
                        peer.automatic,
                        peer.courier,
                        peer.host,
                        peer.record_version,
                    )?,
                ),
            ],
        )?;
        if self.own.exchange && peer.exchange && peer.record_version != 2 {
            return err("상대 PC의 톡 형식이 달라요. 양쪽 사이 앱을 업데이트해 주세요.");
        }
        Ok(())
    }

    pub fn parse_peer(&self, input: &str) -> Result<ApprovedCard> {
        if input.len() > 16384 {
            return err("승인 장치 연결 정보가 너무 커요.");
        }
        let peer: ApprovedCard = serde_json::from_str(input)
            .map_err(|_| "관리자 승인 장치의 연결 정보를 넣어 주세요.")?;
        peer.card.validate()?;
        self.validate(&peer)?;
        Ok(peer)
    }
}

impl Probe {
    pub(crate) fn matches_approval(
        &self,
        scope: &crate::team::mesh::identity::automatic::AutoScope,
    ) -> bool {
        self.inner.approved.as_ref().is_some_and(|c| {
            c.team == scope.team
                && c.own.proof.body.device_id == scope.own
                && c.own.proof.body.revision == scope.revision
                && c.own.proof.body.membership_head == scope.head
        })
    }
    pub async fn start_managed(
        vault: Vault,
        team: &str,
        address: Ipv4Addr,
        mode: crate::team::mesh::runtime::Mode,
        lifetime: Duration,
    ) -> Result<Self> {
        use crate::team::mesh::runtime::Mode;
        if lifetime.is_zero() || lifetime > LIFETIME {
            return err("PC 연결 사용 시간이 만료됐어요.");
        }
        #[cfg(not(test))]
        if !interfaces()?.contains(&address) {
            return err("이 PC의 LAN 주소를 선택해 주세요.");
        }
        let mut options = super::auto::Options::default();
        #[cfg(test)]
        if address.is_loopback() {
            options = super::auto::Options::testing();
        }
        options.courier = mode == Mode::Courier;
        Self::start_mode_auto(
            address,
            false,
            lifetime,
            Some((vault, team.into())),
            mode != Mode::Approved,
            mode.automatic().then_some(options),
        )
        .await
    }
    pub async fn start_courier(vault: Vault, team: &str, address: Ipv4Addr) -> Result<Self> {
        if !interfaces()?.contains(&address) {
            return err("이 PC의 LAN 주소를 선택해 주세요.");
        }
        Self::start_mode_auto(
            address,
            false,
            LIFETIME,
            Some((vault, team.into())),
            true,
            Some(super::auto::Options {
                courier: true,
                ..Default::default()
            }),
        )
        .await
    }
    pub async fn start_automatic(vault: Vault, team: &str, address: Ipv4Addr) -> Result<Self> {
        if !interfaces()?.contains(&address) {
            return err("이 PC의 LAN 주소를 선택해 주세요.");
        }
        Self::start_mode_auto(
            address,
            false,
            LIFETIME,
            Some((vault, team.into())),
            true,
            Some(super::auto::Options::default()),
        )
        .await
    }
    pub async fn start_transfer(vault: Vault, team: &str, address: Ipv4Addr) -> Result<Self> {
        if !interfaces()?.contains(&address) {
            return err("이 PC의 LAN 주소를 선택해 주세요.");
        }
        Self::start_mode_exchange(address, false, LIFETIME, Some((vault, team.into())), true).await
    }
    /// 기존 임시 연결 실험과 달리, 현재 승인된 장치만 열고 등록할 수 있다.
    pub async fn start_approved(vault: Vault, team: &str, address: Ipv4Addr) -> Result<Self> {
        if !interfaces()?.contains(&address) {
            return err("이 PC의 LAN 주소를 선택해 주세요.");
        }
        Self::start_mode(address, false, LIFETIME, Some((vault, team.into()))).await
    }
}

impl Inner {
    pub(super) fn alpns(&self) -> Vec<Vec<u8>> {
        let mut protocols = vec![self.alpn().to_vec()];
        if self.approved.as_ref().is_some_and(|c| c.own.exchange) {
            protocols.push(super::exchange::ALPN.to_vec());
        }
        if self.approved.as_ref().is_some_and(|c| c.own.courier) {
            protocols.push(super::exchange::COURIER_ALPN.to_vec());
        }
        if self.approved.as_ref().is_some_and(|c| c.own.host) {
            protocols.push(super::host::ALPN.to_vec());
        }
        protocols
    }
    pub(super) fn alpn(&self) -> &[u8] {
        if self.approved.is_some() {
            b"sai-approved-probe/1"
        } else {
            b"sai-probe/1"
        }
    }

    pub(super) async fn validate_peer(&self, peer: &Peer) -> Result<()> {
        match (&self.approved, &peer.approval) {
            (None, None) => Ok(()),
            (Some(context), Some(peer)) => {
                let vault = context.vault.clone();
                let team = context.team.clone();
                let own = context.own.clone();
                let peer = peer.clone();
                tokio::task::spawn_blocking(move || {
                    let context = Context { vault, team, own };
                    context.validate(&peer)
                })
                .await
                .map_err(|_| "장치 승인을 확인하지 못했어요.")?
            }
            _ => err("장치 승인 방식이 일치하지 않아요."),
        }
    }
}
