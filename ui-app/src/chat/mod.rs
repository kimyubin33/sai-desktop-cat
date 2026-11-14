//! 대화창: 대화 · 정리함 · 협업 · 설정 탭.
mod collab;
mod drive;
mod ext;
mod notes;
pub mod schedule;
mod settings;
mod talk;

use crate::api::{call, listen};
use crate::dom::*;
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;
use wasm_bindgen::JsCast;
use web_sys::{HtmlCanvasElement, HtmlElement};

#[derive(Default)]
pub struct State {
    pub topics: Vec<Value>,
    pub settings: Value,
    /// 지금 대화 { id, mode, step }
    pub session: Option<Value>,
    pub busy: bool,
    pub queue: VecDeque<String>,
    pub stream_seq: u32,
    pub streaming: Option<(String, HtmlElement)>,
    pub talking_until: f64,
    pub sched_card: Option<HtmlElement>,
    pub active_team_name: String,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

/// 상태 읽고 쓰기 (await 를 걸치지 말 것)
pub fn st<R>(f: impl FnOnce(&mut State) -> R) -> R {
    STATE.with(|s| f(&mut s.borrow_mut()))
}

pub fn s(v: &Value, k: &str) -> String {
    match &v[k] {
        Value::String(x) => x.clone(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

pub fn n(v: &Value, k: &str) -> f64 {
    v[k].as_f64().or_else(|| v[k].as_str().and_then(|x| x.parse().ok())).unwrap_or(0.0)
}

pub fn b(v: &Value, k: &str) -> bool {
    match &v[k] {
        Value::Bool(x) => *x,
        Value::Null => false,
        Value::String(x) => !x.is_empty(),
        Value::Number(x) => x.as_f64().unwrap_or(0.0) != 0.0,
        _ => true,
    }
}

pub fn arr(v: &Value, k: &str) -> Vec<Value> {
    v[k].as_array().cloned().unwrap_or_default()
}

/// 숫자를 "3" 또는 "2.5" 처럼
pub fn num_text(x: f64) -> String {
    let r = (x * 10.0).round() / 10.0;
    if (r - r.round()).abs() < 1e-9 {
        format!("{}", r.round() as i64)
    } else {
        format!("{r:.1}")
    }
}

fn two(n: u32) -> String {
    format!("{n:02}")
}

/// "11월 14일 오전 07:55"
pub fn fmt_date(ms: f64) -> String {
    let d = js_sys::Date::new(&ms.into());
    let h = d.get_hours();
    format!("{}월 {}일 {} {}:{}", d.get_month() + 1, d.get_date(), if h < 12 { "오전" } else { "오후" }, two(if h % 12 == 0 { 12 } else { h % 12 }), two(d.get_minutes()))
}

/// "오전 07:55"
pub fn fmt_clock(ms: f64) -> String {
    let d = js_sys::Date::new(&ms.into());
    let h = d.get_hours();
    format!("{} {}:{}", if h < 12 { "오전" } else { "오후" }, two(if h % 12 == 0 { 12 } else { h % 12 }), two(d.get_minutes()))
}

pub fn topic_label(id: &str) -> String {
    st(|s| s.topics.iter().find(|t| t["id"] == id).map(|t| self::s(t, "label"))).unwrap_or_else(|| id.to_string())
}

// ---------- 탭 ----------
pub fn show_tab(tab: &str) {
    for b in q_all(".tabs button") {
        toggle(&b, "active", b.get_attribute("data-tab").as_deref() == Some(tab));
    }
    for t in q_all(".tab") {
        toggle(&t, "active", t.id() == format!("tab-{tab}"));
    }
    match tab {
        "notes" => spawn(notes::render()),
        "collab" => collab::render(true),
        "drive" => drive::render(),
        "settings" => {
            spawn(settings::load());
            spawn(settings::render_team(None));
            spawn(ext::render());
            spawn(settings::render_update(None));
        }
        _ => {}
    }
}

pub fn current_tab() -> String {
    q(".tabs button.active").and_then(|b| b.get_attribute("data-tab")).unwrap_or_else(|| "chat".into())
}

// ---------- 머리말 ----------
pub fn update_engine(s: &Value) {
    set_text("catName", &self::s(s, "catName"));
    set_text(
        "engine",
        if b(s, "hasApiKey") {
            if b(s, "usingSharedKey") {
                "Claude 연결됨 (팀 공유 키)"
            } else {
                "Claude 연결됨"
            }
        } else {
            "기본 상담 흐름"
        },
    );
    settings::render_team_key(s);
}

/// 머리말의 팀 고르기 (팀이 둘 이상일 때)
pub fn refresh_team_switch(info: &Value) {
    drive::switch_team(&s(info, "activeTeamId"));
    let Some(sel) = by_id("teamSwitch") else { return };
    clear(&sel);
    let teams = arr(info, "teams");
    for t in &teams {
        let _ = sel.append_child(&el("option").attr("value", &s(t, "teamId")).text(&s(t, "name")).flag("selected", b(t, "active")).0);
    }
    toggle(&sel, "hidden", teams.len() < 2);
}

pub fn start() {
    // 아바타
    let avatar: HtmlCanvasElement = by_id("avatar").unwrap().unchecked_into();
    let mut frame = 0u32;
    every(220, move || {
        frame += 1;
        let talking = now() < st(|s| s.talking_until) || st(|s| s.busy);
        crate::sprite::render(&avatar, if talking { "talk" } else { "sit" }, frame, -1, frame % 9 != 0);
    });

    // 탭
    if let Some(tabs) = q(".tabs") {
        on(&tabs, "click", |e| {
            if let Some(t) = event_target(&e) {
                if let Ok(Some(b)) = t.closest("button[data-tab]") {
                    show_tab(&b.get_attribute("data-tab").unwrap_or_default());
                }
            }
        });
    }
    listen("chat:show-tab", |v| show_tab(v.as_str().unwrap_or("chat")));
    let opened_from_peek = Rc::new(Cell::new(false));
    let opened = opened_from_peek.clone();
    listen("chat:team-talk", move |notice| {
        let Some(team) = notice.as_str().or_else(|| notice["teamId"].as_str()) else { return };
        opened.set(true);
        drive::switch_team(team);
        if let Some(select) = by_id("teamSwitch") {
            set_val(&select, team);
        }
        drive::select_talk();
        show_tab("drive");
        let team = team.to_owned();
        spawn(async move {
            if let Ok(info) = call("team:status", Value::Null).await {
                if drive::viewing(&team) && s(&info, "activeTeamId") == team {
                    refresh_team_switch(&info);
                }
            }
        });
    });
    listen("chat:start-explain", |v| {
        show_tab("chat");
        let t = v.as_str().map(String::from);
        spawn(talk::start_mode("explain".into(), t));
    });
    listen("chat:focus-task", |v| {
        collab::focus(v.as_str().unwrap_or(""));
        show_tab("collab");
    });

    if let Some(sel) = by_id("teamSwitch") {
        let sel2 = sel.clone();
        on(&sel, "change", move |_| {
            let id = val(&sel2);
            drive::switch_team(&id);
            spawn(async move {
                let info = match call("team:switch", json!(id)).await {
                    Ok(info) => Ok(info),
                    Err(_) => call("team:status", Value::Null).await,
                };
                if let Ok(info) = info {
                    if drive::viewing(&id) {
                        refresh_team_switch(&info);
                        show_tab(&current_tab());
                    }
                }
            });
        });
    }

    talk::bind();
    drive::bind();
    settings::bind();

    spawn(async move {
        let topics = call("topics:list", Value::Null).await.ok().and_then(|v| v.as_array().cloned()).unwrap_or_default();
        st(|s| s.topics = topics);
        if let Ok(set) = call("settings:get", Value::Null).await {
            st(|s| s.settings = set.clone());
            update_engine(&set);
        }
        if let Ok(info) = call("team:status", Value::Null).await {
            if !opened_from_peek.get() {
                refresh_team_switch(&info);
            }
        }
        if opened_from_peek.get() {
            return;
        }
        if let Some(t) = query_param("task") {
            collab::focus(&t);
        }
        show_tab(&query_param("tab").unwrap_or_else(|| "chat".into()));
        if let Some(t) = query_param("explain") {
            talk::start_mode("explain".into(), Some(t)).await;
        }
    });

    // 개발용: 앱의 시연 기능이 화면을 움직일 때 쓴다
    talk::expose_demo();
}
