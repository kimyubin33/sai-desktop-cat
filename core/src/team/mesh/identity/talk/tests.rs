use super::super::{keys::random_id, transfer::Scope};
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
        let dir = std::env::temp_dir().join(format!("sai-talk-{}", random_id().unwrap()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = Vault::new(dir.join("a.sqlite3"));
        let b = Vault::new(dir.join("b.sqlite3"));
        let c = Vault::new(dir.join("c.sqlite3"));
        for v in [&a, &b, &c] {
            v.create_device().unwrap();
        }
        let first = a.create_team("PC 톡").unwrap();
        let team = first.manifest.body.team_id.clone();
        for v in [&b, &c] {
            v.import(
                &serde_json::to_string(&first).unwrap(),
                &first.manifest.fingerprint().unwrap(),
            )
            .unwrap();
        }
        let t = Self { dir, a, b, c, team };
        t.enroll(&t.b);
        t
    }
    fn enroll(&self, v: &Vault) {
        let p = self
            .a
            .approve(
                &self.team,
                &serde_json::to_string(&v.request(&self.team).unwrap()).unwrap(),
            )
            .unwrap();
        for other in [&self.b, &self.c] {
            other
                .import(&serde_json::to_string(&p).unwrap(), "")
                .unwrap();
        }
    }
    fn scope(&self, v: &Vault) -> TalkScope {
        v.talk_feed(&self.team, 0).unwrap().scope
    }
    fn wire(&self, from: &Vault, to: &Vault) -> Scope {
        let s = self.scope(from);
        Scope {
            team: s.team_id,
            own: s.device_id,
            peer: self.scope(to).device_id,
            revision: s.revision,
            head: s.membership_head,
        }
    }
    fn post(&self, v: &Vault, text: &str) -> String {
        v.post_talk(&self.scope(v), &random_id().unwrap(), text)
            .unwrap()
    }
    fn delete(&self, v: &Vault, id: &str) -> String {
        let feed = v.talk_feed(&self.team, 0).unwrap();
        let row = feed.messages.iter().find(|r| r.event_id == id).unwrap();
        v.delete_talk(&feed.scope, &random_id().unwrap(), id, row.sequence)
            .unwrap()
    }
    fn send(&self, from: &Vault, to: &Vault) {
        let a = self.wire(from, to);
        let b = self.wire(to, from);
        let nonce = random_id().unwrap();
        let frontier = to.receive_frontier(&b, &nonce).unwrap();
        let mut cursor = from.begin_push(&a, &frontier, &nonce).unwrap();
        for n in 0..2048 {
            let Some(delivery) = from.next_delivery(&a, &cursor).unwrap() else {
                return;
            };
            let receipt = to.store_delivery(&b, &delivery, &nonce).unwrap();
            from.accept_receipt(&a, &delivery, &receipt, &nonce, &mut cursor)
                .unwrap();
            assert!(n < 2047);
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
            .starts_with("sai-talk-"));
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn signed_author_and_receipt_status_do_not_depend_on_a_legacy_uid_or_server() {
    let t = Team::new();
    let id = t.post(&t.a, "PC 톡 원문");
    let pending = t.a.talk_feed(&t.team, 0).unwrap();
    assert_eq!(pending.messages[0].state, "local-pending");
    t.send(&t.a, &t.b);
    let incoming = t.b.talk_feed(&t.team, 0).unwrap();
    let m = &incoming.messages[0];
    assert_eq!(m.event_id, id);
    assert_eq!(m.author, pending.scope.device_id);
    assert!(m.incoming);
    assert_eq!(m.state, "received");
    assert!(!m.can_delete);
    assert!(!m.prepared);
    assert_eq!(
        m.member_id,
        t.a.export(&t.team).unwrap().state().unwrap().devices[&m.author].member_id
    );
    assert_eq!(
        t.a.talk_feed(&t.team, 0).unwrap().messages[0].state,
        "peer-stored"
    );
}
#[test]
fn retries_keep_the_original_post_time_id_and_delete_request() {
    let t = Team::new();
    let scope = t.scope(&t.a);
    let request = random_id().unwrap();
    let first =
        t.a.append_action(
            &t.team,
            &request,
            "같은 톡",
            Some(Action::Post { created_at: 1 }),
            Some(&scope),
        )
        .unwrap();
    assert_eq!(first, t.a.post_talk(&scope, &request, "같은 톡").unwrap());
    assert_eq!(
        t.a.talk_feed(&t.team, 0).unwrap().messages[0].created_at,
        Some(1)
    );
    assert!(t.a.post_talk(&scope, &request, "다른 톡").is_err());
    let deletion = random_id().unwrap();
    let id = t.a.delete_talk(&scope, &deletion, &first, 1).unwrap();
    assert_eq!(t.a.delete_talk(&scope, &deletion, &first, 1).unwrap(), id);
    assert_eq!(t.a.local_status(&t.team).unwrap().stored, 2);
}
#[test]
fn stale_audience_scope_never_silently_adds_new_recipients() {
    let t = Team::new();
    let scope = t.scope(&t.a);
    t.enroll(&t.c);
    assert!(t
        .a
        .post_talk(&scope, &random_id().unwrap(), "새 수신자 동의 없음")
        .is_err());
    assert_eq!(t.a.local_status(&t.team).unwrap().stored, 0);
    let mut wrong = t.scope(&t.a);
    wrong.device_id = t.scope(&t.b).device_id;
    assert!(t
        .a
        .post_talk(&wrong, &random_id().unwrap(), "다른 장치")
        .is_err());
}
#[test]
fn only_the_writing_device_can_delete_a_pc_post_and_preparation_text_is_kept() {
    let t = Team::new();
    let id = t.post(&t.a, "작성자만 삭제");
    t.send(&t.a, &t.b);
    assert!(t
        .b
        .delete_talk(&t.scope(&t.b), &random_id().unwrap(), &id, 1)
        .is_err());
    let prepared =
        t.a.append_local(&t.team, &random_id().unwrap(), "기존 준비 기록")
            .unwrap();
    assert!(t
        .a
        .delete_talk(&t.scope(&t.a), &random_id().unwrap(), &prepared, 2)
        .is_err());
    assert!(t
        .a
        .delete_talk(&t.scope(&t.a), &random_id().unwrap(), &id, 2)
        .is_err());
    assert_eq!(t.a.local_status(&t.team).unwrap().stored, 2);
}
#[test]
fn delayed_delete_and_original_replay_do_not_restore_the_body_on_either_screen() {
    let t = Team::new();
    let id = t.post(&t.a, "삭제할 본문-unique-body");
    let a = t.wire(&t.a, &t.b);
    let b = t.wire(&t.b, &t.a);
    let nonce = random_id().unwrap();
    let frontier = t.b.receive_frontier(&b, &nonce).unwrap();
    let cursor = t.a.begin_push(&a, &frontier, &nonce).unwrap();
    let original = t.a.next_delivery(&a, &cursor).unwrap().unwrap();
    t.send(&t.a, &t.b);
    t.delete(&t.a, &id);
    assert!(t.a.talk_feed(&t.team, 0).unwrap().messages[0].deleted);
    assert!(!t.b.talk_feed(&t.team, 0).unwrap().messages[0].deleted);
    t.send(&t.a, &t.b);
    t.b.store_delivery(&b, &original, &nonce).unwrap();
    for v in [&t.a, &t.b] {
        let f = v.talk_feed(&t.team, 0).unwrap();
        assert_eq!(f.messages.len(), 1);
        assert!(f.messages[0].deleted);
        assert!(f.messages[0].text.is_empty());
        assert!(!f.messages[0].can_delete);
        assert!(!v.local_status(&t.team).unwrap().messages[0]
            .text
            .contains("unique-body"));
    }
    assert_eq!(
        t.a.required()
            .unwrap()
            .query_row("SELECT count(*) FROM mesh_local_events", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        2
    );
}
#[test]
fn header_only_delete_survives_membership_change_without_redistributing_old_text() {
    let t = Team::new();
    let id = t.post(&t.a, "과거 수신자만 받은 본문");
    t.send(&t.a, &t.b);
    t.delete(&t.a, &id);
    t.enroll(&t.c);
    t.send(&t.a, &t.b);
    let b = t.b.talk_feed(&t.team, 0).unwrap();
    assert!(b.messages[0].deleted);
    assert!(b.messages[0].text.is_empty());
    assert_eq!(
        t.a.talk_feed(&t.team, 0).unwrap().messages[0].delete_state,
        Some("reference-stored")
    );
    t.send(&t.a, &t.c);
    assert!(t.c.talk_feed(&t.team, 0).unwrap().messages.is_empty());
    let c = t.c.required().unwrap();
    assert_eq!(
        c.query_row(
            "SELECT count(*) FROM mesh_received WHERE payload IS NOT NULL",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
}
#[test]
fn deleting_an_old_epoch_post_sends_only_the_signed_reference_to_new_members() {
    let t = Team::new();
    let id = t.post(&t.a, "공유 범위가 바뀌기 전 톡");
    t.send(&t.a, &t.b);
    t.enroll(&t.c);
    t.delete(&t.a, &id);
    t.send(&t.a, &t.b);
    t.send(&t.a, &t.c);
    assert!(t.b.talk_feed(&t.team, 0).unwrap().messages[0].deleted);
    assert!(t.c.talk_feed(&t.team, 0).unwrap().messages.is_empty());
}
#[test]
fn deletion_insert_failure_rolls_back_without_hiding_the_original() {
    let t = Team::new();
    let id = t.post(&t.a, "실패하면 유지");
    let conn = t.a.required().unwrap();
    conn.execute_batch("CREATE TRIGGER fail_delete BEFORE INSERT ON mesh_local_events BEGIN SELECT RAISE(ABORT,'test');END;").unwrap();
    let request = random_id().unwrap();
    assert!(t.a.delete_talk(&t.scope(&t.a), &request, &id, 1).is_err());
    assert!(!t.a.talk_feed(&t.team, 0).unwrap().messages[0].deleted);
    assert_eq!(t.a.local_status(&t.team).unwrap().stored, 1);
    conn.execute_batch("DROP TRIGGER fail_delete;").unwrap();
    t.a.delete_talk(&t.scope(&t.a), &request, &id, 1).unwrap();
    assert!(t.a.talk_feed(&t.team, 0).unwrap().messages[0].deleted);
}
#[test]
fn pagination_applies_all_tombstones_before_selecting_the_visible_window() {
    let t = Team::new();
    let first = t.post(&t.a, "오래된 첫 톡");
    t.delete(&t.a, &first);
    for n in 0..52 {
        t.post(&t.a, &format!("새 톡 {n}"));
    }
    let latest = t.a.talk_feed(&t.team, 0).unwrap();
    assert_eq!(latest.messages.len(), 50);
    assert_eq!(latest.next_offset, Some(50));
    let older = t.a.talk_feed(&t.team, 50).unwrap();
    assert_eq!(older.messages.len(), 3);
    assert!(older.messages[0].deleted);
    assert_eq!(older.total, 53);
    assert!(t.a.talk_feed(&t.team, 2049).is_err());
}
#[test]
fn revoked_device_can_read_its_local_history_but_cannot_post_or_delete() {
    let t = Team::new();
    let id = t.post(&t.b, "기존 본문");
    let s = t.scope(&t.b);
    let packet = t.a.revoke(&t.team, &s.device_id).unwrap();
    t.b.import(&serde_json::to_string(&packet).unwrap(), "")
        .unwrap();
    let feed = t.b.talk_feed(&t.team, 0).unwrap();
    assert!(!feed.can_write);
    assert!(!feed.messages[0].can_delete);
    assert!(t
        .b
        .post_talk(&feed.scope, &random_id().unwrap(), "강퇴 뒤 쓰기")
        .is_err());
    assert!(t
        .b
        .delete_talk(&feed.scope, &random_id().unwrap(), &id, 1)
        .is_err());
}
#[test]
fn typed_metadata_is_committed_and_v1_bytes_still_have_the_original_shape() {
    let t = Team::new();
    t.a.append_local(&t.team, &random_id().unwrap(), "v1 원문")
        .unwrap();
    let id = t.post(&t.a, "v2 원문");
    let conn = t.a.required().unwrap();
    let p = t.a.export(&t.team).unwrap();
    let own = t.scope(&t.a).device_id;
    let records = journal::read_chain(&conn, &p, &own).unwrap();
    let old = journal::read_body_data(&records[0]).unwrap();
    assert!(!serde_json::to_string(&old).unwrap().contains("action"));
    assert!(!serde_json::to_string(&records[0].event)
        .unwrap()
        .contains("retract"));
    let mut body = journal::read_body_data(&records[1]).unwrap();
    body.action = Some(Action::Post { created_at: 42 });
    assert!(body.validate(&records[1].event).is_err());
    t.delete(&t.a, &id);
    let records = journal::read_chain(&conn, &p, &own).unwrap();
    let mut event = records[2].event.clone();
    event.header.retract.as_mut().unwrap().sequence = 2;
    event.header.retract.as_mut().unwrap().target = "00".repeat(32);
    assert!(event.validate(&p).is_err());
}

#[test]
fn a_valid_signature_from_another_author_cannot_hide_someone_elses_post() {
    use super::super::{
        keys::{digest, signed_bytes},
        transfer::make_delivery,
    };
    let t = Team::new();
    let target = t.post(&t.a, "A의 원문");
    t.send(&t.a, &t.b);
    t.post(&t.b, "B의 원문");
    t.send(&t.b, &t.a);
    let own = t.scope(&t.b).device_id;
    let conn = t.b.required().unwrap();
    let packet = t.b.export(&t.team).unwrap();
    let key = t.b.key(&conn, "device").unwrap().unwrap();
    let records = journal::read_chain(&conn, &packet, &own).unwrap();
    let mut event = records[0].event.clone();
    event.header.sequence = 2;
    event.header.parent = records[0].id.clone();
    event.header.request = random_id().unwrap();
    event.header.retract = Some(Retraction {
        target: target.clone(),
        sequence: 1,
    });
    let action = Some(Action::Delete {
        target,
        sequence: 1,
    });
    let salt = random_id().unwrap();
    event.header.content_hash = digest(
        "sai-mesh/talk-content/2",
        &signed_bytes("sai-mesh/talk-body/2", &(salt.as_str(), "", &action)).unwrap(),
    );
    event.signature = key.sign(journal::DOMAIN, &event.header).unwrap();
    let id = event.id().unwrap();
    let body = Body {
        version: 2,
        event_id: id.clone(),
        salt,
        text: String::new(),
        action,
    };
    let stored = Stored {
        event,
        id,
        sealed: crate::secret::protect_device_secret(&serde_json::to_vec(&body).unwrap()).unwrap(),
    };
    let delivery = make_delivery(&key, &packet, &stored, &t.scope(&t.a).device_id, true).unwrap();
    t.a.store_delivery(&t.wire(&t.a, &t.b), &delivery, &random_id().unwrap())
        .unwrap();
    let feed = t.a.talk_feed(&t.team, 0).unwrap();
    assert_eq!(feed.messages.len(), 2);
    assert!(feed.messages.iter().all(|r| !r.deleted));
}
