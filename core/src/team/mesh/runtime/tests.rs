use super::*;
use std::sync::atomic::{AtomicU64, Ordering};
fn network() -> NetworkScope {
    NetworkScope {
        adapter: "01".repeat(16),
        network: "02".repeat(16),
        subnet: "192.168.20.0".parse().unwrap(),
        mask: "255.255.255.0".parse().unwrap(),
        trusted: true,
    }
}
struct Mock {
    clock: AtomicU64,
    current: Mutex<Option<(NetworkScope, Ipv4Addr)>>,
}
impl Mock {
    fn new() -> Self {
        Self {
            clock: AtomicU64::new(1000),
            current: Mutex::new(Some((network(), Ipv4Addr::LOCALHOST))),
        }
    }
    fn advance(&self, ms: u64) {
        self.clock.fetch_add(ms, Ordering::SeqCst);
    }
}
impl Environment for Mock {
    fn now(&self) -> u64 {
        self.clock.load(Ordering::SeqCst)
    }
    fn inspect(&self, address: Ipv4Addr) -> Result<NetworkScope> {
        self.current
            .lock()
            .unwrap()
            .as_ref()
            .filter(|(_, a)| *a == address)
            .map(|(s, _)| s.clone())
            .ok_or("network unavailable".into())
    }
    fn locate(&self, scope: &NetworkScope) -> Result<Ipv4Addr> {
        self.current
            .lock()
            .unwrap()
            .as_ref()
            .filter(|(s, _)| s == scope)
            .map(|(_, a)| *a)
            .ok_or("network changed".into())
    }
}
struct Team {
    dir: std::path::PathBuf,
    vault: Vault,
    other: Vault,
    team: String,
    env: Arc<Mock>,
}
impl Team {
    fn new() -> Self {
        let dir =
            std::env::temp_dir().join(format!("sai-runtime-{}", Vault::local_draft_id().unwrap()));
        std::fs::create_dir_all(&dir).unwrap();
        let vault = Vault::new(dir.join("a.sqlite3"));
        let other = Vault::new(dir.join("b.sqlite3"));
        vault.create_device().unwrap();
        other.create_device().unwrap();
        let first = vault.create_team("실행 수명 검증").unwrap();
        let team = first.manifest.body.team_id.clone();
        other
            .import(
                &serde_json::to_string(&first).unwrap(),
                &first.manifest.fingerprint().unwrap(),
            )
            .unwrap();
        let packet = vault
            .approve(
                &team,
                &serde_json::to_string(&other.request(&team).unwrap()).unwrap(),
            )
            .unwrap();
        other
            .import(&serde_json::to_string(&packet).unwrap(), "")
            .unwrap();
        Self {
            dir,
            vault,
            other,
            team,
            env: Arc::new(Mock::new()),
        }
    }
    fn controller(&self) -> Controller {
        Controller::with_environment(self.vault.clone(), self.env.clone())
    }
    fn config(&self, restore: bool) -> Config {
        Config {
            team: self.team.clone(),
            address: Ipv4Addr::LOCALHOST,
            mode: Mode::Courier,
            restore,
        }
    }
    fn db(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(self.dir.join("a.sqlite3")).unwrap()
    }
    fn append(&self) {
        self.vault
            .append_local(
                &self.team,
                &Vault::local_draft_id().unwrap(),
                "수명 변경 뒤에도 남는 원본",
            )
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
            .starts_with("sai-runtime-"));
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
fn endpoint(status: &RuntimeStatus) -> std::net::SocketAddrV4 {
    let card: super::super::probe::ApprovedCard =
        serde_json::from_str(status.session.card.as_ref().unwrap()).unwrap();
    card.card.addresses[0]
}
async fn released(address: std::net::SocketAddrV4) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if std::net::TcpListener::bind(address).is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn cold_start_without_a_profile_creates_no_identity_or_listener() {
    let t = Team::new();
    let path = t.dir.join("missing.sqlite3");
    let c = Controller::with_environment(Vault::new(&path), t.env.clone());
    c.tick().await;
    assert!(!c.status().await.session.running);
    assert_eq!(c.status().await.lifecycle.phase, "off");
    assert!(!path.exists());
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remembered_consent_restores_on_new_app_with_new_certificate_and_unchanged_records() {
    let t = Team::new();
    t.append();
    let own = t.vault.status().unwrap().device.unwrap();
    let c = t.controller();
    let first = c.start(t.config(true)).await.unwrap();
    let address = endpoint(&first);
    c.event(Event::Exit);
    c.tick().await;
    released(address).await;
    let next = t.controller();
    next.tick().await;
    let status = next.status().await;
    assert!(status.session.running);
    assert_ne!(first.session.session, status.session.session);
    assert_ne!(first.session.fingerprint, status.session.fingerprint);
    assert_eq!(t.vault.status().unwrap().device.unwrap(), own);
    assert_eq!(t.vault.local_status(&t.team).unwrap().stored, 1);
    next.stop(status.session.session.as_deref()).await.unwrap();
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ephemeral_consent_does_not_resume_or_restore_on_another_app() {
    let t = Team::new();
    let c = t.controller();
    let status = c.start(t.config(false)).await.unwrap();
    let address = endpoint(&status);
    c.event(Event::Suspend);
    assert!(!c.status().await.session.running);
    c.tick().await;
    released(address).await;
    c.event(Event::Resume);
    c.tick().await;
    assert!(!c.status().await.session.running);
    assert_eq!(c.status().await.lifecycle.phase, "needs-consent");
    let next = t.controller();
    next.tick().await;
    assert!(!next.status().await.session.running);
    assert!(t.vault.runtime_profile().unwrap().is_none());
    c.event(Event::Suspend);
    c.event(Event::Resume);
    let fresh = c.start(t.config(false)).await.unwrap();
    c.tick().await;
    assert!(c.status().await.session.running);
    assert_eq!(fresh.session.session, c.status().await.session.session);
    c.stop(fresh.session.session.as_deref()).await.unwrap();
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resume_keeps_the_original_deadline_and_duplicate_resume_does_not_rotate_again() {
    let t = Team::new();
    let c = t.controller();
    let first = c.start(t.config(true)).await.unwrap();
    let old = c
        .checker(first.session.session.as_ref().unwrap())
        .await
        .unwrap();
    t.env.advance(60_000);
    c.event(Event::Suspend);
    c.tick().await;
    released(endpoint(&first)).await;
    t.env.advance(30_000);
    c.event(Event::Resume);
    c.tick().await;
    let next = c.status().await;
    assert!(next.session.running);
    assert_ne!(first.session.session, next.session.session);
    assert_eq!(next.lifecycle.remaining_seconds, 810);
    assert!(old.check("unknown").await.is_err());
    c.event(Event::Resume);
    c.tick().await;
    assert_eq!(next.session.session, c.status().await.session.session);
    c.stop(next.session.session.as_deref()).await.unwrap();
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sleeping_past_the_budget_never_renews_the_session_and_stop_clears_future_startup() {
    let t = Team::new();
    let c = t.controller();
    c.start(t.config(true)).await.unwrap();
    c.event(Event::Suspend);
    c.tick().await;
    t.env.advance(BUDGET + 1);
    c.event(Event::Resume);
    c.tick().await;
    assert!(!c.status().await.session.running);
    assert_eq!(c.status().await.lifecycle.phase, "expired");
    c.tick().await;
    assert!(!c.status().await.session.running);
    c.stop(None).await.unwrap();
    let next = t.controller();
    next.tick().await;
    assert!(!next.status().await.session.running);
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_changed_network_cannot_restore_and_same_scope_dhcp_uses_a_fresh_session() {
    let t = Team::new();
    let c = t.controller();
    let first = c.start(t.config(true)).await.unwrap();
    let mut wrong = network();
    wrong.network = "03".repeat(16);
    *t.env.current.lock().unwrap() = Some((wrong, Ipv4Addr::LOCALHOST));
    t.env.advance(1000);
    c.tick().await;
    assert!(!c.status().await.session.running);
    released(endpoint(&first)).await;
    t.env.advance(5000);
    c.tick().await;
    assert!(!c.status().await.session.running);
    *t.env.current.lock().unwrap() = Some((network(), "127.0.0.2".parse().unwrap()));
    t.env.advance(5000);
    c.tick().await;
    let status = c.status().await;
    assert!(status.session.running);
    assert_eq!(
        endpoint(&status).ip(),
        &"127.0.0.2".parse::<Ipv4Addr>().unwrap()
    );
    assert_ne!(first.session.session, status.session.session);
    assert_eq!(status.lifecycle.remaining_seconds, 889);
    c.stop(status.session.session.as_deref()).await.unwrap();
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn approval_changes_while_asleep_require_new_consent_and_preserve_the_source() {
    let t = Team::new();
    t.append();
    let c = t.controller();
    c.start(t.config(true)).await.unwrap();
    let signed = t.vault.runtime_profile().unwrap().unwrap();
    c.event(Event::Suspend);
    c.tick().await;
    let other = t.other.status().unwrap().device.unwrap().device_id;
    t.vault.revoke(&t.team, &other).unwrap();
    assert!(t.vault.validate_auto_scope(&signed.approval()).is_err());
    assert!(t
        .vault
        .remember_runtime(&signed.approval(), signed.mode(), signed.network().clone())
        .is_err());
    c.event(Event::Resume);
    c.tick().await;
    assert!(!c.status().await.session.running);
    assert_eq!(c.status().await.lifecycle.phase, "needs-consent");
    assert!(t.vault.runtime_profile().is_err());
    assert_eq!(t.vault.local_status(&t.team).unwrap().stored, 1);
    c.stop(None).await.unwrap();
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tampered_startup_modes_or_networks_fail_closed() {
    let t = Team::new();
    let c = t.controller();
    let first = c.start(t.config(true)).await.unwrap();
    c.event(Event::Exit);
    c.tick().await;
    released(endpoint(&first)).await;
    let conn = t.db();
    let text: String = conn
        .query_row("SELECT data FROM mesh_runtime_profile", [], |r| r.get(0))
        .unwrap();
    let mut p: serde_json::Value = serde_json::from_str(&text).unwrap();
    p["body"]["network"]["network"] = "04".repeat(16).into();
    conn.execute("UPDATE mesh_runtime_profile SET data=?1", [p.to_string()])
        .unwrap();
    let next = t.controller();
    next.tick().await;
    assert!(!next.status().await.session.running);
    assert_eq!(next.status().await.lifecycle.phase, "needs-consent");
    next.stop(None).await.unwrap();
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn public_network_or_nonautomatic_mode_cannot_persist_consent() {
    let t = Team::new();
    let c = t.controller();
    let mut public = network();
    public.trusted = false;
    *t.env.current.lock().unwrap() = Some((public, Ipv4Addr::LOCALHOST));
    assert!(c.start(t.config(true)).await.is_err());
    assert!(!c.status().await.session.running);
    *t.env.current.lock().unwrap() = Some((network(), Ipv4Addr::LOCALHOST));
    let mut config = t.config(true);
    config.mode = Mode::Transfer;
    assert!(c.start(config).await.is_err());
    assert!(t.vault.runtime_profile().unwrap().is_none());
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stale_stop_cannot_cancel_a_new_session_and_current_stop_forgets_startup() {
    let t = Team::new();
    let c = t.controller();
    let status = c.start(t.config(true)).await.unwrap();
    assert!(c.stop(Some("stale")).await.is_err());
    assert_eq!(status.session.session, c.status().await.session.session);
    assert!(c.status().await.session.running);
    c.stop(status.session.session.as_deref()).await.unwrap();
    released(endpoint(&status)).await;
    assert!(t.vault.runtime_profile().unwrap().is_none());
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn save_or_forget_failures_are_reported_without_claiming_the_listener_is_running() {
    let t = Team::new();
    let c = t.controller();
    let conn = t.db();
    conn.execute_batch("CREATE TRIGGER deny_save BEFORE INSERT ON mesh_runtime_profile BEGIN SELECT RAISE(ABORT,'test');END;").unwrap();
    assert!(c.start(t.config(true)).await.is_err());
    assert!(!c.status().await.session.running);
    conn.execute_batch("DROP TRIGGER deny_save;").unwrap();
    let status = c.start(t.config(true)).await.unwrap();
    conn.execute_batch("CREATE TRIGGER deny_forget BEFORE DELETE ON mesh_runtime_profile BEGIN SELECT RAISE(ABORT,'test');END;").unwrap();
    assert!(c.stop(status.session.session.as_deref()).await.is_err());
    assert!(!c.status().await.session.running);
    assert!(c.status().await.lifecycle.notice.is_some());
    c.tick().await;
    assert!(!c.status().await.session.running);
    assert!(t.vault.runtime_profile().unwrap().is_some());
    conn.execute_batch("DROP TRIGGER deny_forget;").unwrap();
    c.stop(None).await.unwrap();
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_missed_power_callback_is_detected_by_elapsed_execution_gap() {
    let t = Team::new();
    let c = t.controller();
    let first = c.start(t.config(true)).await.unwrap();
    t.env.advance(20_000);
    c.tick().await;
    let status = c.status().await;
    assert!(status.session.running);
    assert_ne!(status.session.session, first.session.session);
    assert_eq!(status.lifecycle.remaining_seconds, 880);
    c.stop(status.session.session.as_deref()).await.unwrap();
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropping_the_app_worker_cancels_networking_and_prevents_late_restarts() {
    let t = Team::new();
    let c = t.controller();
    let first = c.start(t.config(true)).await.unwrap();
    let worker = c.worker();
    drop(worker);
    released(endpoint(&first)).await;
    assert!(!c.status().await.session.running);
    assert!(c.start(t.config(true)).await.is_err());
}
#[test]
fn v4_upgrade_is_atomic_and_does_not_enable_any_profile() {
    let t = Team::new();
    t.append();
    let key = t.vault.status().unwrap().device.unwrap();
    let conn = t.db();
    conn.execute_batch("DROP TABLE mesh_legacy_archive; DROP TABLE mesh_runtime_profile;PRAGMA user_version=4;")
        .unwrap();
    assert_eq!(t.vault.status().unwrap().device.unwrap(), key);
    assert!(t.vault.runtime_profile().unwrap().is_none());
    assert_eq!(t.vault.local_status(&t.team).unwrap().stored, 1);
    conn.execute_batch("PRAGMA user_version=4;").unwrap();
    assert!(t.vault.status().is_err());
    assert_eq!(
        conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
            .unwrap(),
        4
    );
}
#[test]
fn network_scope_requires_guid_bindings_and_a_canonical_private_subnet() {
    assert!(network().validate().is_ok());
    for (mask, subnet) in [
        ("255.0.255.0", "192.168.20.0"),
        ("255.255.255.0", "192.168.20.2"),
        ("255.255.255.0", "8.8.8.0"),
    ] {
        let mut n = network();
        n.mask = mask.parse().unwrap();
        n.subnet = subnet.parse().unwrap();
        assert!(n.validate().is_err());
    }
    let mut n = network();
    n.adapter = "invalid".into();
    assert!(n.validate().is_err());
}

struct Gated {
    base: Mock,
    entered: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    released: (Mutex<bool>, std::sync::Condvar),
}
impl Environment for Gated {
    fn now(&self) -> u64 {
        self.base.now()
    }
    fn inspect(&self, address: Ipv4Addr) -> Result<NetworkScope> {
        if let Some(tx) = self.entered.lock().unwrap().take() {
            let _ = tx.send(());
        }
        let mut ready = self.released.0.lock().unwrap();
        while !*ready {
            ready = self.released.1.wait(ready).unwrap();
        }
        self.base.inspect(address)
    }
    fn locate(&self, scope: &NetworkScope) -> Result<Ipv4Addr> {
        self.base.locate(scope)
    }
}
impl Gated {
    fn release(&self) {
        *self.released.0.lock().unwrap() = true;
        self.released.1.notify_all();
    }
}
struct Release(Arc<Gated>);
impl Drop for Release {
    fn drop(&mut self) {
        self.0.release();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_preempts_an_inflight_start_and_no_late_startup_consent_is_committed() {
    let t = Team::new();
    let (tx, rx) = tokio::sync::oneshot::channel();
    let env = Arc::new(Gated {
        base: Mock::new(),
        entered: Mutex::new(Some(tx)),
        released: (Mutex::new(false), std::sync::Condvar::new()),
    });
    let _release = Release(env.clone());
    let c = Controller::with_environment(t.vault.clone(), env.clone());
    let start = c.clone();
    let config = t.config(true);
    let starting = tokio::spawn(async move { start.start(config).await });
    rx.await.unwrap();
    let stop = c.clone();
    let stopping = tokio::spawn(async move { stop.stop(None).await });
    tokio::time::timeout(Duration::from_secs(5), async {
        while !c.0.control.lock().unwrap().stopping {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    env.release();
    assert!(starting.await.unwrap().is_err());
    assert!(!stopping.await.unwrap().unwrap().session.running);
    assert!(t.vault.runtime_profile().unwrap().is_none());
    c.tick().await;
    assert!(!c.status().await.session.running);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exit_preempts_an_inflight_start_and_resume_cannot_reopen_it() {
    let t = Team::new();
    let (tx, rx) = tokio::sync::oneshot::channel();
    let env = Arc::new(Gated {
        base: Mock::new(),
        entered: Mutex::new(Some(tx)),
        released: (Mutex::new(false), std::sync::Condvar::new()),
    });
    let _release = Release(env.clone());
    let c = Controller::with_environment(t.vault.clone(), env.clone());
    let start = c.clone();
    let config = t.config(true);
    let starting = tokio::spawn(async move { start.start(config).await });
    rx.await.unwrap();
    c.event(Event::Exit);
    env.release();
    assert!(starting.await.unwrap().is_err());
    c.event(Event::Resume);
    c.tick().await;
    assert!(!c.status().await.session.running);
    assert!(t.vault.runtime_profile().unwrap().is_none());
    assert!(c.start(t.config(true)).await.is_err());
}

#[test]
#[ignore = "requires a real Windows LAN profile, selected with SAI_MESH_PROFILE_TEST"]
fn selected_windows_profile_is_read_without_changing_its_category() {
    let address = std::env::var("SAI_MESH_PROFILE_TEST")
        .expect("set a local private IPv4")
        .parse()
        .unwrap();
    let scope = SystemEnvironment.inspect(address).unwrap();
    scope.validate().unwrap();
    assert_eq!(SystemEnvironment.locate(&scope).unwrap(), address);
}
