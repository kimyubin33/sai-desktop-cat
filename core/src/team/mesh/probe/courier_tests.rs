use super::*;

struct Team {
    dir: std::path::PathBuf,
    a: Vault,
    b: Vault,
    c: Vault,
    team: String,
}
impl Team {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("sai-courier-net-{}", random_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = Vault::new(dir.join("a.sqlite3"));
        let b = Vault::new(dir.join("b.sqlite3"));
        let c = Vault::new(dir.join("c.sqlite3"));
        for v in [&a, &b, &c] {
            v.create_device().unwrap();
        }
        let first = a.create_team("중계 연결 검증").unwrap();
        let team = first.manifest.body.team_id.clone();
        for v in [&b, &c] {
            v.import(
                &serde_json::to_string(&first).unwrap(),
                &first.manifest.fingerprint().unwrap(),
            )
            .unwrap();
        }
        for v in [&b, &c] {
            let p = a
                .approve(
                    &team,
                    &serde_json::to_string(&v.request(&team).unwrap()).unwrap(),
                )
                .unwrap();
            for to in [&b, &c] {
                to.import(&serde_json::to_string(&p).unwrap(), "").unwrap();
            }
        }
        Self { dir, a, b, c, team }
    }
    async fn start(&self, v: &Vault, courier: bool) -> Probe {
        let mut options = super::super::auto::Options::testing();
        options.courier = courier;
        Probe::start_mode_auto(
            Ipv4Addr::LOCALHOST,
            false,
            LIFETIME,
            Some((v.clone(), self.team.clone())),
            true,
            Some(options),
        )
        .await
        .unwrap()
    }
    async fn manual(&self, v: &Vault, courier: bool) -> Probe {
        let mut probe = self.start(v, courier).await;
        probe.automatic.take(); // 이 테스트는 주기 작업과 경합하지 않고 실제 TLS 왕복만 확인한다.
        probe.host.take();
        probe
    }
    fn append(&self) {
        self.a
            .append_local(
                &self.team,
                &Vault::local_draft_id().unwrap(),
                "작성 PC 종료 뒤에 받을 톡",
            )
            .unwrap();
    }
    fn copies(&self, name: &str) -> i64 {
        let c = rusqlite::Connection::open(self.dir.join(format!("{name}.sqlite3"))).unwrap();
        c.query_row("SELECT count(*) FROM mesh_courier", [], |r| r.get(0))
            .unwrap()
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
            .starts_with("sai-courier-net-"));
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
async fn pair(a: &Probe, b: &Probe) -> (String, String) {
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
    (b_id, a_id)
}
async fn wait(mut ready: impl FnMut() -> bool, a: &Probe, b: &Probe) {
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
        "courier timeout: a={}, b={}",
        serde_json::to_string(&a.status()).unwrap(),
        serde_json::to_string(&b.status()).unwrap()
    );
}
fn announce(from: &Probe, to: &Probe) {
    let encoded = from
        .inner
        .automatic
        .as_ref()
        .unwrap()
        .advertisement()
        .unwrap();
    assert!(to.inner.automatic.as_ref().unwrap().ingest(encoded));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mutual_tls_courier_survives_author_absence_and_both_runtime_restarts() {
    let t = Team::new();
    t.append();
    let a = t.manual(&t.a, true).await;
    let b = t.manual(&t.b, true).await;
    let (bi, _) = pair(&a, &b).await;
    let report = a.checker().exchange(&bi).await.unwrap();
    assert!(report.relay_sent > 0);
    assert!(t.copies("b") >= 3);
    assert_eq!(t.a.local_status(&t.team).unwrap().pending, 1);
    a.shutdown().await;
    b.shutdown().await;
    let b = t.manual(&t.b, true).await;
    let c = t.manual(&t.c, true).await;
    let (ci, _) = pair(&b, &c).await;
    b.checker().exchange(&ci).await.unwrap();
    assert_eq!(t.c.local_status(&t.team).unwrap().received, 1);
    assert_eq!(t.a.local_status(&t.team).unwrap().pending, 1);
    c.shutdown().await;
    let a = t.manual(&t.a, true).await;
    let (ai, _) = pair(&b, &a).await;
    b.checker().exchange(&ai).await.unwrap();
    assert_eq!(t.a.local_status(&t.team).unwrap().pending, 0);
    a.shutdown().await;
    b.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mixed_consent_uses_direct_protocol_and_forging_courier_flag_invalidates_card() {
    let t = Team::new();
    t.append();
    let a = t.manual(&t.a, true).await;
    let b = t.manual(&t.b, false).await;
    let mut forged: serde_json::Value = serde_json::from_str(&b.status().card.unwrap()).unwrap();
    forged["courier"] = true.into();
    assert!(a.checker().add_peer(forged.to_string()).await.is_err());
    let (bi, _) = pair(&a, &b).await;
    let report = a.checker().exchange(&bi).await.unwrap();
    assert_eq!(report.sent, 1);
    assert_eq!(report.relay_sent, 0);
    assert_eq!(report.relay_received, 0);
    assert_eq!(t.copies("a"), 0);
    assert_eq!(t.copies("b"), 0);
    assert!(!a.status().peers[0].courier_enabled);
    a.shutdown().await;
    b.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn automatic_discovery_hands_ciphertext_to_a_later_pc_while_author_is_stopped() {
    let t = Team::new();
    t.append();
    let a = t.start(&t.a, true).await;
    let b = t.start(&t.b, true).await;
    announce(&b, &a);
    wait(|| t.copies("b") >= 3, &a, &b).await;
    a.shutdown().await;
    let c = t.start(&t.c, true).await;
    announce(&c, &b);
    wait(
        || t.c.local_status(&t.team).unwrap().received == 1 && t.copies("b") >= 4,
        &b,
        &c,
    )
    .await;
    assert_eq!(t.a.local_status(&t.team).unwrap().pending, 1);
    c.shutdown().await;
    let a = t.start(&t.a, true).await;
    announce(&a, &b);
    wait(|| t.a.local_status(&t.team).unwrap().pending == 0, &a, &b).await;
    assert_eq!(t.c.local_status(&t.team).unwrap().received, 1);
    a.shutdown().await;
    b.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bounded_batches_drain_unprepared_envelopes_and_multiple_authors_receive_chains() {
    let t = Team::new();
    for n in 0..10 {
        t.a.append_local(
            &t.team,
            &Vault::local_draft_id().unwrap(),
            &format!("쪽 나누기 {n}"),
        )
        .unwrap();
    }
    let a = t.manual(&t.a, true).await;
    let b = t.manual(&t.b, true).await;
    let (bi, _) = pair(&a, &b).await;
    let mut done = false;
    for _ in 0..10 {
        let r = match a.checker().exchange(&bi).await {
            Ok(report) => report,
            // 다른 암호화 테스트와 CPU를 공유해 제한에 닿아도 커밋한 경계에서 이어야 한다.
            Err(error) if error.starts_with("기록 교환 시간이 지났어요.") => continue,
            Err(error) => panic!("unexpected exchange error: {error}"),
        };
        assert!(r.relay_sent <= BATCH && r.relay_received <= BATCH);
        if !r.more {
            done = true;
            break;
        }
    }
    assert!(done);
    a.shutdown().await;
    let c = t.manual(&t.c, true).await;
    let (ci, _) = pair(&b, &c).await;
    done = false;
    for _ in 0..15 {
        let r = match b.checker().exchange(&ci).await {
            Ok(report) => report,
            Err(error) if error.starts_with("기록 교환 시간이 지났어요.") => continue,
            Err(error) => panic!("unexpected exchange error: {error}"),
        };
        assert!(r.relay_sent <= BATCH && r.relay_received <= BATCH);
        if !r.more {
            done = true;
            break;
        }
    }
    assert!(done);
    assert_eq!(t.c.local_status(&t.team).unwrap().received, 10);
    b.shutdown().await;
    c.shutdown().await;
}
