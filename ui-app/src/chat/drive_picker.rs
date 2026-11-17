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
        if p.teams {
            "초대받은 팀 연결"
        } else {
            "내 Drive에서 가져오기"
        },
    );
    set_text(
        "drivePickerNote",
        if p.teams {
            "내 계정에 공유된 사이 팀 폴더를 연결해요."
        } else {
            "선택한 파일의 사본을 팀 폴더에 추가해요. 원본은 유지되고, 사본은 팀원에게 공유돼요."
        },
    );
    toggle(&by_id("drivePickerTools").unwrap(), "hidden", p.teams);
    toggle(&by_id("drivePickerImport").unwrap(), "hidden", p.teams);
    set_val(&by_id("drivePickerQuery").unwrap(), "");
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
            "연결 →"
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
        button = button
            .flag(
                "disabled",
                loading || (!team_mode && !folder && !b(row, "canCopy")),
            )
            .kid(el("span").text(&name))
            .kid(el("small").text(suffix))
            .click(move |_| {
                if team_mode {
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
    set_text("drivePickerMessage", "Drive 목록을 확인하고 있어요…");
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
            let empty = rows.is_empty();
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
            set_text(
                "drivePickerMessage",
                if empty {
                    "표시할 항목이 없어요."
                } else {
                    ""
                },
            );
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
