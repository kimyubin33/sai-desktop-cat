use std::sync::Arc;

use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::{ClientConfig, RootCertStore, ServerConfig};

use super::card::{PeerCard, NAME};
use crate::{err, Result};

pub(super) struct Identity {
    pub certificate: Vec<u8>,
    pub(super) key: Vec<u8>,
}

impl Drop for Identity {
    fn drop(&mut self) {
        zeroize::Zeroize::zeroize(&mut self.key);
    }
}

impl Identity {
    pub fn new() -> Result<Self> {
        let identity = rcgen::generate_simple_self_signed(vec![NAME.into()])
            .map_err(|_| "연결 실험의 임시 인증서를 만들지 못했어요.")?;
        Ok(Self {
            certificate: identity.cert.der().to_vec(),
            key: identity.signing_key.serialize_der(),
        })
    }

    fn key(&self) -> PrivateKeyDer<'static> {
        PrivatePkcs8KeyDer::from(self.key.clone()).into()
    }

    pub fn client(&self, peer: &PeerCard) -> Result<ClientConfig> {
        let mut roots = RootCertStore::empty();
        roots
            .add(CertificateDer::from(peer.der()?))
            .map_err(|_| "상대 인증서를 읽지 못했어요.")?;
        let mut config =
            ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_protocol_versions(&[&rustls::version::TLS13])
                .map_err(|_| "TLS 설정을 만들지 못했어요.")?
                .with_root_certificates(roots)
                .with_client_auth_cert(
                    vec![CertificateDer::from(self.certificate.clone())],
                    self.key(),
                )
                .map_err(|_| "임시 장치 인증을 설정하지 못했어요.")?;
        config.alpn_protocols = vec![b"sai-probe/1".to_vec()];
        Ok(config)
    }

    pub fn server(&self, peers: &[PeerCard]) -> Result<ServerConfig> {
        if peers.is_empty() {
            return err("먼저 상대 PC의 연결 정보를 등록해 주세요.");
        }
        let mut roots = RootCertStore::empty();
        for peer in peers {
            roots
                .add(CertificateDer::from(peer.der()?))
                .map_err(|_| "등록한 인증서를 읽지 못했어요.")?;
        }
        let verifier = rustls::server::WebPkiClientVerifier::builder_with_provider(
            Arc::new(roots),
            Arc::new(rustls::crypto::ring::default_provider()),
        )
        .build()
        .map_err(|_| "상대 장치 인증을 설정하지 못했어요.")?;
        let mut config =
            ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_protocol_versions(&[&rustls::version::TLS13])
                .map_err(|_| "TLS 설정을 만들지 못했어요.")?
                .with_client_cert_verifier(verifier)
                .with_single_cert(
                    vec![CertificateDer::from(self.certificate.clone())],
                    self.key(),
                )
                .map_err(|_| "수신 인증서를 설정하지 못했어요.")?;
        config.alpn_protocols = vec![b"sai-probe/1".to_vec()];
        // 실험 종료·등록 변경 뒤 이전 세션이 인증을 우회해 재개되지 않게 한다.
        config.send_tls13_tickets = 0;
        Ok(config)
    }
}
