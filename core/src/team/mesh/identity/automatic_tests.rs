use super::super::keys::random_id;
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
        let dir = std::env::temp_dir().join(format!("sai-auto-keys-{}", random_id().unwrap()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = Vault::new(dir.join("a.sqlite3"));
        let b = Vault::new(dir.join("b.sqlite3"));
        let c = Vault::new(dir.join("c.sqlite3"));
        for v in [&a, &b, &c] {
            v.create_device().unwrap();
        }
        let first = a.create_team("자동 연결 키 검증").unwrap();
        let team = first.manifest.body.team_id.clone();
        for v in [&b, &c] {
            v.import(
                &serde_json::to_string(&first).unwrap(),
                &first.manifest.fingerprint().unwrap(),
            )
            .unwrap();
        }
        for v in [&b, &c] {
            a.approve(
                &team,
                &serde_json::to_string(&v.request(&team).unwrap()).unwrap(),
            )
            .unwrap();
        }
        let packet = a.export(&team).unwrap();
        for v in [&b, &c] {
            v.import(&serde_json::to_string(&packet).unwrap(), "")
                .unwrap();
        }
        Self { dir, a, b, c, team }
    }
    fn scope(&self, v: &Vault) -> AutoScope {
        let state = v.export(&self.team).unwrap().state().unwrap();
        AutoScope {
            team: self.team.clone(),
            own: v.status().unwrap().device.unwrap().device_id,
            revision: state.revision,
            head: state.head,
        }
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
            .starts_with("sai-auto-keys-"));
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn discovery_tags_are_pair_specific_and_change_with_each_runtime() {
    let t = Team::new();
    let a = t.scope(&t.a);
    let b = t.scope(&t.b);
    let c = t.scope(&t.c);
    let ak = t.a.auto_routes(&a).unwrap();
    let bk = t.b.auto_routes(&b).unwrap();
    let ck = t.c.auto_routes(&c).unwrap();
    let session = random_id().unwrap();
    let tag = ak
        .offers(&session)
        .unwrap()
        .into_iter()
        .find(|tag| ak.receiver(&session, tag).as_deref() == Some(b.own.as_str()))
        .unwrap();
    assert_eq!(bk.match_offer(&session, &tag), Some(a.own.clone()));
    assert!(ck.match_offer(&session, &tag).is_none());
    assert!(bk.match_offer(&random_id().unwrap(), &tag).is_none());
    assert!(bk.match_offer(&session, &"00".repeat(32)).is_none());
    assert!(!tag.contains(&a.own));
    assert!(!tag.contains(&b.own));
    assert!(!tag.contains(&t.team));
}

#[test]
fn bootstrap_cards_are_encrypted_for_the_recipient_and_bound_to_both_fresh_nonces() {
    let t = Team::new();
    let a = t.scope(&t.a);
    let b = t.scope(&t.b);
    let c = t.scope(&t.c);
    let aad = BootAad {
        stage: Stage::Request,
        session: random_id().unwrap(),
        tag: "ab".repeat(32),
        server_nonce: random_id().unwrap(),
        client_nonce: random_id().unwrap(),
    };
    let card = "approved-card-private-metadata-test";
    let sealed = t.a.seal_auto(&a, &b.own, &aad, card).unwrap();
    assert!(!serde_json::to_string(&sealed).unwrap().contains(card));
    assert_eq!(
        t.b.open_auto(&b, &a.own, &aad, &sealed).unwrap().as_str(),
        card
    );
    assert!(t.c.open_auto(&c, &a.own, &aad, &sealed).is_err());
    for field in ["server", "client", "session", "tag", "stage"] {
        let mut changed = aad.clone();
        match field {
            "server" => changed.server_nonce = random_id().unwrap(),
            "client" => changed.client_nonce = random_id().unwrap(),
            "session" => changed.session = random_id().unwrap(),
            "tag" => changed.tag = "00".repeat(32),
            _ => changed.stage = Stage::Reply,
        }
        assert!(
            t.b.open_auto(&b, &a.own, &changed, &sealed).is_err(),
            "{field}"
        );
    }
}

#[test]
fn another_members_signature_cannot_impersonate_the_expected_bootstrap_peer() {
    let t = Team::new();
    let a = t.scope(&t.a);
    let b = t.scope(&t.b);
    let c = t.scope(&t.c);
    let aad = BootAad {
        stage: Stage::Reply,
        session: random_id().unwrap(),
        tag: "ab".repeat(32),
        server_nonce: random_id().unwrap(),
        client_nonce: random_id().unwrap(),
    };
    let wrong = t.c.seal_auto(&c, &b.own, &aad, "old-signed-card").unwrap();
    assert!(t.b.open_auto(&b, &a.own, &aad, &wrong).is_err());
    let sealed = t.a.seal_auto(&a, &b.own, &aad, "card").unwrap();
    let mut value = serde_json::to_value(&sealed).unwrap();
    value["sealed"]["ciphertext"] = serde_json::json!("AAAA");
    assert!(t
        .b
        .open_auto(&b, &a.own, &aad, &serde_json::from_value(value).unwrap())
        .is_err());
}

#[test]
fn membership_changes_invalidate_old_routes_and_stop_bootstrap_operations() {
    let t = Team::new();
    let a = t.scope(&t.a);
    let b = t.scope(&t.b);
    let c = t.scope(&t.c);
    let aad = BootAad {
        stage: Stage::Request,
        session: random_id().unwrap(),
        tag: "ab".repeat(32),
        server_nonce: random_id().unwrap(),
        client_nonce: random_id().unwrap(),
    };
    let sealed = t.a.seal_auto(&a, &b.own, &aad, "card").unwrap();
    let packet = t.a.revoke(&t.team, &c.own).unwrap();
    t.b.import(&serde_json::to_string(&packet).unwrap(), "")
        .unwrap();
    assert!(t.a.validate_auto_scope(&a).is_err());
    assert!(t.a.seal_auto(&a, &b.own, &aad, "card").is_err());
    assert!(t.b.open_auto(&b, &a.own, &aad, &sealed).is_err());
    let next_a = t.a.auto_routes(&t.scope(&t.a)).unwrap();
    let old = t.b.auto_routes(&b);
    assert!(old.is_err());
    let old_keys = t.c.auto_routes(&c);
    assert!(old_keys.is_ok()); // An offline PC can still know its stale local history.
    let next_b = t.b.auto_routes(&t.scope(&t.b)).unwrap();
    assert_eq!(
        next_b.match_offer(&aad.session, &next_a.offers(&aad.session).unwrap()[0]),
        Some(a.own)
    );
}

#[test]
fn automatic_peer_limit_fails_explicitly_without_dropping_approved_devices() {
    let t = Team::new();
    for _ in 0..7 {
        let key = super::super::keys::Material::generate(true).unwrap();
        let packet = t.a.export(&t.team).unwrap();
        let request =
            super::super::model::EnrollmentRequest::create(&key, &packet.manifest).unwrap();
        t.a.approve(&t.team, &serde_json::to_string(&request).unwrap())
            .unwrap();
    }
    assert!(t.a.auto_routes(&t.scope(&t.a)).is_err());
    assert_eq!(
        t.a.export(&t.team).unwrap().state().unwrap().devices.len(),
        10
    );
}
