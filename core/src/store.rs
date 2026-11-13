//! 간단한 JSON 파일 저장소.
//! 개인 데이터(private/)와 공유 가능 데이터(shared/)를 서로 다른 폴더·파일에 분리해 저장한다.
use crate::util::deep_merge;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct Store {
    inner: Arc<Mutex<Inner>>,
}

struct Inner {
    file: Option<PathBuf>,
    data: Value,
    dirty: bool,
}

impl Store {
    pub fn open(file: impl AsRef<Path>, defaults: Value) -> Self {
        let file = file.as_ref().to_path_buf();
        let mut data = defaults;
        if let Ok(raw) = std::fs::read_to_string(&file) {
            if let Ok(v) = serde_json::from_str::<Value>(&raw) {
                deep_merge(&mut data, &v);
            }
        }
        Store { inner: Arc::new(Mutex::new(Inner { file: Some(file), data, dirty: false })) }
    }

    /// 파일 없이 메모리에만 (테스트용)
    pub fn memory(defaults: Value) -> Self {
        Store { inner: Arc::new(Mutex::new(Inner { file: None, data: defaults, dirty: false })) }
    }

    /// 지금 내용의 복사본
    pub fn get(&self) -> Value {
        self.inner.lock().unwrap().data.clone()
    }

    /// 읽기만 (복사 없이)
    pub fn read<R>(&self, f: impl FnOnce(&Value) -> R) -> R {
        f(&self.inner.lock().unwrap().data)
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
        let (file, text) = {
            let mut g = self.inner.lock().unwrap();
            if !g.dirty {
                return;
            }
            g.dirty = false;
            match &g.file {
                Some(f) => (f.clone(), serde_json::to_string_pretty(&g.data).unwrap_or_default()),
                None => return,
            }
        };
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = file.with_extension("json.tmp");
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, &file);
        }
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
}

impl Stores {
    pub fn open(dir: impl AsRef<Path>) -> Self {
        let d = dir.as_ref();
        Stores {
            settings: Store::open(d.join("settings.json"), default_settings()),
            private_conv: Store::open(d.join("private").join("conversations.json"), json!({ "sessions": [] })),
            shared_outbox: Store::open(d.join("shared").join("outbox.json"), json!({ "items": [] })),
            team_cache: Store::open(d.join("shared").join("team-cache.json"), json!({ "teams": {}, "shownProposals": {} })),
            schedule: Store::open(d.join("private").join("schedule.json"), json!({})),
            advice_history: Store::open(d.join("advice-history.json"), json!({ "recent": [] })),
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
        }
    }

    pub fn flush_all(&self) {
        for s in [&self.settings, &self.private_conv, &self.shared_outbox, &self.team_cache, &self.schedule, &self.advice_history] {
            s.flush();
        }
    }
}
