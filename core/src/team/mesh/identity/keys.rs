use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::{self, Ed25519KeyPair, KeyPair};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::Zeroize;

use super::model::DevicePublic;
use crate::{err, Result};

pub(super) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub(super) fn unhex<const N: usize>(input: &str) -> Result<[u8; N]> {
    if input.len() != N * 2
        || !input
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return err("공개키 또는 서명 형식이 올바르지 않아요.");
    }
    let mut bytes = [0; N];
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&input[i * 2..i * 2 + 2], 16)
            .map_err(|_| "잘못된 키 형식이에요.")?;
    }
    Ok(bytes)
}

pub(super) fn digest(domain: &str, bytes: &[u8]) -> String {
    let mut hash = Sha256::new();
    hash.update(domain.as_bytes());
    hash.update([0]);
    hash.update(bytes);
    hex(&hash.finalize())
}

pub(super) fn random_id() -> Result<String> {
    let mut bytes = [0; 16];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| "안전한 난수를 생성하지 못했어요.")?;
    Ok(hex(&bytes))
}

pub(super) fn signed_bytes<T: Serialize>(domain: &str, value: &T) -> Result<Vec<u8>> {
    let mut bytes = domain.as_bytes().to_vec();
    bytes.push(0);
    // 고정된 구조체의 필드 순서와 정수·문자열 형식이 v1 서명 규약이다.
    bytes.extend(serde_json::to_vec(value).map_err(|_| "승인 자료를 직렬화하지 못했어요.")?);
    Ok(bytes)
}

pub(super) fn verify<T: Serialize>(
    public: &str,
    domain: &str,
    body: &T,
    proof: &str,
) -> Result<()> {
    let key = unhex::<32>(public)?;
    let signature = unhex::<64>(proof)?;
    signature::UnparsedPublicKey::new(&signature::ED25519, key)
        .verify(&signed_bytes(domain, body)?, &signature)
        .map_err(|_| "장치 또는 관리자 서명을 확인하지 못했어요.".into())
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Material {
    version: u8,
    signing: Vec<u8>,
    encryption: Option<[u8; 32]>,
}

impl Drop for Material {
    fn drop(&mut self) {
        self.signing.zeroize();
        self.encryption.zeroize();
    }
}

impl Material {
    pub fn generate(device: bool) -> Result<Self> {
        let signing = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new())
            .map_err(|_| "서명 키를 생성하지 못했어요.")?
            .as_ref()
            .to_vec();
        let encryption = if device {
            let mut bytes = [0; 32];
            SystemRandom::new()
                .fill(&mut bytes)
                .map_err(|_| "암호화 키를 생성하지 못했어요.")?;
            Some(bytes)
        } else {
            None
        };
        Ok(Self {
            version: 1,
            signing,
            encryption,
        })
    }

    fn pair(&self) -> Result<Ed25519KeyPair> {
        if self.version != 1 {
            return err("지원하지 않는 개인키 버전이에요.");
        }
        Ed25519KeyPair::from_pkcs8(&self.signing)
            .map_err(|_| "보관된 개인키가 손상됐어요. 자동으로 교체하지 않았어요.".into())
    }

    pub fn signing_public(&self) -> Result<String> {
        Ok(hex(self.pair()?.public_key().as_ref()))
    }

    pub fn sign<T: Serialize>(&self, domain: &str, value: &T) -> Result<String> {
        Ok(hex(self
            .pair()?
            .sign(&signed_bytes(domain, value)?)
            .as_ref()))
    }

    pub fn device(&self) -> Result<DevicePublic> {
        let seed = self.encryption.ok_or("장치 암호화 키가 없어요.")?;
        let encryption_key = hex(PublicKey::from(&StaticSecret::from(seed)).as_bytes());
        let signing_key = self.signing_public()?;
        let device_id = digest(
            "sai-mesh/device-id/1",
            format!("{signing_key}:{encryption_key}").as_bytes(),
        );
        Ok(DevicePublic {
            version: 1,
            device_id,
            signing_key,
            encryption_key,
        })
    }

    pub fn is_device(&self) -> bool {
        self.encryption.is_some()
    }

    pub(super) fn decryption_seed(&self) -> Result<zeroize::Zeroizing<[u8; 32]>> {
        self.encryption
            .map(zeroize::Zeroizing::new)
            .ok_or_else(|| "장치 암호화 키가 없어요.".into())
    }
}
