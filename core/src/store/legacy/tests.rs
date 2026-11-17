use super::*;
use crate::store::{default_settings, Store, Stores};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("sai-preserve-{:016x}", rand::random::<u64>()));
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }
    fn write(&self, name: &str, data: &[u8]) {
        let path = self.0.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, data).unwrap();
    }
    fn json(&self, name: &str, data: Value) {
        self.write(name, &serde_json::to_vec_pretty(&data).unwrap());
    }
    fn bytes(&self, name: &str) -> Vec<u8> {
        fs::read(self.0.join(name)).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn settings() -> Value {
    json!({"teams":[{"teamId":"team-a","name":"이전 팀","displayName":"내 이름","joinCode":"PRIVATE_JOIN_CODE","session":{"token":"PRIVATE_SESSION"}}],
        "googleTokensEnc":"PRIVATE_GOOGLE_TOKEN", "apiKeyEnc":"PRIVATE_CLAUDE_KEY", "openAiKeyEnc":"PRIVATE_OPENAI_KEY",
        "googleClient":{"clientSecret":"PRIVATE_OAUTH_SECRET"}, "firebaseSessionEnc":"PRIVATE_FIREBASE_TOKEN", "unknown":{"value":"PRIVATE_UNKNOWN"}})
}
fn cache() -> Value {
    json!({"teams":{"team-a":{
        "members":[{"uid":"member-a","displayName":"팀원","joinCode":"PRIVATE_MEMBER_CODE","credentials":{"token":"PRIVATE_NESTED_TOKEN"}}],
        "receivedAdvice":[{"text":"PRIVATE_ADVICE"}], "integrations":{"claude":{"key":"PRIVATE_SHARED_KEY"}},
        "shares":[{"text":"PRIVATE_TALK"}], "collab":{
            "projects":[{"id":"project-a","title":"이전 프로젝트","goal":"자료 정리","unknown":"PRIVATE_PROJECT_FIELD"}],
            "tasks":{"project-a":[{"id":"task-a","title":"조사","status":"doing","assignee":"member-a","apiKey":"PRIVATE_TASK_KEY"}]},
            "files":{"project-a":[{"id":"file-a","name":"기획서","url":"https://drive.google.com/file/d/example/view","localPath":"PRIVATE_LOCAL_PATH","downloadToken":"PRIVATE_DOWNLOAD_TOKEN"}]}
        }
    }},"google":{"email":"PRIVATE_OLD_ACCOUNT"}})
}

#[test]
fn export_is_selected_local_data_and_preserves_original_bytes_and_previous_exports() {
    let fixture = Fixture::new();
    fixture.json("settings.json", settings());
    fixture.json("shared/team-cache.json", cache());
    fixture.write("shared/outbox.json", b"PRIVATE_PENDING_DELIVERY");
    fixture.write("private/conversations.json", b"PRIVATE_CONVERSATION");
    fixture.write("mesh/identity.sqlite3", b"PRIVATE_ENCRYPTED_PC_ARCHIVE");
    let before = [
        "settings.json",
        "shared/team-cache.json",
        "shared/outbox.json",
        "private/conversations.json",
        "mesh/identity.sqlite3",
    ]
    .map(|p| (p, fixture.bytes(p)));
    let archive = Archive::read(&fixture.0).unwrap();
    assert_eq!(
        (
            archive.summary.teams,
            archive.summary.projects,
            archive.summary.tasks,
            archive.summary.files
        ),
        (1, 1, 1, 1)
    );
    assert!(archive.summary.pc_archive_present);
    let path = archive.export(&fixture.0).unwrap();
    let bytes = fs::read(&path).unwrap();
    let exported: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        exported["sources"][0]["teams"][0]["projects"][0]["tasks"][0]["title"],
        "조사"
    );
    assert!(!String::from_utf8(bytes.clone())
        .unwrap()
        .contains("PRIVATE_"));
    let next = archive.export(&fixture.0).unwrap();
    assert_ne!(path, next);
    assert_eq!(fs::read(&path).unwrap(), bytes);
    for (name, bytes) in before {
        assert_eq!(fixture.bytes(name), bytes, "source changed: {name}");
    }
    assert!(fs::read_dir(export_folder(&fixture.0, false).unwrap())
        .unwrap()
        .all(|e| e.unwrap().path().extension().unwrap() == "json"));
}

#[test]
fn scopes_are_separate_and_orphaned_project_data_is_kept() {
    let fixture = Fixture::new();
    fixture.json(
        "settings.json",
        json!({"teamServer":{"serverId":"server-a"},"teams":[{"teamId":"same","name":"현재"}],
        "teamServerProfiles":{"firebase":{"teams":[{"teamId":"same","name":"이전"}]}}}),
    );
    fixture.json(
        "shared/team-cache.json",
        json!({"teams":{"same":{"collab":{"tasks":{"missing-project":[{"title":"보관된 일"}]}}}}}),
    );
    fixture.json(
        "shared/servers/server-a/team-cache.json",
        json!({"teams":{"same":{}}}),
    );
    fixture.json(
        "shared/servers/server-b/team-cache.json",
        json!({"teams":{"orphan":{}}}),
    );
    let archive = Archive::read(&fixture.0).unwrap();
    assert_eq!(archive.summary.teams, 3);
    assert_eq!(archive.summary.projects, 1);
    assert_eq!(archive.summary.tasks, 1);
    let sources = archive.document["sources"].as_array().unwrap();
    assert_eq!(sources[0]["teams"][0]["name"], "이전");
    assert_eq!(sources[1]["teams"][0]["name"], "현재");
    assert_eq!(sources[0]["teams"][0]["projects"][0]["cachedOnly"], true);
}

#[test]
fn damaged_unsupported_and_oversized_sources_fail_without_a_partial_success() {
    let fixture = Fixture::new();
    fixture.json("settings.json", settings());
    for raw in [
        b"{broken".as_slice(),
        b"[]",
        br#"{"teams":{"a":{"collab":{"tasks":42}}}}"#,
    ] {
        fixture.write("shared/team-cache.json", raw);
        assert!(Archive::read(&fixture.0).is_err());
        assert_eq!(fixture.bytes("shared/team-cache.json"), raw);
        assert!(!fixture.0.join(EXPORT_FOLDER).exists());
    }
    let path = fixture.0.join("shared/team-cache.json");
    File::create(path).unwrap().set_len(MAX_FILE + 1).unwrap();
    assert!(Archive::read(&fixture.0).is_err());
}

#[test]
fn file_boundaries_and_unsafe_profile_paths_are_rejected() {
    let fixture = Fixture::new();
    let outside = Fixture::new();
    outside.json("settings.json", settings());
    let mut reader = Reader {
        root: fs::canonicalize(&fixture.0).unwrap(),
        total: 0,
    };
    assert!(reader.json(&outside.0.join("settings.json")).is_err());
    fixture.json(
        "settings.json",
        json!({"teamServerProfiles":{"../outside":{"teams":[]}}}),
    );
    assert!(Archive::read(&fixture.0).is_err());
    fixture.json(
        "settings.json",
        json!({"teamServer":{"serverId":42},"teams":[]}),
    );
    assert!(Archive::read(&fixture.0).is_err());
}

#[test]
fn unexpected_nested_values_do_not_become_exported_secrets_and_empty_install_stays_empty() {
    let fixture = Fixture::new();
    let archive = Archive::read(&fixture.0).unwrap();
    assert_eq!(archive.summary.teams, 0);
    assert!(archive.export(&fixture.0).is_err());
    assert!(!fixture.0.join(EXPORT_FOLDER).exists());
    fixture.json(
        "settings.json",
        json!({"teams":[{"teamId":"a","name":{"token":"PRIVATE_NAME_PAYLOAD"}}]}),
    );
    let archive = Archive::read(&fixture.0).unwrap();
    assert!(!archive
        .document
        .to_string()
        .contains("PRIVATE_NAME_PAYLOAD"));
    assert!(archive.document["sources"][0]["teams"][0]["name"].is_null());
}

#[test]
fn unreadable_or_invalid_store_never_overwrites_the_original_on_update_or_replace() {
    let fixture = Fixture::new();
    for original in [b"{incomplete".as_slice(), b"[]", b"null"] {
        fixture.write("settings.json", original);
        let store = Store::open(fixture.0.join("settings.json"), default_settings());
        assert!(store.load_error().is_some());
        store.update(|v| v["frequency"] = json!("low"));
        assert!(store.try_flush().is_err());
        store.replace(default_settings());
        store.flush();
        assert_eq!(fixture.bytes("settings.json"), original);
    }
    fs::create_dir(fixture.0.join("directory.json")).unwrap();
    let store = Store::open(fixture.0.join("directory.json"), json!({}));
    assert!(store.load_error().is_some());
    store.replace(json!({"new":true}));
    assert!(store.try_flush().is_err());
    assert!(fixture.0.join("directory.json").is_dir());
}

#[test]
fn separate_connection_state_preserves_legacy_cache_and_unknown_settings_fields() {
    let fixture = Fixture::new();
    fixture.json("settings.json", settings());
    fixture.json("shared/team-cache.json", cache());
    let before = fixture.bytes("shared/team-cache.json");
    let stores = Stores::open(&fixture.0);
    stores
        .google_connection
        .update(|v| v["google"] = json!({"lastResult":"new connection only"}));
    stores.settings.update(|v| v["frequency"] = json!("low"));
    stores.flush_all();
    assert_eq!(fixture.bytes("shared/team-cache.json"), before);
    let saved: Value = serde_json::from_slice(&fixture.bytes("settings.json")).unwrap();
    assert_eq!(saved["unknown"]["value"], "PRIVATE_UNKNOWN");
    assert_eq!(saved["teams"], settings()["teams"]);
    let next = Stores::open(&fixture.0);
    assert_eq!(
        next.google_connection.get()["google"]["lastResult"],
        "new connection only"
    );
}
