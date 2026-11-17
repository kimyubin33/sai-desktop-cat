//! 간단한 JSON 파일 저장소.
//! 개인 데이터(private/)와 공유 가능 데이터(shared/)를 서로 다른 폴더·파일에 분리해 저장한다.
use crate::util::deep_merge;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub mod team_server;
pub mod legacy;

#[derive(Clone)]
pub struct Store {
    inner: Arc<Mutex<Inner>>,
}

struct Inner {
    file: Option<PathBuf>,
    data: Value,
    overlay: Option<Value>,
    dirty: bool,
    load_error: Option<String>,
}

impl Store {
    pub fn open(file: impl AsRef<Path>, defaults: Value) -> Self {
        let file = file.as_ref().to_path_buf();
        let mut data = defaults;
        let load_error = match std::fs::read_to_string(&file) {
            Ok(raw) => match serde_json::from_str::<Value>(&raw) {
                Ok(v) if !data.is_object() || v.is_object() => { deep_merge(&mut data, &v); None }
                _ => Some("저장된 자료의 형식을 읽지 못했어요. 원본을 보호하기 위해 저장을 중단했어요.".into()),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => Some("저장된 자료를 읽지 못했어요. 원본을 보호하기 위해 저장을 중단했어요.".into()),
        };
        Store { inner: Arc::new(Mutex::new(Inner { file: Some(file), data, overlay: None, dirty: false, load_error })) }
    }

    /// 파일 없이 메모리에만 (테스트용)
    pub fn memory(defaults: Value) -> Self {
        Store { inner: Arc::new(Mutex::new(Inner { file: None, data: defaults, overlay: None, dirty: false, load_error: None })) }
    }

    /// 읽기 실패를 빈 자료로 간주해 원본을 덮어쓰지 않는다. 내용이나 비밀 값을 오류에 넣지 않는다.
    pub fn load_error(&self) -> Option<String> { self.inner.lock().unwrap().load_error.clone() }

    /// 지금 내용의 복사본
    pub fn get(&self) -> Value {
        let g = self.inner.lock().unwrap();
        let mut value = g.data.clone();
        if let Some(overlay) = &g.overlay { deep_merge(&mut value, overlay); }
        value
    }

    /// 읽기만 (복사 없이)
    pub fn read<R>(&self, f: impl FnOnce(&Value) -> R) -> R {
        let g = self.inner.lock().unwrap();
        if let Some(overlay) = &g.overlay {
            let mut value = g.data.clone();
            deep_merge(&mut value, overlay);
            f(&value)
        } else { f(&g.data) }
    }

    /// Account preferences affect readers only. They never enter the local settings file.
    pub fn set_overlay(&self, overlay: Option<Value>) {
        self.inner.lock().unwrap().overlay = overlay;
    }

    pub fn update<R>(&self, f: impl FnOnce(&mut Value) -> R) -> R {
        let mut g = self.inner.lock().unwrap();
        let r = f(&mut g.data);
        g.dirty = true;
        r
    }

    pub fn replace(&self, v: Value) {
        let mut g = self.inner.lock().unwrap();
        g.data = v;
        g.dirty = true;
    }

    /// 바뀐 내용이 있으면 파일에 쓴다 (임시 파일에 쓴 뒤 바꿔치기)
    pub fn flush(&self) {
        let _ = self.try_flush();
    }

    pub fn try_flush(&self) -> std::io::Result<()> {
        let mut g = self.inner.lock().unwrap();
        if !g.dirty { return Ok(()); }
        if let Some(error) = &g.load_error { return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, error.clone())); }
        if let Some(file) = &g.file {
            if let Some(dir) = file.parent() { std::fs::create_dir_all(dir)?; }
            let tmp = file.with_extension("json.tmp");
            std::fs::write(&tmp, serde_json::to_vec_pretty(&g.data)?)?;
            std::fs::rename(&tmp, file)?;
        }
        g.dirty = false;
        Ok(())
    }

    /// 검증한 로그인 정보처럼 즉시 보관해야 하는 변경. 쓰기 실패 시 메모리의 기존 값도 유지한다.
    pub fn commit(&self, change: impl FnOnce(&mut Value)) -> std::io::Result<()> {
        let mut g = self.inner.lock().unwrap();
        if let Some(error) = &g.load_error { return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, error.clone())); }
        let mut next = g.data.clone();
        change(&mut next);
        if let Some(file) = &g.file {
            if let Some(dir) = file.parent() { std::fs::create_dir_all(dir)?; }
            let tmp = file.with_extension("json.tmp");
            std::fs::write(&tmp, serde_json::to_vec_pretty(&next)?)?;
            std::fs::rename(tmp, file)?;
        }
        g.data = next;
        g.dirty = false;
        Ok(())
    }
}

pub fn default_settings() -> Value {
    json!({
        "catName": "사이",
        "onboarded": false,
        // 말 거는 빈도: low / normal / high — 끄는 선택지는 없다
        "frequency": "normal",
        "quietUntil": 0,
        "focusUntil": 0,
        "dnd": null,
        "dampenedTopics": {},
        "ai": { "enabled": false, "model": "claude-sonnet-5-5" },
        "apiKeyEnc": null,
        "retentionDays": 30,
        "position": null,
        "firebase": null,
        "firebaseSessionEnc": null,
        "teams": [],
        "activeTeamId": null,
        "googleClient": null,
        "googleTokensEnc": null,
        "googleTaskLinks": {},
        "telegramAway": false
    })
}

/// 앱이 쓰는 저장소 묶음
#[derive(Clone)]
pub struct Stores {
    pub settings: Store,
    /// 개인 대화 원문: 이 기기에만 저장하고 어떤 공유 경로에도 넣지 않는다
    pub private_conv: Store,
    /// 공유 대기함: 일반화된 주제와 문안만 저장 (원문·이름·날짜 세부 없음)
    pub shared_outbox: Store,
    /// 팀 캐시: 팀원 이름, 받은 주제, 팀 약속 제안 (원문 없음)
    pub team_cache: Store,
    /// 내 작업 시간표와 일정 조정 대화
    pub schedule: Store,
    pub advice_history: Store,
    /// Google 연결 상태는 이전 팀 캐시와 별도로 기록한다.
    pub google_connection: Store,
}

impl Stores {
    pub fn open(dir: impl AsRef<Path>) -> Self {
        let d = dir.as_ref();
        let settings = Store::open(d.join("settings.json"), default_settings());
        let shared = settings.read(|s| team_server::folder(d, &s["teamServer"]));
        Stores {
            settings,
            private_conv: Store::open(d.join("private").join("conversations.json"), json!({ "sessions": [] })),
            shared_outbox: Store::open(shared.join("outbox.json"), json!({ "items": [] })),
            team_cache: Store::open(shared.join("team-cache.json"), json!({ "teams": {}, "shownProposals": {} })),
            schedule: Store::open(d.join("private").join("schedule.json"), json!({})),
            advice_history: Store::open(d.join("advice-history.json"), json!({ "recent": [] })),
            google_connection: Store::open(d.join("private/google-connection.json"), json!({})),
        }
    }

    pub fn memory() -> Self {
        Stores {
            settings: Store::memory(default_settings()),
            private_conv: Store::memory(json!({ "sessions": [] })),
            shared_outbox: Store::memory(json!({ "items": [] })),
            team_cache: Store::memory(json!({ "teams": {}, "shownProposals": {} })),
            schedule: Store::memory(json!({})),
            advice_history: Store::memory(json!({ "recent": [] })),
            google_connection: Store::memory(json!({})),
        }
    }

    pub fn flush_all(&self) {
        for s in [&self.settings, &self.private_conv, &self.shared_outbox, &self.team_cache, &self.schedule, &self.advice_history, &self.google_connection] {
            s.flush();
        }
    }
}
