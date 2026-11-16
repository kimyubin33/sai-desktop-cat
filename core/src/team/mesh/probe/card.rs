use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::net::{Ipv4Addr, SocketAddrV4};

use crate::{err, Result};

pub const VERSION: u8 = 1;
pub const MAX_CARD: usize = 8192;
pub const MAX_PEERS: usize = 8;
pub const NAME: &str = "sai-probe.local";

/// 공개 인증서만 담는다. 이 정보는 사용자가 별도 경로로 확인하고 양쪽에서 등록한다.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeerCard {
    pub protocol: u8,
    pub session: String,
    pub certificate: String,
    pub addresses: Vec<SocketAddrV4>,
}

pub fn fingerprint(der: &[u8]) -> String {
    format!("{:x}", Sha256::digest(der))
}

pub fn valid_session(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

pub fn permitted_ip(ip: Ipv4Addr) -> bool {
    ip.is_private() || ip.is_loopback()
}

impl PeerCard {
    pub fn parse(input: &str) -> Result<Self> {
        if input.len() > MAX_CARD {
            return err("연결 정보가 너무 길어요.");
        }
        let card: Self = serde_json::from_str(input)
            .map_err(|_| "상대 PC의 연결 정보를 그대로 붙여 넣어 주세요.")?;
        card.validate()?;
        Ok(card)
    }

    pub fn validate(&self) -> Result<()> {
        if self.protocol != VERSION || !valid_session(&self.session) {
            return err("호환되는 PC 연결 실험 정보가 아니에요.");
        }
        if self.addresses.is_empty()
            || self.addresses.len() > 4
            || self
                .addresses
                .iter()
                .any(|a| a.port() == 0 || !permitted_ip(*a.ip()))
        {
            return err("이번 실험은 사설 IPv4 LAN 주소만 지원해요.");
        }
        self.der()?;
        Ok(())
    }

    pub fn der(&self) -> Result<Vec<u8>> {
        if self.certificate.len() > 4096 {
            return err("인증서 정보가 너무 커요.");
        }
        let der = STANDARD
            .decode(&self.certificate)
            .map_err(|_| "상대 인증서 형식을 확인해 주세요.")?;
        if der.is_empty() || der.len() > 2048 {
            return err("상대 인증서 크기를 확인해 주세요.");
        }
        Ok(der)
    }

    pub fn fingerprint(&self) -> Result<String> {
        Ok(fingerprint(&self.der()?))
    }
}
