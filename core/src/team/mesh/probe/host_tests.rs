use super::*;

fn choice(id: &str, now: Instant) -> Choice {
    Choice::new(id.into(), format!("session-{id}"), now, Options::default())
}
fn confirm(c: &mut Choice, id: &str, t: Instant) {
    c.confirmed(id.into(), format!("session-{id}"), t);
}

#[test]
fn settling_and_stable_device_order_ignore_arrival_order() {
    let t = Instant::now();
    let mut c = choice("c", t);
    confirm(&mut c, "b", t);
    confirm(&mut c, "a", t);
    assert!(!c.update(t + Duration::from_secs(1)));
    assert!(c.status(t).device_id.is_none());
    assert!(c.update(t + Duration::from_secs(2)));
    assert_eq!(
        c.status(t + Duration::from_secs(2)).device_id.as_deref(),
        Some("a")
    );
}
#[test]
fn live_incumbent_is_held_then_a_stable_better_candidate_takes_over() {
    let t = Instant::now();
    let mut c = choice("c", t);
    c.update(t + Duration::from_secs(2));
    confirm(&mut c, "a", t + Duration::from_secs(3));
    assert!(!c.update(t + Duration::from_secs(5)));
    assert_eq!(
        c.status(t + Duration::from_secs(5)).device_id.as_deref(),
        Some("c")
    );
    c.update(t + Duration::from_secs(12));
    assert_eq!(
        c.status(t + Duration::from_secs(12)).device_id.as_deref(),
        Some("a")
    );
}
#[test]
fn expiry_bypasses_hold_and_short_interruptions_preserve_the_incumbent() {
    let t = Instant::now();
    let mut c = choice("b", t);
    c.options.hold = Duration::from_secs(60);
    confirm(&mut c, "a", t);
    c.update(t + Duration::from_secs(2));
    c.update(t + Duration::from_secs(14));
    assert_eq!(
        c.status(t + Duration::from_secs(14)).device_id.as_deref(),
        Some("a")
    );
    c.update(t + Duration::from_secs(15));
    assert_eq!(
        c.status(t + Duration::from_secs(15)).device_id.as_deref(),
        Some("b")
    );
}
#[test]
fn only_a_new_success_extends_the_monotonic_lease() {
    let t = Instant::now();
    let mut c = choice("b", t);
    confirm(&mut c, "a", t);
    c.update(t + Duration::from_secs(2));
    confirm(&mut c, "a", t + Duration::from_secs(10));
    c.update(t + Duration::from_secs(24));
    assert_eq!(
        c.status(t + Duration::from_secs(24)).device_id.as_deref(),
        Some("a")
    );
    c.update(t + Duration::from_secs(25));
    assert_eq!(
        c.status(t + Duration::from_secs(25)).device_id.as_deref(),
        Some("b")
    );
}
#[test]
fn a_replaced_session_needs_a_new_confirmation_and_settle_period() {
    let t = Instant::now();
    let mut c = choice("b", t);
    confirm(&mut c, "a", t);
    c.update(t + Duration::from_secs(2));
    c.reconcile(
        &BTreeMap::from([("a".into(), "new-session".into())]),
        t + Duration::from_secs(3),
    );
    c.update(t + Duration::from_secs(3));
    assert_eq!(
        c.status(t + Duration::from_secs(3)).device_id.as_deref(),
        Some("b")
    );
    c.confirmed("a".into(), "new-session".into(), t + Duration::from_secs(4));
    c.update(t + Duration::from_secs(14));
    assert_eq!(
        c.status(t + Duration::from_secs(14)).session.as_deref(),
        Some("new-session")
    );
}
#[test]
fn split_views_converge_after_fresh_reconnection_without_voting_or_authority_changes() {
    let t = Instant::now();
    let mut a = choice("a", t);
    let mut b = choice("b", t);
    let mut c = choice("c", t);
    confirm(&mut b, "c", t);
    confirm(&mut c, "b", t);
    for p in [&mut a, &mut b, &mut c] {
        p.update(t + Duration::from_secs(2));
    }
    assert_eq!(
        a.status(t + Duration::from_secs(2)).device_id.as_deref(),
        Some("a")
    );
    assert_eq!(
        c.status(t + Duration::from_secs(2)).device_id.as_deref(),
        Some("b")
    );
    for (p, ids) in [
        (&mut a, ["b", "c"]),
        (&mut b, ["a", "c"]),
        (&mut c, ["a", "b"]),
    ] {
        for id in ids {
            confirm(p, id, t + Duration::from_secs(20));
        }
        p.update(t + Duration::from_secs(22));
        assert_eq!(
            p.status(t + Duration::from_secs(22)).device_id.as_deref(),
            Some("a")
        );
    }
}
#[test]
fn status_hides_expired_peers_without_consuming_an_election_transition() {
    let t = Instant::now();
    let mut c = choice("b", t);
    confirm(&mut c, "a", t);
    c.update(t + Duration::from_secs(2));
    let changes = c.changes;
    let status = c.status(t + Duration::from_secs(16));
    assert!(status.device_id.is_none());
    assert_eq!(status.candidates, 1);
    assert_eq!(c.changes, changes);
    assert!(c.update(t + Duration::from_secs(16)));
}
#[test]
fn fallback_exchange_continues_and_representative_connections_keep_normal_frequency() {
    let base = Duration::from_secs(5);
    assert_eq!(exchange_idle(base, "b", "a", Some("a")), base);
    assert_eq!(exchange_idle(base, "a", "c", Some("a")), base);
    assert_eq!(
        exchange_idle(base, "b", "c", Some("a")),
        Duration::from_secs(15)
    );
    assert_eq!(exchange_idle(base, "b", "c", None), base);
}

#[cfg(windows)]
mod windows {
    use super::*;
    use crate::team::mesh::identity::Vault;
    struct Team {
        dir: std::path::PathBuf,
        vaults: Vec<Vault>,
        team: String,
    }
    impl Team {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!("sai-host-{}", random_id()));
            std::fs::create_dir_all(&dir).unwrap();
            let mut vaults: Vec<_> = (0..3)
                .map(|i| Vault::new(dir.join(format!("{i}.sqlite3"))))
                .collect();
            for v in &vaults {
                v.create_device().unwrap();
            }
            let first = vaults[0].create_team("대표 인계 검증").unwrap();
            let team = first.manifest.body.team_id.clone();
            for v in &vaults[1..] {
                v.import(
                    &serde_json::to_string(&first).unwrap(),
                    &first.manifest.fingerprint().unwrap(),
                )
                .unwrap();
            }
            for v in &vaults[1..] {
                let packet = vaults[0]
                    .approve(
                        &team,
                        &serde_json::to_string(&v.request(&team).unwrap()).unwrap(),
                    )
                    .unwrap();
                for to in &vaults[1..] {
                    to.import(&serde_json::to_string(&packet).unwrap(), "")
                        .unwrap();
                }
            }
            vaults.sort_by_key(|v| v.status().unwrap().device.unwrap().device_id);
            Self { dir, vaults, team }
        }
        async fn start(&self, index: usize, courier: bool) -> Probe {
            let mut options = super::super::super::auto::Options::testing();
            options.courier = courier;
            Probe::start_mode_auto(
                Ipv4Addr::LOCALHOST,
                false,
                LIFETIME,
                Some((self.vaults[index].clone(), self.team.clone())),
                true,
                Some(options),
            )
            .await
            .unwrap()
        }
        fn id(&self, index: usize) -> String {
            self.vaults[index]
                .status()
                .unwrap()
                .device
                .unwrap()
                .device_id
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
                .starts_with("sai-host-"));
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
    async fn pair(a: &Probe, b: &Probe) {
        a.checker()
            .add_peer(b.status().card.unwrap())
            .await
            .unwrap();
        b.checker()
            .add_peer(a.status().card.unwrap())
            .await
            .unwrap();
    }
    async fn wait(mut ready: impl FnMut() -> bool) {
        tokio::time::timeout(Duration::from_secs(60), async {
            loop {
                if ready() {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .expect("host view did not converge");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn authenticated_hosts_converge_then_replace_the_stopped_representative() {
        let t = Team::new();
        let expected = t.id(0);
        let next = t.id(1);
        let packet = t.vaults[0].export(&t.team).unwrap();
        let a = t.start(0, true).await;
        let b = t.start(1, true).await;
        let c = t.start(2, true).await;
        pair(&a, &b).await;
        pair(&a, &c).await;
        pair(&b, &c).await;
        wait(|| {
            [&a, &b, &c].iter().all(|p| {
                p.status().host.device_id.as_deref() == Some(expected.as_str())
                    && p.status().host.candidates == 3
            })
        })
        .await;
        let old_address = a.inner.card.addresses[0];
        a.shutdown().await;
        wait(|| {
            [&b, &c].iter().all(|p| {
                p.status().host.device_id.as_deref() == Some(next.as_str())
                    && p.status().host.candidates == 2
            })
        })
        .await;
        assert!(b.status().host.is_self);
        assert!(!c.status().host.is_self);
        for v in &t.vaults {
            assert_eq!(v.export(&t.team).unwrap(), packet);
            v.append_local(
                &t.team,
                &Vault::local_draft_id().unwrap(),
                "대표 변경 중에도 로컬 저장",
            )
            .unwrap();
        }
        assert!(TcpListener::bind(old_address).await.is_ok());
        b.shutdown().await;
        c.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn forged_host_capability_is_rejected_and_direct_only_peers_never_become_candidates() {
        let t = Team::new();
        let a = t.start(1, true).await;
        let b = t.start(0, false).await;
        let mut value: serde_json::Value = serde_json::from_str(&b.status().card.unwrap()).unwrap();
        value["host"] = true.into();
        assert!(a.checker().add_peer(value.to_string()).await.is_err());
        pair(&a, &b).await;
        wait(|| a.status().host.is_self).await;
        assert_eq!(a.status().host.candidates, 1);
        assert!(!b.status().host.enabled);
        assert!(!a.status().peers[0].host_enabled);
        a.shutdown().await;
        b.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_new_authenticated_session_replaces_a_stopped_hosts_certificate() {
        let t = Team::new();
        let id = t.id(0);
        let a = t.start(0, true).await;
        let b = t.start(1, true).await;
        pair(&a, &b).await;
        wait(|| b.status().host.device_id.as_deref() == Some(id.as_str())).await;
        let old = a.inner.card.session.clone();
        a.shutdown().await;
        let a = t.start(0, true).await;
        let new = a.inner.card.session.clone();
        assert_ne!(old, new);
        b.checker()
            .add_peer(a.status().card.unwrap())
            .await
            .unwrap();
        wait(|| b.status().host.is_self && b.status().host.candidates == 1).await;
        let ad = a.inner.automatic.as_ref().unwrap().advertisement().unwrap();
        assert!(b.inner.automatic.as_ref().unwrap().ingest(ad));
        wait(|| b.status().host.session.as_deref() == Some(new.as_str())).await;
        assert_eq!(b.status().peers.len(), 1);
        a.shutdown().await;
        b.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn membership_change_stops_the_host_role_and_releases_its_listener() {
        let t = Team::new();
        let a = t.start(0, true).await;
        wait(|| a.status().host.is_self).await;
        let address = a.inner.card.addresses[0];
        let admin = t
            .vaults
            .iter()
            .find(|v| v.status().unwrap().teams[0].admin_here)
            .unwrap();
        let packet = admin.revoke(&t.team, &t.id(0)).unwrap();
        t.vaults[0]
            .import(&serde_json::to_string(&packet).unwrap(), "")
            .unwrap();
        wait(|| !a.status().running).await;
        assert!(!a.status().host.enabled);
        assert!(a.status().host.device_id.is_none());
        assert!(a.status().notice.is_some());
        wait(|| std::net::TcpListener::bind(address).is_ok()).await;
        a.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn representative_handoff_delivers_the_offline_authors_ciphertext_and_returns_its_receipt(
    ) {
        let t = Team::new();
        let original = t.id(0);
        let replacement = t.id(1);
        t.vaults[0]
            .append_local(
                &t.team,
                &Vault::local_draft_id().unwrap(),
                "대표 인계 중에도 남아 있는 암호문",
            )
            .unwrap();
        let a = t.start(0, true).await;
        let b = t.start(1, true).await;
        pair(&a, &b).await;
        wait(|| {
            let v = &t.vaults[1];
            b.status().host.device_id.as_deref() == Some(original.as_str())
                && v.local_status(&t.team).unwrap().received == 1
                && b.status().automatic.successes > 0
        })
        .await;
        // 첫 직접 수신만으로 중계 사본 커밋을 단정하지 않는다.
        let scope = crate::team::mesh::identity::transfer::Scope {
            team: t.team.clone(),
            own: t.id(1),
            peer: t.id(0),
            revision: t.vaults[1]
                .export(&t.team)
                .unwrap()
                .state()
                .unwrap()
                .revision,
            head: t.vaults[1].export(&t.team).unwrap().state().unwrap().head,
        };
        wait(|| t.vaults[1].courier_inventory(&scope).unwrap().len() >= 3).await;
        a.shutdown().await;
        wait(|| b.status().host.is_self && b.status().host.candidates == 1).await;
        let c = t.start(2, true).await;
        pair(&b, &c).await;
        wait(|| {
            t.vaults[2].local_status(&t.team).unwrap().received == 1
                && c.status().host.device_id.as_deref() == Some(replacement.as_str())
        })
        .await;
        assert_eq!(t.vaults[0].local_status(&t.team).unwrap().pending, 1);
        let a = t.start(0, true).await;
        assert!(b
            .inner
            .automatic
            .as_ref()
            .unwrap()
            .ingest(a.inner.automatic.as_ref().unwrap().advertisement().unwrap()));
        assert!(c
            .inner
            .automatic
            .as_ref()
            .unwrap()
            .ingest(a.inner.automatic.as_ref().unwrap().advertisement().unwrap()));
        wait(|| {
            t.vaults[0].local_status(&t.team).unwrap().pending == 0
                && [&a, &b, &c]
                    .iter()
                    .all(|p| p.status().host.device_id.as_deref() == Some(original.as_str()))
        })
        .await;
        assert_eq!(t.vaults[2].local_status(&t.team).unwrap().received, 1);
        a.shutdown().await;
        b.shutdown().await;
        c.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stopping_cancels_an_inflight_heartbeat_and_releases_the_listener() {
        use tokio::io::AsyncReadExt;
        let t = Team::new();
        let mut a = t.start(0, true).await;
        let mut b = t.start(1, true).await;
        a.automatic.take();
        b.automatic.take();
        pair(&a, &b).await;
        let address = a.inner.card.addresses[0];
        let remote = b.inner.card.addresses[0];
        b.shutdown().await;
        let listener = TcpListener::bind(remote).await.unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let blackhole = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let _ = tx.send(());
            let mut bytes = [0u8; 1024];
            loop {
                if socket.read(&mut bytes).await.unwrap_or(0) == 0 {
                    break;
                }
            }
        });
        tokio::time::timeout(Duration::from_secs(10), rx)
            .await
            .unwrap()
            .unwrap();
        let now = Instant::now();
        a.shutdown().await;
        assert!(now.elapsed() < Duration::from_secs(1));
        tokio::time::timeout(Duration::from_secs(5), blackhole)
            .await
            .unwrap()
            .unwrap();
        assert!(TcpListener::bind(address).await.is_ok());
    }
}
