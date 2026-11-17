//! 말풍선 조언을 고양이 창과 연결한다: 평소 조언, 팀 약속 제안, 업무 진행 확인, 업데이트 안내.
use crate::app::{open_chat, App};
use crate::cat;
use sai_core::advice::{next_delay_ms, pick_advice, remember_shown, silence_reason, Entry};
use sai_core::time::now_ms;
use sai_core::util::J;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const LATER_MS: i64 = 20 * 60 * 1000;
const BUSY_RETRY_MS: i64 = 60 * 1000;

#[derive(Default)]
struct St {
    shown: HashMap<String, Value>,
    seq: u64,
    pending_update: Option<String>,
    next_at: i64,
    later: Option<(i64, Value)>,
}

pub struct Advisor {
    st: Mutex<St>,
}

fn act(id: &str, label: &str, primary: bool) -> Value {
    json!({ "id": id, "label": label, "primary": primary })
}

fn entry_value(e: &Entry) -> Value {
    json!({ "id": e.id, "kind": e.kind, "topic": e.topic, "text": e.text, "sourceLabel": format!("— {}", e.source_label) })
}

impl Advisor {
    pub fn new() -> Advisor {
        Advisor {
            st: Mutex::new(St::default()),
        }
    }

    pub fn start(&self, a: &Arc<App>) {
        if !a.stores.settings.read(|s| s.b("onboarded")) {
            let a2 = a.clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(Duration::from_millis(2500)).await;
                a2.advisor.greet(&a2);
            });
        }
        self.schedule(a, None);
        let a2 = a.clone();
        tauri::async_runtime::spawn(async move {
            let mut iv = tokio::time::interval(Duration::from_secs(5));
            loop {
                iv.tick().await;
                a2.advisor.poll(&a2);
            }
        });
    }

    /// 다음에 말을 걸 때를 정한다
    pub fn schedule(&self, a: &Arc<App>, delay: Option<i64>) {
        let ms = delay.unwrap_or_else(|| a.stores.settings.read(|s| next_delay_ms(s)) as i64);
        self.st.lock().unwrap().next_at = now_ms() + ms;
    }

    fn poll(&self, a: &Arc<App>) {
        let now = now_ms();
        let (due, later) = {
            let mut st = self.st.lock().unwrap();
            let due = now >= st.next_at;
            let later = match &st.later {
                Some((t, _)) if *t <= now => st.later.take().map(|x| x.1),
                _ => None,
            };
            (due, later)
        };
        if let Some(entry) = later {
            if silence_reason(&a.stores.settings.get(), now).is_none()
                && !a.cat.lock().unwrap().is_busy()
            {
                self.show(a, entry, None);
            }
        }
        if due {
            self.on_timer(a);
        }
    }

    fn on_timer(&self, a: &Arc<App>) {
        let s = a.stores.settings.get();
        if silence_reason(&s, now_ms()).is_some() {
            return self.schedule(a, None);
        }
        let (busy, visible) = {
            let c = a.cat.lock().unwrap();
            (c.is_busy(), c.visible)
        };
        if busy || !visible {
            return self.schedule(a, Some(BUSY_RETRY_MS));
        }
        if !self.maybe_show_update(a) {
            self.speak_now(a);
        }
        self.schedule(a, None);
    }

    fn greet(&self, a: &Arc<App>) {
        let name = a.stores.settings.read(|s| s.s("catName"));
        self.show(
            a,
            json!({ "id": "greet", "kind": "system", "text": format!("안녕하세요, 저는 {name}예요. 가끔 말을 걸게요. 저를 클릭하면 편하게 이야기할 수 있고, 끌어서 옮길 수도 있어요.") }),
            None,
        );
    }

    /// 사용자가 메뉴에서 직접 요청했을 때는 조용한 모드여도 말한다
    pub fn speak_now(&self, a: &Arc<App>) {
        let recent: Vec<String> = a.stores.advice_history.read(|h| h.strs("recent"));
        let boosts = HashMap::new();
        let e = pick_advice(&recent, &boosts, now_ms());
        let next = remember_shown(&recent, &e.id);
        a.stores
            .advice_history
            .update(|h| h["recent"] = json!(next));
        self.show(a, entry_value(&e), None);
    }

    pub fn notify_update(&self, a: &Arc<App>, version: &str) {
        self.st.lock().unwrap().pending_update = Some(version.into());
        let quiet = silence_reason(&a.stores.settings.get(), now_ms()).is_some();
        let (busy, visible) = {
            let c = a.cat.lock().unwrap();
            (c.is_busy(), c.visible)
        };
        if !quiet && !busy && visible {
            self.maybe_show_update(a);
        }
    }

    fn maybe_show_update(&self, a: &Arc<App>) -> bool {
        let Some(v) = self.st.lock().unwrap().pending_update.clone() else {
            return false;
        };
        self.show(
            a,
            json!({ "id": "update", "kind": "update", "text": format!("새 버전 {v}이 준비됐어요. 지금 다시 시작해서 적용할까요?"), "sourceLabel": "— 사이 업데이트",
                    "actions": [act("update-later", "나중에", false), act("update-install", "지금 적용", true)] }),
            None,
        );
        true
    }

    /// 업데이트 확인 결과 같은 짧은 안내
    pub fn info(&self, a: &Arc<App>, text: &str) {
        cat::hide_bubble(&a.handle, a);
        self.show(a, json!({ "id": "info", "kind": "info", "text": text, "sourceLabel": "— 사이", "actions": [act("close", "확인", false)] }), None);
    }

    fn actions_for(&self, _a: &Arc<App>, e: &Value) -> Value {
        if e["actions"].is_array() {
            return e["actions"].clone();
        }
        json!([
            act("later", "나중에", false),
            act("chat", "AI와 이야기", true)
        ])
    }

    fn show(&self, a: &Arc<App>, entry: Value, text: Option<&str>) -> bool {
        let id = {
            let mut st = self.st.lock().unwrap();
            st.seq += 1;
            let id = format!("b{}", st.seq);
            st.shown.insert(id.clone(), entry.clone());
            id
        };
        let payload = json!({
            "id": id, "text": text.map(String::from).unwrap_or_else(|| entry.s("text")), "sourceLabel": entry.s("sourceLabel"),
            "topic": entry["topic"], "kind": entry.s("kind"), "actions": self.actions_for(a, &entry),
        });
        cat::show_bubble(&a.handle, a, payload)
    }

    pub fn handle_action(&self, a: &Arc<App>, id: &str, action: &str) {
        let Some(entry) = self.st.lock().unwrap().shown.remove(id) else {
            return;
        };
        if entry.s("id") == "greet" {
            a.stores.settings.update(|s| s["onboarded"] = json!(true));
        }
        match action {
            "later" => self.st.lock().unwrap().later = Some((now_ms() + LATER_MS, entry)),
            "chat" => open_chat(&a.handle, a, "chat", json!({})),
            "update-install" => {
                self.st.lock().unwrap().pending_update = None;
                a.updater.install(a);
            }
            // 다시 묻지 않는다
            "update-later" => self.st.lock().unwrap().pending_update = None,
            _ => {}
        }
    }
}
