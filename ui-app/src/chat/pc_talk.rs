use super::{s, show_tab};
use crate::{api::call, dom::*};
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
};
use wasm_bindgen::JsCast;

#[derive(Default)]
struct State {
    team: String,
    feed: Value,
    offset: usize,
    poll: i32,
    drafts: BTreeMap<String, (String, Option<String>)>,
    deletes: BTreeMap<String, String>,
}
thread_local! {
    static STATE:RefCell<State>=RefCell::new(State::default());
    static GENERATION:Cell<u64>=const{Cell::new(0)};
    static READING:Cell<bool>=const{Cell::new(false)};
    static WRITING:Cell<bool>=const{Cell::new(false)};
}
fn state<T>(f: impl FnOnce(&mut State) -> T) -> T {
    STATE.with(|s| f(&mut s.borrow_mut()))
}
fn team() -> String {
    state(|s| s.team.clone())
}
fn input() -> String {
    by_id("pcTalkText").map(|e| val(&e)).unwrap_or_default()
}
fn consent() -> bool {
    by_id("pcTalkConsent").is_some_and(|e| checked(&e))
}
fn message(text: &str) {
    set_text("pcTalkMessage", text);
}
fn short(id: &str) -> String {
    id.chars().take(12).collect()
}
fn controls() {
    let busy = WRITING.with(Cell::get);
    let feed = state(|s| s.feed.clone());
    let write = feed["canWrite"] == true && consent();
    for (id, disabled) in [
        ("pcTalkTeam", busy),
        ("pcTalkSend", busy || !write || input().trim().is_empty()),
        ("pcTalkConsent", busy || feed["canWrite"] != true),
        ("pcTalkRefresh", busy || team().is_empty()),
        ("pcTalkOlder", busy || !feed["nextOffset"].is_number()),
        ("pcTalkNewer", busy || state(|s| s.offset) == 0),
    ] {
        if let Some(e) = by_id(id) {
            set_disabled(&e, disabled);
        }
    }
    if let Some(list) = by_id("pcTalkFeed") {
        for b in q_all_in(&list, "button[data-delete]") {
            set_disabled(&b, busy || !write);
        }
    }
}
fn remember() {
    let text = input();
    let team = team();
    if team.is_empty() {
        return;
    }
    state(|s| {
        let entry = s.drafts.entry(team).or_default();
        if entry.0 != text {
            *entry = (text, None);
        }
    });
}
fn change_team(next: String) {
    if team() == next {
        return;
    }
    remember();
    let text = state(|s| {
        s.team = next.clone();
        s.feed = Value::Null;
        s.offset = 0;
        s.drafts.get(&next).map(|d| d.0.clone()).unwrap_or_default()
    });
    GENERATION.with(|g| g.set(g.get() + 1));
    if let Some(e) = by_id("pcTalkConsent") {
        set_checked(&e, false);
    }
    if let Some(e) = by_id("pcTalkText") {
        set_val(&e, &text);
    }
    if let Some(e) = by_id("pcTalkFeed") {
        clear(&e);
    }
    set_text("pcTalkAudience", "");
    set_text("pcTalkConnection", "");
    message("");
    controls();
    spawn(refresh(0));
}
pub(super) fn render() {
    spawn(load_teams());
    let old = state(|s| s.poll);
    stop(old);
    let id = every(3000, || {
        if super::current_tab() != "drive" || !super::drive::pc_selected() {
            stop(state(|s| s.poll));
            return;
        }
        spawn(refresh(state(|s| s.offset)));
    });
    state(|s| s.poll = id);
}
async fn load_teams() {
    let generation = GENERATION.with(Cell::get);
    let result = call("mesh-talk:teams", Value::Null).await;
    if generation != GENERATION.with(Cell::get) {
        return;
    }
    match result {
        Ok(rows) => {
            let previous = team();
            let list = rows.as_array().cloned().unwrap_or_default();
            let next = list
                .iter()
                .find(|r| r["teamId"] == previous)
                .or_else(|| list.first())
                .map(|r| s(r, "teamId"))
                .unwrap_or_default();
            if let Some(select) = by_id("pcTalkTeam") {
                clear(&select);
                let _ = select.append_child(&el("option").attr("value", "").text("PC 팀 선택").0);
                for row in &list {
                    let id = s(row, "teamId");
                    let _ = select.append_child(
                        &el("option")
                            .attr("value", &id)
                            .flag("selected", id == next)
                            .text(&format!("{} · {}", s(row, "name"), short(&id)))
                            .0,
                    );
                }
            }
            if next.is_empty() {
                change_team(String::new());
                message("설정에서 PC 팀 장치와 관리자 승인 정보를 먼저 등록해 주세요.");
                controls();
            } else if next == previous {
                spawn(refresh(state(|s| s.offset)));
            } else {
                change_team(next);
            }
        }
        Err(error) => message(&error),
    }
}
async fn refresh(offset: usize) {
    let selected = team();
    if selected.is_empty() || WRITING.with(Cell::get) || READING.with(|b| b.replace(true)) {
        return;
    }
    let generation = GENERATION.with(Cell::get);
    let result = call("mesh-talk:feed", json!({"teamId":selected,"offset":offset})).await;
    READING.with(|b| b.set(false));
    if generation != GENERATION.with(Cell::get) {
        spawn_refresh();
        return;
    }
    match result {
        Ok(feed) => {
            let changed = state(|s| s.feed["scope"] != feed["scope"]);
            if changed {
                if let Some(e) = by_id("pcTalkConsent") {
                    set_checked(&e, false);
                }
            }
            let redraw = state(|s| s.feed["messages"] != feed["messages"] || s.offset != offset);
            state(|s| {
                s.feed = feed.clone();
                s.offset = offset;
            });
            draw_info(&feed);
            if redraw {
                draw_messages(&feed);
            }
            controls();
        }
        Err(error) => {
            state(|s| s.feed = Value::Null);
            if let Some(e) = by_id("pcTalkFeed") {
                clear(&e);
            }
            if let Some(e) = by_id("pcTalkConsent") {
                set_checked(&e, false);
            }
            message(&error);
            controls();
        }
    }
}
fn spawn_refresh() {
    spawn(refresh(state(|s| s.offset)));
}
fn label(state: &str) -> &'static str {
    match state {
        "local-only" => "다른 수신 장치 없이 이 PC에 저장",
        "local-pending" => "이 PC 저장 완료 · 상대 저장 확인 대기",
        "peer-stored" => "대상 PC들의 저장 확인 완료",
        "reference-stored" => "대상 PC들의 삭제 요청 저장 확인 완료",
        "approval-held" => "승인 범위 변경으로 일부 전달 보류",
        "legacy-held" => "이전 준비 기록 · 전송 보류",
        "received" => "이 PC 수신·저장 완료",
        "header-only" => "서명된 삭제 요청 확인",
        _ => "기록 상태 확인 중",
    }
}
fn draw_info(feed: &Value) {
    let devices = feed["activeDevices"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let own = s(&feed["scope"], "deviceId");
    let others = devices
        .iter()
        .filter_map(Value::as_str)
        .filter(|id| *id != own)
        .map(short)
        .collect::<Vec<_>>();
    set_text("pcTalkAudience",&format!("{} · 승인 이력 {} · 내 장치 {} · 대상 장치 {}대{} · 본문은 작성 시점의 승인 장치에만 전달해요.",s(feed,"teamName"),s(&feed["scope"],"revision"),short(&own),others.len(),if others.is_empty(){String::new()}else{format!(" ({})",others.join(", "))}));
    let connection = &feed["connection"];
    let active = connection["running"] == true
        && connection["teamId"] == feed["teamId"]
        && connection["exchangeEnabled"] == true;
    set_text("pcTalkConnection",&format!("{} · 발신 대기·보류 {}개 · 충돌 {}개 · 보관 원본 {}개 / 수신 본문 {}개 · 전체 톡 {}개{}",
        if active{"이 PC 팀의 암호화 연결 실행 중"}else{"이 팀의 연결이 꺼져 있어요. 톡은 이 PC에 먼저 저장돼요"},s(feed,"pending"),s(feed,"conflicts"),s(feed,"stored"),s(feed,"received"),s(feed,"total"),if feed["canWrite"]==true{""}else{" · 새 저장·삭제 요청 불가"}));
}
fn draw_messages(feed: &Value) {
    let Some(list) = by_id("pcTalkFeed") else {
        return;
    };
    let near = list.scroll_height() - list.scroll_top() - list.client_height() < 120;
    clear(&list);
    let rows = feed["messages"].as_array().cloned().unwrap_or_default();
    if rows.is_empty() {
        let _ = list.append_child(
            &el("p")
                .cls("empty")
                .text(
                    "이 PC에 표시할 톡이 없어요. 연결 전에 작성해도 이 PC에 먼저 저장할 수 있어요.",
                )
                .0,
        );
    }
    for row in rows {
        let author = s(&row, "author");
        let incoming = row["incoming"] == true;
        let deleted = row["deleted"] == true;
        let mut bubble = el("div")
            .cls(if incoming {
                "drive-msg"
            } else {
                "drive-msg mine"
            })
            .attr(
                "title",
                &format!(
                    "작성 장치 {author}\n승인 구성원 ID {}\n원본 {}",
                    s(&row, "memberId"),
                    s(&row, "eventId")
                ),
            )
            .kid(el("div").cls("drive-who").text(&format!(
                "{} · 서명 확인 장치 {} · 순번 {}",
                if incoming { "받은 톡" } else { "내 톡" },
                short(&author),
                s(&row, "sequence")
            )))
            .kid(el("div").cls("drive-text").text(if deleted {
                "삭제 요청을 확인한 톡이에요."
            } else {
                row["text"].as_str().unwrap_or("")
            }));
        let time = if row["createdAt"].is_number() {
            super::fmt_date(row["createdAt"].as_f64().unwrap_or(0.0))
        } else {
            "기존 준비 기록 · 작성 시각 없음".into()
        };
        let mut footer = el("div").cls("drive-foot").text(&format!(
            "{time} · {}{}",
            label(&s(&row, "state")),
            if deleted {
                format!(" · 삭제: {}", label(&s(&row, "deleteState")))
            } else {
                String::new()
            }
        ));
        if row["canDelete"] == true {
            footer = footer.kid(
                el("button")
                    .attr("type", "button")
                    .cls("link")
                    .attr("data-delete", &s(&row, "eventId"))
                    .text("삭제 요청"),
            );
        }
        bubble = bubble.kid(footer);
        let _ = list.append_child(&bubble.0);
    }
    if near {
        list.set_scroll_top(list.scroll_height());
    }
}
async fn write(delete: Option<Value>) {
    if WRITING.with(|b| b.replace(true)) {
        return;
    }
    remember();
    controls();
    let generation = GENERATION.with(Cell::get);
    let selected = team();
    let feed = state(|s| s.feed.clone());
    let sent = input();
    let operation = async {
        if !consent() || feed["canWrite"] != true {
            return Err("대상 승인 장치를 확인하고 PC 톡 사용을 선택해 주세요.".to_string());
        }
        let key = delete
            .as_ref()
            .map(|row| format!("{}:{}", selected, s(row, "eventId")));
        let existing = state(|s| {
            if let Some(key) = &key {
                s.deletes.get(key).cloned()
            } else {
                s.drafts.get(&selected).and_then(|d| d.1.clone())
            }
        });
        let request = if let Some(id) = existing {
            id
        } else {
            let id = call("mesh-talk:draft-id", Value::Null)
                .await?
                .as_str()
                .ok_or("저장 요청 ID를 만들지 못했어요.")?
                .to_owned();
            state(|s| {
                if let Some(key) = &key {
                    s.deletes.insert(key.clone(), id.clone());
                } else {
                    s.drafts
                        .insert(selected.clone(), (sent.clone(), Some(id.clone())));
                }
            });
            id
        };
        let mut args = json!({"scope":feed["scope"],"requestId":request,"consent":true});
        let channel = if let Some(row) = &delete {
            args["eventId"] = row["eventId"].clone();
            args["sequence"] = row["sequence"].clone();
            "mesh-talk:delete"
        } else {
            args["text"] = json!(sent.trim());
            "mesh-talk:post"
        };
        call(channel, args).await?;
        Ok::<(), String>(())
    }
    .await;
    WRITING.with(|b| b.set(false));
    if generation == GENERATION.with(Cell::get) {
        match operation {
            Ok(()) => {
                if delete.is_none() {
                    if input() == sent {
                        if let Some(e) = by_id("pcTalkText") {
                            set_val(&e, "");
                        }
                        state(|s| {
                            s.drafts.remove(&selected);
                        });
                    }
                }
                message(if delete.is_some() {
                    "삭제 요청을 이 PC에 저장했어요. 연결된 상대 PC는 검증 후 반영하며, 기존 암호문을 물리적으로 지우는 작업은 아니에요."
                } else {
                    "이 PC에 저장했어요. 상대 PC의 저장 확인은 각 톡의 상태에서 확인하세요."
                });
                spawn(refresh(0));
            }
            Err(error) => message(&error),
        }
    }
    controls();
}
pub(super) fn bind() {
    if let Some(e) = by_id("pcTalkTeam") {
        let select = e.clone();
        on(&e, "change", move |_| change_team(val(&select)));
    }
    if let Some(e) = by_id("pcTalkText") {
        on(&e, "input", |_| {
            remember();
            controls();
        });
        on(&e, "keydown", |event| {
            let key: &web_sys::KeyboardEvent = event.unchecked_ref();
            if key.key() == "Enter" && !key.shift_key() && !key.is_composing() {
                event.prevent_default();
                spawn(write(None));
            }
        });
    }
    if let Some(e) = by_id("pcTalkConsent") {
        on(&e, "change", |_| controls());
    }
    if let Some(e) = by_id("pcTalkSend") {
        on(&e, "click", |_| spawn(write(None)));
    }
    if let Some(e) = by_id("pcTalkRefresh") {
        on(&e, "click", |_| spawn(refresh(0)));
    }
    if let Some(e) = by_id("pcTalkOlder") {
        on(&e, "click", |_| {
            if let Some(offset) = state(|s| s.feed["nextOffset"].as_u64()) {
                spawn(refresh(offset as usize));
            }
        });
    }
    if let Some(e) = by_id("pcTalkNewer") {
        on(&e, "click", |_| {
            spawn(refresh(state(|s| s.offset.saturating_sub(50))))
        });
    }
    if let Some(e) = by_id("pcTalkSettings") {
        on(&e, "click", |_| show_tab("settings"));
    }
    if let Some(e) = by_id("pcTalkFeed") {
        on(&e, "click", |event| {
            let Some(button) = event
                .target()
                .and_then(|v| v.dyn_into::<web_sys::Element>().ok())
                .and_then(|e| e.closest("button[data-delete]").ok().flatten())
            else {
                return;
            };
            let Some(id) = button.get_attribute("data-delete") else {
                return;
            };
            let row = state(|s| {
                s.feed["messages"]
                    .as_array()
                    .and_then(|rows| {
                        rows.iter()
                            .find(|r| r["eventId"] == id && r["canDelete"] == true)
                    })
                    .cloned()
            });
            if let Some(row) = row {
                if consent() && confirm("이 톡의 서명된 삭제 요청을 남길까요? 상대 PC는 연결 뒤 반영하며, 이미 받은 내용의 회수나 디스크의 완전 삭제를 보장하지 않아요."){spawn(write(Some(row)));}
            }
        });
    }
    controls();
}
