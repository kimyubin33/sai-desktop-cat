use super::*;
use crate::store::Stores;
use crate::team::{Backend, Hub};
use serde_json::json;
struct Team {
    dir: std::path::PathBuf,
    vault: Vault,
    other: Vault,
    target: String,
    backend: Arc<Backend>,
    data: Arc<std::sync::Mutex<Value>>,
}
impl Team {
    fn new() -> Self {
        let dir =
            std::env::temp_dir().join(format!("sai-archive-{}", Vault::local_draft_id().unwrap()));
        std::fs::create_dir_all(&dir).unwrap();
        let vault = Vault::new(dir.join("a.sqlite3"));
        let other = Vault::new(dir.join("b.sqlite3"));
        vault.create_device().unwrap();
        other.create_device().unwrap();
        let first = vault.create_team("이전 대상").unwrap();
        let target = first.manifest.body.team_id.clone();
        other
            .import(
                &serde_json::to_string(&first).unwrap(),
                &first.manifest.fingerprint().unwrap(),
            )
            .unwrap();
        let packet = vault
            .approve(
                &target,
                &serde_json::to_string(&other.request(&target).unwrap()).unwrap(),
            )
            .unwrap();
        other
            .import(&serde_json::to_string(&packet).unwrap(), "")
            .unwrap();
        let data = Backend::new_shared_memory();
        {
            let mut d = data.lock().unwrap();
            d["docs"]["teams/old"] = json!({"name":"기존 팀","createdBy":"owner"});
            d["docs"]["teams/old/members/owner"] = json!({"displayName":"관리자"});
            d["docs"]["teams/old/admins/owner"] = json!({"key":"test-role"});
        }
        Self {
            dir,
            vault,
            other,
            target,
            backend: Arc::new(Backend::memory_shared(data.clone(), "owner")),
            data,
        }
    }
    fn row(&self, id: &str, text: &str) {
        self.data.lock().unwrap()["docs"][format!("teams/old/shares/{id}")] =
            json!({"by":"old-author","at":1000,"text":text});
    }
    fn importer(&self) -> Importer {
        Importer::new(self.vault.clone())
    }
    fn db(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(self.dir.join("a.sqlite3")).unwrap()
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
            .starts_with("sai-archive-"));
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
async fn commit(i: &Importer, t: &Team, p: &Preview, ids: &[&str]) -> Result<ImportReport> {
    i.commit(
        t.backend.clone(),
        "old",
        &t.target,
        &p.token,
        ids.iter().map(|s| s.to_string()).collect(),
        true,
    )
    .await
}

#[test]
fn only_exact_public_text_schema_is_eligible_and_private_metadata_never_enters_preview() {
    let source = Source {
        scope: "00".repeat(32),
        kind: "memory".into(),
        team_id: "old".into(),
        team_name: "기존".into(),
        reader_uid: "owner".into(),
    };
    let snap=Snapshot::from_rows(source,vec![
        json!({"id":"normal","text":"공개 톡","by":"uid","at":1}),
        json!({"id":"url","text":"설명","by":"uid","at":1,"url":"https://drive.google.com/"}),
        json!({"id":"private","text":"개인 문안","by":"uid","at":1,"toMember":"other"}),
        json!({"id":"key","text":"복사 금지","by":"uid","at":1,"apiKey":"secret-must-not-be-copied"}),
        json!({"id":"aip-proposal","text":"약속","by":"uid","at":1}),
        json!({"id":"bad","text":"x","by":"uid","at":"yesterday"}),
    ],10).unwrap();
    assert_eq!(snap.rows.len(), 1);
    assert_eq!(snap.rows[0].id, "normal");
    assert_eq!(snap.excluded.values().sum::<usize>(), 5);
    assert!(!serde_json::to_string(&snap.rows)
        .unwrap()
        .contains("secret-must-not-be-copied"));
}

#[tokio::test]
async fn preview_reads_beyond_the_screen_cache_and_does_not_write_the_archive() {
    let t = Team::new();
    for n in 0..350 {
        t.row(&format!("m{n:04}"), "서버의 공개 톡");
    }
    let stores = Stores::memory();
    let data = t.data.clone();
    let hub = Hub::new(
        stores.settings,
        stores.shared_outbox,
        stores.team_cache,
        Box::new(move || Some(Backend::memory_shared(data.clone(), "owner"))),
        Some(0),
        Arc::new(crate::time::now_ms),
    );
    // 화면에는 일부만 저장되어 있어도 이전 조회는 이 캐시를 읽지 않는다.
    hub.0.team_cache.update(|c| {
        c["teams"]["old"]["shares"] = json!([{"id":"cached-deleted","text":"서버에서 삭제된 캐시"}])
    });
    let i = t.importer();
    let p = i
        .preview(t.backend.clone(), "old", &t.target)
        .await
        .unwrap();
    assert_eq!(p.candidates.len(), 350);
    assert!(!p.candidates.iter().any(|r| r.id == "cached-deleted"));
    assert_eq!(i.page(&t.target, 0).await.unwrap().total, 0);
}

#[tokio::test]
async fn selected_import_preserves_provenance_stays_local_and_retries_without_duplication() {
    let t = Team::new();
    t.row("first", "archive-unique-body-not-plaintext");
    t.row("second", "이번에는 선택 안 함");
    let before = t.data.lock().unwrap().clone();
    let i = t.importer();
    let p = i
        .preview(t.backend.clone(), "old", &t.target)
        .await
        .unwrap();
    let report = commit(&i, &t, &p, &["first"]).await.unwrap();
    assert_eq!(report.imported, 1);
    let page = i.page(&t.target, 0).await.unwrap();
    assert_eq!(page.total, 1);
    let row = &page.messages[0];
    assert_eq!(row.legacy_id, "first");
    assert_eq!(row.claimed_by, "old-author");
    assert_eq!(row.claimed_at, 1000);
    assert_eq!(row.source_team, "old");
    assert_eq!(row.text, "archive-unique-body-not-plaintext");
    assert_eq!(*t.data.lock().unwrap(), before);
    let retry = commit(&i, &t, &p, &["first"]).await.unwrap();
    assert_eq!(retry.total, 1);
    assert_eq!(retry.imported, 1); // Same committed report, not another insert.
    let p2 = i
        .preview(t.backend.clone(), "old", &t.target)
        .await
        .unwrap();
    assert_eq!(
        p2.candidates
            .iter()
            .find(|r| r.id == "first")
            .unwrap()
            .state,
        "existing"
    );
    let report = commit(&i, &t, &p2, &["first"]).await.unwrap();
    assert_eq!(report.imported, 0);
    assert_eq!(report.existing, 1);
    let conn = t.db();
    for table in [
        "mesh_local_events",
        "mesh_local_outbox",
        "mesh_local_send_policy",
        "mesh_courier",
        "mesh_sent_receipts",
    ] {
        assert_eq!(
            conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
    for suffix in ["", "-wal"] {
        if let Ok(bytes) = std::fs::read(t.dir.join(format!("a.sqlite3{suffix}"))) {
            assert!(!bytes
                .windows(row.text.len())
                .any(|w| w == row.text.as_bytes()));
        }
    }
}

#[tokio::test]
async fn changed_or_deleted_selected_source_is_rejected_without_partial_import() {
    for delete in [false, true] {
        let t = Team::new();
        t.row("a", "원문");
        t.row("b", "다른 톡");
        let i = t.importer();
        let p = i
            .preview(t.backend.clone(), "old", &t.target)
            .await
            .unwrap();
        if delete {
            t.data.lock().unwrap()["docs"]
                .as_object_mut()
                .unwrap()
                .remove("teams/old/shares/a");
        } else {
            t.row("a", "변경한 원문");
        }
        assert!(commit(&i, &t, &p, &["a", "b"]).await.is_err());
        assert_eq!(i.page(&t.target, 0).await.unwrap().total, 0);
    }
}

#[tokio::test]
async fn added_unselected_messages_are_not_silently_included_in_the_import() {
    let t = Team::new();
    t.row("a", "선택");
    let i = t.importer();
    let p = i
        .preview(t.backend.clone(), "old", &t.target)
        .await
        .unwrap();
    t.row("b", "미리보기 뒤 새 톡");
    assert_eq!(commit(&i, &t, &p, &["a"]).await.unwrap().imported, 1);
    assert_eq!(
        i.page(&t.target, 0).await.unwrap().messages[0].legacy_id,
        "a"
    );
}

#[tokio::test]
async fn original_deletion_after_import_does_not_pretend_to_delete_the_explicit_snapshot() {
    let t = Team::new();
    t.row("a", "명시적으로 별도 보관");
    let i = t.importer();
    let p = i
        .preview(t.backend.clone(), "old", &t.target)
        .await
        .unwrap();
    commit(&i, &t, &p, &["a"]).await.unwrap();
    t.data.lock().unwrap()["docs"]
        .as_object_mut()
        .unwrap()
        .remove("teams/old/shares/a");
    assert_eq!(i.page(&t.target, 0).await.unwrap().total, 1);
}

#[tokio::test]
async fn legacy_and_pc_administrator_permissions_are_both_required_and_rechecked() {
    let t = Team::new();
    t.row("a", "권한 검사");
    let i = t.importer();
    let denied = Arc::new(Backend::memory_shared(t.data.clone(), "outsider"));
    assert!(i.preview(denied, "old", &t.target).await.is_err());
    let member = Importer::new(t.other.clone());
    assert!(member
        .preview(t.backend.clone(), "old", &t.target)
        .await
        .is_err());
    let p = i
        .preview(t.backend.clone(), "old", &t.target)
        .await
        .unwrap();
    t.data.lock().unwrap()["docs"]
        .as_object_mut()
        .unwrap()
        .remove("teams/old/admins/owner");
    assert!(commit(&i, &t, &p, &["a"]).await.is_err());
    assert_eq!(i.page(&t.target, 0).await.unwrap().total, 0);
}

#[tokio::test]
async fn destination_approval_change_rejects_the_old_preview_and_keeps_preexisting_messages() {
    let t = Team::new();
    t.row("a", "옮길 공개 톡");
    t.vault
        .append_local(
            &t.target,
            &Vault::local_draft_id().unwrap(),
            "새 PC 톡 원본",
        )
        .unwrap();
    let i = t.importer();
    let p = i
        .preview(t.backend.clone(), "old", &t.target)
        .await
        .unwrap();
    let other = t.other.status().unwrap().device.unwrap().device_id;
    t.vault.revoke(&t.target, &other).unwrap();
    assert!(commit(&i, &t, &p, &["a"]).await.is_err());
    assert_eq!(i.page(&t.target, 0).await.unwrap().total, 0);
    assert_eq!(t.vault.local_status(&t.target).unwrap().stored, 1);
}

#[tokio::test]
async fn selection_requires_exact_token_team_backend_ids_and_explicit_consent() {
    let t = Team::new();
    t.row("a", "선택 검증");
    let i = t.importer();
    let p = i
        .preview(t.backend.clone(), "old", &t.target)
        .await
        .unwrap();
    assert!(i
        .commit(
            t.backend.clone(),
            "old",
            &t.target,
            &p.token,
            vec!["a".into()],
            false
        )
        .await
        .is_err());
    assert!(i
        .commit(
            t.backend.clone(),
            "old",
            &t.target,
            "wrong",
            vec!["a".into()],
            true
        )
        .await
        .is_err());
    assert!(i
        .commit(
            t.backend.clone(),
            "other",
            &t.target,
            &p.token,
            vec!["a".into()],
            true
        )
        .await
        .is_err());
    assert!(commit(&i, &t, &p, &["missing"]).await.is_err());
    assert!(commit(&i, &t, &p, &["a", "a"]).await.is_err());
    let backend = Arc::new(Backend::memory_shared(t.data.clone(), "owner"));
    assert!(i
        .commit(backend, "old", &t.target, &p.token, vec!["a".into()], true)
        .await
        .is_err());
    assert_eq!(i.page(&t.target, 0).await.unwrap().total, 0);
}

#[tokio::test]
async fn expired_or_replaced_preview_cannot_commit() {
    let t = Team::new();
    t.row("a", "미리보기 시간");
    let i = t.importer();
    let p = i
        .preview(t.backend.clone(), "old", &t.target)
        .await
        .unwrap();
    i.plan.lock().await.as_mut().unwrap().expires = Instant::now() - Duration::from_secs(1);
    assert!(commit(&i, &t, &p, &["a"]).await.is_err());
    let next = i
        .preview(t.backend.clone(), "old", &t.target)
        .await
        .unwrap();
    assert_ne!(p.token, next.token);
    assert!(commit(&i, &t, &p, &["a"]).await.is_err());
}

#[tokio::test]
async fn a_conflicting_archived_id_is_preserved_and_aborts_the_entire_selection() {
    let t = Team::new();
    t.row("a", "먼저 보관한 원문");
    let i = t.importer();
    let p = i
        .preview(t.backend.clone(), "old", &t.target)
        .await
        .unwrap();
    commit(&i, &t, &p, &["a"]).await.unwrap();
    t.row("a", "서버에서 바뀐 원문");
    t.row("b", "새 톡");
    let p = i
        .preview(t.backend.clone(), "old", &t.target)
        .await
        .unwrap();
    assert_eq!(
        p.candidates.iter().find(|r| r.id == "a").unwrap().state,
        "conflict"
    );
    assert!(commit(&i, &t, &p, &["a", "b"]).await.is_err());
    let page = i.page(&t.target, 0).await.unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.messages[0].text, "먼저 보관한 원문");
}

#[tokio::test]
async fn a_failed_second_insert_rolls_back_the_whole_import_and_retry_keeps_source_ids() {
    let t = Team::new();
    t.row("a", "첫째");
    t.row("b", "둘째");
    let i = t.importer();
    let p = i
        .preview(t.backend.clone(), "old", &t.target)
        .await
        .unwrap();
    let conn = t.db();
    conn.execute_batch("CREATE TRIGGER fail_second BEFORE INSERT ON mesh_legacy_archive WHEN (SELECT count(*) FROM mesh_legacy_archive)>0 BEGIN SELECT RAISE(ABORT,'test');END;").unwrap();
    assert!(commit(&i, &t, &p, &["a", "b"]).await.is_err());
    assert_eq!(i.page(&t.target, 0).await.unwrap().total, 0);
    conn.execute_batch("DROP TRIGGER fail_second;").unwrap();
    assert_eq!(commit(&i, &t, &p, &["a", "b"]).await.unwrap().imported, 2);
}

#[tokio::test]
async fn tampered_header_or_ciphertext_is_not_displayed_or_overwritten() {
    let t = Team::new();
    t.row("a", "암호문 검증");
    let i = t.importer();
    let p = i
        .preview(t.backend.clone(), "old", &t.target)
        .await
        .unwrap();
    commit(&i, &t, &p, &["a"]).await.unwrap();
    let conn = t.db();
    let header: String = conn
        .query_row("SELECT header FROM mesh_legacy_archive", [], |r| r.get(0))
        .unwrap();
    let mut h: Value = serde_json::from_str(&header).unwrap();
    h["body"]["imported_at"] = json!(42);
    conn.execute("UPDATE mesh_legacy_archive SET header=?1", [h.to_string()])
        .unwrap();
    assert!(i.page(&t.target, 0).await.is_err());
    conn.execute(
        "UPDATE mesh_legacy_archive SET header=?1,sealed=x'0001'",
        [header],
    )
    .unwrap();
    assert!(i.page(&t.target, 0).await.is_err());
}

#[tokio::test]
async fn another_vault_cannot_display_the_same_account_blob_as_its_own_device_archive() {
    let t = Team::new();
    t.row("a", "다른 장치로 가장 금지");
    let i = t.importer();
    let p = i
        .preview(t.backend.clone(), "old", &t.target)
        .await
        .unwrap();
    commit(&i, &t, &p, &["a"]).await.unwrap();
    let src = t.db();
    let (key, header, sealed): (String, String, Vec<u8>) = src
        .query_row(
            "SELECT key,header,sealed FROM mesh_legacy_archive",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    let dst = rusqlite::Connection::open(t.dir.join("b.sqlite3")).unwrap();
    dst.execute(
        "INSERT INTO mesh_legacy_archive(team,key,header,sealed) VALUES(?1,?2,?3,?4)",
        rusqlite::params![t.target, key, header, sealed],
    )
    .unwrap();
    assert!(Importer::new(t.other.clone())
        .page(&t.target, 0)
        .await
        .is_err());
}

#[tokio::test]
async fn archival_pages_are_bounded_and_independent_of_live_journal_counts() {
    let t = Team::new();
    for n in 0..55 {
        t.row(&format!("m{n:03}"), "보관 목록");
    }
    let i = t.importer();
    let p = i
        .preview(t.backend.clone(), "old", &t.target)
        .await
        .unwrap();
    let ids = p.candidates.iter().map(|r| r.id.clone()).collect();
    i.commit(t.backend.clone(), "old", &t.target, &p.token, ids, true)
        .await
        .unwrap();
    let first = i.page(&t.target, 0).await.unwrap();
    let second = i.page(&t.target, 50).await.unwrap();
    assert_eq!(first.messages.len(), 50);
    assert_eq!(first.next_offset, Some(50));
    assert_eq!(second.messages.len(), 5);
    assert!(second.next_offset.is_none());
    assert_eq!(t.vault.local_status(&t.target).unwrap().stored, 0);
    assert_eq!(t.vault.local_status(&t.target).unwrap().pending, 0);
}

#[test]
fn v5_upgrade_preserves_keys_and_does_not_create_archive_rows() {
    let t = Team::new();
    let own = t.vault.status().unwrap().device.unwrap();
    let conn = t.db();
    conn.execute_batch("DROP TABLE mesh_legacy_archive;PRAGMA user_version=5;")
        .unwrap();
    assert_eq!(t.vault.status().unwrap().device.unwrap(), own);
    assert_eq!(
        conn.query_row("SELECT count(*) FROM mesh_legacy_archive", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    conn.execute_batch("PRAGMA user_version=5;").unwrap();
    assert!(t.vault.status().is_err());
    assert_eq!(
        conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
            .unwrap(),
        5
    );
}
