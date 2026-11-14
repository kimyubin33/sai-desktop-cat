//! 새 팀 톡을 바탕화면 오른쪽 위에 잠깐 표시한다. 창이 닫힌 상태에서도 각 팀을 확인한다.
use crate::app::{open_chat, App};
use sai_core::advice::silence_reason;
use sai_core::time::now_ms;
use sai_core::util::J;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

const WIDTH: f64 = 346.0;
const HEIGHT: f64 = 102.0;
const DISPLAY: Duration = Duration::from_millis(8500);
const QUEUE_LIMIT: usize = 32;
const CLOCK_SKEW_MS: i64 = 2 * 60_000;

#[derive(Default)]
struct Cursor {
    // 시각은 보낸 컴퓨터의 시계다. 순서가 뒤바뀌어 도착해도 새 문서 ID를 기준으로 판정한다.
    known: HashSet<String>,
    last_poll: Option<i64>,
    last_since: Option<i64>,
    oldest: Option<(i64, String)>,
}

impl Cursor {
    fn since(&self, now: i64) -> Option<i64> {
        self.last_poll.map(|at| now.min(at).saturating_sub(CLOCK_SKEW_MS).max(self.last_since.unwrap_or(i64::MIN)))
    }

    fn accept(&mut self, rows: &[Value], my_uid: &str, poll_started: i64) -> Vec<Value> {
        let needs_baseline = self.last_poll.is_none();
        let oldest = rows.first().map(|r| (r.i("at"), r.s("id")));
        let mut new = Vec::new();
        for row in rows {
            let id = row.s("id");
            // 최근 목록에서 문서를 지우면 맨 앞에 오래된 기록이 다시 들어올 수 있다.
            if !id.is_empty() && !self.known.contains(&id) && !needs_baseline
                && self.oldest.as_ref().map_or(true, |(at, key)| (row.i("at"), id.as_str()) >= (*at, key.as_str()))
                && !row.s("by").is_empty() && row.s("by") != my_uid && row.s("url").is_empty() && !row.s("text").trim().is_empty()
            {
                new.push(row.clone());
            }
        }
        self.known = rows.iter().map(|r| r.s("id")).collect();
        self.oldest = if rows.len() == sai_core::team::backend::RECENT_SHARE_LIMIT { oldest } else { None };
        self.last_since = self.since(poll_started);
        self.last_poll = Some(poll_started);
        new
    }
}

#[derive(Default)]
struct DisplayState {
    ready: bool,
    pending: VecDeque<Value>,
    current: Option<(Value, Instant)>,
}

#[derive(Default)]
pub struct Peeks(Mutex<DisplayState>);

fn allowed(a: &App) -> bool {
    a.cat.lock().unwrap().visible && a.stores.settings.read(|s| silence_reason(s, now_ms()).is_none())
}

pub fn start(a: &Arc<App>) {
    let poller = a.clone();
    tauri::async_runtime::spawn(async move {
        let mut cursors: HashMap<String, Cursor> = HashMap::new();
        loop {
            let teams = poller.hub.all();
            cursors.retain(|id, _| teams.iter().any(|t| t.id.as_ref() == Some(id)));
            for team in teams {
                let (Some(id), Ok(backend)) = (team.id.as_ref(), team.backend()) else { continue };
                let cursor = cursors.entry(id.clone()).or_default();
                // 발신자의 시계가 앞서 있어도 다른 팀원의 톡이 밀려나지 않도록 수신 기기의 시각을 쓴다.
                let started = now_ms();
                let since = cursor.since(started);
                let Ok(Ok(rows)) = tokio::time::timeout(Duration::from_secs(10), backend.recent_shares(id, since)).await else { continue };
                let Some(uid) = backend.session().and_then(|s| s.os("uid")).filter(|s| !s.is_empty()) else { continue };
                let new = cursor.accept(&rows, &uid, started);
                if new.is_empty() || !team.is_connected() || !allowed(&poller) {
                    continue;
                }
                let members = team.members(true);
                let team_name = team.name();
                let mut state = poller.peeks.0.lock().unwrap();
                for msg in new {
                    if state.pending.len() >= QUEUE_LIMIT {
                        state.pending.pop_front();
                    }
                    let sender = members.iter().find(|m| m["uid"] == msg["by"]).and_then(|m| m.os("displayName")).unwrap_or_else(|| "팀원".into());
                    state.pending.push_back(json!({ "id": msg["id"], "teamId": id, "teamName": team_name, "byName": sender, "text": msg["text"] }));
                }
            }
            tokio::time::sleep(Duration::from_secs(8)).await;
        }
    });
    let display = a.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            tick(&display);
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    });
}

fn place(a: &App, w: &WebviewWindow) {
    let monitor = a.handle.cursor_position().ok().and_then(|p| a.handle.monitor_from_point(p.x, p.y).ok().flatten())
        .or_else(|| a.handle.primary_monitor().ok().flatten());
    if let Some(m) = monitor {
        let area = m.work_area();
        let width = ((WIDTH * m.scale_factor()).round() as u32).min(area.size.width);
        let height = ((HEIGHT * m.scale_factor()).round() as u32).min(area.size.height);
        let _ = w.set_position(PhysicalPosition::new(area.position.x + (area.size.width - width) as i32, area.position.y));
        let _ = w.set_size(PhysicalSize::new(width, height));
    }
}

fn tick(a: &Arc<App>) {
    let mut s = a.peeks.0.lock().unwrap();
    if s.current.is_none() && s.pending.is_empty() {
        return;
    }
    let w = a.handle.get_webview_window("peek");
    if !allowed(a) {
        s.pending.clear();
        s.current = None;
        if let Some(w) = w { let _ = w.hide(); }
        return;
    }
    if let Some((notice, started)) = s.current.as_ref() {
        if started.elapsed() < DISPLAY && a.hub.team(notice["teamId"].as_str()).is_some() {
            return;
        }
        s.current = None;
        if let Some(w) = &w { let _ = w.hide(); }
    }
    // 고양이/대화창과 별개인 작은 투명 창. 첫 톡을 받을 때만 만든다.
    if s.pending.is_empty() { return; }
    let Some(w) = w else {
        s.ready = false;
        drop(s);
        let _ = WebviewWindowBuilder::new(&a.handle, "peek", WebviewUrl::App("peek/index.html".into()))
            .title("사이 팀 톡").inner_size(WIDTH, HEIGHT).transparent(true).decorations(false)
            .resizable(false).skip_taskbar(true).shadow(false).always_on_top(true)
            .visible_on_all_workspaces(true).focusable(false).focused(false).visible(false).build();
        return;
    };
    if !s.ready { return; }
    while let Some(notice) = s.pending.pop_front() {
        if a.hub.team(notice["teamId"].as_str()).is_none() { continue; }
        place(a, &w);
        if w.emit_to("peek", "peek:message", &notice).is_ok() && w.show().is_ok() {
            s.current = Some((notice, Instant::now()));
        }
        break;
    }
}

// peek 창에서 호출할 수 있는 명령만 분리한다. 버튼이 가리킨 알림과 현재 알림이 같아야 한다.
pub fn handle(a: &Arc<App>, ch: &str, args: &Value) -> Result<Value, String> {
    let mut state = a.peeks.0.lock().unwrap();
    if ch == "peek:ready" {
        state.ready = true;
        return Ok(state.current.as_ref().map(|(n, _)| n.clone()).unwrap_or(Value::Null));
    }
    if ch != "peek:open" && ch != "peek:dismiss" { return Err("허용되지 않은 요청이에요.".into()); }
    let Some((notice, _)) = &state.current else { return Ok(Value::Null) };
    if notice["teamId"] != args["teamId"] || notice["id"] != args["id"] { return Ok(Value::Null); }
    let id = notice.s("teamId");
    state.current = None;
    if let Some(w) = a.handle.get_webview_window("peek") { let _ = w.hide(); }
    drop(state);
    if ch == "peek:open" {
        a.hub.set_active(&id)?;
        open_chat(&a.handle, a, "drive", json!({ "teamTalk": id }));
    }
    Ok(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn talk(id: &str, at: i64, from: &str) -> Value {
        json!({ "id": id, "at": at, "by": from, "text": id })
    }

    #[test]
    fn baseline_is_silent_then_distinguishes_text_sender_and_ids_even_with_clock_skew() {
        let history = talk("기존", 100_000, "other");
        let mut c = Cursor::default();
        assert!(c.accept(&[history.clone()], "me", 100_000).is_empty());
        let mut file = talk("설명 포함 첨부", 50_000, "other");
        file["url"] = json!("https://drive.google.com/a");
        let a = talk("팀원 1", 50_000, "other");
        let b = talk("팀원 2", 50_000, "other");
        let rows = vec![a.clone(), file, talk("내 메시지", 50_000, "me"), b.clone(), history];
        assert_eq!(c.accept(&rows, "me", 100_000), vec![a, b]);
        assert!(c.accept(&rows, "me", 100_000).is_empty());
    }

    #[test]
    fn deletion_from_full_window_does_not_announce_old_history() {
        let mut c = Cursor::default();
        let rows: Vec<_> = (1..=sai_core::team::backend::RECENT_SHARE_LIMIT).map(|i| talk(&i.to_string(), i as i64, "other")).collect();
        assert!(c.accept(&rows, "me", 100_000).is_empty());
        let mut after = rows;
        after.pop();
        after.insert(0, talk("older", 0, "other"));
        assert!(c.accept(&after, "me", 100_000).is_empty());
    }
}
