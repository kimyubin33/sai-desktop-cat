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
const PROPOSAL_REPEAT_MS: i64 = 6 * 60 * 60 * 1000;

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
        Advisor { st: Mutex::new(St::default()) }
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
            if silence_reason(&a.stores.settings.get(), now).is_none() && !a.cat.lock().unwrap().is_busy() {
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
        if !self.maybe_show_update(a) && !self.maybe_show_proposal(a) && !self.maybe_show_progress(a) {
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
        let boosts = a.hub.boosts();
        let e = pick_advice(&recent, &boosts, now_ms());
        let next = remember_shown(&recent, &e.id);
        a.stores.advice_history.update(|h| h["recent"] = json!(next));
        self.show(a, entry_value(&e), None);
    }

    /// 아직 답하지 않은 팀 약속 제안이 있으면 (같은 제안은 6시간에 한 번만) 말풍선으로 묻는다
    fn maybe_show_proposal(&self, a: &Arc<App>) -> bool {
        if !a.hub.any_connected() {
            return false;
        }
        let shown = a.stores.team_cache.read(|c| c.obj("shownProposals"));
        let now = now_ms();
        let Some(p) = a.hub.pending_proposals_for_me().into_iter().find(|p| shown.get(&p.s("id")).and_then(|v| v.as_i64()).map(|t| now - t > PROPOSAL_REPEAT_MS).unwrap_or(true)) else {
            return false;
        };
        let pid = p.s("id");
        a.stores.team_cache.update(|c| {
            if !c["shownProposals"].is_object() {
                c["shownProposals"] = json!({});
            }
            c["shownProposals"][&pid] = json!(now);
        });
        let team = if p.s("teamName").is_empty() { String::new() } else { format!("{} · ", p.s("teamName")) };
        self.show(
            a,
            json!({ "id": format!("proposal-{pid}"), "kind": "proposal", "proposalId": pid, "text": format!("팀에 이런 약속을 맞춰 보자는 제안이 있어요. \"{}\"", p.s("text")),
                    "sourceLabel": format!("— {team}팀 약속 제안 · 지금 {}/{}명 동의", p.i("agree"), p.i("memberCount")) }),
            None,
        );
        true
    }

    /// 업무 진행 확인 (배정 확인, 시작, 막힘, 준비물, 마감) — 지금 보고 있는 팀부터
    pub fn maybe_show_progress(&self, a: &Arc<App>) -> bool {
        if !a.hub.any_connected() {
            return false;
        }
        let active = a.hub.active();
        let mut teams = vec![active.clone()];
        teams.extend(a.hub.all().into_iter().filter(|t| t.id != active.id));
        let multi = a.hub.teams().len() > 1;
        for t in teams.into_iter().filter(|t| t.id.is_some()) {
            let Some(n) = t.nudge() else { continue };
            t.mark_nudged(&n.s("key"));
            // 자리를 비웠으면 말풍선 대신 Telegram 으로 (연결했고, 설정에서 켜 둔 경우)
            if crate::system::is_away() && a.stores.settings.read(|s| s.b("telegramAway")) {
                let linked = if t.tg_is_linked() { Some(t.clone()) } else { a.hub.all().into_iter().find(|x| x.tg_is_linked()) };
                if let Some(target) = linked {
                    let project = t.active_project().map(|p| p.s("title")).unwrap_or_default();
                    let text = n.s("text");
                    tauri::async_runtime::spawn(async move {
                        target.tg_nudge_me(&text, &project).await;
                    });
                    return true;
                }
            }
            let source = if multi { format!("{} · {}", t.name(), n.s("source")) } else { n.s("source") };
            self.show(
                a,
                json!({ "id": format!("progress-{}", n.s("key")), "kind": "progress", "taskId": n["taskId"], "teamId": t.id, "actions": n["actions"], "text": n.s("text"), "sourceLabel": format!("— {source}") }),
                None,
            );
            return true;
        }
        false
    }

    // ---------- 업데이트 ----------
    /// 새 버전을 다 받았을 때. 조용한 시간이 아니면 바로, 아니면 다음 말할 때 묻는다
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
        let Some(v) = self.st.lock().unwrap().pending_update.clone() else { return false };
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

    /// 대화창 등 다른 곳에서 고양이가 직접 한마디 하게 할 때
    pub fn say(&self, a: &Arc<App>, text: &str) {
        self.show(a, json!({ "id": "say", "kind": "system", "text": text }), None);
    }

    fn actions_for(&self, a: &Arc<App>, e: &Value) -> Value {
        if e["actions"].is_array() {
            return e["actions"].clone();
        }
        match e.s("kind").as_str() {
            "proposal" => json!([act("later", "나중에", false), act("agree", "좋아요", true)]),
            "system" => json!([act("later", "나중에", false), act("chat", "이야기하기", true)]),
            kind => {
                let mut v = vec![act("later", "나중에", false)];
                // 받은 조언인지 아닌지 드러나지 않도록, 팀에 연결되어 있으면 모든 협업 조언에 같은 버튼을 단다
                if kind == "teamwork" && a.hub.any_connected() {
                    v.push(act("mine", "내 사정도 있어요", false));
                }
                v.push(act("chat", "이야기하기", true));
                json!(v)
            }
        }
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
        let Some(entry) = self.st.lock().unwrap().shown.remove(id) else { return };
        if entry.s("id") == "greet" {
            a.stores.settings.update(|s| s["onboarded"] = json!(true));
        }
        let task_id = entry.s("taskId");
        let team = a.hub.team(entry.os("teamId").as_deref()).filter(|t| t.id.is_some()).or_else(|| a.hub.find_task_team(&task_id));
        match action {
            "later" => self.st.lock().unwrap().later = Some((now_ms() + LATER_MS, entry)),
            "mine" => {
                if let Some(t) = entry.os("topic") {
                    open_chat(&a.handle, a, "chat", json!({ "explain": t }));
                }
            }
            "agree" => {
                let (a2, pid) = (a.clone(), entry.s("proposalId"));
                tauri::async_runtime::spawn(async move {
                    let _ = a2.hub.respond(&pid, "agree").await;
                });
            }
            "chat" => open_chat(&a.handle, a, "chat", json!({})),
            "update-install" => {
                self.st.lock().unwrap().pending_update = None;
                a.updater.install(a);
            }
            // 다시 묻지 않는다
            "update-later" => self.st.lock().unwrap().pending_update = None,
            "open-collab" | "task-object" => open_chat(&a.handle, a, "collab", if task_id.is_empty() { json!({}) } else { json!({ "task": task_id }) }),
            "task-accept" | "task-start" | "task-blocked" | "task-done" => {
                let Some(t) = team else { return };
                let (a2, act) = (a.clone(), action.to_string());
                tauri::async_runtime::spawn(async move {
                    let (r, thanks) = match act.as_str() {
                        "task-accept" => (t.accept(&task_id).await, "고마워요. 시작할 수 있을 때 알려 드릴게요."),
                        "task-start" => (t.set_status(&task_id, "doing").await, "좋아요. 막히면 언제든 말해 주세요."),
                        "task-blocked" => (t.set_status(&task_id, "blocked").await, "팀에 도움이 필요하다고 알려 둘게요. 어디서 막혔는지 협업 탭에 적어 두면 더 빨라요."),
                        _ => (t.set_status(&task_id, "done").await, "수고했어요! 완료로 표시해 둘게요."),
                    };
                    tokio::time::sleep(Duration::from_millis(400)).await;
                    match r {
                        Ok(()) => {
                            a2.advisor.say(&a2, thanks);
                            // 맡은 일이 바뀌면 시간표도 다시 짠다
                            a2.schedule.view(false).await;
                        }
                        Err(e) => a2.advisor.say(&a2, &format!("처리하지 못했어요: {e}")),
                    }
                });
            }
            _ => {}
        }
    }
}
