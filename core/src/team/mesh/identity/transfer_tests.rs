use super::super::keys::random_id;
use super::*;
use std::path::PathBuf;

struct Team {
    path: PathBuf,
    a: Vault,
    b: Vault,
    c: Vault,
    team: String,
}
impl Team {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("sai-transfer-{}", random_id().unwrap()));
        std::fs::create_dir_all(&path).unwrap();
        let a = Vault::new(path.join("a.sqlite3"));
        let b = Vault::new(path.join("b.sqlite3"));
        let c = Vault::new(path.join("c.sqlite3"));
        a.create_device().unwrap();
        b.create_device().unwrap();
        c.create_device().unwrap();
        let first = a.create_team("암호화 교환").unwrap();
        let team = first.manifest.body.team_id.clone();
        for v in [&b, &c] {
            v.import(
                &serde_json::to_string(&first).unwrap(),
                &first.manifest.fingerprint().unwrap(),
            )
            .unwrap();
        }
        let packet = a
            .approve(
                &team,
                &serde_json::to_string(&b.request(&team).unwrap()).unwrap(),
            )
            .unwrap();
        for v in [&b, &c] {
            v.import(&serde_json::to_string(&packet).unwrap(), "")
                .unwrap();
        }
        Self {
            path,
            a,
            b,
            c,
            team,
        }
    }
    fn enroll_c(&self) {
        let packet = self
            .a
            .approve(
                &self.team,
                &serde_json::to_string(&self.c.request(&self.team).unwrap()).unwrap(),
            )
            .unwrap();
        for v in [&self.b, &self.c] {
            v.import(&serde_json::to_string(&packet).unwrap(), "")
                .unwrap();
        }
    }
    fn scope(&self, from: &Vault, to: &Vault) -> Scope {
        let state = from.export(&self.team).unwrap().state().unwrap();
        Scope {
            team: self.team.clone(),
            own: from.status().unwrap().device.unwrap().device_id,
            peer: to.status().unwrap().device.unwrap().device_id,
            revision: state.revision,
            head: state.head,
        }
    }
    fn append(&self, v: &Vault, text: &str) -> String {
        v.append_local(&self.team, &random_id().unwrap(), text)
            .unwrap()
    }
    fn first(&self, from: &Vault, to: &Vault, nonce: &str) -> (Scope, Scope, Cursor, Delivery) {
        let a = self.scope(from, to);
        let b = self.scope(to, from);
        let frontier = to.receive_frontier(&b, nonce).unwrap();
        let cursor = from.begin_push(&a, &frontier, nonce).unwrap();
        let delivery = from.next_delivery(&a, &cursor).unwrap().unwrap();
        (a, b, cursor, delivery)
    }
}
impl Drop for Team {
    fn drop(&mut self) {
        assert_eq!(self.path.parent(), Some(std::env::temp_dir().as_path()));
        assert!(self
            .path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("sai-transfer-"));
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[test]
fn hpke_ciphertext_is_recipient_specific_and_plaintext_is_absent_from_wire_and_sqlite() {
    let t = Team::new();
    t.enroll_c();
    let text = "encrypted-transfer-unique-private-body";
    t.append(&t.a, text);
    let nonce = random_id().unwrap();
    let (_, b, _, delivery) = t.first(&t.a, &t.b, &nonce);
    let encoded = serde_json::to_string(&delivery).unwrap();
    assert!(!encoded.contains(text));
    let conn = t.c.required().unwrap();
    let key = t.c.key(&conn, "device").unwrap().unwrap();
    assert!(delivery.open(&key).is_err());
    assert!(crypt::open(
        &key,
        &signed_bytes("sai-mesh/body-aad/1", &delivery.context).unwrap(),
        delivery.payload.as_ref().unwrap()
    )
    .is_err());
    let receipt = t.b.store_delivery(&b, &delivery, &nonce).unwrap();
    assert!(receipt.has_body());
    let reopened = Vault::new(t.path.join("b.sqlite3"))
        .local_status(&t.team)
        .unwrap();
    assert_eq!(reopened.received, 1);
    assert_eq!(reopened.messages[0].text, text);
    assert!(reopened.messages[0].incoming);
    for suffix in ["", "-wal"] {
        if let Ok(bytes) = std::fs::read(t.path.join(format!("b.sqlite3{suffix}"))) {
            assert!(!bytes.windows(text.len()).any(|w| w == text.as_bytes()));
        }
    }
    let (_, c, _, other) = t.first(&t.a, &t.c, &nonce);
    assert_ne!(encoded, serde_json::to_string(&other).unwrap());
    assert!(t.c.store_delivery(&c, &delivery, &nonce).is_err());
    t.c.store_delivery(&c, &other, &nonce).unwrap();
}

#[test]
fn only_each_signed_body_receipt_clears_its_own_recipient_obligation() {
    let t = Team::new();
    t.enroll_c();
    t.append(&t.a, "모든 수신 장치 확인");
    let nonce = random_id().unwrap();
    let (a, b, mut cursor, delivery) = t.first(&t.a, &t.b, &nonce);
    let receipt = t.b.store_delivery(&b, &delivery, &nonce).unwrap();
    for change in ["nonce", "receiver", "signature", "event"] {
        let mut bad = receipt.clone();
        match change {
            "nonce" => bad.body.challenge = random_id().unwrap(),
            "receiver" => bad.body.receiver = t.c.status().unwrap().device.unwrap().device_id,
            "event" => bad.body.event_id = "00".repeat(32),
            _ => bad.signature = "00".repeat(64),
        }
        assert!(t
            .a
            .accept_receipt(&a, &delivery, &bad, &nonce, &mut cursor)
            .is_err());
    }
    assert_eq!(t.a.local_status(&t.team).unwrap().pending, 1);
    t.a.accept_receipt(&a, &delivery, &receipt, &nonce, &mut cursor)
        .unwrap();
    t.a.accept_receipt(&a, &delivery, &receipt, &nonce, &mut cursor)
        .unwrap();
    assert_eq!(t.a.local_status(&t.team).unwrap().pending, 1);
    let (a, c, mut cursor, delivery) = t.first(&t.a, &t.c, &nonce);
    let receipt = t.c.store_delivery(&c, &delivery, &nonce).unwrap();
    t.a.accept_receipt(&a, &delivery, &receipt, &nonce, &mut cursor)
        .unwrap();
    let status = t.a.local_status(&t.team).unwrap();
    assert_eq!(status.pending, 0);
    assert_eq!(status.messages[0].state, "peer-stored");
}

#[test]
fn commit_failure_never_returns_a_receipt_and_lost_ack_retries_without_duplicate_messages() {
    let t = Team::new();
    t.append(&t.a, "오류와 재시도");
    let nonce = random_id().unwrap();
    let (_, b, _, delivery) = t.first(&t.a, &t.b, &nonce);
    let conn = t.b.required().unwrap();
    conn.execute_batch("CREATE TRIGGER deny_incoming BEFORE INSERT ON mesh_received BEGIN SELECT RAISE(ABORT,'test failure'); END;").unwrap();
    assert!(t.b.store_delivery(&b, &delivery, &nonce).is_err());
    assert_eq!(t.b.local_status(&t.team).unwrap().received, 0);
    assert_eq!(
        conn.query_row("SELECT count(*) FROM mesh_receive_heads", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    conn.execute_batch("DROP TRIGGER deny_incoming;").unwrap();
    t.b.store_delivery(&b, &delivery, &nonce).unwrap(); // Commit succeeded, but its receipt is lost.
    assert_eq!(t.a.local_status(&t.team).unwrap().pending, 1);
    let fresh = random_id().unwrap();
    let (a, b, mut cursor, retry) = t.first(&t.a, &t.b, &fresh);
    assert_eq!(retry.event.id().unwrap(), delivery.event.id().unwrap());
    assert_ne!(retry.hash().unwrap(), delivery.hash().unwrap());
    let proof = t.b.store_delivery(&b, &retry, &fresh).unwrap();
    t.a.accept_receipt(&a, &retry, &proof, &fresh, &mut cursor)
        .unwrap();
    assert_eq!(t.b.local_status(&t.team).unwrap().received, 1);
    assert_eq!(t.a.local_status(&t.team).unwrap().pending, 0);
}

#[test]
fn receipt_save_failure_keeps_the_source_pending_even_when_the_peer_has_committed() {
    let t = Team::new();
    t.append(&t.a, "영수증 저장 실패");
    let nonce = random_id().unwrap();
    let (a, b, mut cursor, delivery) = t.first(&t.a, &t.b, &nonce);
    let receipt = t.b.store_delivery(&b, &delivery, &nonce).unwrap();
    let conn = t.a.required().unwrap();
    conn.execute_batch("CREATE TRIGGER deny_ack BEFORE INSERT ON mesh_sent_receipts BEGIN SELECT RAISE(ABORT,'test failure'); END;").unwrap();
    assert!(t
        .a
        .accept_receipt(&a, &delivery, &receipt, &nonce, &mut cursor)
        .is_err());
    assert_eq!(t.a.local_status(&t.team).unwrap().pending, 1);
    assert_eq!(t.b.local_status(&t.team).unwrap().received, 1);
    conn.execute_batch("DROP TRIGGER deny_ack;").unwrap();
    t.a.accept_receipt(&a, &delivery, &receipt, &nonce, &mut cursor)
        .unwrap();
    assert_eq!(t.a.local_status(&t.team).unwrap().pending, 0);
}

#[test]
fn changed_membership_sends_only_old_headers_and_never_reencrypts_old_bodies() {
    let t = Team::new();
    t.append(&t.a, "이전 승인 범위의 비밀");
    t.enroll_c();
    t.append(&t.a, "새 승인 이력에서 작성한 톡");
    for to in [&t.b, &t.c] {
        let nonce = random_id().unwrap();
        let (a, b, mut cursor, first) = t.first(&t.a, to, &nonce);
        assert!(first.payload.is_none());
        let receipt = to.store_delivery(&b, &first, &nonce).unwrap();
        assert!(!receipt.has_body());
        t.a.accept_receipt(&a, &first, &receipt, &nonce, &mut cursor)
            .unwrap();
        let next = t.a.next_delivery(&a, &cursor).unwrap().unwrap();
        assert_eq!(next.event.header.sequence, 2);
        assert!(next.payload.is_some());
        let receipt = to.store_delivery(&b, &next, &nonce).unwrap();
        t.a.accept_receipt(&a, &next, &receipt, &nonce, &mut cursor)
            .unwrap();
        let status = to.local_status(&t.team).unwrap();
        assert_eq!(status.received, 1);
        assert_eq!(status.messages[0].text, "새 승인 이력에서 작성한 톡");
    }
    let status = t.a.local_status(&t.team).unwrap();
    assert_eq!(status.pending, 1);
    assert_eq!(status.messages[0].state, "approval-held");
}

#[test]
fn restoring_an_old_source_and_reusing_a_sequence_is_detected_by_the_peer_frontier() {
    let t = Team::new();
    let original = t.append(&t.a, "동료에게 보관된 원본");
    let nonce = random_id().unwrap();
    let (_, b, _, delivery) = t.first(&t.a, &t.b, &nonce);
    t.b.store_delivery(&b, &delivery, &nonce).unwrap();
    let conn = t.a.required().unwrap();
    conn.execute_batch("BEGIN IMMEDIATE; DELETE FROM mesh_sent_receipts; DELETE FROM mesh_local_send_policy; DELETE FROM mesh_local_outbox; DELETE FROM mesh_local_events; DELETE FROM mesh_local_heads; COMMIT;").unwrap();
    let changed = t.append(&t.a, "백업 복원 후 다른 원본");
    assert_ne!(original, changed);
    let a = t.scope(&t.a, &t.b);
    let b = t.scope(&t.b, &t.a);
    let frontier = t.b.receive_frontier(&b, &nonce).unwrap();
    assert!(t.a.begin_push(&a, &frontier, &nonce).is_err());
    assert!(t
        .a
        .append_local(&t.team, &random_id().unwrap(), "순번 추가 재사용")
        .is_err());
    let status = t.a.local_status(&t.team).unwrap();
    assert!(status.write_blocked);
    assert!(!status.can_write);
    assert_eq!(
        t.b.local_status(&t.team).unwrap().messages[0].text,
        "동료에게 보관된 원본"
    );
}

#[test]
fn signed_sequence_conflicts_are_preserved_and_block_further_incoming_events() {
    let t = Team::new();
    t.append(&t.a, "보존할 원본");
    let nonce = random_id().unwrap();
    let (a, b, _, delivery) = t.first(&t.a, &t.b, &nonce);
    t.b.store_delivery(&b, &delivery, &nonce).unwrap();
    let conn = t.a.required().unwrap();
    let key = t.a.key(&conn, "device").unwrap().unwrap();
    let packet = t.a.export(&t.team).unwrap();
    let mut forged = delivery.clone();
    forged.event.header.request = random_id().unwrap();
    forged.event.signature = key.sign(journal::DOMAIN, &forged.event.header).unwrap();
    forged.context.event_id = forged.event.id().unwrap();
    let records = journal::read_chain(&conn, &packet, &a.own).unwrap();
    let mut body = journal::read_body_data(&records[0]).unwrap();
    body.event_id = forged.context.event_id.clone();
    let plain = Zeroizing::new(serde_json::to_vec(&body).unwrap());
    forged.payload = Some(
        crypt::seal(
            &packet.state().unwrap().devices[&a.peer]
                .device
                .encryption_key,
            &signed_bytes("sai-mesh/body-aad/1", &forged.context).unwrap(),
            &plain,
        )
        .unwrap(),
    );
    forged.signature = key
        .sign(DELIVERY, &(&forged.event, &forged.context, &forged.payload))
        .unwrap();
    assert!(t.b.store_delivery(&b, &forged, &nonce).is_err());
    assert!(t.b.receive_frontier(&b, &nonce).is_err());
    let status = t.b.local_status(&t.team).unwrap();
    assert_eq!(status.received, 1);
    assert_eq!(status.messages[0].text, "보존할 원본");
}

#[test]
fn wrong_recipient_tampered_ciphertext_and_missing_prefix_do_not_get_storage_receipts() {
    let t = Team::new();
    t.append(&t.a, "첫 원본");
    t.append(&t.a, "둘째 원본");
    let nonce = random_id().unwrap();
    let (a, b, _, delivery) = t.first(&t.a, &t.b, &nonce);
    let mut bad = delivery.clone();
    bad.context.receiver = a.own.clone();
    assert!(t.b.store_delivery(&b, &bad, &nonce).is_err());
    let conn = t.a.required().unwrap();
    let key = t.a.key(&conn, "device").unwrap().unwrap();
    let mut bad = delivery.clone();
    bad.payload.as_mut().unwrap().ciphertext = "AAAA".into();
    bad.signature = key
        .sign(DELIVERY, &(&bad.event, &bad.context, &bad.payload))
        .unwrap();
    assert!(t.b.store_delivery(&b, &bad, &nonce).is_err());
    let cursor = Cursor {
        sequence: 1,
        id: delivery.event.id().unwrap(),
    };
    // Pretend A already holds B's acknowledgement without modifying A's persistent state.
    let records = journal::read_chain(&conn, &t.a.export(&t.team).unwrap(), &a.own).unwrap();
    let mut second = t.a.next_delivery(&a, &cursor).unwrap().unwrap();
    // The first unacknowledged body is deliberately resent; force the next row for a gap test.
    second.event = records[1].event.clone();
    second.context.event_id = second.event.id().unwrap();
    let body = journal::read_body_data(&records[1]).unwrap();
    let plain = Zeroizing::new(serde_json::to_vec(&body).unwrap());
    second.payload = Some(
        crypt::seal(
            &t.a.export(&t.team).unwrap().state().unwrap().devices[&a.peer]
                .device
                .encryption_key,
            &signed_bytes("sai-mesh/body-aad/1", &second.context).unwrap(),
            &plain,
        )
        .unwrap(),
    );
    second.signature = key
        .sign(DELIVERY, &(&second.event, &second.context, &second.payload))
        .unwrap();
    assert!(t.b.store_delivery(&b, &second, &nonce).is_err());
    assert_eq!(t.b.local_status(&t.team).unwrap().received, 0);
}

#[test]
fn missing_or_corrupt_received_ciphertext_cannot_be_reported_as_a_valid_frontier() {
    let t = Team::new();
    t.append(&t.a, "수신 본문 보호");
    let nonce = random_id().unwrap();
    let (_, b, _, delivery) = t.first(&t.a, &t.b, &nonce);
    t.b.store_delivery(&b, &delivery, &nonce).unwrap();
    let conn = t.b.required().unwrap();
    let encoded: String = conn
        .query_row("SELECT payload FROM mesh_received", [], |r| r.get(0))
        .unwrap();
    conn.execute("UPDATE mesh_received SET payload=NULL", [])
        .unwrap();
    assert!(t.b.receive_frontier(&b, &nonce).is_err());
    conn.execute(
        "UPDATE mesh_received SET payload=?1",
        [encoded.replace("hpke-x25519", "broken-suite")],
    )
    .unwrap();
    assert!(t.b.receive_frontier(&b, &nonce).is_err());
}

#[test]
fn revoked_peer_and_stale_session_scopes_cannot_transfer_or_acknowledge() {
    let t = Team::new();
    t.append(&t.a, "승인 해제 경합");
    let nonce = random_id().unwrap();
    let (a, b, mut cursor, delivery) = t.first(&t.a, &t.b, &nonce);
    let receipt = t.b.store_delivery(&b, &delivery, &nonce).unwrap();
    let packet = t.a.revoke(&t.team, &a.peer).unwrap();
    t.b.import(&serde_json::to_string(&packet).unwrap(), "")
        .unwrap();
    assert!(t
        .a
        .accept_receipt(&a, &delivery, &receipt, &nonce, &mut cursor)
        .is_err());
    assert!(t.a.next_delivery(&a, &cursor).is_err());
    assert!(t.b.store_delivery(&b, &delivery, &nonce).is_err());
    assert_eq!(t.a.local_status(&t.team).unwrap().pending, 1);
}

#[test]
fn a_peers_unproven_frontier_claim_cannot_lock_the_authors_device() {
    let t = Team::new();
    t.append(&t.a, "잠그지 않을 원본");
    let nonce = random_id().unwrap();
    let a = t.scope(&t.a, &t.b);
    let b = t.scope(&t.b, &t.a);
    let mut frontier = t.b.receive_frontier(&b, &nonce).unwrap();
    frontier.body.sequence = 500;
    frontier.body.event_id = "00".repeat(32);
    let conn = t.b.required().unwrap();
    let key = t.b.key(&conn, "device").unwrap().unwrap();
    frontier.signature = key.sign(FRONTIER, &frontier.body).unwrap();
    assert!(t.a.begin_push(&a, &frontier, &nonce).is_err());
    assert!(t.a.local_status(&t.team).unwrap().can_write);
    t.append(&t.a, "잘못된 주장 뒤에도 저장 가능");
}

#[test]
fn v2_local_only_records_remain_held_after_upgrade_while_new_prepared_records_can_transfer() {
    let t = Team::new();
    t.append(&t.a, "이전 단계에서 로컬 확인용으로 보관한 내용");
    let before = t.a.status().unwrap().device.unwrap();
    let conn = t.a.required().unwrap();
    // Published v2 used a smaller DPAPI blob CHECK; reconstruct that schema with real records.
    conn.execute_batch("DROP TABLE mesh_legacy_archive; DROP TABLE mesh_runtime_profile; DROP TABLE mesh_courier; DROP TABLE mesh_received; DROP TABLE mesh_receive_heads; DROP TABLE mesh_sent_receipts; DROP TABLE mesh_transfer_blocks; DROP TABLE mesh_local_send_policy;
        CREATE TEMP TABLE saved_events AS SELECT * FROM mesh_local_events; CREATE TEMP TABLE saved_outbox AS SELECT * FROM mesh_local_outbox;
        DROP TABLE mesh_local_outbox; DROP TABLE mesh_local_events;
        CREATE TABLE mesh_local_events(team TEXT NOT NULL REFERENCES mesh_teams(team_id),author TEXT NOT NULL,sequence INTEGER NOT NULL CHECK(sequence>0),
            id TEXT NOT NULL,request TEXT NOT NULL,event TEXT NOT NULL,sealed BLOB NOT NULL CHECK(length(sealed)<=65536),
            PRIMARY KEY(team,author,sequence),UNIQUE(team,id),UNIQUE(team,author,request));
        INSERT INTO mesh_local_events SELECT * FROM saved_events;
        CREATE TABLE mesh_local_outbox(team TEXT NOT NULL,id TEXT NOT NULL,PRIMARY KEY(team,id),FOREIGN KEY(team,id) REFERENCES mesh_local_events(team,id));
        INSERT INTO mesh_local_outbox SELECT * FROM saved_outbox; DROP TABLE saved_events; DROP TABLE saved_outbox; PRAGMA user_version=2;").unwrap();
    let status = t.a.local_status(&t.team).unwrap();
    assert_eq!(status.messages[0].state, "legacy-held");
    assert_eq!(status.pending, 1);
    assert_eq!(t.a.status().unwrap().device.unwrap(), before);
    let text = format!("{}x", "\n\"".repeat(16383));
    t.append(&t.a, &text);
    assert!(
        conn.query_row(
            "SELECT length(sealed) FROM mesh_local_events WHERE sequence=2",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap()
            > 65536
    );
    assert_eq!(
        conn.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| r
            .get::<_, i64>(
            0
        ))
        .unwrap(),
        0
    );
    let nonce = random_id().unwrap();
    let (a, b, mut cursor, old) = t.first(&t.a, &t.b, &nonce);
    assert!(old.payload.is_none());
    let receipt = t.b.store_delivery(&b, &old, &nonce).unwrap();
    t.a.accept_receipt(&a, &old, &receipt, &nonce, &mut cursor)
        .unwrap();
    let next = t.a.next_delivery(&a, &cursor).unwrap().unwrap();
    let receipt = t.b.store_delivery(&b, &next, &nonce).unwrap();
    t.a.accept_receipt(&a, &next, &receipt, &nonce, &mut cursor)
        .unwrap();
    assert_eq!(t.a.local_status(&t.team).unwrap().pending, 1);
    let received = t.b.local_status(&t.team).unwrap();
    assert_eq!(received.received, 1);
    assert_eq!(received.messages[0].text, text);
    conn.execute(
        "UPDATE mesh_local_send_policy SET enabled=1 WHERE id=?1",
        [old.event.id().unwrap()],
    )
    .unwrap();
    assert!(t.a.next_delivery(&a, &cursor).is_err()); // A flipped bit is not a signed send intent.
}

#[test]
fn send_intent_signatures_cannot_be_swapped_between_local_records() {
    let t = Team::new();
    let first = t.append(&t.a, "첫 발신 동의");
    let second = t.append(&t.a, "둘째 발신 동의");
    let conn = t.a.required().unwrap();
    let proof: String = conn
        .query_row(
            "SELECT proof FROM mesh_local_send_policy WHERE id=?1",
            [second],
            |r| r.get(0),
        )
        .unwrap();
    conn.execute(
        "UPDATE mesh_local_send_policy SET proof=?1 WHERE id=?2",
        params![proof, first],
    )
    .unwrap();
    assert!(t.a.local_status(&t.team).is_err());
    assert!(t
        .a
        .append_local(&t.team, &random_id().unwrap(), "추가 쓰기")
        .is_err());
}
