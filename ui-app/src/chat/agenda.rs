use super::{b, s};
use crate::{api::call, dom::*};
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use wasm_bindgen::JsCast;
use web_sys::HtmlDialogElement;
thread_local! {
    static EPOCH:Cell<u64>=const{Cell::new(0)};
    static GEN:Cell<u64>=const{Cell::new(0)};
    static BUSY:Cell<bool>=const{Cell::new(false)};
    static LINKING:Cell<bool>=const{Cell::new(false)};
    static SAVING:Cell<bool>=const{Cell::new(false)};
    static BLOCKED:Cell<bool>=const{Cell::new(false)};
    static VERIFY:Cell<bool>=const{Cell::new(false)};
    static CONTEXT:RefCell<Value>=const{RefCell::new(Value::Null)};
}
fn dialog() -> HtmlDialogElement {
    by_id("calendarDialog").unwrap().unchecked_into()
}
fn forget(c: &Value) {
    if !s(c, "reviewId").is_empty() {
        let args = json!({"epoch":c["epoch"],"folderId":c["folderId"],"reviewId":c["reviewId"]});
        spawn(async move {
            let _ = call("calendar:forget", args).await;
        });
    }
}
fn acknowledgments() {
    for id in ["calendarAck", "calendarDeleteAck"] {
        set_checked(&by_id(id).unwrap(), false);
    }
}
fn reset() {
    GEN.with(|g| g.set(g.get() + 1));
    CONTEXT.with(|c| {
        forget(&c.borrow());
        *c.borrow_mut() = Value::Null;
    });
    BUSY.with(|b| b.set(false));
    SAVING.with(|b| b.set(false));
    BLOCKED.with(|b| b.set(false));
    VERIFY.with(|b| b.set(false));
    if LINKING.with(|l| l.replace(false)) {
        spawn(async {
            let _ = call("calendar:cancel", Value::Null).await;
        });
    }
    acknowledgments();
    set_val(&by_id("calendarDate").unwrap(), "");
    for id in [
        "calendarTaskTitle",
        "calendarAccount",
        "calendarCurrentTitle",
        "calendarCurrentDates",
    ] {
        set_text(id, "");
    }
    let _ = by_id("calendarDeletePanel")
        .unwrap()
        .remove_attribute("open");
}
fn controls() {
    let busy = BUSY.with(Cell::get);
    let blocked = BLOCKED.with(Cell::get);
    CONTEXT.with(|c| {
        let c = c.borrow();
        let ready = c["heads"].is_array();
        let state = s(&c["calendar"], "state");
        let done = c["entity"]["status"] == "done";
        let manage =
            state == "existing" && b(&c["calendar"], "manageable") && !s(&c, "reviewId").is_empty();
        let create = state == "missing" && !done;
        let edit = manage && !done;
        let can_date = ready && !done && (create || edit || state == "unconnected");
        set_disabled(
            &by_id("calendarDate").unwrap(),
            busy || blocked || !can_date,
        );
        set_disabled(&by_id("calendarConnect").unwrap(), busy || !ready);
        set_text(
            "calendarConnect",
            if b(&c, "connected") {
                "일정 권한 확인 ↻"
            } else {
                "내 Calendar 연결 →"
            },
        );
        set_disabled(
            &by_id("calendarReload").unwrap(),
            busy || c["taskId"].is_null(),
        );
        toggle(
            &by_id("calendarCurrent").unwrap(),
            "hidden",
            state != "existing",
        );
        toggle(
            &by_id("calendarSaveSection").unwrap(),
            "hidden",
            !(create || edit),
        );
        set_text(
            "calendarSaveLabel",
            if edit {
                "현재 일정과 변경할 내용을 확인했고, 업무 제목과 선택한 날짜로 수정하겠습니다."
            } else {
                "제목과 날짜를 확인했고, 내 기본 캘린더에 추가하겠습니다."
            },
        );
        set_text(
            "calendarAdd",
            if edit {
                "확인한 내용으로 수정 →"
            } else {
                "내 캘린더에 추가 →"
            },
        );
        let date = val(&by_id("calendarDate").unwrap());
        let changed =
            c["calendar"]["title"] != c["entity"]["title"] || c["calendar"]["date"] != date;
        set_disabled(
            &by_id("calendarAck").unwrap(),
            busy || blocked || !(create || edit),
        );
        set_disabled(
            &by_id("calendarAdd").unwrap(),
            busy || blocked
                || !b(&c, "connected")
                || !(create || edit)
                || date.is_empty()
                || !checked(&by_id("calendarAck").unwrap())
                || (edit && !changed),
        );
        toggle(&by_id("calendarDeletePanel").unwrap(), "hidden", !manage);
        set_disabled(
            &by_id("calendarDeleteAck").unwrap(),
            busy || blocked || !manage,
        );
        set_disabled(
            &by_id("calendarDelete").unwrap(),
            busy || blocked || !manage || !checked(&by_id("calendarDeleteAck").unwrap()),
        );
    });
    toggle(
        &by_id("calendarCancel").unwrap(),
        "hidden",
        !LINKING.with(Cell::get),
    );
    toggle(
        &by_id("calendarVerify").unwrap(),
        "hidden",
        !VERIFY.with(Cell::get),
    );
    set_disabled(&by_id("calendarVerify").unwrap(), busy);
}
pub fn account(v: &Value) {
    let epoch = v["epoch"].as_u64().unwrap_or(0);
    if epoch < EPOCH.with(Cell::get) {
        return;
    }
    if EPOCH.with(|e| e.replace(epoch)) != epoch || !b(v, "signedIn") {
        reset();
        dialog().close();
    }
}
fn close() {
    if SAVING.with(Cell::get)&&!confirm("이미 전송한 일정의 추가·수정·삭제는 창을 닫아도 완료될 수 있어요. 내 기본 캘린더에서 확인해 주세요. 닫을까요?"){return;}
    dialog().close();
}
pub fn open(epoch: u64, folder: String, task: String) {
    if dialog().open() || epoch != EPOCH.with(Cell::get) {
        return;
    }
    reset();
    CONTEXT.with(|c| *c.borrow_mut() = json!({"epoch":epoch,"folderId":folder,"taskId":task}));
    set_text("calendarMessage", "최신 할 일과 일정을 확인하고 있어요…");
    controls();
    if dialog().show_modal().is_err() {
        reset();
        return;
    }
    spawn(reload(false, None));
}
async fn reload(keep_date: bool, notice: Option<String>) {
    if BUSY.with(|b| b.replace(true)) {
        return;
    }
    let old = CONTEXT.with(|c| c.borrow().clone());
    let date = if keep_date {
        val(&by_id("calendarDate").unwrap())
    } else {
        String::new()
    };
    acknowledgments();
    BLOCKED.with(|b| b.set(true));
    controls();
    let gen = GEN.with(Cell::get);
    set_text(
        "calendarMessage",
        "Google에서 최신 일정과 업무를 확인하고 있어요…",
    );
    let result = call(
        "calendar:preview",
        json!({"epoch":old["epoch"],"folderId":old["folderId"],"taskId":old["taskId"]}),
    )
    .await;
    if gen != GEN.with(Cell::get) {
        if let Ok(v) = result {
            forget(&v);
        }
        return;
    }
    BUSY.with(|b| b.set(false));
    match result {
        Ok(v)
            if v["epoch"] == old["epoch"]
                && v["folderId"] == old["folderId"]
                && v["taskId"] == old["taskId"] =>
        {
            forget(&old);
            set_text("calendarTaskTitle", &s(&v["entity"], "title"));
            set_text(
                "calendarAccount",
                &format!("{} · 내 기본 캘린더", s(&v, "email")),
            );
            let suggested = if !date.is_empty() {
                date
            } else if !s(&v["entity"], "dueDate").is_empty() {
                s(&v["entity"], "dueDate")
            } else {
                s(&v["calendar"], "date")
            };
            set_val(&by_id("calendarDate").unwrap(), &suggested);
            set_text("calendarCurrentTitle", &s(&v["calendar"], "title"));
            let dates = if b(&v["calendar"], "manageable") {
                format!("{} (종일)", s(&v["calendar"], "date"))
            } else if !s(&v["calendar"], "date").is_empty() {
                format!(
                    "{}부터 {} 전까지 (종일)",
                    s(&v["calendar"], "date"),
                    s(&v["calendar"], "endDate")
                )
            } else {
                format!(
                    "{} → {} {}",
                    s(&v["calendar"], "dateTime"),
                    s(&v["calendar"], "endDateTime"),
                    s(&v["calendar"], "timeZone")
                )
            };
            set_text("calendarCurrentDates", &dates);
            let message = if !s(&v, "calendarError").is_empty() {
                s(&v, "calendarError")
            } else {
                match s(&v["calendar"],"state").as_str(){"unconnected"=>"Calendar 일정 권한을 연결하면 기존 일정을 먼저 확인해요.".into(),"missing" if v["entity"]["status"]=="done"=>"완료한 업무에는 새 일정을 추가하지 않아요.".into(),"missing"=>"내 기본 캘린더에 등록된 일정이 없어요. 날짜를 확인해 추가할 수 있어요.".into(),"deleted"=>"삭제된 일정이에요. 자동으로 다시 만들지 않아요.".into(),"existing" if !b(&v["calendar"],"manageable")=>"시간·반복·참석자·추가 내용 또는 일정 버전을 확인해야 하는 일정이에요. Calendar에서 직접 관리해 주세요.".into(),"existing"=>"현재 Calendar 일정과 위의 업무 제목·선택한 날짜를 비교해 주세요. 확인한 내용만 수정하거나 내 일정만 삭제해요.".into(),_=>"일정을 확인하지 못했어요. 연결과 최신 내용을 다시 확인해 주세요.".into()}
            };
            CONTEXT.with(|c| *c.borrow_mut() = v);
            BLOCKED.with(|b| b.set(false));
            VERIFY.with(|v| v.set(false));
            set_text(
                "calendarMessage",
                &if let Some(notice) = notice {
                    format!("{notice} {message}")
                } else {
                    message
                },
            );
        }
        Ok(v) => {
            forget(&v);
            set_text(
                "calendarMessage",
                "계정이나 팀이 바뀌었어요. 다시 선택해 주세요.",
            );
        }
        Err(e) => set_text("calendarMessage", &e),
    }
    controls();
}
async fn connect() {
    if BUSY.with(|b| b.replace(true)) {
        return;
    }
    LINKING.with(|l| l.set(true));
    controls();
    set_text(
        "calendarMessage",
        "브라우저에서 현재 Google 계정의 일정 권한을 허용해 주세요. 기존 Drive 로그인은 유지해요.",
    );
    let gen = GEN.with(Cell::get);
    let epoch = CONTEXT.with(|c| c.borrow()["epoch"].clone());
    let result = call("calendar:connect", json!({"epoch":epoch})).await;
    if gen != GEN.with(Cell::get) {
        return;
    }
    LINKING.with(|l| l.set(false));
    BUSY.with(|b| b.set(false));
    match result {
        Ok(v) if v["epoch"] == epoch && b(&v, "connected") => {
            reload(true, Some("Calendar 연결을 확인했어요.".into())).await
        }
        Ok(_) => set_text(
            "calendarMessage",
            "Calendar 연결 상태를 다시 확인해 주세요.",
        ),
        Err(e) => set_text("calendarMessage", &e),
    }
    controls();
}
async fn write(remove: bool) {
    if BLOCKED.with(Cell::get) || BUSY.with(Cell::get) {
        return;
    }
    let c = CONTEXT.with(|c| c.borrow().clone());
    let existing = c["calendar"]["state"] == "existing";
    if !b(&c, "connected")
        || !(if remove {
            checked(&by_id("calendarDeleteAck").unwrap())
        } else {
            checked(&by_id("calendarAck").unwrap())
        })
    {
        return;
    }
    if remove || existing {
        if !b(&c["calendar"], "manageable") || s(&c, "reviewId").is_empty() {
            return;
        }
    } else if c["calendar"]["state"] != "missing" {
        return;
    }
    let date = val(&by_id("calendarDate").unwrap());
    let action = if remove {
        json!({"kind":"delete"})
    } else {
        json!({"kind":"update","date":date})
    };
    let (route, args) = if existing {
        (
            "calendar:change",
            json!({"epoch":c["epoch"],"folderId":c["folderId"],"change":{"reviewId":c["reviewId"],"action":action,"confirmed":true}}),
        )
    } else {
        (
            "calendar:add",
            json!({"epoch":c["epoch"],"folderId":c["folderId"],"add":{"taskId":c["taskId"],"heads":c["heads"],"date":date,"confirmed":true}}),
        )
    };
    BUSY.with(|b| b.set(true));
    SAVING.with(|b| b.set(true));
    BLOCKED.with(|b| b.set(true));
    controls();
    let gen = GEN.with(Cell::get);
    set_text(
        "calendarMessage",
        "확인한 일정에 요청을 보내고 결과를 확인하고 있어요…",
    );
    let result = call(route, args).await;
    if gen != GEN.with(Cell::get) {
        return;
    }
    BUSY.with(|b| b.set(false));
    SAVING.with(|b| b.set(false));
    match result {
        Ok(v) if v["epoch"] == c["epoch"] && v["folderId"] == c["folderId"] => {
            if existing {
                change_result(v).await;
            } else {
                let message = match s(&v, "state").as_str() {
                    "registered" => "내 기본 캘린더에 추가한 것을 확인했어요.",
                    "existing" => "이미 등록된 일정을 유지했어요.",
                    "deleted" => "삭제된 일정은 다시 만들지 않았어요.",
                    _ => "등록 결과를 최신 일정에서 확인해 주세요.",
                };
                reload(true, Some(message.into())).await;
            }
        }
        Ok(_) => set_text(
            "calendarMessage",
            "계정이 바뀌었어요. 해당 계정의 Calendar에서 결과를 확인해 주세요.",
        ),
        Err(e) => {
            VERIFY.with(|v| v.set(existing));
            set_text(
                "calendarMessage",
                &format!(
                    "{e} {}",
                    if existing {
                        "전송 결과 다시 확인은 조회만 해요."
                    } else {
                        "최신 내용을 확인한 뒤 같은 할 일을 다시 선택해 주세요."
                    }
                ),
            );
        }
    }
    controls();
}
async fn change_result(v: Value) {
    let state = s(&v, "state");
    let message = s(&v, "message");
    if ["updated", "deleted", "unchanged"].contains(&state.as_str()) {
        VERIFY.with(|v| v.set(false));
        reload(true, Some(message)).await;
    } else {
        VERIFY.with(|v| v.set(state == "uncertain" || state == "conflict"));
        BLOCKED.with(|b| b.set(true));
        set_text("calendarMessage", &message);
    }
}
async fn verify() {
    if !VERIFY.with(Cell::get) || BUSY.with(|b| b.replace(true)) {
        return;
    }
    controls();
    let gen = GEN.with(Cell::get);
    let args = CONTEXT.with(|c| {
        let c = c.borrow();
        json!({"epoch":c["epoch"],"folderId":c["folderId"],"reviewId":c["reviewId"]})
    });
    let result = call("calendar:check-change", args).await;
    if gen != GEN.with(Cell::get) {
        return;
    }
    BUSY.with(|b| b.set(false));
    match result {
        Ok(v) => change_result(v).await,
        Err(e) => set_text("calendarMessage", &e),
    }
    controls();
}
pub fn bind() {
    on(&by_id("calendarClose").unwrap(), "click", |_| close());
    on(&dialog(), "cancel", |e| {
        e.prevent_default();
        close();
    });
    on(&dialog(), "close", |_| reset());
    on(&by_id("calendarConnect").unwrap(), "click", |_| {
        spawn(connect())
    });
    on(&by_id("calendarAdd").unwrap(), "click", |_| {
        spawn(write(false))
    });
    on(&by_id("calendarDelete").unwrap(), "click", |_| {
        spawn(write(true))
    });
    on(&by_id("calendarVerify").unwrap(), "click", |_| {
        spawn(verify())
    });
    on(&by_id("calendarReload").unwrap(), "click", |_| {
        spawn(reload(true, None))
    });
    on(&by_id("calendarCancel").unwrap(), "click", |_| {
        spawn(async {
            let _ = call("calendar:cancel", Value::Null).await;
        })
    });
    for id in ["calendarAck", "calendarDeleteAck"] {
        on(&by_id(id).unwrap(), "change", |_| controls());
    }
    on(&by_id("calendarDate").unwrap(), "input", |_| {
        acknowledgments();
        controls();
    });
}
