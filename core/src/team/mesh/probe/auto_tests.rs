use super::*;

#[test]
fn retries_back_off_without_exceeding_the_limit() {
    let options = Options::default();
    assert_eq!(retry_delay(&options, 1, 0), Duration::from_secs(1));
    assert_eq!(retry_delay(&options, 2, 0), Duration::from_secs(2));
    assert!(retry_delay(&options, 3, 255) > Duration::from_secs(4));
    assert_eq!(retry_delay(&options, 100, 255), Duration::from_secs(30));
}

#[test]
fn only_selected_subnet_unicast_endpoints_are_eligible() {
    let network = Network {
        address: Ipv4Addr::new(192, 168, 1, 10),
        mask: Ipv4Addr::new(255, 255, 255, 0),
    };
    assert!(network.permits("192.168.1.22:5000".parse().unwrap()));
    for address in [
        "192.168.2.22:5000",
        "192.168.1.0:5000",
        "192.168.1.255:5000",
        "192.168.1.22:0",
        "8.8.8.8:5000",
        "127.0.0.1:5000",
    ] {
        assert!(!network.permits(address.parse().unwrap()), "{address}");
    }
}

#[tokio::test]
async fn bootstrap_frames_are_bounded_before_payload_allocation() {
    let (mut a, mut b) = tokio::io::duplex(16);
    a.write_u32((MAX_FRAME + 1) as u32).await.unwrap();
    assert!(read(&mut b).await.is_err());
}

#[cfg(windows)]
mod windows {
    use super::*;
    use crate::team::mesh::identity::Vault;
    struct Team {
        dir: std::path::PathBuf,
        a: Vault,
        b: Vault,
        team: String,
    }
    impl Team {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!("sai-auto-net-{}", random_id()));
            std::fs::create_dir_all(&dir).unwrap();
            let a = Vault::new(dir.join("a.sqlite3"));
            let b = Vault::new(dir.join("b.sqlite3"));
            a.create_device().unwrap();
            b.create_device().unwrap();
            let first = a.create_team("자동 연결 검증").unwrap();
            let team = first.manifest.body.team_id.clone();
            b.import(
                &serde_json::to_string(&first).unwrap(),
                &first.manifest.fingerprint().unwrap(),
            )
            .unwrap();
            let packet = a
                .approve(
                    &team,
                    &serde_json::to_string(&b.request(&team).unwrap()).unwrap(),
                )
                .unwrap();
            b.import(&serde_json::to_string(&packet).unwrap(), "")
                .unwrap();
            Self { dir, a, b, team }
        }
        async fn start(&self, vault: &Vault) -> Probe {
            Probe::start_mode_auto(
                Ipv4Addr::LOCALHOST,
                false,
                LIFETIME,
                Some((vault.clone(), self.team.clone())),
                true,
                Some(Options::testing()),
            )
            .await
            .unwrap()
        }
        fn append(&self, vault: &Vault, text: &str) {
            vault
                .append_local(&self.team, &Vault::local_draft_id().unwrap(), text)
                .unwrap();
        }
    }
    impl Drop for Team {
        fn drop(&mut self) {
            assert_eq!(self.dir.parent(), Some(std::env::temp_dir().as_path()));
            assert!(self
                .dir
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("sai-auto-net-"));
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
    async fn wait_until(mut ready: impl FnMut() -> bool) {
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                if ready() {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .expect("automatic connection did not converge");
    }
    fn announce(from: &Probe, to: &Probe) {
        assert!(to.inner.automatic.as_ref().unwrap().ingest(
            from.inner
                .automatic
                .as_ref()
                .unwrap()
                .advertisement()
                .unwrap()
        ));
    }
    async fn wait_pair(a: &Probe, b: &Probe, mut ready: impl FnMut() -> bool) {
        let result = tokio::time::timeout(Duration::from_secs(60), async {
            loop {
                if ready() {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await;
        assert!(
            result.is_ok(),
            "automatic pair did not converge: a={}, b={}",
            serde_json::to_string(&a.status().automatic).unwrap(),
            serde_json::to_string(&b.status().automatic).unwrap()
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn one_discovered_endpoint_authenticates_both_peers_and_exchanges_without_manual_pairing()
    {
        let t = Team::new();
        t.append(&t.a, "자동 연결 A");
        t.append(&t.b, "자동 연결 B");
        let a = t.start(&t.a).await;
        let b = t.start(&t.b).await;
        announce(&b, &a);
        wait_pair(&a, &b, || {
            let left = t.a.local_status(&t.team).unwrap();
            let right = t.b.local_status(&t.team).unwrap();
            left.received == 1 && right.received == 1 && left.pending == 0 && right.pending == 0
        })
        .await;
        assert_eq!(t.a.local_status(&t.team).unwrap().pending, 0);
        assert_eq!(t.b.local_status(&t.team).unwrap().pending, 0);
        assert_eq!(a.status().peers.len(), 1);
        assert_eq!(b.status().peers.len(), 1);
        a.shutdown().await;
        b.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn manually_registered_candidate_still_completes_fresh_reciprocal_bootstrap() {
        let t = Team::new();
        t.append(&t.a, "수동 등록 뒤 자동 재연결");
        t.append(&t.b, "반대 방향 기록");
        let a = t.start(&t.a).await;
        let b = t.start(&t.b).await;
        let session = b
            .checker()
            .add_peer(a.status().card.unwrap())
            .await
            .unwrap();
        assert!(!b.inner.peers.lock().unwrap()[&session].bootstrapped);
        assert!(a.status().peers.is_empty());
        wait_until(|| b.status().automatic.paired == 1).await;
        announce(&a, &b);
        wait_pair(&a, &b, || {
            t.a.local_status(&t.team).unwrap().pending == 0
                && t.b.local_status(&t.team).unwrap().pending == 0
        })
        .await;
        assert!(b.inner.peers.lock().unwrap()[&session].bootstrapped);
        assert_eq!(a.status().peers.len(), 1);
        a.shutdown().await;
        b.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn restarting_a_peer_refreshes_its_certificate_without_duplicate_records() {
        let t = Team::new();
        t.append(&t.a, "첫 기록");
        let a = t.start(&t.a).await;
        let b = t.start(&t.b).await;
        announce(&b, &a);
        wait_until(|| t.b.local_status(&t.team).unwrap().received == 1).await;
        let old = b.inner.automatic.as_ref().unwrap().advertisement().unwrap();
        let old_fp = b.status().fingerprint;
        b.shutdown().await;
        t.append(&t.a, "재접속 뒤 대기 기록");
        let b = t.start(&t.b).await;
        assert_ne!(old_fp, b.status().fingerprint);
        announce(&b, &a);
        wait_pair(&a, &b, || {
            t.b.local_status(&t.team).unwrap().received == 2
                && t.a.local_status(&t.team).unwrap().pending == 0
        })
        .await;
        assert_eq!(a.status().peers.len(), 1);
        assert_eq!(a.status().peers[0].session, b.inner.card.session);
        a.inner.automatic.as_ref().unwrap().ingest(old);
        t.append(&t.a, "과거 광고 재입력 뒤 기록");
        a.checker().wake();
        wait_until(|| t.b.local_status(&t.team).unwrap().received == 3).await;
        assert_eq!(a.status().peers[0].session, b.inner.card.session);
        a.shutdown().await;
        b.shutdown().await;
    }

    #[tokio::test]
    async fn unknown_and_out_of_subnet_advertisements_never_create_candidates() {
        let t = Team::new();
        let a = t.start(&t.a).await;
        let b = t.start(&t.b).await;
        let state = a.inner.automatic.as_ref().unwrap();
        let real = b.inner.automatic.as_ref().unwrap().advertisement().unwrap();
        let encoded = serde_json::to_string(&real).unwrap();
        assert!(!encoded.contains(&t.team));
        assert!(!encoded.contains(&b.status().device_id.unwrap()));
        assert!(!encoded.contains("certificate"));
        let mut bad = real.clone();
        bad.tags = vec!["00".repeat(32)];
        assert!(!state.ingest(bad));
        let mut bad = real.clone();
        bad.addresses = vec!["192.168.0.2:7000".parse().unwrap()];
        assert!(!state.ingest(bad));
        let mut bad = real.clone();
        bad.version = 2;
        assert!(!state.ingest(bad));
        assert_eq!(state.status(true).discovered, 0);
        assert!(a.status().peers.is_empty());
        a.shutdown().await;
        b.shutdown().await;
    }

    #[tokio::test]
    async fn approval_changes_stop_the_runtime_and_release_the_listener() {
        let t = Team::new();
        let a = t.start(&t.a).await;
        let address = a.inner.card.addresses[0];
        let id = t.a.status().unwrap().device.unwrap().device_id;
        t.a.revoke(&t.team, &id).unwrap();
        wait_until(|| !a.status().running).await;
        assert!(a.status().notice.is_some());
        wait_until(|| std::net::TcpListener::bind(address).is_ok()).await;
        a.shutdown().await;
    }

    #[tokio::test]
    async fn a_local_source_conflict_stops_automation_without_resetting_the_source() {
        let t = Team::new();
        t.append(&t.a, "보존할 원본");
        let a = t.start(&t.a).await;
        let own = t.a.status().unwrap().device.unwrap().device_id;
        let peer = t.b.status().unwrap().device.unwrap().device_id;
        let conn = rusqlite::Connection::open(t.dir.join("a.sqlite3")).unwrap();
        conn.execute("INSERT INTO mesh_transfer_blocks(team,author,peer,expected,observed) VALUES(?1,?2,?3,?4,?5)",rusqlite::params![t.team,own,peer,"00".repeat(32),"11".repeat(32)]).unwrap();
        wait_until(|| !a.status().running).await;
        let status = t.a.local_status(&t.team).unwrap();
        assert!(status.write_blocked);
        assert_eq!(status.messages[0].text, "보존할 원본");
        assert!(a.status().notice.is_some());
        a.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn automatic_paging_drains_more_than_one_batch() {
        let t = Team::new();
        for n in 0..10 {
            t.append(&t.a, &format!("자동 쪽 나누기 {n}"));
        }
        let a = t.start(&t.a).await;
        let b = t.start(&t.b).await;
        announce(&b, &a);
        wait_pair(&a, &b, || {
            t.b.local_status(&t.team).unwrap().received == 10
                && t.a.local_status(&t.team).unwrap().pending == 0
        })
        .await;
        a.shutdown().await;
        b.shutdown().await;
    }

    #[tokio::test]
    async fn manual_exchange_permission_does_not_grant_automatic_exchange_permission() {
        let t = Team::new();
        t.append(&t.a, "수동 교환만 허용한 상대에게 보낼 기록");
        let a = t.start(&t.a).await;
        let b = Probe::start_mode_exchange(
            Ipv4Addr::LOCALHOST,
            false,
            LIFETIME,
            Some((t.b.clone(), t.team.clone())),
            true,
        )
        .await
        .unwrap();
        let raw = b.status().card.unwrap();
        let mut forged: serde_json::Value = serde_json::from_str(&raw).unwrap();
        forged["automatic"] = serde_json::json!(true);
        assert!(a.add_peer(&forged.to_string()).is_err());
        let id = a.add_peer(&raw).unwrap();
        b.add_peer(&a.status().card.unwrap()).unwrap();
        tokio::time::sleep(Duration::from_millis(350)).await;
        assert_eq!(t.b.local_status(&t.team).unwrap().received, 0);
        assert_eq!(t.a.local_status(&t.team).unwrap().pending, 1);
        a.checker().exchange(&id).await.unwrap();
        assert_eq!(t.b.local_status(&t.team).unwrap().received, 1);
        a.shutdown().await;
        b.shutdown().await;
    }

    #[tokio::test]
    async fn stopping_cancels_an_unfinished_bootstrap_and_releases_its_port() {
        let t = Team::new();
        let a = t.start(&t.a).await;
        let advertisement = a.inner.automatic.as_ref().unwrap().advertisement().unwrap();
        let address = advertisement.addresses[0];
        let mut socket = TcpStream::connect(address).await.unwrap();
        socket.write_all(MAGIC).await.unwrap();
        write(
            &mut socket,
            &Frame::Hello(Hello {
                version: 1,
                session: advertisement.session,
                tag: advertisement.tags[0].clone(),
                client_nonce: random_id(),
            }),
        )
        .await
        .unwrap();
        assert!(matches!(
            read(&mut socket).await.unwrap(),
            Frame::Challenge { .. }
        ));
        let checker = a.checker();
        a.shutdown().await;
        checker.wake();
        assert!(
            tokio::time::timeout(Duration::from_secs(2), socket.read_u8())
                .await
                .unwrap()
                .is_err()
        );
        assert!(TcpListener::bind(address).await.is_ok());
    }

    #[tokio::test]
    async fn automatic_expiry_stops_the_listener_and_reports_why_it_stopped() {
        let t = Team::new();
        let a = Probe::start_mode_auto(
            Ipv4Addr::LOCALHOST,
            false,
            Duration::from_millis(150),
            Some((t.a.clone(), t.team.clone())),
            true,
            Some(Options::testing()),
        )
        .await
        .unwrap();
        let address = a.inner.card.addresses[0];
        wait_until(|| !a.status().running && a.status().notice.is_some()).await;
        wait_until(|| std::net::TcpListener::bind(address).is_ok()).await;
        assert!(!a.status().automatic.enabled);
        a.shutdown().await;
    }

    #[tokio::test]
    async fn a_captured_bootstrap_request_cannot_answer_a_new_server_challenge() {
        let t = Team::new();
        let a = t.start(&t.a).await;
        let b = t.start(&t.b).await;
        let advertisement = a.inner.automatic.as_ref().unwrap().advertisement().unwrap();
        let address = advertisement.addresses[0];
        let hello = Hello {
            version: 1,
            session: advertisement.session.clone(),
            tag: advertisement.tags[0].clone(),
            client_nonce: random_id(),
        };
        let mut socket = TcpStream::connect(address).await.unwrap();
        socket.write_all(MAGIC).await.unwrap();
        write(
            &mut socket,
            &Frame::Hello(Hello {
                version: hello.version,
                session: hello.session.clone(),
                tag: hello.tag.clone(),
                client_nonce: hello.client_nonce.clone(),
            }),
        )
        .await
        .unwrap();
        let nonce = match read(&mut socket).await.unwrap() {
            Frame::Challenge { nonce } => nonce,
            _ => panic!("missing challenge"),
        };
        let aad = BootAad {
            stage: Stage::Request,
            session: hello.session.clone(),
            tag: hello.tag.clone(),
            server_nonce: nonce.clone(),
            client_nonce: hello.client_nonce.clone(),
        };
        let scope = b.inner.automatic.as_ref().unwrap().routes.scope.clone();
        let own = serde_json::to_string(&b.inner.approved.as_ref().unwrap().own).unwrap();
        let peer = a.status().device_id.unwrap();
        let captured = t.b.seal_auto(&scope, &peer, &aad, &own).unwrap();
        write(&mut socket, &Frame::Request(captured.clone()))
            .await
            .unwrap();
        assert!(matches!(read(&mut socket).await.unwrap(), Frame::Reply(_)));
        tokio::time::sleep(Duration::from_millis(20)).await;
        let mut replay = TcpStream::connect(address).await.unwrap();
        replay.write_all(MAGIC).await.unwrap();
        write(&mut replay, &Frame::Hello(hello)).await.unwrap();
        let fresh = match read(&mut replay).await.unwrap() {
            Frame::Challenge { nonce } => nonce,
            _ => panic!("missing fresh challenge"),
        };
        assert_ne!(fresh, nonce);
        write(&mut replay, &Frame::Request(captured)).await.unwrap();
        assert!(read(&mut replay).await.is_err());
        a.shutdown().await;
        b.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "requires an explicitly selected real LAN interface"]
    async fn approved_devices_discover_and_exchange_on_the_selected_real_lan() {
        let address: Ipv4Addr = std::env::var("SAI_MESH_AUTO_LAN_TEST")
            .expect("set SAI_MESH_AUTO_LAN_TEST to this PC's private IPv4 address")
            .parse()
            .unwrap();
        let t = Team::new();
        t.append(&t.a, "실제 LAN 자동 발견");
        let a = Probe::start_automatic(t.a.clone(), &t.team, address)
            .await
            .unwrap();
        let b = Probe::start_automatic(t.b.clone(), &t.team, address)
            .await
            .unwrap();
        wait_pair(&a, &b, || {
            t.b.local_status(&t.team).unwrap().received == 1
                && t.a.local_status(&t.team).unwrap().pending == 0
        })
        .await;
        a.shutdown().await;
        b.shutdown().await;
    }
}
