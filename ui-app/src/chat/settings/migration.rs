use super::{click, s, v};
use crate::{api::call, dom::*};
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    collections::BTreeSet,
};
use wasm_bindgen::JsCast;
thread_local! {
    static TARGET:RefCell<String>=const{RefCell::new(String::new())};
    static PREVIEW:RefCell<Value>=const{RefCell::new(Value::Null)};
    static PAGE:RefCell<Value>=const{RefCell::new(Value::Null)};
    static SELECTED:RefCell<BTreeSet<String>>=const{RefCell::new(BTreeSet::new())};
    static BUSY:Cell<bool>=const{Cell::new(false)};
    static GEN:Cell<u64>=const{Cell::new(0)};
}
fn target() -> String {
    super::identity::team_id()
}
fn controls() {
    let busy = BUSY.with(Cell::get);
    let ready = PREVIEW.with(|p| p.borrow()["token"].is_string());
    let count = SELECTED.with(|s| s.borrow().len());
    for (id, disabled) in [
        ("meshMigrationSource", busy),
        (
            "meshMigrationPreview",
            busy || target().is_empty()
                || v("meshMigrationSource").is_empty()
                || !super::identity::admin_here(),
        ),
        ("meshMigrationSelect", busy || !ready),
        ("meshMigrationClear", busy || !ready),
        ("meshMigrationConsent", busy || !ready),
        (
            "meshMigrationCommit",
            busy || !ready
                || count == 0
                || count > 200
                || !by_id("meshMigrationConsent").is_some_and(|e| checked(&e)),
        ),
        ("meshArchiveRefresh", busy || target().is_empty()),
        (
            "meshArchivePrevious",
            busy || PAGE.with(|p| p.borrow()["offset"].as_u64().unwrap_or(0) == 0),
        ),
        (
            "meshArchiveNext",
            busy || !PAGE.with(|p| p.borrow()["nextOffset"].is_number()),
        ),
    ] {
        if let Some(e) = by_id(id) {
            set_disabled(&e, disabled);
        }
    }
    if let Some(list) = by_id("meshMigrationRows") {
        for input in q_all_in(&list, "input") {
            set_disabled(
                &input,
                busy || input.get_attribute("data-eligible").as_deref() != Some("true"),
            );
        }
    }
    summary();
}
fn summary() {
    PREVIEW.with(|p|{let p=p.borrow();if !p.is_object(){set_text("meshMigrationSummary","서버에서 새로 읽은 공개 톡을 확인해 선택하세요. 원본 서버는 바꾸지 않아요.");return;}
        let excluded=p["excluded"].as_object().map(|m|m.iter().map(|(key,n)|format!("{} {}개",match key.as_str(){"file"=>"파일 링크","unknown-fields"=>"추가 필드 포함","linked-proposal"=>"약속 제안",_=>"형식 확인 불가"},n)).collect::<Vec<_>>().join(" · ")).unwrap_or_default();
        set_text("meshMigrationSummary",&format!("{} ({}) → PC 팀 {} · 새로 조회 {}개 · 공개 톡 후보 {}개 · 선택 {}개/최대 200개 · {} · 미리보기는 5분 동안 유효해요.",s(&p,"sourceName"),s(&p,"sourceKind"),s(&p,"targetTeam").chars().take(12).collect::<String>(),s(&p,"scanned"),p["candidates"].as_array().map_or(0,Vec::len),SELECTED.with(|s|s.borrow().len()),if excluded.is_empty(){"제외 없음"}else{&excluded}));
    });
}
fn reset() {
    GEN.with(|g| g.set(g.get() + 1));
    PREVIEW.with(|p| p.replace(Value::Null));
    SELECTED.with(|s| s.borrow_mut().clear());
    if let Some(e) = by_id("meshMigrationRows") {
        clear(&e);
    }
    if let Some(e) = by_id("meshMigrationConsent") {
        set_checked(&e, false);
    }
    set_text("meshMigrationMessage", "");
    controls();
}
pub(super) fn selected_changed() {
    let next = target();
    let changed = TARGET.with(|s| {
        if *s.borrow() == next {
            false
        } else {
            s.replace(next);
            true
        }
    });
    if changed {
        reset();
        PAGE.with(|p| p.replace(Value::Null));
        if let Some(e) = by_id("meshArchiveRows") {
            clear(&e);
        }
        set_text(
            "meshArchiveSummary",
            "선택한 PC 팀의 로컬 보관 목록을 읽는 중이에요.",
        );
        spawn(read_archive(0));
    }
    controls();
}
pub(super) async fn load() {
    if let Ok(rows) = call("mesh-migration:sources", Value::Null).await {
        let previous = v("meshMigrationSource");
        if let Some(select) = by_id("meshMigrationSource") {
            clear(&select);
            let _ = select.append_child(&el("option").attr("value", "").text("기존 팀 선택").0);
            for row in rows.as_array().into_iter().flatten() {
                let id = s(row, "teamId");
                let _ = select.append_child(
                    &el("option")
                        .attr("value", &id)
                        .flag("selected", id == previous)
                        .text(&s(row, "name"))
                        .0,
                );
            }
        }
    }
    selected_changed();
    controls();
}
fn begin() -> Option<u64> {
    if BUSY.with(|b| b.replace(true)) {
        None
    } else {
        controls();
        Some(GEN.with(Cell::get))
    }
}
fn finish(generation: u64) {
    BUSY.with(|b| b.set(false));
    controls();
    if GEN.with(Cell::get) != generation {
        spawn(read_archive(0));
    }
}
fn candidates() {
    let Some(list) = by_id("meshMigrationRows") else {
        return;
    };
    clear(&list);
    PREVIEW.with(|p|for row in p.borrow()["candidates"].as_array().into_iter().flatten(){
        let id=s(row,"id");let available=row["state"]=="new";
        let state=match s(row,"state").as_str(){"existing"=>"이미 이 PC에 보관됨","conflict"=>"같은 ID의 보관 내용이 달라 보류",_=>"새 보관 후보"};
        let item=el("div").cls("item").kid(el("div").cls("main")
            .kid(el("label").kid(el("input").attr("type","checkbox").attr("data-legacy-id",&id).attr("data-eligible",if available{"true"}else{"false"}).flag("checked",SELECTED.with(|s|s.borrow().contains(&id))).flag("disabled",!available)).txt(&format!("{id} · {state}")))
            .kid(el("p").cls("hint").text(&format!("원본 작성 UID {} · 기존 기록 시각 {} · 원래 작성자의 새 서명을 확인한 기록은 아니에요.",s(row,"claimedBy"),super::super::fmt_clock(row["claimedAt"].as_f64().unwrap_or(0.0)))))
            .kid(el("p").cls("mesh-local-text").text(&s(row,"text"))));let _=list.append_child(&item.0);
    });
    controls();
}
async fn preview() {
    reset();
    let Some(generation) = begin() else {
        return;
    };
    let target = target();
    let source = v("meshMigrationSource");
    set_text(
        "meshMigrationMessage",
        "기존 서버의 관리자 권한과 공개 톡을 새로 확인하는 중이에요…",
    );
    let result = call(
        "mesh-migration:preview",
        json!({"sourceTeam":source,"targetTeam":target}),
    )
    .await;
    if generation == GEN.with(Cell::get) {
        match result {
            Ok(info) => {
                PREVIEW.with(|p| p.replace(info));
                candidates();
                set_text(
                    "meshMigrationMessage",
                    "원문과 대상 PC 팀을 확인한 뒤 보관할 톡을 선택하세요.",
                );
            }
            Err(error) => set_text("meshMigrationMessage", &error),
        }
    }
    finish(generation);
}
async fn commit() {
    let Some(generation) = begin() else {
        return;
    };
    let plan = PREVIEW.with(|p| p.borrow().clone());
    let ids: Vec<_> = SELECTED.with(|s| s.borrow().iter().cloned().collect());
    set_text(
        "meshMigrationMessage",
        "선택한 원본의 변경·삭제를 다시 확인하고 이 PC에 보관하는 중이에요…",
    );
    let result=call("mesh-migration:commit",json!({"token":plan["token"],"sourceTeam":plan["sourceTeam"],"targetTeam":plan["targetTeam"],"selected":ids,"consent":by_id("meshMigrationConsent").is_some_and(|e|checked(&e))})).await;
    if generation == GEN.with(Cell::get) {
        match result {
            Ok(report) => {
                reset();
                set_text("meshMigrationMessage",&format!("새로 보관 {}개 · 이미 보관 {}개 · 이 PC의 보관 합계 {}개. 원본 서버를 바꾸거나 다른 PC에 전송하지 않았어요.",s(&report,"imported"),s(&report,"existing"),s(&report,"total")));
            }
            Err(error) => set_text("meshMigrationMessage", &error),
        }
    }
    finish(generation);
}
async fn read_archive(offset: usize) {
    let selected = target();
    if selected.is_empty() {
        set_text("meshArchiveSummary", "PC 팀을 먼저 선택해 주세요.");
        return;
    }
    let Some(generation) = begin() else {
        return;
    };
    let result = call(
        "mesh-migration:archive",
        json!({"targetTeam":selected,"offset":offset}),
    )
    .await;
    if generation == GEN.with(Cell::get) {
        match result {
            Ok(info) => {
                set_text("meshArchiveSummary",&format!("이 PC의 보관 기록 {}개 · {}번째부터 · 원본 수정·삭제가 자동 반영되지 않는 별도 사본이에요.",s(&info,"total"),offset+1));
                if let Some(list) = by_id("meshArchiveRows") {
                    clear(&list);
                    for row in info["messages"].as_array().into_iter().flatten() {
                        let item = el("div").cls("item").kid(
                            el("div")
                                .cls("main")
                                .kid(el("b").text(&format!(
                                    "{} · 원본 ID {}",
                                    s(row, "sourceName"),
                                    s(row, "legacyId")
                                )))
                                .kid(el("p").cls("hint").text(&format!(
                                    "원본 작성 UID {} · 기존 기록 시각 {} · 이 PC 전용 확인 사본",
                                    s(row, "claimedBy"),
                                    super::super::fmt_clock(
                                        row["claimedAt"].as_f64().unwrap_or(0.0)
                                    )
                                )))
                                .kid(el("p").cls("mesh-local-text").text(&s(row, "text"))),
                        );
                        let _ = list.append_child(&item.0);
                    }
                }
                PAGE.with(|p| p.replace(info));
            }
            Err(error) => {
                set_text("meshArchiveSummary", &error);
                if let Some(list) = by_id("meshArchiveRows") {
                    clear(&list);
                }
                PAGE.with(|p| p.replace(Value::Null));
            }
        }
    }
    finish(generation);
}
pub(super) fn bind() {
    if let Some(select) = by_id("meshMigrationSource") {
        on(&select, "change", |_| reset());
    }
    if let Some(consent) = by_id("meshMigrationConsent") {
        on(&consent, "change", |_| controls());
    }
    if let Some(rows) = by_id("meshMigrationRows") {
        on(&rows, "change", |event| {
            let Some(input) = event
                .target()
                .and_then(|t| t.dyn_into::<web_sys::HtmlElement>().ok())
            else {
                return;
            };
            let Some(id) = input.get_attribute("data-legacy-id") else {
                return;
            };
            if input.get_attribute("data-eligible").as_deref() != Some("true") {
                return;
            }
            SELECTED.with(|s| {
                if checked(&input) {
                    s.borrow_mut().insert(id);
                } else {
                    s.borrow_mut().remove(&id);
                }
            });
            controls();
        });
    }
    click("meshMigrationPreview", |_| spawn(preview()));
    click("meshMigrationCommit", |_| spawn(commit()));
    click("meshMigrationClear", |_| {
        SELECTED.with(|s| s.borrow_mut().clear());
        candidates();
    });
    click("meshMigrationSelect", |_| {
        let ids = PREVIEW.with(|p| {
            p.borrow()["candidates"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|r| r["state"] == "new")
                .take(200)
                .map(|r| s(r, "id"))
                .collect()
        });
        SELECTED.with(|s| s.replace(ids));
        candidates();
    });
    click("meshArchiveRefresh", |_| spawn(read_archive(0)));
    click("meshArchivePrevious", |_| {
        let offset = PAGE.with(|p| p.borrow()["offset"].as_u64().unwrap_or(0) as usize);
        spawn(read_archive(offset.saturating_sub(50)));
    });
    click("meshArchiveNext", |_| {
        if let Some(offset) = PAGE.with(|p| p.borrow()["nextOffset"].as_u64()) {
            spawn(read_archive(offset as usize));
        }
    });
    controls();
}
