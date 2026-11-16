use super::keys::{digest, Material};
use super::model::*;

fn fixture() -> (Material, Material, ApprovalPacket) {
    let authority = Material::generate(false).unwrap();
    let device = Material::generate(true).unwrap();
    let manifest = TeamManifest::create(&authority, "시험 PC 팀").unwrap();
    (
        authority,
        device,
        ApprovalPacket {
            version: 1,
            manifest,
            events: Vec::new(),
        },
    )
}

fn approve(
    packet: &mut ApprovalPacket,
    authority: &Material,
    device: &Material,
) -> EnrollmentRequest {
    let request = EnrollmentRequest::create(device, &packet.manifest).unwrap();
    packet
        .append(
            authority,
            MembershipAction::Approve {
                request: request.clone(),
                member_id: super::keys::random_id().unwrap(),
            },
        )
        .unwrap();
    request
}

#[test]
fn requests_bind_both_public_keys_and_the_exact_team() {
    let (authority, device, packet) = fixture();
    let request = EnrollmentRequest::create(&device, &packet.manifest).unwrap();
    request.validate(&packet.manifest).unwrap();
    let other = TeamManifest::create(&authority, "다른 팀").unwrap();
    assert!(request.validate(&other).is_err());
    let replacement = Material::generate(true).unwrap().device().unwrap();
    let mut tampered = request.clone();
    tampered.body.device = replacement;
    assert!(tampered.validate(&packet.manifest).is_err());
    let mut tampered = request;
    tampered.body.nonce = super::keys::random_id().unwrap();
    assert!(tampered.validate(&packet.manifest).is_err());
}

#[test]
fn device_keys_and_another_authority_cannot_issue_approvals() {
    let (_authority, device, mut packet) = fixture();
    let request = EnrollmentRequest::create(&device, &packet.manifest).unwrap();
    let action = MembershipAction::Approve {
        request,
        member_id: super::keys::random_id().unwrap(),
    };
    assert!(packet.append(&device, action.clone()).is_err());
    assert!(packet
        .append(&Material::generate(false).unwrap(), action)
        .is_err());
    assert_eq!(packet.state().unwrap().revision, 0);
}

#[test]
fn sequence_parent_and_signature_changes_are_rejected() {
    let (authority, device, mut packet) = fixture();
    approve(&mut packet, &authority, &device);
    let id = device.device().unwrap().device_id;
    packet
        .append(&authority, MembershipAction::Revoke { device_id: id })
        .unwrap();
    for field in ["revision", "parent", "signature"] {
        let mut bad = packet.clone();
        match field {
            "revision" => bad.events[1].body.revision += 1,
            "parent" => bad.events[1].body.parent = "00".repeat(32),
            _ => bad.events[1].signature = "00".repeat(64),
        }
        assert!(bad.state().is_err(), "{field}");
    }
    let mut reversed = packet;
    reversed.events.reverse();
    assert!(reversed.state().is_err());
}

#[test]
fn a_revoked_request_cannot_be_replayed_as_a_new_approval() {
    let (authority, device, mut packet) = fixture();
    let request = approve(&mut packet, &authority, &device);
    let id = device.device().unwrap().device_id;
    packet
        .append(
            &authority,
            MembershipAction::Revoke {
                device_id: id.clone(),
            },
        )
        .unwrap();
    assert!(!packet.state().unwrap().devices[&id].active);
    assert!(packet
        .append(
            &authority,
            MembershipAction::Approve {
                request,
                member_id: super::keys::random_id().unwrap()
            }
        )
        .is_err());
}

#[test]
fn weak_encryption_keys_unknown_fields_and_large_packets_are_rejected() {
    let (_authority, device, packet) = fixture();
    let mut public = device.device().unwrap();
    public.encryption_key = "00".repeat(32);
    public.device_id = digest(
        "sai-mesh/device-id/1",
        format!("{}:{}", public.signing_key, public.encryption_key).as_bytes(),
    );
    assert!(public.validate().is_err());
    let request = EnrollmentRequest::create(&device, &packet.manifest).unwrap();
    let mut value = serde_json::to_value(&request).unwrap();
    value["body"]["admin"] = serde_json::json!(true);
    assert!(EnrollmentRequest::parse(&value.to_string()).is_err());
    assert!(ApprovalPacket::parse(&"x".repeat(MAX_PACKET + 1)).is_err());
    let bytes = serde_json::to_string(&packet).unwrap();
    assert!(
        ApprovalPacket::parse(&bytes.replace("\"version\":1", "\"version\":1,\"version\":1"))
            .is_err()
    );
}

#[cfg(windows)]
mod windows {
    use super::super::vault::{Protector, Vault};
    use super::*;
    use rusqlite::{params, Connection};
    use std::path::PathBuf;
    use std::sync::Arc;

    struct TestDir(PathBuf);
    impl TestDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "sai-identity-{}",
                super::super::keys::random_id().unwrap()
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn vault(&self, name: &str) -> Vault {
            Vault::new(self.0.join(format!("{name}.sqlite3")))
        }
    }
    impl Drop for TestDir {
        fn drop(&mut self) {
            assert_eq!(self.0.parent(), Some(std::env::temp_dir().as_path()));
            assert!(self
                .0
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("sai-identity-"));
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn text<T: serde::Serialize>(value: &T) -> String {
        serde_json::to_string(value).unwrap()
    }

    #[test]
    fn status_is_read_only_and_device_keys_survive_reopening() {
        let dir = TestDir::new();
        let vault = dir.vault("a");
        assert!(vault.status().unwrap().device.is_none());
        assert!(!dir.0.join("a.sqlite3").exists());
        let first = vault.create_device().unwrap();
        assert_eq!(first, vault.create_device().unwrap());
        drop(vault);
        assert_eq!(dir.vault("a").status().unwrap().device.unwrap(), first);
        assert_eq!(dir.vault("a").create_device().unwrap(), first);
    }

    #[test]
    fn offline_enrollment_revocation_and_stale_import_preserve_authority() {
        let dir = TestDir::new();
        let admin = dir.vault("admin");
        let member = dir.vault("member");
        admin.create_device().unwrap();
        let device = member.create_device().unwrap();
        let initial = admin.create_team("우리 PC 팀").unwrap();
        let id = &initial.manifest.body.team_id;
        assert!(member.import(&text(&initial), "").is_err());
        assert!(member.import(&text(&initial), &"00".repeat(32)).is_err());
        assert!(member.status().unwrap().teams.is_empty());
        member
            .import(&text(&initial), &initial.manifest.fingerprint().unwrap())
            .unwrap();
        let request = member.request(id).unwrap();
        assert!(member.approve(id, &text(&request)).is_err());
        let approved = admin.approve(id, &text(&request)).unwrap();
        assert_eq!(approved.events.len(), 2);
        assert_eq!(admin.approve(id, &text(&request)).unwrap(), approved);
        member.import(&text(&approved), "").unwrap();
        assert!(!member.status().unwrap().teams[0].admin_here);
        assert!(admin.status().unwrap().teams[0].admin_here);
        assert!(approved.state().unwrap().devices[&device.device_id].active);
        let revoked = admin.revoke(id, &device.device_id).unwrap();
        member.import(&text(&revoked), "").unwrap();
        member.import(&text(&approved), "").unwrap();
        let after = member.export(id).unwrap();
        assert_eq!(after.events.len(), 3);
        assert!(!after.state().unwrap().devices[&device.device_id].active);
        assert!(admin.approve(id, &text(&request)).is_err());
        let new_request = member.request(id).unwrap();
        let readmitted = admin.approve(id, &text(&new_request)).unwrap();
        assert_eq!(readmitted.events.len(), 4);
        assert!(readmitted.state().unwrap().devices[&device.device_id].active);
    }

    #[test]
    fn another_local_admin_key_does_not_grant_foreign_team_permissions() {
        let dir = TestDir::new();
        let first = dir.vault("first");
        let second = dir.vault("second");
        first.create_device().unwrap();
        second.create_device().unwrap();
        let team = first.create_team("A").unwrap();
        second.create_team("B").unwrap();
        assert!(second
            .import(&text(&team), &team.manifest.fingerprint().unwrap())
            .is_err());
        let id = &team.manifest.body.team_id;
        assert!(second
            .approve(id, &text(&first.request(id).unwrap()))
            .is_err());
        assert!(second
            .revoke(id, &first.status().unwrap().device.unwrap().device_id)
            .is_err());
        assert!(second
            .status()
            .unwrap()
            .teams
            .iter()
            .all(|t| &t.team_id != id));
    }

    #[test]
    fn same_revision_forks_are_rejected_without_changing_known_history() {
        let dir = TestDir::new();
        let receiver = dir.vault("receiver");
        let (authority, first, mut original) = fixture();
        let second = Material::generate(true).unwrap();
        approve(&mut original, &authority, &first);
        receiver
            .import(&text(&original), &original.manifest.fingerprint().unwrap())
            .unwrap();
        let mut fork = ApprovalPacket {
            version: 1,
            manifest: original.manifest.clone(),
            events: Vec::new(),
        };
        approve(&mut fork, &authority, &second);
        assert!(receiver.import(&text(&fork), "").is_err());
        assert_eq!(
            receiver.export(&original.manifest.body.team_id).unwrap(),
            original
        );
    }

    #[test]
    fn failed_import_rolls_back_earlier_events_in_the_same_packet() {
        let dir = TestDir::new();
        let receiver = dir.vault("receiver");
        let (authority, first, mut packet) = fixture();
        approve(&mut packet, &authority, &first);
        receiver
            .import(&text(&packet), &packet.manifest.fingerprint().unwrap())
            .unwrap();
        let initial = packet.clone();
        approve(&mut packet, &authority, &Material::generate(true).unwrap());
        approve(&mut packet, &authority, &Material::generate(true).unwrap());
        let conn = Connection::open(dir.0.join("receiver.sqlite3")).unwrap();
        conn.execute_batch("CREATE TRIGGER deny_third BEFORE INSERT ON mesh_membership WHEN NEW.revision=3 BEGIN SELECT RAISE(ABORT,'test failure'); END;").unwrap();
        assert!(receiver.import(&text(&packet), "").is_err());
        assert_eq!(
            receiver.export(&initial.manifest.body.team_id).unwrap(),
            initial
        );
    }

    struct Refuse;
    impl Protector for Refuse {
        fn seal(&self, _: &[u8]) -> crate::Result<Vec<u8>> {
            Err("test protection refused".into())
        }
        fn open(&self, _: &[u8]) -> crate::Result<Vec<u8>> {
            Err("test opening refused".into())
        }
    }

    #[test]
    fn protection_failure_never_creates_a_plaintext_key() {
        let dir = TestDir::new();
        let vault = Vault::with_protector(dir.0.join("failed.sqlite3"), Arc::new(Refuse));
        assert!(vault.create_device().is_err());
        let conn = Connection::open(dir.0.join("failed.sqlite3")).unwrap();
        assert_eq!(
            conn.query_row("SELECT count(*) FROM mesh_keys", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row("SELECT count(*) FROM mesh_key_checkpoints", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    #[test]
    fn missing_private_keys_are_not_silently_regenerated() {
        let dir = TestDir::new();
        let vault = dir.vault("vault");
        vault.create_device().unwrap();
        let team = vault.create_team("A").unwrap();
        let conn = Connection::open(dir.0.join("vault.sqlite3")).unwrap();
        for slot in ["device", "authority"] {
            let sealed: Vec<u8> = conn
                .query_row("SELECT sealed FROM mesh_keys WHERE slot=?1", [slot], |r| {
                    r.get(0)
                })
                .unwrap();
            conn.execute("DELETE FROM mesh_keys WHERE slot=?1", [slot])
                .unwrap();
            assert!(vault.status().is_err());
            if slot == "device" {
                assert!(vault.create_device().is_err());
            } else {
                assert!(vault.create_team("B").is_err());
            }
            assert_eq!(
                conn.query_row(
                    "SELECT count(*) FROM mesh_keys WHERE slot=?1",
                    [slot],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
                0
            );
            conn.execute(
                "INSERT INTO mesh_keys(slot,sealed) VALUES(?1,?2)",
                params![slot, sealed],
            )
            .unwrap();
        }
        assert_eq!(vault.export(&team.manifest.body.team_id).unwrap(), team);
    }

    #[test]
    fn a_member_cannot_create_or_replace_the_trusted_operating_administrator() {
        let dir = TestDir::new();
        let admin = dir.vault("admin");
        admin.create_device().unwrap();
        let first = admin.create_team("A").unwrap();
        let second = admin.create_team("B").unwrap();
        assert_eq!(
            first.manifest.body.authority_key,
            second.manifest.body.authority_key
        );
        assert_eq!(
            first.manifest.fingerprint().unwrap(),
            second.manifest.fingerprint().unwrap()
        );
        let member = dir.vault("member");
        member.create_device().unwrap();
        member
            .import(&text(&first), &first.manifest.fingerprint().unwrap())
            .unwrap();
        assert!(member.create_team("임의 관리자").is_err());
        let conn = Connection::open(dir.0.join("member.sqlite3")).unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT count(*) FROM mesh_keys WHERE slot='authority'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row(
                "SELECT count(*) FROM mesh_key_checkpoints WHERE slot='authority'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        member
            .import(&text(&second), &second.manifest.fingerprint().unwrap())
            .unwrap();
        assert_eq!(member.status().unwrap().teams.len(), 2);
        assert!(member
            .status()
            .unwrap()
            .teams
            .iter()
            .all(|team| !team.admin_here));
    }

    #[test]
    fn a_failed_public_key_checkpoint_rolls_back_the_private_key() {
        let dir = TestDir::new();
        let vault = dir.vault("vault");
        // Initialize the schema without generating a key.
        let (_, _, packet) = fixture();
        vault
            .import(&text(&packet), &packet.manifest.fingerprint().unwrap())
            .unwrap();
        let conn = Connection::open(dir.0.join("vault.sqlite3")).unwrap();
        conn.execute_batch("CREATE TRIGGER deny_checkpoint BEFORE INSERT ON mesh_key_checkpoints BEGIN SELECT RAISE(ABORT,'test failure'); END;").unwrap();
        assert!(vault.create_device().is_err());
        assert_eq!(
            conn.query_row("SELECT count(*) FROM mesh_keys", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert!(vault.status().unwrap().device.is_none());
    }

    #[test]
    fn key_ciphertext_is_bound_to_its_slot_and_corruption_is_not_replaced() {
        let dir = TestDir::new();
        let vault = dir.vault("vault");
        vault.create_device().unwrap();
        vault.create_team("A").unwrap();
        let conn = Connection::open(dir.0.join("vault.sqlite3")).unwrap();
        let authority: Vec<u8> = conn
            .query_row(
                "SELECT sealed FROM mesh_keys WHERE slot='authority'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        conn.execute(
            "UPDATE mesh_keys SET sealed=?1 WHERE slot='device'",
            params![authority],
        )
        .unwrap();
        assert!(vault.create_device().is_err());
        conn.execute(
            "UPDATE mesh_keys SET sealed=?1 WHERE slot='device'",
            [b"raw:do-not-accept".as_slice()],
        )
        .unwrap();
        assert!(vault.create_device().is_err());
        assert_eq!(
            conn.query_row(
                "SELECT sealed FROM mesh_keys WHERE slot='device'",
                [],
                |r| r.get::<_, Vec<u8>>(0)
            )
            .unwrap(),
            b"raw:do-not-accept"
        );
    }

    #[test]
    fn dpapi_blob_is_encrypted_and_cannot_be_opened_when_tampered() {
        let plain = b"private-device-key-test-value";
        let mut sealed = crate::secret::protect_device_secret(plain).unwrap();
        assert!(!sealed.windows(plain.len()).any(|w| w == plain));
        assert_eq!(
            crate::secret::unprotect_device_secret(&sealed).unwrap(),
            plain
        );
        let last = sealed.len() - 1;
        sealed[last] ^= 0x80;
        assert!(crate::secret::unprotect_device_secret(&sealed).is_err());
    }

    #[test]
    fn concurrent_approvals_have_distinct_revisions_and_survive_reopening() {
        let dir = TestDir::new();
        let admin = dir.vault("admin");
        admin.create_device().unwrap();
        let initial = admin.create_team("A").unwrap();
        let team = initial.manifest.body.team_id.clone();
        let requests: Vec<_> = (0..2)
            .map(|_| {
                text(
                    &EnrollmentRequest::create(
                        &Material::generate(true).unwrap(),
                        &initial.manifest,
                    )
                    .unwrap(),
                )
            })
            .collect();
        let gate = Arc::new(std::sync::Barrier::new(2));
        let jobs: Vec<_> = requests
            .into_iter()
            .map(|request| {
                let vault = dir.vault("admin");
                let gate = gate.clone();
                let team = team.clone();
                std::thread::spawn(move || {
                    gate.wait();
                    vault.approve(&team, &request).unwrap();
                })
            })
            .collect();
        for job in jobs {
            job.join().unwrap();
        }
        let stored = dir.vault("admin").export(&team).unwrap();
        assert_eq!(stored.events.len(), 3);
        assert_eq!(stored.state().unwrap().devices.len(), 3);
    }

    #[test]
    fn malformed_database_is_preserved_and_public_exports_contain_no_secrets() {
        let dir = TestDir::new();
        let file = dir.0.join("bad.sqlite3");
        std::fs::write(&file, b"this is not a database").unwrap();
        assert!(Vault::new(&file).create_device().is_err());
        assert_eq!(std::fs::read(&file).unwrap(), b"this is not a database");
        let vault = dir.vault("valid");
        vault.create_device().unwrap();
        let packet = vault.create_team("A").unwrap();
        let public = text(&packet) + &text(&vault.status().unwrap());
        for private_marker in [
            "pkcs8",
            "\"material\"",
            "\"signing\":",
            "\"sealed\"",
            "PRIVATE KEY",
            "refreshToken",
        ] {
            assert!(!public.contains(private_marker));
        }
        let db = Connection::open(dir.0.join("valid.sqlite3")).unwrap();
        let sealed: Vec<u8> = db
            .query_row(
                "SELECT sealed FROM mesh_keys WHERE slot='device'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let plain =
            crate::secret::unprotect_device_secret(&sealed[b"sai-mesh/dpapi/1\0".len()..]).unwrap();
        for suffix in ["", "-wal"] {
            let path = dir.0.join(format!("valid.sqlite3{suffix}"));
            if let Ok(file) = std::fs::read(path) {
                assert!(!file.windows(plain.len()).any(|window| window == plain));
            }
        }
    }

    fn local_team(dir: &TestDir) -> (Vault, String) {
        let vault = dir.vault("local");
        vault.create_device().unwrap();
        let team = vault
            .create_team("로컬 톡 시험")
            .unwrap()
            .manifest
            .body
            .team_id;
        (vault, team)
    }

    #[test]
    fn local_messages_and_outbox_survive_restart_and_retries_do_not_duplicate() {
        let dir = TestDir::new();
        let (vault, team) = local_team(&dir);
        let request = Vault::local_draft_id().unwrap();
        let id = vault
            .append_local(&team, &request, "첫 줄\n둘째 줄")
            .unwrap();
        assert_eq!(
            vault
                .append_local(&team, &request, "첫 줄\n둘째 줄")
                .unwrap(),
            id
        );
        assert!(vault.append_local(&team, &request, "다른 내용").is_err());
        vault
            .append_local(&team, &Vault::local_draft_id().unwrap(), "다음 톡")
            .unwrap();
        drop(vault);
        let status = dir.vault("local").local_status(&team).unwrap();
        assert_eq!((status.stored, status.pending), (2, 0));
        assert!(status.can_write);
        assert_eq!(status.messages[0].event_id, id);
        assert_eq!(status.messages[0].text, "첫 줄\n둘째 줄");
        assert_eq!(status.messages[1].sequence, 2);
        assert_eq!(status.messages[1].state, "local-only");
    }

    #[test]
    fn outbox_failure_rolls_back_event_and_sequence_then_retry_uses_sequence_one() {
        let dir = TestDir::new();
        let (vault, team) = local_team(&dir);
        let conn = Connection::open(dir.0.join("local.sqlite3")).unwrap();
        conn.execute_batch("CREATE TRIGGER deny_queue BEFORE INSERT ON mesh_local_outbox BEGIN SELECT RAISE(ABORT,'test failure'); END;").unwrap();
        let request = Vault::local_draft_id().unwrap();
        assert!(vault.append_local(&team, &request, "저장 실패").is_err());
        for table in ["mesh_local_events", "mesh_local_heads", "mesh_local_outbox"] {
            assert_eq!(
                conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r
                    .get::<_, i64>(0))
                    .unwrap(),
                0
            );
        }
        conn.execute_batch("DROP TRIGGER deny_queue;").unwrap();
        vault.append_local(&team, &request, "저장 실패").unwrap();
        assert_eq!(vault.local_status(&team).unwrap().messages[0].sequence, 1);
    }

    #[test]
    fn local_body_is_protected_and_swapping_or_changing_it_is_detected() {
        let dir = TestDir::new();
        let (vault, team) = local_team(&dir);
        let plain = "private-local-message-unique-value";
        vault
            .append_local(&team, &Vault::local_draft_id().unwrap(), plain)
            .unwrap();
        vault
            .append_local(&team, &Vault::local_draft_id().unwrap(), "다른 톡")
            .unwrap();
        let conn = Connection::open(dir.0.join("local.sqlite3")).unwrap();
        let encoded: String = conn
            .query_row(
                "SELECT event FROM mesh_local_events WHERE sequence=1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let sealed: Vec<u8> = conn
            .query_row(
                "SELECT sealed FROM mesh_local_events WHERE sequence=1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(!encoded.contains(plain));
        assert!(!sealed.windows(plain.len()).any(|w| w == plain.as_bytes()));
        for suffix in ["", "-wal"] {
            if let Ok(bytes) = std::fs::read(dir.0.join(format!("local.sqlite3{suffix}"))) {
                assert!(!bytes.windows(plain.len()).any(|w| w == plain.as_bytes()));
            }
        }
        conn.execute(
            "UPDATE mesh_local_events SET sealed=?1 WHERE sequence=2",
            [sealed],
        )
        .unwrap();
        assert!(vault.local_status(&team).is_err());
        let mut event: serde_json::Value = serde_json::from_str(&encoded).unwrap();
        event["signature"] = serde_json::json!("00".repeat(64));
        conn.execute(
            "UPDATE mesh_local_events SET event=?1 WHERE sequence=1",
            [event.to_string()],
        )
        .unwrap();
        assert!(vault.local_status(&team).is_err());
    }

    #[test]
    fn missing_tail_or_outbox_cannot_reset_the_device_sequence() {
        let dir = TestDir::new();
        let (vault, team) = local_team(&dir);
        vault
            .append_local(&team, &Vault::local_draft_id().unwrap(), "한 개")
            .unwrap();
        let conn = Connection::open(dir.0.join("local.sqlite3")).unwrap();
        conn.execute("DELETE FROM mesh_local_outbox", []).unwrap();
        assert!(vault.local_status(&team).is_err());
        assert!(vault
            .append_local(&team, &Vault::local_draft_id().unwrap(), "추가")
            .is_err());
        conn.execute("DELETE FROM mesh_local_send_policy", [])
            .unwrap();
        conn.execute("DELETE FROM mesh_local_events", []).unwrap();
        assert!(vault
            .append_local(&team, &Vault::local_draft_id().unwrap(), "순번 재사용 시도")
            .is_err());
        assert_eq!(
            conn.query_row("SELECT sequence FROM mesh_local_heads", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }

    #[test]
    fn revoked_local_device_can_read_history_but_cannot_write_or_start_sessions() {
        let dir = TestDir::new();
        let (vault, team) = local_team(&dir);
        vault
            .append_local(&team, &Vault::local_draft_id().unwrap(), "과거 원본")
            .unwrap();
        let device = vault.status().unwrap().device.unwrap();
        vault.revoke(&team, &device.device_id).unwrap();
        let status = vault.local_status(&team).unwrap();
        assert!(!status.can_write);
        assert_eq!(status.messages[0].text, "과거 원본");
        assert!(vault
            .append_local(&team, &Vault::local_draft_id().unwrap(), "추가")
            .is_err());
        assert!(vault.prove_session(&team, "00".repeat(32)).is_err());
    }

    #[test]
    fn concurrent_local_appends_are_serialized_without_losing_an_outbox_item() {
        let dir = TestDir::new();
        let (vault, team) = local_team(&dir);
        let gate = Arc::new(std::sync::Barrier::new(2));
        let jobs: Vec<_> = (0..2)
            .map(|n| {
                let vault = vault.clone();
                let team = team.clone();
                let gate = gate.clone();
                std::thread::spawn(move || {
                    gate.wait();
                    vault
                        .append_local(
                            &team,
                            &Vault::local_draft_id().unwrap(),
                            &format!("동시 {n}"),
                        )
                        .unwrap()
                })
            })
            .collect();
        let ids: Vec<_> = jobs.into_iter().map(|job| job.join().unwrap()).collect();
        assert_ne!(ids[0], ids[1]);
        let status = vault.local_status(&team).unwrap();
        assert_eq!((status.stored, status.pending), (2, 0));
        assert_eq!(
            status
                .messages
                .iter()
                .map(|m| m.sequence)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
    }

    #[test]
    fn v1_upgrade_preserves_device_keys_and_authority_without_creating_new_identity() {
        let dir = TestDir::new();
        let (vault, team) = local_team(&dir);
        let device = vault.status().unwrap().device.unwrap();
        let packet = vault.export(&team).unwrap();
        let conn = Connection::open(dir.0.join("local.sqlite3")).unwrap();
        conn.execute_batch("DROP TABLE mesh_legacy_archive; DROP TABLE mesh_runtime_profile; DROP TABLE mesh_courier; DROP TABLE mesh_received; DROP TABLE mesh_receive_heads; DROP TABLE mesh_sent_receipts; DROP TABLE mesh_transfer_blocks; DROP TABLE mesh_local_send_policy; DROP TABLE mesh_local_outbox; DROP TABLE mesh_local_heads; DROP TABLE mesh_local_events; PRAGMA user_version=1;").unwrap();
        assert_eq!(vault.status().unwrap().device.unwrap(), device);
        assert_eq!(vault.export(&team).unwrap(), packet);
        assert_eq!(vault.local_status(&team).unwrap().stored, 0);
        assert_eq!(
            conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                .unwrap(),
            6
        );
    }

    #[test]
    fn malformed_inputs_and_unapproved_devices_never_create_local_events() {
        let dir = TestDir::new();
        let (vault, team) = local_team(&dir);
        let request = Vault::local_draft_id().unwrap();
        for text in [
            "".to_string(),
            " \n".into(),
            "x".repeat(super::super::journal::MAX_TEXT + 1),
            "bad\0text".into(),
        ] {
            assert!(vault.append_local(&team, &request, &text).is_err());
        }
        assert!(vault.append_local(&team, "bad-request", "text").is_err());
        let member = dir.vault("unapproved");
        member.create_device().unwrap();
        let packet = vault.export(&team).unwrap();
        member
            .import(&text(&packet), &packet.manifest.fingerprint().unwrap())
            .unwrap();
        assert!(member
            .append_local(&team, &request, "not-approved")
            .is_err());
        assert!(!member.local_status(&team).unwrap().can_write);
        assert_eq!(vault.local_status(&team).unwrap().stored, 0);
    }
}
