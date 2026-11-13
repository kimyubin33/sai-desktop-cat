//! 개인 대화 세션을 관리한다. 대화 원문은 private/conversations.json 에만 저장된다.
use crate::ai::Ai;
use crate::counseling::{build_summary, create_session, handle_user, mode_label, Reply, Session, Turn};
use crate::store::Store;
use crate::time::{now_ms, DAY};
use crate::util::{arr_mut, J};
use crate::{err, Result};
use serde_json::{json, Value};
use std::sync::Arc;

#[derive(Clone)]
pub struct Chat {
    conv: Store,
    settings: Store,
    ai: Arc<Ai>,
}

impl Chat {
    pub fn new(conv: Store, settings: Store, ai: Arc<Ai>) -> Chat {
        let c = Chat { conv, settings, ai };
        c.purge_old();
        c
    }

    fn sessions(&self) -> Vec<Session> {
        self.conv.read(|d| d.arr("sessions").into_iter().filter_map(|s| serde_json::from_value(s).ok()).collect())
    }

    pub fn find(&self, id: &str) -> Option<Session> {
        self.sessions().into_iter().find(|s| s.id == id)
    }

    fn save(&self, s: &Session) {
        let v = serde_json::to_value(s).unwrap();
        self.conv.update(|d| {
            let list = arr_mut(d, "sessions");
            match list.iter().position(|x| x.s("id") == s.id) {
                Some(i) => list[i] = v,
                None => list.push(v),
            }
        });
    }

    pub fn purge_old(&self) {
        let days = self.settings.read(|s| s.i("retentionDays"));
        if days <= 0 {
            return;
        }
        let cutoff = now_ms() - days * DAY;
        self.conv.update(|d| {
            arr_mut(d, "sessions").retain(|s| {
                let last = s.arr("turns").last().map(|t| t.i("at")).unwrap_or_else(|| s.i("startedAt"));
                last >= cutoff
            })
        });
    }

    async fn polish(&self, s: &mut Session, replies: Vec<Reply>) -> Vec<Reply> {
        let mut out = vec![];
        for r in replies {
            let Some(intent) = r.intent.clone().filter(|_| !r.fixed) else {
                out.push(r);
                continue;
            };
            match self.ai.polish_reply(s, &intent, &r.text).await {
                Some(better) => {
                    // 세션 기록의 마지막 고양이 말도 다듬은 문장으로 바꾼다
                    if let Some(t) = s.turns.iter_mut().rev().find(|t| t.role == "cat" && t.text == r.text) {
                        t.text = better.clone();
                    }
                    let mut nr = r.clone();
                    nr.text = better;
                    nr.ai = true;
                    out.push(nr);
                }
                None => out.push(r),
            }
        }
        out
    }

    pub async fn start(&self, mode: &str, topic: Option<&str>) -> Value {
        let (mut s, replies) = create_session(mode, topic, now_ms());
        let polished = self.polish(&mut s, replies).await;
        self.save(&s);
        json!({ "session": public_view(&s, false), "replies": polished })
    }

    pub async fn send(&self, id: &str, text: &str, skip: bool, on_text: Option<&(dyn Fn(&str) + Send + Sync)>) -> Result<Value> {
        let Some(mut s) = self.find(id) else { return err("대화를 찾을 수 없어요.") };
        let before = s.turns.len();
        let h = handle_user(&mut s, text, now_ms(), skip);
        let new_summary = h.summary.clone();
        let safety = h.safety.clone();
        let replies: Vec<Reply> = if h.safety.is_some() {
            h.replies
        } else if s.mode == "free" && self.ai.has_key() {
            // 자유 대화는 정해진 흐름 대신 Claude 와 그대로 이야기한다 (안전 확인은 위에서 먼저 한다)
            let added: Vec<Turn> = s.turns.split_off(before);
            s.turns.extend(added.into_iter().filter(|t| t.role == "user"));
            let noop = |_: &str| {};
            let cb: &(dyn Fn(&str) + Send + Sync) = on_text.unwrap_or(&noop);
            match self.ai.free_chat(&s, cb).await {
                Some(answer) => {
                    s.turns.push(Turn { role: "cat".into(), text: answer.clone(), at: now_ms(), ai: true, ..Default::default() });
                    let mut r = Reply::cat(answer);
                    r.ai = true;
                    r.streamed = on_text.is_some();
                    vec![r]
                }
                None => {
                    let why = self.ai.last_error().map(|e| format!(" (AI 연결 오류: {})", e.chars().take(120).collect::<String>())).unwrap_or_default();
                    let mut rs = h.replies;
                    for r in &rs {
                        s.turns.push(Turn { role: "cat".into(), text: r.text.clone(), at: now_ms(), ..Default::default() });
                    }
                    if let Some(last) = rs.last_mut() {
                        last.text.push_str(&why);
                    }
                    rs
                }
            }
        } else {
            self.polish(&mut s, h.replies).await
        };
        if let Some(sm) = new_summary.clone() {
            s.summary = Some(self.ai.enrich_summary(&s, sm).await);
        }
        self.save(&s);
        Ok(json!({
            "session": public_view(&s, false),
            "replies": replies,
            "summary": if new_summary.is_some() { s.summary.clone() } else { None },
            "safety": safety,
        }))
    }

    /// 사용자가 정리 카드를 직접 고친 경우
    pub fn update_summary(&self, id: &str, patch: &Value) -> Value {
        let Some(mut s) = self.find(id) else { return Value::Null };
        let Some(mut sm) = s.summary.clone() else { return Value::Null };
        for k in ["reportedEvent", "agreement", "impact", "needs", "actionRequests", "toConfirm", "topics", "situation", "reason", "nextTime"] {
            if let Some(v) = patch.get(k) {
                sm[k] = v.clone();
            }
        }
        sm["editedByUser"] = json!(true);
        s.summary = Some(sm.clone());
        self.save(&s);
        sm
    }

    pub fn summarize_now(&self, id: &str) -> Value {
        let Some(mut s) = self.find(id) else { return Value::Null };
        if s.mode == "free" {
            return Value::Null;
        }
        s.step = "done".into();
        let sm = build_summary(&s);
        s.summary = Some(sm.clone());
        self.save(&s);
        sm
    }

    pub fn list(&self) -> Value {
        let mut v: Vec<Session> = self.sessions();
        v.sort_by(|a, b| b.started_at.cmp(&a.started_at));
        json!(v
            .iter()
            .map(|s| json!({
                "id": s.id, "mode": s.mode, "modeLabel": mode_label(&s.mode), "startedAt": s.started_at,
                "preview": s.turns.iter().find(|t| t.role == "user").map(|t| t.text.chars().take(40).collect::<String>()).unwrap_or_else(|| "(아직 이야기 없음)".into()),
                "hasSummary": s.summary.is_some(), "safetyHold": s.safety_hold.is_some(),
            }))
            .collect::<Vec<_>>())
    }

    pub fn get(&self, id: &str) -> Value {
        self.find(id).map(|s| public_view(&s, true)).unwrap_or(Value::Null)
    }

    pub fn delete(&self, id: &str) {
        self.conv.update(|d| arr_mut(d, "sessions").retain(|s| s.s("id") != id));
    }

    pub fn delete_all(&self) {
        self.conv.update(|d| d["sessions"] = json!([]));
    }
}

pub fn public_view(s: &Session, with_turns: bool) -> Value {
    let mut v = json!({ "id": s.id, "mode": s.mode, "step": s.step, "startedAt": s.started_at, "summary": s.summary, "safetyHold": s.safety_hold });
    if with_turns {
        v["turns"] = serde_json::to_value(&s.turns).unwrap();
    }
    v
}
