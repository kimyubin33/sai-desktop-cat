use super::{arr, b, profile, s};
use crate::{
    api::{call, listen},
    dom::*,
};
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    collections::BTreeSet,
};
thread_local! {
    static EPOCH:Cell<u64>=const{Cell::new(0)};
    static GENERATION:Cell<u64>=const{Cell::new(0)};
    static BUSY:Cell<bool>=const{Cell::new(false)};
    static SELECTED:RefCell<Option<Value>>=const{RefCell::new(None)};
    static NEXT:RefCell<String>=const{RefCell::new(String::new())};
    static REQUEST:RefCell<String>=const{RefCell::new(String::new())};
    static SEEN:RefCell<BTreeSet<String>>=const{RefCell::new(BTreeSet::new())};
}
fn selected() -> Option<Value> {
    SELECTED.with(|s| s.borrow().clone())
}
fn generation() -> u64 {
    GENERATION.with(|g| {
        let n = g.get() + 1;
        g.set(n);
        n
    })
}
fn busy_controls(busy: bool) {
    for e in q_all("#filesPanel button") {
        set_disabled(&e, busy);
    }
    set_disabled(&by_id("fileCancel").unwrap(), false);
}
fn close() {
    if BUSY.with(|busy| busy.replace(false)) {
        spawn(async {
            let _ = call("files:cancel", Value::Null).await;
        });
    }
    SELECTED.with(|s| *s.borrow_mut() = None);
    generation();
    REQUEST.with(|r| r.borrow_mut().clear());
    toggle(&by_id("filesPanel").unwrap(), "hidden", true);
    toggle(&by_id("teamOverview").unwrap(), "hidden", false);
    toggle(&by_id("teamList").unwrap(), "hidden", false);
    toggle(&by_id("fileProgress").unwrap(), "hidden", true);
    toggle(&by_id("fileCancel").unwrap(), "hidden", true);
}
pub fn account(v: &Value) {
    let epoch = v["epoch"].as_u64().unwrap_or(0);
    if epoch < EPOCH.with(Cell::get) {
        return;
    }
    if EPOCH.with(|e| e.replace(epoch)) != epoch || !b(v, "signedIn") {
        close();
        BUSY.with(|b| b.set(false));
        clear(&by_id("fileList").unwrap());
        clear(&by_id("uploadQueue").unwrap());
        toggle(&by_id("fileProgress").unwrap(), "hidden", true);
        toggle(&by_id("fileCancel").unwrap(), "hidden", true);
        set_text("fileMessage", "");
    }
}
pub fn teams_changed(rows: &[Value]) {
    if let Some(team) = selected() {
        if !rows.iter().any(|r| r["folderId"] == team["folderId"]) {
            close();
        }
    }
}
pub fn show(team: Value) {
    if BUSY.with(Cell::get) {
        set_text(
            "fileMessage",
            "진행 중인 파일 작업을 완료하거나 취소한 뒤 다른 팀을 선택해 주세요.",
        );
        return;
    }
    if team["epoch"].as_u64() != Some(EPOCH.with(Cell::get)) {
        return;
    }
    super::collab::select(&s(&team, "folderId"));
    set_text("filesTitle", &s(&team, "name"));
    SELECTED.with(|s| *s.borrow_mut() = Some(team));
    generation();
    clear(&by_id("fileList").unwrap());
    clear(&by_id("uploadQueue").unwrap());
    toggle(&by_id("filesPanel").unwrap(), "hidden", false);
    toggle(&by_id("teamOverview").unwrap(), "hidden", true);
    toggle(&by_id("teamList").unwrap(), "hidden", true);
    by_id("filesPanel").unwrap().scroll_into_view();
    spawn(load(false));
}
fn size(v: &Value) -> String {
    let Some(n) = v.as_u64() else {
        return "Google 문서".into();
    };
    if n >= 1024 * 1024 {
        format!("{:.1} MB", n as f64 / 1048576.)
    } else if n >= 1024 {
        format!("{:.0} KB", n as f64 / 1024.)
    } else {
        format!("{n} B")
    }
}
fn file_row(file: &Value) -> crate::dom::El {
    let id = s(file, "id");
    let download_id = id.clone();
    let mut actions = el("div").cls("file-row-actions");
    if b(file, "canAnalyze") {
        let file_id = id.clone();
        actions = actions.kid(
            el("button")
                .cls("text-button")
                .attr("type", "button")
                .attr("data-icon", "sparkle")
                .attr("title", "AI로 할 일 정리")
                .text("AI")
                .click(move |_| {
                    if let Some(team) = selected() {
                        super::analysis::open(team, Some(file_id.clone()));
                    }
                }),
        );
    }
    if b(file, "canDownload") {
        actions = actions.kid(
            el("button")
                .cls("subtle")
                .attr("type", "button")
                .attr("data-icon", "download")
                .attr("aria-label", "파일 받기")
                .attr("title", "파일 받기")
                .text("")
                .click(move |_| action("files:download", json!({"fileId":download_id}))),
        );
    }
    actions = actions.kid(
        el("button")
            .cls("text-button")
            .attr("type", "button")
            .attr("data-icon", "external")
            .attr("aria-label", "Google Drive에서 열기")
            .attr("title", "Google Drive에서 열기")
            .text("")
            .click(move |_| action("files:open", json!({"fileId":id}))),
    );
    el("article")
        .cls("file-row")
        .kid(
            el("div")
                .cls("file-row-info")
                .kid(el("strong").text(&s(file, "name")))
                .kid(el("small").text(&size(&file["size"]))),
        )
        .kid(actions)
}
fn queue(v: &Value, can_retry: bool) {
    let root = by_id("uploadQueue").unwrap();
    clear(&root);
    let rows = v.as_array().map(Vec::as_slice).unwrap_or_default();
    toggle(
        &by_id("uploadQueueSection").unwrap(),
        "hidden",
        rows.is_empty(),
    );
    for item in rows {
        let operation = s(item, "operation");
        let discard = operation.clone();
        let name = s(item, "name");
        let drive = s(item, "origin") == "drive";
        let retry_channel = if drive {
            "files:retry-import"
        } else {
            "files:retry"
        };
        let discard_channel = if drive {
            "files:discard-import"
        } else {
            "files:discard"
        };
        let info = el("div")
            .cls("file-row-info")
            .kid(el("strong").text(&name))
            .kid(el("small").text(&format!(
                "{} · {}",
                size(&item["size"]),
                if drive {
                    "Drive에서 사본 추가 대기"
                } else {
                    "이 PC에 보관한 사본"
                }
            )));
        let actions = el("div")
            .cls("file-row-actions")
            .kid(
                el("button")
                    .cls("subtle")
                    .attr("type", "button")
                    .flag("disabled", !can_retry)
                    .text(if b(item, "uncertain") { "결과 확인" } else { "재시도" })
                    .click(move |_| action(retry_channel, json!({"operation":operation}))),
            )
            .kid(
                el("button")
                    .cls("text-button")
                    .attr("type", "button")
                    .text("대기 삭제")
                    .click(move |_| {
                        if confirm(&format!("‘{name}’의 이 기기 전송 대기를 지울까요? 원본과 이미 저장된 Drive 사본은 유지돼요.")) {
                            action(discard_channel, json!({"operation":discard}));
                        }
                    }),
            );
        let row = el("article")
            .cls("file-row pending-file")
            .kid(info)
            .kid(actions);
        let _ = root.append_child(&row.build());
    }
}
async fn load(more: bool) {
    if BUSY.with(Cell::get) {
        return;
    }
    let Some(team) = selected() else {
        return;
    };
    let gen = generation();
    let epoch = EPOCH.with(Cell::get);
    let page = if more {
        NEXT.with(|p| p.borrow().clone())
    } else {
        String::new()
    };
    if more && page.is_empty() {
        return;
    }
    if !more {
        clear(&by_id("fileList").unwrap());
        SEEN.with(|s| s.borrow_mut().clear());
    }
    busy_controls(true);
    set_text("fileMessage", "불러오는 중…");
    set_disabled(&by_id("filesBack").unwrap(), false);
    let response = call(
        "files:list",
        json!({"epoch":epoch,"folderId":team["folderId"],"page":page}),
    )
    .await;
    if gen != GENERATION.with(Cell::get) || BUSY.with(Cell::get) {
        return;
    }
    busy_controls(false);
    match response {
        Ok(v) => {
            if !profile::accept(&v) || v["epoch"].as_u64() != Some(EPOCH.with(Cell::get)) {
                return;
            }
            let error = s(&v, "error");
            let can_upload = error.is_empty() && b(&v["team"], "canUpload");
            set_disabled(&by_id("fileUpload").unwrap(), !can_upload);
            set_disabled(&by_id("filePick").unwrap(), !can_upload);
            set_disabled(&by_id("fileAnalyze").unwrap(), !error.is_empty());
            queue(&v["pending"], can_upload);
            let list = by_id("fileList").unwrap();
            for row in arr(&v, "files") {
                if SEEN.with(|seen| seen.borrow_mut().insert(s(row, "id"))) {
                    let _ = list.append_child(&file_row(row).build());
                }
            }
            let next = s(&v, "nextPage");
            NEXT.with(|p| *p.borrow_mut() = next.clone());
            toggle(&by_id("fileMore").unwrap(), "hidden", next.is_empty());
            set_text(
                "fileMessage",
                if !error.is_empty() {
                    &error
                } else if SEEN.with(|s| s.borrow().is_empty()) {
                    "파일 없음"
                } else {
                    ""
                },
            );
        }
        Err(e) => {
            set_text("fileMessage", &e);
            for id in ["fileUpload", "filePick", "fileMore", "fileAnalyze"] {
                set_disabled(&by_id(id).unwrap(), true);
            }
        }
    }
}
fn action(channel: &'static str, mut args: Value) {
    if BUSY.with(|b| b.replace(true)) {
        return;
    }
    let Some(team) = selected() else {
        BUSY.with(|b| b.set(false));
        return;
    };
    let epoch = EPOCH.with(Cell::get);
    let gen = generation();
    let request = format!("file-{}-{gen}", js_sys::Date::now() as u64);
    REQUEST.with(|r| *r.borrow_mut() = request.clone());
    args["epoch"] = json!(epoch);
    args["folderId"] = team["folderId"].clone();
    args["request"] = json!(request);
    busy_controls(true);
    toggle(
        &by_id("fileCancel").unwrap(),
        "hidden",
        channel == "files:open" || channel == "files:discard",
    );
    toggle(&by_id("fileProgress").unwrap(), "hidden", false);
    let _ = by_id("fileProgress").unwrap().remove_attribute("value");
    set_text(
        "fileMessage",
        match channel {
            "files:upload" => {
                "팀 폴더에 올릴 파일을 골라 주세요. 파일은 25MB까지 전송할 수 있어요."
            }
            "files:download" => "저장할 이름과 위치를 골라 주세요. 기존 파일은 덮어쓰지 않아요.",
            "files:import" | "files:retry-import" => {
                "선택한 파일을 팀 폴더로 복사하고 저장 결과를 확인하고 있어요…"
            }
            _ => "파일 작업을 확인하고 있어요…",
        },
    );
    spawn(async move {
        let result = call(channel, args).await;
        if gen != GENERATION.with(Cell::get) || epoch != EPOCH.with(Cell::get) {
            return;
        }
        BUSY.with(|b| b.set(false));
        REQUEST.with(|r| r.borrow_mut().clear());
        toggle(&by_id("fileProgress").unwrap(), "hidden", true);
        toggle(&by_id("fileCancel").unwrap(), "hidden", true);
        load(false).await;
        if epoch != EPOCH.with(Cell::get) {
            return;
        }
        match result {
            Ok(v) => {
                let message = if b(&v, "cancelled") {
                    String::from("파일 선택을 취소했어요.")
                } else if channel == "files:download" {
                    format!("‘{}’ 파일을 저장했어요.", s(&v, "savedName"))
                } else if channel == "files:upload" || channel == "files:retry" {
                    String::from("Drive 파일의 내용과 저장 결과를 확인했어요.")
                } else if !s(&v, "message").is_empty() {
                    s(&v, "message")
                } else if channel == "files:discard-import" {
                    String::from(
                        "가져오기 대기 기록을 지웠어요. 원본과 이미 저장된 사본은 유지돼요.",
                    )
                } else if channel == "files:discard" {
                    String::from(
                        "이 기기의 전송 대기 사본을 지웠어요. 원본과 Drive 파일은 유지돼요.",
                    )
                } else {
                    String::from("Drive에서 파일을 열었어요.")
                };
                set_text("fileMessage", &message);
            }
            Err(e) => set_text("fileMessage", &e),
        }
    });
}
pub fn bind() {
    on(&by_id("fileAnalyze").unwrap(), "click", |_| {
        if let Some(team) = selected() {
            super::analysis::open(team, None);
        }
    });
    for (id, ch) in [("fileUpload", "files:upload")] {
        on(&by_id(id).unwrap(), "click", move |_| action(ch, json!({})));
    }
    on(&by_id("filePick").unwrap(), "click", |_| {
        if !BUSY.with(Cell::get) {
            if let Some(team) = selected() {
                super::drive_picker::files(team);
            }
        }
    });
    on(&by_id("filesBack").unwrap(), "click", |_| {
        if !BUSY.with(Cell::get) {
            close();
        }
    });
    on(&by_id("fileRefresh").unwrap(), "click", |_| {
        spawn(load(false))
    });
    on(&by_id("fileMore").unwrap(), "click", |_| spawn(load(true)));
    on(&by_id("fileCancel").unwrap(), "click", |_| {
        spawn(async {
            let _ = call("files:cancel", Value::Null).await;
        })
    });
    listen("files:progress", |v| {
        if v["epoch"].as_u64() != Some(EPOCH.with(Cell::get))
            || s(&v, "request") != REQUEST.with(|r| r.borrow().clone())
            || !BUSY.with(Cell::get)
        {
            return;
        }
        let done = v["done"].as_u64().unwrap_or(0);
        let total = v["total"].as_u64().unwrap_or(0);
        if total > 0 {
            let percent = done.saturating_mul(100) / total;
            let _ = by_id("fileProgress")
                .unwrap()
                .set_attribute("value", &percent.min(100).to_string());
        }
        set_text(
            "fileMessage",
            match s(&v, "phase").as_str() {
                "sending" => "파일을 보내고 있어요…",
                "receiving" => "파일을 받고 있어요…",
                "verifying" => "Drive에 저장된 내용을 확인하고 있어요…",
                "copying" => "팀 폴더에 사본을 추가하고 있어요…",
                _ => "전송 사본을 준비하고 있어요…",
            },
        );
    });
}
pub fn import(epoch: u64, folder: &str, ids: Vec<String>) {
    if epoch == EPOCH.with(Cell::get) && selected().is_some_and(|s| s["folderId"] == folder) {
        action("files:import", json!({"fileIds":ids}));
    }
}
