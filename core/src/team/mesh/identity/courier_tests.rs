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
        let dir = std::env::temp_dir().join(format!("sai-courier-{}", random_id().unwrap()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = Vault::new(dir.join("a.sqlite3"));
        let b = Vault::new(dir.join("b.sqlite3"));
        let c = Vault::new(dir.join("c.sqlite3"));
        for v in [&a, &b, &c] {
            v.create_device().unwrap();
        }
        let first = a.create_team("중계 검증").unwrap();
        let team = first.manifest.body.team_id.clone();
        for v in [&b, &c] {
            v.import(
                &serde_json::to_string(&first).unwrap(),
                &first.manifest.fingerprint().unwrap(),
            )
            .unwrap();
        }
        for v in [&b, &c] {
            let packet = a
                .approve(
                    &team,
                    &serde_json::to_string(&v.request(&team).unwrap()).unwrap(),
                )
                .unwrap();
            for other in [&b, &c] {
                other
                    .import(&serde_json::to_string(&packet).unwrap(), "")
                    .unwrap();
            }
        }
        Self { dir, a, b, c, team }
    }
    fn scope(&self, from: &Vault, to: &Vault) -> Scope {
        let s = from.export(&self.team).unwrap().state().unwrap();
        Scope {
            team: self.team.clone(),
            own: from.status().unwrap().device.unwrap().device_id,
            peer: to.status().unwrap().device.unwrap().device_id,
            revision: s.revision,
            head: s.head,
        }
    }
    fn append(&self, text: &str) {
        self.a
            .append_local(&self.team, &random_id().unwrap(), text)
            .unwrap();
    }
    fn prepare(&self) -> Vec<Parcel> {
        let s = self.scope(&self.a, &self.b);
        self.a.prepare_courier(&s, &random_id().unwrap()).unwrap();
        self.a.courier_batch(&s, &[]).unwrap().0
    }
    fn send(&self, from: &Vault, to: &Vault) {
        let a = self.scope(from, to);
        let b = self.scope(to, from);
        let nonce = random_id().unwrap();
        from.prepare_courier(&a, &nonce).unwrap();
        loop {
            let inventory = to.courier_inventory(&b).unwrap();
            let (items, more) = from.courier_batch(&a, &inventory).unwrap();
            for item in items {
                to.store_courier(&b, &item, &nonce).unwrap();
            }
            if !more {
                break;
            }
        }
    }
    fn for_c(&self, items: &[Parcel]) -> Parcel {
        let c = self.scope(&self.c, &self.b).own;
        items
            .iter()
            .find(|p| p.delivery().context.receiver == c)
            .unwrap()
            .clone()
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
            .starts_with("sai-courier-"));
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn offline_author_delivery_and_end_recipient_confirmation_survive_courier_restart() {
    let t = Team::new();
    let text = "courier-private-body-never-plain-in-wire";
    t.append(text);
    t.send(&t.a, &t.b);
    assert_eq!(t.a.local_status(&t.team).unwrap().pending, 1);
    assert_eq!(t.c.local_status(&t.team).unwrap().received, 0);
    let b = Vault::new(t.dir.join("b.sqlite3"));
    // A와의 연결이나 A의 키를 사용하지 않고 재실행한 B가 C에게 전달한다.
    t.send(&b, &t.c);
    assert_eq!(t.c.local_status(&t.team).unwrap().messages[0].text, text);
    assert_eq!(t.a.local_status(&t.team).unwrap().pending, 1);
    t.send(&t.c, &b);
    t.send(&b, &t.a);
    assert_eq!(t.a.local_status(&t.team).unwrap().pending, 0);
    assert_eq!(
        t.a.local_status(&t.team).unwrap().messages[0].state,
        "peer-stored"
    );
    t.send(&b, &t.c);
    assert_eq!(t.c.local_status(&t.team).unwrap().received, 1);
    let s = t.scope(&b, &t.c);
    let conn = b.required().unwrap();
    let packet = b.export(&t.team).unwrap();
    let bkey = b.key(&conn, "device").unwrap().unwrap();
    for (_, p) in load(&conn, &packet).unwrap() {
        assert!(!serde_json::to_string(&p).unwrap().contains(text));
        if p.delivery().context.receiver == s.peer {
            assert!(p.delivery().open(&bkey).is_err());
        }
    }
    for suffix in ["", "-wal"] {
        if let Ok(bytes) = std::fs::read(t.dir.join(format!("b.sqlite3{suffix}"))) {
            assert!(!bytes.windows(text.len()).any(|w| w == text.as_bytes()));
        }
    }
}

#[test]
fn pc_deletion_and_confirmation_survive_a_courier_hop_while_the_author_is_offline() {
    let t = Team::new();
    let scope = t.a.talk_feed(&t.team, 0).unwrap().scope;
    let target =
        t.a.post_talk(&scope, &random_id().unwrap(), "중계 PC 톡 원문")
            .unwrap();
    t.a.delete_talk(&scope, &random_id().unwrap(), &target, 1)
        .unwrap();
    t.send(&t.a, &t.b);
    let courier = Vault::new(t.dir.join("b.sqlite3"));
    t.send(&courier, &t.c);
    let received = t.c.talk_feed(&t.team, 0).unwrap();
    assert_eq!(received.messages.len(), 1);
    assert!(received.messages[0].deleted);
    assert!(received.messages[0].text.is_empty());
    t.send(&t.c, &courier);
    t.send(&courier, &t.a);
    let sent = t.a.talk_feed(&t.team, 0).unwrap();
    assert_eq!(sent.messages[0].delete_state, Some("reference-stored"));
    assert_eq!(sent.pending, 0);
}

#[test]
fn gaps_are_stored_as_ciphertext_then_promoted_in_author_order_without_duplicates() {
    let t = Team::new();
    t.append("첫째");
    t.append("둘째");
    let items = t.prepare();
    let c = t.scope(&t.c, &t.b).own;
    let mut deliveries: Vec<_> = items
        .into_iter()
        .filter(|p| p.delivery().context.receiver == c)
        .collect();
    deliveries.sort_by_key(|p| p.delivery().event.header.sequence);
    let scope = t.scope(&t.c, &t.b);
    let nonce = random_id().unwrap();
    t.c.store_courier(&scope, &deliveries[1], &nonce).unwrap();
    assert_eq!(t.c.local_status(&t.team).unwrap().received, 0);
    t.c.store_courier(&scope, &deliveries[0], &nonce).unwrap();
    t.c.prepare_courier(&scope, &nonce).unwrap();
    assert_eq!(t.c.local_status(&t.team).unwrap().received, 2);
    let inventory = t.c.courier_inventory(&scope).unwrap();
    t.c.store_courier(&scope, &deliveries[0], &random_id().unwrap())
        .unwrap();
    assert_eq!(inventory, t.c.courier_inventory(&scope).unwrap());
}

#[test]
fn tampered_ciphertexts_targets_signatures_and_confirmations_never_clear_source_pending() {
    let t = Team::new();
    t.append("변조 금지");
    let p = t.for_c(&t.prepare());
    let scope = t.scope(&t.c, &t.b);
    let nonce = random_id().unwrap();
    for field in ["signature", "receiver", "head", "payload"] {
        let mut value = serde_json::to_value(&p).unwrap();
        let d = &mut value["value"];
        match field {
            "signature" => d["signature"] = "00".repeat(64).into(),
            "receiver" => d["context"]["receiver"] = "00".repeat(32).into(),
            "head" => d["context"]["membership_head"] = "00".repeat(32).into(),
            _ => d["payload"] = serde_json::Value::Null,
        }
        let bad: Parcel = serde_json::from_value(value).unwrap();
        assert!(t.c.store_courier(&scope, &bad, &nonce).is_err());
    }
    assert_eq!(t.c.local_status(&t.team).unwrap().received, 0);
    t.c.store_courier(&scope, &p, &nonce).unwrap();
    let (items, _) = t.c.courier_batch(&scope, &[]).unwrap();
    let proof = items
        .iter()
        .find(|p| matches!(p, Parcel::Confirmation { .. }))
        .unwrap();
    let mut value = serde_json::to_value(proof).unwrap();
    value["value"]["receipt"]["signature"] = "00".repeat(64).into();
    assert!(t
        .a
        .store_courier(
            &t.scope(&t.a, &t.b),
            &serde_json::from_value(value).unwrap(),
            &nonce
        )
        .is_err());
    assert_eq!(t.a.local_status(&t.team).unwrap().pending, 1);
}

#[test]
fn a_valid_receipt_for_a_different_encryption_is_not_accepted_as_courier_confirmation() {
    let t = Team::new();
    t.append("정확한 암호문 확인");
    t.prepare();
    let ac = t.scope(&t.a, &t.c);
    let ca = t.scope(&t.c, &t.a);
    let nonce = random_id().unwrap();
    let frontier = t.c.receive_frontier(&ca, &nonce).unwrap();
    let cursor = t.a.begin_push(&ac, &frontier, &nonce).unwrap();
    let delivery = t.a.next_delivery(&ac, &cursor).unwrap().unwrap();
    let receipt = t.c.store_delivery(&ca, &delivery, &nonce).unwrap();
    let p = Parcel::Confirmation { delivery, receipt };
    assert!(t.a.store_courier(&t.scope(&t.a, &t.b), &p, &nonce).is_err());
    assert_eq!(t.a.local_status(&t.team).unwrap().pending, 1);
}

#[test]
fn changed_approval_holds_preexisting_ciphertexts_and_old_bodies_are_not_reencrypted() {
    let t = Team::new();
    t.append("승인 변경 전 본문");
    let old = t.for_c(&t.prepare());
    let b = t.scope(&t.a, &t.b).peer;
    let packet = t.a.revoke(&t.team, &b).unwrap();
    t.c.import(&serde_json::to_string(&packet).unwrap(), "")
        .unwrap();
    t.b.import(&serde_json::to_string(&packet).unwrap(), "")
        .unwrap();
    let ca = t.scope(&t.c, &t.a);
    let ac = t.scope(&t.a, &t.c);
    let nonce = random_id().unwrap();
    assert!(t.c.store_courier(&ca, &old, &nonce).is_err());
    t.a.prepare_courier(&ac, &nonce).unwrap();
    let fresh = t.a.courier_batch(&ac, &[]).unwrap().0;
    assert!(fresh.iter().all(|p| p.delivery().payload.is_none()));
    for p in fresh {
        t.c.store_courier(&ca, &p, &nonce).unwrap();
    }
    assert_eq!(t.c.local_status(&t.team).unwrap().received, 0);
    assert_eq!(
        t.a.local_status(&t.team).unwrap().messages[0].state,
        "approval-held"
    );
    assert!(t.b.prepare_courier(&t.scope(&t.b, &t.a), &nonce).is_err());
}

#[test]
fn legacy_local_only_policy_stays_header_only_in_courier_storage() {
    let t = Team::new();
    t.append("로컬 전용");
    t.a.required()
        .unwrap()
        .execute(
            "UPDATE mesh_local_send_policy SET enabled=0,public=NULL,proof=NULL",
            [],
        )
        .unwrap();
    let items = t.prepare();
    assert!(!items.is_empty());
    assert!(items.iter().all(|p| p.delivery().payload.is_none()));
    assert_eq!(
        t.a.local_status(&t.team).unwrap().messages[0].state,
        "legacy-held"
    );
}

#[test]
fn failed_courier_commit_returns_no_confirmation_and_retry_preserves_received_body() {
    let t = Team::new();
    t.append("커밋 실패와 재시도");
    let p = t.for_c(&t.prepare());
    let scope = t.scope(&t.c, &t.b);
    let nonce = random_id().unwrap();
    let conn = t.c.required().unwrap();
    conn.execute_batch("CREATE TRIGGER deny_confirmation BEFORE INSERT ON mesh_courier WHEN NEW.kind='confirmation' BEGIN SELECT RAISE(ABORT,'test'); END;").unwrap();
    assert!(t.c.store_courier(&scope, &p, &nonce).is_err());
    assert_eq!(t.c.local_status(&t.team).unwrap().received, 1);
    assert_eq!(t.c.courier_inventory(&scope).unwrap().len(), 1);
    assert_eq!(t.a.local_status(&t.team).unwrap().pending, 1);
    conn.execute_batch("DROP TRIGGER deny_confirmation;")
        .unwrap();
    t.c.prepare_courier(&scope, &nonce).unwrap();
    assert_eq!(t.c.courier_inventory(&scope).unwrap().len(), 2);
    assert_eq!(t.c.local_status(&t.team).unwrap().received, 1);
}

#[test]
fn author_signed_fork_is_preserved_as_a_block_instead_of_overwriting_any_record() {
    let t = Team::new();
    t.append("원본");
    let p = t.for_c(&t.prepare());
    let scope = t.scope(&t.b, &t.a);
    let nonce = random_id().unwrap();
    t.b.store_courier(&scope, &p, &nonce).unwrap();
    let mut fork = p.delivery().clone();
    let conn = t.a.required().unwrap();
    let key = t.a.key(&conn, "device").unwrap().unwrap();
    fork.event.header.request = random_id().unwrap();
    fork.event.signature = key.sign(journal::DOMAIN, &fork.event.header).unwrap();
    let mut json = serde_json::to_value(&fork).unwrap();
    json["context"]["event_id"] = fork.event.id().unwrap().into();
    json["context"]["body_allowed"] = false.into();
    json["payload"] = serde_json::Value::Null;
    fork = serde_json::from_value(json).unwrap();
    let signature = key
        .sign(
            "sai-mesh/delivery/1",
            &(&fork.event, &fork.context, &fork.payload),
        )
        .unwrap();
    let mut value = serde_json::to_value(&fork).unwrap();
    value["signature"] = signature.into();
    fork = serde_json::from_value(value).unwrap();
    assert!(t
        .b
        .store_courier(&scope, &Parcel::Delivery(fork), &nonce)
        .is_err());
    assert_eq!(t.b.courier_inventory(&scope).unwrap().len(), 1);
    assert_eq!(t.b.local_status(&t.team).unwrap().conflicts, 1);
}

#[test]
fn v3_upgrade_preserves_keys_and_source_and_failed_migration_leaves_v3_intact() {
    let t = Team::new();
    t.append("이전 저장소");
    let device = t.a.status().unwrap().device.unwrap();
    let conn = t.a.required().unwrap();
    conn.execute_batch("DROP TABLE mesh_legacy_archive; DROP TABLE mesh_runtime_profile; DROP TABLE mesh_courier; PRAGMA user_version=3;")
        .unwrap();
    assert_eq!(t.a.status().unwrap().device.unwrap(), device);
    assert_eq!(
        t.a.local_status(&t.team).unwrap().messages[0].text,
        "이전 저장소"
    );
    assert_eq!(
        conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
            .unwrap(),
        6
    );
    conn.execute_batch("PRAGMA user_version=3;").unwrap();
    assert!(t.a.required().is_err()); // Existing unexpected table is never replaced.
    assert_eq!(
        conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
            .unwrap(),
        3
    );
    assert_eq!(
        conn.query_row("SELECT count(*) FROM mesh_local_events", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn oversized_duplicate_inventories_and_storage_tampering_are_rejected() {
    let t = Team::new();
    t.append("목록 한도");
    t.prepare();
    let scope = t.scope(&t.a, &t.b);
    assert!(t
        .a
        .courier_batch(&scope, &vec!["00".repeat(32); MAX_ITEMS + 1])
        .is_err());
    assert!(t
        .a
        .courier_batch(&scope, &["00".repeat(32), "00".repeat(32)])
        .is_err());
    t.a.required()
        .unwrap()
        .execute("UPDATE mesh_courier SET receiver='corrupt'", [])
        .unwrap();
    assert!(t.a.courier_inventory(&scope).is_err());
}

#[test]
fn a_confirmation_alone_contains_the_authentic_ciphertext_for_a_rolled_back_recipient() {
    let t = Team::new();
    t.append("다시 받을 본문");
    let item = t.for_c(&t.prepare());
    let scope = t.scope(&t.c, &t.b);
    let nonce = random_id().unwrap();
    t.c.store_courier(&scope, &item, &nonce).unwrap();
    let proof =
        t.c.courier_batch(&scope, &[])
            .unwrap()
            .0
            .into_iter()
            .find(|p| matches!(p, Parcel::Confirmation { .. }))
            .unwrap();
    t.c.required()
        .unwrap()
        .execute_batch(
            "DELETE FROM mesh_received; DELETE FROM mesh_receive_heads; DELETE FROM mesh_courier;",
        )
        .unwrap();
    t.c.store_courier(&scope, &proof, &nonce).unwrap();
    let status = t.c.local_status(&t.team).unwrap();
    assert_eq!(status.received, 1);
    assert_eq!(status.messages[0].text, "다시 받을 본문");
}

#[test]
fn relayed_author_signature_blocks_a_rolled_back_source_before_it_reuses_a_sequence() {
    let t = Team::new();
    t.append("되돌려진 작성 PC");
    let item = t.for_c(&t.prepare());
    t.a.required().unwrap().execute_batch("DELETE FROM mesh_local_send_policy; DELETE FROM mesh_local_outbox; DELETE FROM mesh_sent_receipts; DELETE FROM mesh_local_events; DELETE FROM mesh_local_heads; DELETE FROM mesh_courier;").unwrap();
    assert!(t
        .a
        .store_courier(&t.scope(&t.a, &t.b), &item, &random_id().unwrap())
        .is_err());
    assert!(t.a.local_status(&t.team).unwrap().write_blocked);
    assert!(t
        .a
        .append_local(&t.team, &random_id().unwrap(), "순번 재사용 금지")
        .is_err());
}
