//! 승인된 장치 쌍만 해석하는 발견 표식과 새 도전에 묶인 암호화된 연결 정보.
use super::{
    crypt::{self, CipherBox},
    keys::{hex, signed_bytes, unhex, verify, Material},
    model::ApprovalPacket,
    session::local_member,
    vault::Vault,
};
use crate::{err, Result};
use ring::{hkdf, hmac};
use serde::{Deserialize, Serialize};
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::Zeroizing;

const INFO: &[u8] = b"sai-mesh/auto-bootstrap/1";
pub(crate) const MAX_AUTO_PEERS: usize = 8;

#[derive(Clone)]
pub(crate) struct AutoScope {
    pub team: String,
    pub own: String,
    pub revision: u64,
    pub head: String,
}

impl AutoScope {
    fn load(
        &self,
        vault: &Vault,
        conn: &rusqlite::Connection,
        peer: Option<&str>,
    ) -> Result<(Material, ApprovalPacket)> {
        let (key, packet) = local_member(vault, conn, &self.team)?;
        if super::transfer::blocked(conn, &self.team, &self.own)? {
            return err("이 장치 원본의 충돌을 확인해야 해서 자동 연결을 보류했어요.");
        }
        let state = packet.state()?;
        if key.device()?.device_id != self.own
            || state.revision != self.revision
            || state.head != self.head
            || peer.is_some_and(|id| {
                id == self.own || !state.devices.get(id).is_some_and(|m| m.active)
            })
        {
            return err("자동 연결의 장치 승인 이력이 바뀌었어요. 최신 정보로 다시 시작해 주세요.");
        }
        Ok((key, packet))
    }
}

struct Route {
    peer: String,
    key: Zeroizing<[u8; 32]>,
}

pub(crate) struct AutoRoutes {
    pub scope: AutoScope,
    routes: Vec<Route>,
}

impl AutoRoutes {
    pub fn peers(&self) -> Vec<String> {
        self.routes.iter().map(|r| r.peer.clone()).collect()
    }
    fn tag(&self, route: &Route, session: &str, author: &str, receiver: &str) -> Result<String> {
        unhex::<16>(session)?;
        let message = signed_bytes("sai-mesh/auto-tag/1", &(session, author, receiver))?;
        Ok(hex(hmac::sign(
            &hmac::Key::new(hmac::HMAC_SHA256, &*route.key),
            &message,
        )
        .as_ref()))
    }
    pub fn offers(&self, session: &str) -> Result<Vec<String>> {
        self.routes
            .iter()
            .map(|r| self.tag(r, session, &self.scope.own, &r.peer))
            .collect()
    }
    pub fn match_offer(&self, session: &str, tag: &str) -> Option<String> {
        unhex::<32>(tag).ok()?;
        self.routes
            .iter()
            .find(|r| {
                self.tag(r, session, &r.peer, &self.scope.own)
                    .ok()
                    .as_deref()
                    == Some(tag)
            })
            .map(|r| r.peer.clone())
    }
    pub fn receiver(&self, session: &str, tag: &str) -> Option<String> {
        unhex::<32>(tag).ok()?;
        self.routes
            .iter()
            .find(|r| {
                self.tag(r, session, &self.scope.own, &r.peer)
                    .ok()
                    .as_deref()
                    == Some(tag)
            })
            .map(|r| r.peer.clone())
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Stage {
    Request,
    Reply,
}
impl Stage {
    fn domain(self) -> &'static str {
        match self {
            Self::Request => "sai-mesh/auto-request/1",
            Self::Reply => "sai-mesh/auto-reply/1",
        }
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BootAad {
    pub stage: Stage,
    pub session: String,
    pub tag: String,
    pub server_nonce: String,
    pub client_nonce: String,
}
impl BootAad {
    pub fn validate(&self) -> Result<()> {
        unhex::<16>(&self.session)?;
        unhex::<32>(&self.tag)?;
        unhex::<16>(&self.server_nonce)?;
        unhex::<16>(&self.client_nonce)?;
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Message {
    version: u8,
    team: String,
    author: String,
    receiver: String,
    revision: u64,
    head: String,
    aad: BootAad,
    card: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Signed {
    body: Message,
    signature: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Capsule {
    sealed: CipherBox,
}

impl Vault {
    pub(crate) fn auto_routes(&self, scope: &AutoScope) -> Result<AutoRoutes> {
        let conn = self.required()?;
        let tx = conn
            .unchecked_transaction()
            .map_err(|_| "자동 연결 승인을 확인하지 못했어요.")?;
        let (key, packet) = scope.load(self, &tx, None)?;
        let state = packet.state()?;
        let peers: Vec<_> = state
            .devices
            .values()
            .filter(|m| m.active && m.device.device_id != scope.own)
            .collect();
        if peers.len() > MAX_AUTO_PEERS {
            return err(
                "이번 자동 연결은 다른 승인 장치 최대 8대까지 지원해요. 수동 교환을 이용해 주세요.",
            );
        }
        let seed = key.decryption_seed()?;
        let secret = StaticSecret::from(*seed);
        let mut routes = Vec::new();
        for peer in peers {
            let shared =
                secret.diffie_hellman(&PublicKey::from(unhex::<32>(&peer.device.encryption_key)?));
            if !shared.was_contributory() {
                return err("장치 발견용 공개키가 올바르지 않아요.");
            }
            let mut ids = [scope.own.as_str(), peer.device.device_id.as_str()];
            ids.sort();
            let info = signed_bytes("sai-mesh/auto-route/1", &(&scope.team, &scope.head, ids))?;
            let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, b"sai-mesh/auto-route/1")
                .extract(shared.as_bytes());
            let infos = [info.as_slice()];
            let okm = prk
                .expand(&infos, hkdf::HKDF_SHA256)
                .map_err(|_| "발견 키를 준비하지 못했어요.")?;
            let mut derived = Zeroizing::new([0u8; 32]);
            okm.fill(&mut *derived)
                .map_err(|_| "발견 키를 준비하지 못했어요.")?;
            routes.push(Route {
                peer: peer.device.device_id.clone(),
                key: derived,
            });
        }
        Ok(AutoRoutes {
            scope: scope.clone(),
            routes,
        })
    }

    pub(crate) fn validate_auto_scope(&self, scope: &AutoScope) -> Result<()> {
        let conn = self.required()?;
        let tx = conn
            .unchecked_transaction()
            .map_err(|_| "자동 연결 승인을 확인하지 못했어요.")?;
        scope.load(self, &tx, None)?;
        Ok(())
    }

    pub(crate) fn seal_auto(
        &self,
        scope: &AutoScope,
        peer: &str,
        aad: &BootAad,
        card: &str,
    ) -> Result<Capsule> {
        aad.validate()?;
        if card.len() > 8192 {
            return err("자동 연결 정보가 너무 커요.");
        }
        let conn = self.required()?;
        let tx = conn
            .unchecked_transaction()
            .map_err(|_| "자동 연결 승인을 확인하지 못했어요.")?;
        let (key, packet) = scope.load(self, &tx, Some(peer))?;
        let body = Message {
            version: 1,
            team: scope.team.clone(),
            author: scope.own.clone(),
            receiver: peer.into(),
            revision: scope.revision,
            head: scope.head.clone(),
            aad: aad.clone(),
            card: card.into(),
        };
        let message = Signed {
            signature: key.sign(aad.stage.domain(), &body)?,
            body,
        };
        let plain = Zeroizing::new(
            serde_json::to_vec(&message).map_err(|_| "자동 연결 정보를 만들지 못했어요.")?,
        );
        let public = &packet.state()?.devices[peer].device.encryption_key;
        Ok(Capsule {
            sealed: crypt::seal_info(
                public,
                &signed_bytes("sai-mesh/auto-aad/1", aad)?,
                &plain,
                INFO,
            )?,
        })
    }

    pub(crate) fn open_auto(
        &self,
        scope: &AutoScope,
        peer: &str,
        aad: &BootAad,
        capsule: &Capsule,
    ) -> Result<Zeroizing<String>> {
        aad.validate()?;
        let conn = self.required()?;
        let tx = conn
            .unchecked_transaction()
            .map_err(|_| "자동 연결 승인을 확인하지 못했어요.")?;
        let (key, packet) = scope.load(self, &tx, Some(peer))?;
        let plain = crypt::open_info(
            &key,
            &signed_bytes("sai-mesh/auto-aad/1", aad)?,
            &capsule.sealed,
            INFO,
        )?;
        if plain.len() > 12288 {
            return err("자동 연결 정보의 한도를 넘었어요.");
        }
        let mut message: Signed =
            serde_json::from_slice(&plain).map_err(|_| "자동 연결 정보가 올바르지 않아요.")?;
        let b = &message.body;
        if b.version != 1
            || b.team != scope.team
            || b.author != peer
            || b.receiver != scope.own
            || b.revision != scope.revision
            || b.head != scope.head
            || b.aad != *aad
            || b.card.len() > 8192
        {
            return err("이번 자동 연결의 상대와 도전 값이 일치하지 않아요.");
        }
        verify(
            &packet.state()?.devices[peer].device.signing_key,
            aad.stage.domain(),
            b,
            &message.signature,
        )?;
        Ok(Zeroizing::new(std::mem::take(&mut message.body.card)))
    }
}

#[cfg(all(test, windows))]
#[path = "automatic_tests.rs"]
mod tests;
