//! RFC 9180: DHKEM(X25519, HKDF-SHA256), HKDF-SHA256, ChaCha20Poly1305, Base mode.
use super::keys::{unhex, Material};
use crate::{err, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use hpke::{
    aead::ChaCha20Poly1305, kdf::HkdfSha256, kem::X25519HkdfSha256, Deserializable, Kem,
    Serializable,
};
use rand09::SeedableRng;
use ring::rand::SecureRandom;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

type K = X25519HkdfSha256;
const INFO: &[u8] = b"sai-mesh/body/1";
const SUITE: &str = "hpke-x25519-sha256-chacha20poly1305/1";

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CipherBox {
    pub suite: String,
    pub encapped: String,
    pub ciphertext: String,
}

pub(super) fn seal(public: &str, aad: &[u8], plain: &[u8]) -> Result<CipherBox> {
    seal_info(public, aad, plain, INFO)
}

pub(super) fn seal_info(public: &str, aad: &[u8], plain: &[u8], info: &[u8]) -> Result<CipherBox> {
    let public = <K as Kem>::PublicKey::from_bytes(&unhex::<32>(public)?)
        .map_err(|_| "수신 장치 공개키가 올바르지 않아요.")?;
    let mut seed = Zeroizing::new([0u8; 32]);
    ring::rand::SystemRandom::new()
        .fill(&mut *seed)
        .map_err(|_| "암호화용 난수를 생성하지 못했어요.")?;
    let mut rng = rand09::rngs::StdRng::from_seed(*seed);
    let (encapped, ciphertext) = hpke::single_shot_seal::<ChaCha20Poly1305, HkdfSha256, K, _>(
        &hpke::OpModeS::Base,
        &public,
        info,
        plain,
        aad,
        &mut rng,
    )
    .map_err(|_| "수신 장치용 본문을 암호화하지 못했어요.")?;
    Ok(CipherBox {
        suite: SUITE.into(),
        encapped: super::keys::hex(&encapped.to_bytes()),
        ciphertext: STANDARD.encode(ciphertext),
    })
}

pub(super) fn open(key: &Material, aad: &[u8], sealed: &CipherBox) -> Result<Zeroizing<Vec<u8>>> {
    open_info(key, aad, sealed, INFO)
}

pub(super) fn open_info(
    key: &Material,
    aad: &[u8],
    sealed: &CipherBox,
    info: &[u8],
) -> Result<Zeroizing<Vec<u8>>> {
    if sealed.suite != SUITE || sealed.ciphertext.len() > 91000 {
        return err("지원하지 않는 본문 암호화 형식이에요.");
    }
    let seed = key.decryption_seed()?;
    let secret = <K as Kem>::PrivateKey::from_bytes(&*seed)
        .map_err(|_| "보관한 장치 암호화 키를 확인하지 못했어요.")?;
    let encapped = <K as Kem>::EncappedKey::from_bytes(&unhex::<32>(&sealed.encapped)?)
        .map_err(|_| "암호화된 본문 키가 올바르지 않아요.")?;
    let bytes = STANDARD
        .decode(&sealed.ciphertext)
        .map_err(|_| "암호문 형식이 올바르지 않아요.")?;
    if bytes.len() > super::journal::MAX_TEXT * 2 + 2048 {
        return err("암호문 크기 한도를 넘었어요.");
    }
    hpke::single_shot_open::<ChaCha20Poly1305, HkdfSha256, K>(
        &hpke::OpModeR::Base,
        &secret,
        &encapped,
        info,
        &bytes,
        aad,
    )
    .map(Zeroizing::new)
    .map_err(|_| "이 수신 장치에서 본문을 복호화하지 못했어요.".into())
}
