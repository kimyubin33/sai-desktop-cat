//! 사이의 말풍선에 인사, 조언, 업데이트 안내를 표시한다.
use crate::app::{open_chat, App};
use crate::cat;
use sai_core::advice::{next_delay_ms, pick_advice, remember_shown, silence_reason, verbatim_kind, Entry};
use sai_core::local_ai::character::{Moment, Trigger};
use sai_core::time::now_ms;
use sai_core::util::J;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const LATER_MS: i64 = 20 * 60 * 1000;
const BUSY_RETRY_MS: i64 = 60 * 1000;
/// Away this long (no keyboard or mouse) counts as having left the desk.
const AWAY_MS: u64 = 15 * 60 * 1000;
/// Back means input within this window.
const BACK_MS: u64 = 10 * 1000;
/// At most one "welcome back" line in this span.
const RETURN_GAP_MS: i64 = 30 * 60 * 1000;
/// How long the character may think before canned advice is used instead.
const THINK_LIMIT: Duration = Duration::from_secs(90);
const RECENT_LINES: usize = 8;

#[derive(Default)]
struct St {
    shown: HashMap<String, Value>,
    seq: u64,
    pending_update: Option<String>,
    next_at: i64,
    later: Option<(i64, Value)>,
    /// The local model is writing a line; do not start another.
    thinking: bool,
    /// When the user went idle, while they are away.
    away_since: Option<i64>,
    last_return: i64,
    polls: u64,
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
        self.watch_presence(a, now);
        let reap = {
            let mut st = self.st.lock().unwrap();
            st.polls += 1;
            st.polls % 12 == 0
        };
        if reap {
            let a2 = a.clone();
            tauri::async_runtime::spawn(async move { a2.local_ai.reap_idle().await });
        }
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
                self.show(a, entry);
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
            self.speak(a, Trigger::Timer);
        }
        self.schedule(a, None);
    }

    fn greet(&self, a: &Arc<App>) {
        let name = a.stores.settings.read(|s| s.s("catName"));
        self.show(
            a,
            json!({ "id": "greet", "kind": "system", "text": format!("안녕! 내 이름은 {name}. 가끔 말을 걸게. 나를 클릭하면 편하게 이야기할 수 있어.") }),
        );
    }

    /// Notices the user returning to the PC after being away, and greets them once.
    fn watch_presence(&self, a: &Arc<App>, now: i64) {
        let Some(idle) = crate::system::idle_ms() else {
            return;
        };
        let away_for = {
            let mut st = self.st.lock().unwrap();
            if idle >= AWAY_MS {
                st.away_since.get_or_insert(now - idle as i64);
                return;
            }
            if idle > BACK_MS {
                return;
            }
            let Some(since) = st.away_since.take() else {
                return;
            };
            if now - st.last_return < RETURN_GAP_MS {
                return;
            }
            st.last_return = now;
            (now - since) / 60_000
        };
        // Only the local character greets returns; canned lines would feel random here.
        if !a.local_ai.ready() || silence_reason(&a.stores.settings.get(), now).is_some() {
            return;
        }
        let (busy, visible) = {
            let c = a.cat.lock().unwrap();
            (c.is_busy(), c.visible)
        };
        if !busy && visible {
            self.speak(a, Trigger::Returned(away_for));
        }
    }

    /// The character speaks: a line from the local model when it is ready, otherwise canned advice.
    /// `Trigger::Asked` comes from the user and ignores quiet mode.
    pub fn speak(&self, a: &Arc<App>, trigger: Trigger) {
        let returned = matches!(trigger, Trigger::Returned(_));
        if !a.local_ai.ready() {
            if !returned {
                self.speak_now(a);
            }
            return;
        }
        if self.st.lock().unwrap().thinking {
            return;
        }
        let theme = self.next_canned(a);
        // Counseling lines and classics are worded with care; a small model paraphrasing them
        // turns them into guesses ("팀원한테 고마워했어?"), so they are said as written.
        let verbatim = verbatim_kind(theme.kind);
        if verbatim && !returned {
            self.show(a, entry_value(&theme));
            return;
        }
        {
            let mut st = self.st.lock().unwrap();
            if st.thinking {
                return;
            }
            st.thinking = true;
        }
        let moment = Moment {
            name: a.stores.settings.read(|s| s.s("catName")),
            now_ms: now_ms(),
            trigger: trigger.clone(),
            // A greeting on return stands alone rather than reworking a careful line.
            theme: (!verbatim).then(|| theme.text.clone()),
            recent: a.stores.advice_history.read(|h| h.strs("localRecent")),
        };
        let a2 = a.clone();
        tauri::async_runtime::spawn(async move {
            let said = tokio::time::timeout(THINK_LIMIT, a2.local_ai.speak(&moment)).await;
            let advisor = &a2.advisor;
            advisor.st.lock().unwrap().thinking = false;
            let asked = matches!(moment.trigger, Trigger::Asked);
            if !asked && silence_reason(&a2.stores.settings.get(), now_ms()).is_some() {
                return;
            }
            match said {
                Ok(Ok(text)) => {
                    a2.stores.advice_history.update(|h| {
                        let mut recent = h.strs("localRecent");
                        recent.push(text.clone());
                        let skip = recent.len().saturating_sub(RECENT_LINES);
                        h["localRecent"] = json!(recent[skip..].to_vec());
                    });
                    let name = moment.name.trim().to_owned();
                    let label = if name.is_empty() { "사이".to_owned() } else { name };
                    advisor.show(
                        &a2,
                        json!({ "id": "local", "kind": "character", "text": text, "sourceLabel": format!("— {label}") }),
                    );
                }
                // The model was slow or wrote something unusable: the theme line itself is still fine.
                _ if !returned => {
                    advisor.show(&a2, entry_value(&theme));
                }
                _ => {}
            }
        });
    }

    /// Picks the next canned line and records it, so themes rotate whether or not the model is used.
    fn next_canned(&self, a: &Arc<App>) -> Entry {
        let recent: Vec<String> = a.stores.advice_history.read(|h| h.strs("recent"));
        let e = pick_advice(&recent, &HashMap::new(), now_ms());
        let next = remember_shown(&recent, &e.id);
        a.stores.advice_history.update(|h| h["recent"] = json!(next));
        e
    }

    /// 사용자가 메뉴에서 직접 요청했을 때는 조용한 모드여도 말한다
    pub fn speak_now(&self, a: &Arc<App>) {
        let e = self.next_canned(a);
        self.show(a, entry_value(&e));
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
            json!({ "id": "update", "kind": "update", "text": format!("새 버전 {v} 준비됐어. 지금 다시 시작해서 적용할까?"), "sourceLabel": "— 사이 업데이트",
                    "actions": [act("update-later", "나중에", false), act("update-install", "지금 적용", true)] }),
        );
        true
    }

    /// 업데이트 확인 결과 같은 짧은 안내
    pub fn info(&self, a: &Arc<App>, text: &str) {
        cat::hide_bubble(&a.handle, a);
        self.show(a, json!({ "id": "info", "kind": "info", "text": text, "sourceLabel": "— 사이", "actions": [act("close", "확인", false)] }));
    }

    fn show(&self, a: &Arc<App>, entry: Value) -> bool {
        let id = {
            let mut st = self.st.lock().unwrap();
            st.seq += 1;
            let id = format!("b{}", st.seq);
            st.shown.insert(id.clone(), entry.clone());
            id
        };
        let actions = if entry["actions"].is_array() {
            entry["actions"].clone()
        } else {
            json!([
                act("later", "나중에", false),
                act("chat", "AI와 이야기", true)
            ])
        };
        let payload = json!({
            "id": id, "text": entry.s("text"), "sourceLabel": entry.s("sourceLabel"),
            "topic": entry["topic"], "kind": entry.s("kind"), "actions": actions,
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
            "chat" => open_chat(&a.handle, a, "chat"),
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
