use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn local() -> Probe {
    Probe::start_inner(Ipv4Addr::LOCALHOST, false, LIFETIME)
        .await
        .unwrap()
}

fn register(a: &Probe, b: &Probe) -> String {
    a.add_peer(&b.status().card.unwrap()).unwrap()
}

#[tokio::test]
async fn both_devices_must_approve_and_shutdown_releases_the_listener() {
    let a = local().await;
    let b = local().await;
    let bid = register(&a, &b);
    assert!(
        a.checker().check(&bid).await.is_err(),
        "한쪽 등록만으로 인증되면 안 됨"
    );
    let aid = register(&b, &a);
    a.checker().check(&bid).await.unwrap();
    b.checker().check(&aid).await.unwrap();
    assert!(a.status().peers[0].last_success > 0);
    let address = b.inner.card.addresses[0];
    b.shutdown().await;
    assert!(a.checker().check(&bid).await.is_err());
    assert!(a.status().peers[0].last_error.is_some());
    TcpListener::bind(address)
        .await
        .expect("중지한 실험의 포트는 즉시 해제되어야 함");
    a.shutdown().await;
}

#[tokio::test]
async fn a_known_peer_cannot_claim_another_registered_session() {
    let a = local().await;
    let b = local().await;
    let attacker = local().await;
    register(&a, &b);
    register(&a, &attacker);
    register(&attacker, &a);
    let config = attacker.inner.identity.client(&a.inner.card).unwrap();
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
        wire::read(&mut stream).await.is_err(),
        "TLS 인증서와 다른 장치 ID를 사용하면 거부"
    );
}

#[tokio::test]
async fn substituted_certificate_is_rejected_and_keys_never_appear_in_status() {
    let a = local().await;
    let b = local().await;
    let other = local().await;
    register(&b, &a);
    let mut forged = b.inner.card.clone();
    forged.certificate = other.inner.card.certificate.clone();
    let id = a
        .add_peer(&serde_json::to_string(&forged).unwrap())
        .unwrap();
    assert!(a.checker().check(&id).await.is_err());
    let public = serde_json::to_string(&a.status()).unwrap();
    assert!(!public.contains(&STANDARD.encode(&a.inner.identity.key)));
    assert!(!public.contains("PRIVATE KEY"));
    assert!(
        a.add_peer(&b.status().card.unwrap()).is_err(),
        "등록한 세션의 키를 조용히 바꾸면 안 됨"
    );
}

#[tokio::test]
async fn malformed_and_oversized_cards_do_not_change_pairings() {
    let a = local().await;
    let b = local().await;
    assert!(a.add_peer(&a.status().card.unwrap()).is_err());
    assert!(a.add_peer(&"x".repeat(card::MAX_CARD + 1)).is_err());
    for address in ["8.8.8.8:443", "0.0.0.0:9", "224.0.0.1:9", "127.0.0.1:0"] {
        let mut forged = b.inner.card.clone();
        forged.addresses = vec![address.parse().unwrap()];
        assert!(a
            .add_peer(&serde_json::to_string(&forged).unwrap())
            .is_err());
    }
    let mut value = serde_json::to_value(&b.inner.card).unwrap();
    value["command"] = serde_json::json!("run something");
    assert!(a.add_peer(&value.to_string()).is_err());
    assert!(a.status().peers.is_empty());
}

#[tokio::test]
async fn frame_limit_is_checked_before_reading_or_allocating_the_payload() {
    let (mut writer, mut reader) = tokio::io::duplex(32);
    writer.write_u32(u32::MAX).await.unwrap();
    assert!(wire::read(&mut reader).await.is_err());
    let (mut writer, mut reader) = tokio::io::duplex(512);
    let body = br#"{"protocol":1,"session":"0123456789abcdef0123456789abcdef","nonce":"0123456789abcdef0123456789abcdef","command":"exec"}"#;
    writer.write_u32(body.len() as u32).await.unwrap();
    writer.write_all(body).await.unwrap();
    assert!(wire::read(&mut reader).await.is_err());
}

#[tokio::test]
async fn an_expired_or_dropped_experiment_cannot_be_used_again() {
    let a = Probe::start_inner(Ipv4Addr::LOCALHOST, false, Duration::from_millis(300))
        .await
        .unwrap();
    let b = local().await;
    let id = register(&a, &b);
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(!a.status().running);
    assert!(a.checker().check(&id).await.is_err());
    assert!(a.add_peer(&b.status().card.unwrap()).is_err());
    let checker = b.checker();
    let session = a.inner.card.session.clone();
    drop(b);
    assert!(checker.check(&session).await.is_err());
}

#[tokio::test]
async fn an_unapproved_certificate_cannot_get_a_probe_response() {
    let a = local().await;
    let known = local().await;
    let stranger = local().await;
    register(&a, &known);
    let id = register(&stranger, &a);
    assert!(stranger.checker().check(&id).await.is_err());
    assert_eq!(a.status().peers.len(), 1);
}

#[tokio::test]
async fn shutdown_cancels_stalled_inbound_handshakes() {
    let a = local().await;
    let b = local().await;
    register(&a, &b);
    let mut stalled = TcpStream::connect(a.inner.card.addresses[0]).await.unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    a.shutdown().await;
    let mut byte = [0u8; 1];
    let result = tokio::time::timeout(Duration::from_secs(1), stalled.read(&mut byte))
        .await
        .expect("수신 연결이 종료되어야 함");
    assert!(matches!(result, Ok(0)) || result.is_err());
}

#[tokio::test]
async fn stopping_a_probe_also_cancels_an_outbound_handshake() {
    let a = local().await;
    let b = local().await;
    let sink = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let mut card = b.inner.card.clone();
    card.addresses = vec![match sink.local_addr().unwrap() {
        std::net::SocketAddr::V4(address) => address,
        _ => unreachable!(),
    }];
    let id = a.add_peer(&serde_json::to_string(&card).unwrap()).unwrap();
    let (accepted, ready) = tokio::sync::oneshot::channel();
    let sink_task = tokio::spawn(async move {
        let (_socket, _) = sink.accept().await.unwrap();
        let _ = accepted.send(());
        std::future::pending::<()>().await;
    });
    let checker = a.checker();
    let checking = tokio::spawn(async move { checker.check(&id).await });
    ready.await.unwrap();
    a.shutdown().await;
    assert!(tokio::time::timeout(Duration::from_secs(1), checking)
        .await
        .unwrap()
        .unwrap()
        .is_err());
    sink_task.abort();
}

/// 실기기 검증과 구분되는 한 PC의 실제 LAN 멀티캐스트 실험. 주소를 명시한 경우만 실행한다.
#[tokio::test]
#[ignore]
async fn two_sessions_discover_each_other_on_the_selected_lan() {
    let address = std::env::var("SAI_MESH_LAN_TEST")
        .expect("검증할 사설 IPv4 주소 지정")
        .parse()
        .unwrap();
    let a = Probe::start(address).await.unwrap();
    let b = Probe::start(address).await.unwrap();
    assert!(a.discovery_error.is_none(), "{:?}", a.discovery_error);
    assert!(b.discovery_error.is_none(), "{:?}", b.discovery_error);
    let aid = register(&b, &a);
    let bid = register(&a, &b);
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let found_a = b.status().discovered.iter().any(|p| p.session == aid);
            let found_b = a.status().discovered.iter().any(|p| p.session == bid);
            if found_a && found_b {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("선택한 LAN에서 양쪽 mDNS 발견");
    a.checker().check(&bid).await.unwrap();
    b.checker().check(&aid).await.unwrap();
    a.shutdown().await;
    b.shutdown().await;
}
