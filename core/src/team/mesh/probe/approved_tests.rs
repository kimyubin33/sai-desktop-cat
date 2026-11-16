#![cfg(windows)]
use super::*;
use crate::team::mesh::identity::Vault;

struct Pair {
    path: std::path::PathBuf,
    a: Vault,
    b: Vault,
    team: String,
}
impl Pair {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("sai-approved-{}", random_id()));
        std::fs::create_dir_all(&path).unwrap();
        let a = Vault::new(path.join("a.sqlite3"));
        let b = Vault::new(path.join("b.sqlite3"));
        a.create_device().unwrap();
        b.create_device().unwrap();
        let first = a.create_team("승인 연결").unwrap();
        let team = first.manifest.body.team_id.clone();
        b.import(
            &serde_json::to_string(&first).unwrap(),
            &first.manifest.fingerprint().unwrap(),
        )
        .unwrap();
        let approved = a
            .approve(
                &team,
                &serde_json::to_string(&b.request(&team).unwrap()).unwrap(),
            )
            .unwrap();
        b.import(&serde_json::to_string(&approved).unwrap(), "")
            .unwrap();
        Self { path, a, b, team }
    }
    async fn start(&self, vault: &Vault) -> Probe {
        Probe::start_mode(
            Ipv4Addr::LOCALHOST,
            false,
            LIFETIME,
            Some((vault.clone(), self.team.clone())),
        )
        .await
        .unwrap()
    }
}
impl Drop for Pair {
    fn drop(&mut self) {
        assert_eq!(self.path.parent(), Some(std::env::temp_dir().as_path()));
        assert!(self
            .path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("sai-approved-"));
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[tokio::test]
async fn approved_devices_bind_real_mutual_tls_to_their_persistent_identity() {
    let pair = Pair::new();
    let a = pair.start(&pair.a).await;
    let b = pair.start(&pair.b).await;
    let b_id = a
        .checker()
        .add_peer(b.status().card.unwrap())
        .await
        .unwrap();
    let a_id = b
        .checker()
        .add_peer(a.status().card.unwrap())
        .await
        .unwrap();
    a.checker().check(&b_id).await.unwrap();
    b.checker().check(&a_id).await.unwrap();
    assert_eq!(a.status().team_id.as_deref(), Some(pair.team.as_str()));
    assert_eq!(
        a.status().device_id,
        Some(pair.a.status().unwrap().device.unwrap().device_id)
    );
    let address = a.inner.card.addresses[0];
    a.shutdown().await;
    b.shutdown().await;
    assert!(TcpListener::bind(address).await.is_ok());
}

#[tokio::test]
async fn tampered_certificates_addresses_proofs_and_plain_cards_are_rejected_before_pairing() {
    let pair = Pair::new();
    let a = pair.start(&pair.a).await;
    let b = pair.start(&pair.b).await;
    let card = b.status().card.unwrap();
    for field in [
        "certificate",
        "addresses",
        "signature",
        "team",
        "device",
        "revision",
    ] {
        let mut value: serde_json::Value = serde_json::from_str(&card).unwrap();
        match field {
            "certificate" => {
                value["card"]["certificate"] = serde_json::json!(a.inner.card.certificate)
            }
            "addresses" => value["card"]["addresses"] = serde_json::json!(["127.0.0.1:1"]),
            "signature" => value["proof"]["signature"] = serde_json::json!("00".repeat(64)),
            "team" => value["proof"]["body"]["team_id"] = serde_json::json!("00".repeat(32)),
            "device" => {
                value["proof"]["body"]["device_id"] = serde_json::json!(a.status().device_id)
            }
            _ => value["proof"]["body"]["revision"] = serde_json::json!(1),
        }
        assert!(
            a.checker().add_peer(value.to_string()).await.is_err(),
            "{field}"
        );
    }
    assert!(a
        .add_peer(&serde_json::to_string(&b.inner.card).unwrap())
        .is_err());
    assert!(a.status().peers.is_empty());
    assert!(a.add_peer(&"x".repeat(16385)).is_err());
    a.shutdown().await;
    b.shutdown().await;
}

#[tokio::test]
async fn revocation_after_pairing_is_checked_again_on_the_actual_connection() {
    let pair = Pair::new();
    let a = pair.start(&pair.a).await;
    let b = pair.start(&pair.b).await;
    let b_id = a.add_peer(&b.status().card.unwrap()).unwrap();
    b.add_peer(&a.status().card.unwrap()).unwrap();
    a.checker().check(&b_id).await.unwrap();
    let device = pair.b.status().unwrap().device.unwrap().device_id;
    pair.a.revoke(&pair.team, &device).unwrap();
    assert!(a.checker().check(&b_id).await.is_err());
    // B still has the old approval history. B's TLS passes, but A's receiver must refuse it.
    let config = b.inner.identity.client(&a.inner.card).unwrap();
    let mut config = config;
    config.alpn_protocols = vec![b"sai-approved-probe/1".to_vec()];
    let socket = TcpStream::connect(a.inner.card.addresses[0]).await.unwrap();
    let mut stream = TlsConnector::from(Arc::new(config))
        .connect(NAME.try_into().unwrap(), socket)
        .await
        .unwrap();
    wire::write(
        &mut stream,
        &Signal {
            protocol: VERSION,
            session: b.inner.card.session.clone(),
            nonce: random_id(),
        },
    )
    .await
    .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(4), wire::read(&mut stream))
            .await
            .unwrap()
            .is_err()
    );
    a.shutdown().await;
    b.shutdown().await;
}

#[tokio::test]
async fn changing_membership_requires_new_proofs_and_does_not_downgrade_to_plain_probe() {
    let pair = Pair::new();
    let a = pair.start(&pair.a).await;
    let b = pair.start(&pair.b).await;
    let b_id = a.add_peer(&b.status().card.unwrap()).unwrap();
    b.add_peer(&a.status().card.unwrap()).unwrap();
    let packet = pair.a.export(&pair.team).unwrap();
    let extra = Vault::new(pair.path.join("extra.sqlite3"));
    extra.create_device().unwrap();
    extra
        .import(
            &serde_json::to_string(&packet).unwrap(),
            &packet.manifest.fingerprint().unwrap(),
        )
        .unwrap();
    let next = pair
        .a
        .approve(
            &pair.team,
            &serde_json::to_string(&extra.request(&pair.team).unwrap()).unwrap(),
        )
        .unwrap();
    pair.b
        .import(&serde_json::to_string(&next).unwrap(), "")
        .unwrap();
    assert!(a.checker().check(&b_id).await.is_err());
    a.shutdown().await;
    b.shutdown().await;
    let a = pair.start(&pair.a).await;
    let b = pair.start(&pair.b).await;
    let b_id = a.add_peer(&b.status().card.unwrap()).unwrap();
    b.add_peer(&a.status().card.unwrap()).unwrap();
    a.checker().check(&b_id).await.unwrap();
    a.shutdown().await;
    b.shutdown().await;
}

#[tokio::test]
async fn unapproved_devices_cannot_create_signed_connection_cards() {
    let pair = Pair::new();
    let stranger = Vault::new(pair.path.join("stranger.sqlite3"));
    stranger.create_device().unwrap();
    let packet = pair.a.export(&pair.team).unwrap();
    stranger
        .import(
            &serde_json::to_string(&packet).unwrap(),
            &packet.manifest.fingerprint().unwrap(),
        )
        .unwrap();
    assert!(Probe::start_mode(
        Ipv4Addr::LOCALHOST,
        false,
        LIFETIME,
        Some((stranger, pair.team.clone()))
    )
    .await
    .is_err());
}

#[tokio::test]
async fn an_approved_certificate_cannot_negotiate_the_unauthenticated_probe_protocol() {
    let pair = Pair::new();
    let a = pair.start(&pair.a).await;
    let b = pair.start(&pair.b).await;
    a.add_peer(&b.status().card.unwrap()).unwrap();
    b.add_peer(&a.status().card.unwrap()).unwrap();
    // All certificates are correct, but the client offers only the legacy ALPN.
    let config = b.inner.identity.client(&a.inner.card).unwrap();
    let socket = TcpStream::connect(a.inner.card.addresses[0]).await.unwrap();
    let result = tokio::time::timeout(
        Duration::from_secs(4),
        TlsConnector::from(Arc::new(config)).connect(NAME.try_into().unwrap(), socket),
    )
    .await
    .unwrap();
    assert!(result.is_err());
    a.shutdown().await;
    b.shutdown().await;
}

async fn transfer_pair(pair: &Pair) -> (Probe, Probe, String) {
    let a = Probe::start_mode_exchange(
        Ipv4Addr::LOCALHOST,
        false,
        LIFETIME,
        Some((pair.a.clone(), pair.team.clone())),
        true,
    )
    .await
    .unwrap();
    let b = Probe::start_mode_exchange(
        Ipv4Addr::LOCALHOST,
        false,
        LIFETIME,
        Some((pair.b.clone(), pair.team.clone())),
        true,
    )
    .await
    .unwrap();
    let id = a.add_peer(&b.status().card.unwrap()).unwrap();
    b.add_peer(&a.status().card.unwrap()).unwrap();
    (a, b, id)
}

#[tokio::test]
async fn pc_posts_and_signed_deletes_cross_real_tls_and_keep_the_body_hidden_after_reconnect() {
    let pair = Pair::new();
    let scope = pair.a.talk_feed(&pair.team, 0).unwrap().scope;
    let target = pair
        .a
        .post_talk(&scope, &Vault::local_draft_id().unwrap(), "PC 톡 TLS 원문")
        .unwrap();
    let (a, b, id) = transfer_pair(&pair).await;
    a.checker().exchange(&id).await.unwrap();
    assert_eq!(
        pair.b.talk_feed(&pair.team, 0).unwrap().messages[0].text,
        "PC 톡 TLS 원문"
    );
    pair.a
        .delete_talk(&scope, &Vault::local_draft_id().unwrap(), &target, 1)
        .unwrap();
    a.checker().exchange(&id).await.unwrap();
    assert!(pair.b.talk_feed(&pair.team, 0).unwrap().messages[0].deleted);
    assert_eq!(
        pair.a.talk_feed(&pair.team, 0).unwrap().messages[0].delete_state,
        Some("reference-stored")
    );
    a.shutdown().await;
    b.shutdown().await;
    let (a, b, id) = transfer_pair(&pair).await;
    a.checker().exchange(&id).await.unwrap();
    assert!(pair.b.talk_feed(&pair.team, 0).unwrap().messages[0]
        .text
        .is_empty());
    a.shutdown().await;
    b.shutdown().await;
}

#[tokio::test]
async fn record_format_capability_is_signed_and_legacy_exchange_is_refused() {
    use crate::team::mesh::identity::session::binding;
    let pair = Pair::new();
    let (a, b, _) = transfer_pair(&pair).await;
    let mut card = b.inner.approved.as_ref().unwrap().own.clone();
    assert_eq!(card.record_version, 2);
    card.record_version = 0;
    assert!(a.add_peer(&serde_json::to_string(&card).unwrap()).is_err());
    let digest = binding(&serde_json::to_vec(&(card.card.clone(), true)).unwrap());
    card.proof = pair.b.prove_session(&pair.team, digest).unwrap();
    let error = a
        .add_peer(&serde_json::to_string(&card).unwrap())
        .unwrap_err();
    assert!(error.contains("업데이트"));
    a.shutdown().await;
    b.shutdown().await;
}

#[tokio::test]
async fn encrypted_records_and_committed_receipts_cross_real_mutual_tls_in_both_directions() {
    let pair = Pair::new();
    pair.a
        .append_local(
            &pair.team,
            &crate::team::mesh::identity::Vault::local_draft_id().unwrap(),
            "A의 암호화 톡",
        )
        .unwrap();
    pair.b
        .append_local(
            &pair.team,
            &crate::team::mesh::identity::Vault::local_draft_id().unwrap(),
            "B의 암호화 톡",
        )
        .unwrap();
    let (a, b, id) = transfer_pair(&pair).await;
    let report = a.checker().exchange(&id).await.unwrap();
    assert_eq!((report.sent, report.received), (1, 1));
    assert!(!report.more);
    let left = pair.a.local_status(&pair.team).unwrap();
    let right = pair.b.local_status(&pair.team).unwrap();
    assert_eq!(
        (left.pending, right.pending, left.received, right.received),
        (0, 0, 1, 1)
    );
    assert_eq!(
        left.messages.iter().find(|m| m.incoming).unwrap().text,
        "B의 암호화 톡"
    );
    assert_eq!(
        right.messages.iter().find(|m| m.incoming).unwrap().text,
        "A의 암호화 톡"
    );
    let repeat = a.checker().exchange(&id).await.unwrap();
    assert_eq!((repeat.sent, repeat.received), (0, 0));
    assert_eq!(pair.b.local_status(&pair.team).unwrap().received, 1);
    a.shutdown().await;
    b.shutdown().await;
}

#[tokio::test]
async fn data_exchange_requires_both_signed_consents_and_a_card_cannot_upgrade_them() {
    let pair = Pair::new();
    let a = Probe::start_mode_exchange(
        Ipv4Addr::LOCALHOST,
        false,
        LIFETIME,
        Some((pair.a.clone(), pair.team.clone())),
        true,
    )
    .await
    .unwrap();
    let b = pair.start(&pair.b).await;
    let raw = b.status().card.unwrap();
    let mut forged: serde_json::Value = serde_json::from_str(&raw).unwrap();
    forged["exchange"] = serde_json::json!(true);
    assert!(a.add_peer(&forged.to_string()).is_err());
    let id = a.add_peer(&raw).unwrap();
    b.add_peer(&a.status().card.unwrap()).unwrap();
    a.checker().check(&id).await.unwrap();
    assert!(a.checker().exchange(&id).await.is_err());
    assert!(!b.status().exchange_enabled);
    a.shutdown().await;
    b.shutdown().await;
}

#[tokio::test]
async fn receiver_disk_error_closes_exchange_without_confirmation_and_retry_recovers() {
    let pair = Pair::new();
    pair.a
        .append_local(
            &pair.team,
            &crate::team::mesh::identity::Vault::local_draft_id().unwrap(),
            "실제 연결의 저장 실패",
        )
        .unwrap();
    let conn = rusqlite::Connection::open(pair.path.join("b.sqlite3")).unwrap();
    conn.execute_batch("CREATE TRIGGER deny_network_write BEFORE INSERT ON mesh_received BEGIN SELECT RAISE(ABORT,'test failure'); END;").unwrap();
    let (a, b, id) = transfer_pair(&pair).await;
    assert!(a.checker().exchange(&id).await.is_err());
    assert_eq!(pair.a.local_status(&pair.team).unwrap().pending, 1);
    assert_eq!(pair.b.local_status(&pair.team).unwrap().received, 0);
    conn.execute_batch("DROP TRIGGER deny_network_write;")
        .unwrap();
    a.checker().exchange(&id).await.unwrap();
    assert_eq!(pair.a.local_status(&pair.team).unwrap().pending, 0);
    assert_eq!(pair.b.local_status(&pair.team).unwrap().received, 1);
    a.shutdown().await;
    b.shutdown().await;
}

#[tokio::test]
async fn transfer_pages_are_bounded_and_a_second_exchange_finishes_the_remaining_records() {
    let pair = Pair::new();
    for n in 0..10 {
        pair.a
            .append_local(
                &pair.team,
                &crate::team::mesh::identity::Vault::local_draft_id().unwrap(),
                &format!("쪽 나누기 {n}"),
            )
            .unwrap();
    }
    let (a, b, id) = transfer_pair(&pair).await;
    let first = a.checker().exchange(&id).await.unwrap();
    assert_eq!(first.sent, 8);
    assert!(first.more);
    assert_eq!(pair.a.local_status(&pair.team).unwrap().pending, 2);
    let second = a.checker().exchange(&id).await.unwrap();
    assert_eq!(second.sent, 2);
    assert!(!second.more);
    assert_eq!(pair.b.local_status(&pair.team).unwrap().received, 10);
    a.shutdown().await;
    b.shutdown().await;
}

#[tokio::test]
async fn maximum_valid_escaped_body_fits_the_encrypted_wire_frame() {
    let pair = Pair::new();
    let text = format!("{}x", "\n\"".repeat(16383));
    pair.a
        .append_local(
            &pair.team,
            &crate::team::mesh::identity::Vault::local_draft_id().unwrap(),
            &text,
        )
        .unwrap();
    let (a, b, id) = transfer_pair(&pair).await;
    a.checker().exchange(&id).await.unwrap();
    assert_eq!(
        pair.b.local_status(&pair.team).unwrap().messages[0].text,
        text
    );
    a.shutdown().await;
    b.shutdown().await;
}
