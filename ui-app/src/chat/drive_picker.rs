//! Stored Google session only: browsing never launches an OAuth window.
use super::{b, s};
use crate::{api::call, dom::*};
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
};
use wasm_bindgen::JsCast;
use web_sys::HtmlDialogElement;

#[derive(Clone, Default)]
struct Picker {
    epoch: u64,
    teams: bool,
    target: String,
    mode: String,
    parent: String,
    query: String,
    label: String,
    back: Vec<(String, String, String)>,
    rows: Vec<Value>,
    selected: BTreeMap<String, String>,
    next: String,
}
thread_local! {
    static STATE: RefCell<Option<Picker>> = const{RefCell::new(None)};
    static GENERATION: Cell<u64> = const{Cell::new(0)};
    static LOADING: Cell<bool> = const{Cell::new(false)};
}
fn dialog() -> HtmlDialogElement {
    by_id("drivePicker").unwrap().unchecked_into()
}
fn invalidate() -> u64 {
    GENERATION.with(|g| {
        g.set(g.get() + 1);
        g.get()
    })
}
fn reset() {
    invalidate();
    STATE.with(|s| *s.borrow_mut() = None);
    LOADING.with(|v| v.set(false));
}
fn close() {
    reset();
    dialog().close();
}
pub fn account(value: &Value) {
    let stale = STATE.with(|s| {
        s.borrow()
            .as_ref()
            .is_some_and(|p| value["epoch"].as_u64() != Some(p.epoch) || !b(value, "signedIn"))
    });
    if stale {
        close();
    }
}
fn open(p: Picker) {
    if dialog().open() {
        return;
    }
    set_text(
        "drivePickerTitle",
        if p.teams { "팀 연결" } else { "Google" },
    );
    set_text(
        "drivePickerNote",
        if p.teams {
            "팀 선택"
        } else {
            "사본 추가"
        },
    );
    toggle(&by_id("drivePickerTools").unwrap(), "hidden", p.teams);
    toggle(&by_id("drivePickerImport").unwrap(), "hidden", p.teams);
    toggle(&by_id("drivePickerLink").unwrap(), "hidden", !p.teams);
    set_val(&by_id("drivePickerQuery").unwrap(), "");
    set_val(&by_id("drivePickerLinkInput").unwrap(), "");
    STATE.with(|s| *s.borrow_mut() = Some(p));
    if dialog().show_modal().is_err() {
        reset();
        return;
    }
    spawn(load(false));
}
pub fn files(team: Value) {
    open(Picker {
        epoch: team["epoch"].as_u64().unwrap_or(0),
        target: s(&team, "folderId"),
        mode: "folder".into(),
        label: "내 Drive".into(),
        ..Picker::default()
    });
}
pub fn teams(epoch: u64) {
    open(Picker {
        epoch,
        teams: true,
        label: "연결할 수 있는 팀".into(),
        ..Picker::default()
    });
}
fn render() {
    let Some(p) = STATE.with(|s| s.borrow().clone()) else {
        return;
    };
    let loading = LOADING.with(Cell::get);
    set_text("drivePickerLocation", &p.label);
    set_disabled(
        &by_id("drivePickerBack").unwrap(),
        loading || p.back.is_empty(),
    );
    toggle(&by_id("drivePickerBack").unwrap(), "hidden", p.teams);
    set_disabled(
        &by_id("drivePickerImport").unwrap(),
        loading || p.selected.is_empty(),
    );
    set_text(
        "drivePickerImport",
        &format!("사본 추가 ({})", p.selected.len()),
    );
    set_text(
        "drivePickerSelection",
        if p.teams {
            ""
        } else if p.selected.is_empty() {
            "최대 10개 선택"
        } else {
            "선택은 폴더를 이동해도 유지돼요"
        },
    );
    toggle(
        &by_id("drivePickerMore").unwrap(),
        "hidden",
        p.next.is_empty(),
    );
    set_disabled(&by_id("drivePickerMore").unwrap(), loading);
    for e in q_all("#drivePickerTools input, #drivePickerTools button") {
        set_disabled(&e, loading);
    }
    let root = by_id("drivePickerList").unwrap();
    clear(&root);
    for row in &p.rows {
        let id = s(row, "id");
        let name = s(row, "name");
        let folder = b(row, "folder");
        let chosen = p.selected.contains_key(&id);
        let mut button = el("button").cls("drive-choice").attr("type", "button");
        let suffix = if p.teams {
            "참여 →"
        } else if folder {
            "폴더 열기 →"
        } else if chosen {
            "선택됨 ✓"
        } else if !b(row, "canCopy") {
            "복사 제한"
        } else {
            "선택"
        };
        if !folder && !p.teams {
            button = button.attr("aria-pressed", if chosen { "true" } else { "false" });
        }
        let team_mode = p.teams;
        let epoch = p.epoch;
        let owner = s(row, "ownerEmail");
        let shared_by = s(row, "sharedBy");
        let origin = match (owner.as_str(), shared_by.as_str()) {
            ("", "") => "소유자를 확인하지 못했어요".to_owned(),
            (o, sb) if sb.is_empty() || sb.eq_ignore_ascii_case(o) => format!("소유자 {o}"),
            ("", sb) => format!("공유한 사람 {sb}"),
            (o, sb) => format!("소유자 {o} · 공유한 사람 {sb}"),
        };
        button = button
            .flag(
                "disabled",
                loading || (!team_mode && !folder && !b(row, "canCopy")),
            )
            .kid(el("span").text(&name))
            .kid(el("small").text(if team_mode { &origin } else { suffix }))
            .click(move |_| {
                if team_mode {
                    // Anyone can share a folder with anyone; joining is an explicit, informed choice.
                    if !confirm(&format!(
                        "‘{name}’ 팀에 참여할까요?

{origin}
모르는 사람이 공유한 폴더라면 참여하지 마세요. 참여하면 이 팀 폴더로 파일을 올리고 내려받게 돼요."
                    )) {
                        return;
                    }
                    close();
                    super::teams::connect(id.clone(), epoch);
                } else if folder {
                    navigate("folder", &id, &name, true);
                } else {
                    STATE.with(|s| {
                        if let Some(p) = s.borrow_mut().as_mut() {
                            if p.selected.remove(&id).is_none() {
                                if p.selected.len() < 10 {
                                    p.selected.insert(id.clone(), name.clone());
                                } else {
                                    set_text(
                                        "drivePickerMessage",
                                        "파일은 10개까지 선택할 수 있어요.",
                                    );
                                }
                            }
                        }
                    });
                    render();
                }
            });
        let _ = root.append_child(&button.build());
    }
}
fn navigate(mode: &str, parent: &str, label: &str, keep_back: bool) {
    if LOADING.with(Cell::get) {
        return;
    }
    STATE.with(|s| {
        if let Some(p) = s.borrow_mut().as_mut() {
            if keep_back {
                p.back
                    .push((p.mode.clone(), p.parent.clone(), p.label.clone()));
            } else {
                p.back.clear();
            }
            p.mode = mode.into();
            p.parent = parent.into();
            p.label = label.into();
            p.rows.clear();
            p.next.clear();
        }
    });
    spawn(load(false));
}
async fn load(more: bool) {
    let Some(p) = STATE.with(|s| s.borrow().clone()) else {
        return;
    };
    if more && p.next.is_empty() {
        return;
    }
    let generation = invalidate();
    LOADING.with(|v| v.set(true));
    render();
    set_text("drivePickerMessage", "불러오는 중…");
    let page = if more { p.next.as_str() } else { "" };
    let result = call(
        if p.teams {
            "teams:browse"
        } else {
            "files:sources"
        },
        json!({"epoch":p.epoch,"mode":p.mode,"parent":p.parent,"query":p.query,"page":page}),
    )
    .await;
    if generation != GENERATION.with(Cell::get) {
        return;
    }
    LOADING.with(|v| v.set(false));
    match result {
        Ok(v) if v["epoch"].as_u64() == Some(p.epoch) => {
            let rows = v["entries"].as_array().cloned().unwrap_or_default();
            let empty = rows.is_empty() && !more;
            STATE.with(|s| {
                if let Some(p) = s.borrow_mut().as_mut() {
                    if !more {
                        p.rows.clear();
                    }
                    for row in rows {
                        if !p.rows.iter().any(|r| r["id"] == row["id"]) {
                            p.rows.push(row);
                        }
                    }
                    p.next = super::s(&v, "nextPage");
                }
            });
            let mut notes = vec![];
            if p.teams {
                if empty {
                    notes.push(format!(
                        "{} 계정으로 초대받은 팀이 없어요. 초대받은 이메일이 이 계정과 같은지 확인해 주세요. 방금 초대받았다면 Drive 검색에 늦게 잡힐 수 있으니 소유자에게 받은 폴더 링크를 아래에 붙여 넣어 주세요.",
                        s(&v, "email")
                    ));
                }
                if b(&v, "incomplete") {
                    notes.push("Drive가 목록 일부만 돌려줬어요. 보이지 않는 팀은 폴더 링크로 참여해 주세요.".into());
                }
                if v["skipped"].as_u64().unwrap_or(0) > 0 {
                    notes.push(format!(
                        "열 수 없는 팀 폴더 {}개는 빼고 보여 드려요.",
                        v["skipped"]
                    ));
                }
            } else if empty {
                notes.push("표시할 항목이 없어요.".into());
            }
            set_text("drivePickerMessage", &notes.join(" "));
        }
        Ok(_) => {
            close();
            return;
        }
        Err(e) => set_text("drivePickerMessage", &e),
    }
    render();
}
pub fn bind() {
    on(&by_id("drivePickerClose").unwrap(), "click", |_| close());
    on(&dialog(), "close", |_| reset());
    on(&dialog(), "cancel", |_| reset());
    on(&by_id("drivePickerMine").unwrap(), "click", |_| {
        navigate("folder", "", "내 Drive", false)
    });
    on(&by_id("drivePickerShared").unwrap(), "click", |_| {
        navigate("shared", "", "공유받은 자료", false)
    });
    on(&by_id("drivePickerBack").unwrap(), "click", |_| {
        let back = STATE.with(|s| s.borrow_mut().as_mut().and_then(|p| p.back.pop()));
        if let Some((mode, parent, label)) = back {
            STATE.with(|s| {
                if let Some(p) = s.borrow_mut().as_mut() {
                    p.mode = mode;
                    p.parent = parent;
                    p.label = label;
                    p.rows.clear();
                    p.next.clear();
                }
            });
            spawn(load(false));
        }
    });
    on(&by_id("drivePickerSearch").unwrap(), "submit", |e| {
        e.prevent_default();
        let query = val(&by_id("drivePickerQuery").unwrap());
        if query.trim().is_empty() {
            return;
        }
        STATE.with(|s| {
            if let Some(p) = s.borrow_mut().as_mut() {
                p.query = query;
            }
        });
        navigate("search", "", "이름 검색", false);
    });
    on(&by_id("drivePickerLink").unwrap(), "submit", |e| {
        e.prevent_default();
        let link = val(&by_id("drivePickerLinkInput").unwrap()).trim().to_owned();
        let Some(p) = STATE.with(|s| s.borrow().clone()) else {
            return;
        };
        if !p.teams || link.is_empty() {
            return;
        }
        if !link.starts_with("https://drive.google.com/") {
            set_text(
                "drivePickerMessage",
                "https://drive.google.com/ 으로 시작하는 팀 폴더 링크를 붙여 넣어 주세요.",
            );
            return;
        }
        // The link is only a pointer: the app still checks that this account was invited to the folder.
        if !confirm("이 링크의 팀 폴더에 참여할까요?
링크를 보낸 사람이 아는 사람인지 확인해 주세요. 초대받지 않은 계정이면 참여할 수 없어요.") {
            return;
        }
        close();
        super::teams::connect(link, p.epoch);
    });
    on(&by_id("drivePickerMore").unwrap(), "click", |_| {
        spawn(load(true))
    });
    on(&by_id("drivePickerImport").unwrap(), "click", |_| {
        if LOADING.with(Cell::get) {
            return;
        }
        if let Some(p) = STATE.with(|s| s.borrow().clone()) {
            if p.teams || p.selected.is_empty() {
                return;
            }
            close();
            super::files::import(p.epoch, &p.target, p.selected.into_keys().collect());
        }
    });
}
