//! Small shared board; server verifies account, Drive access, and edit heads.
use super::{b, s};
use crate::{api::call, dom::*};
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
thread_local! {
    static EPOCH:Cell<u64>=const{Cell::new(0)};
    static GENERATION:Cell<u64>=const{Cell::new(0)};
    static BUSY:Cell<bool>=const{Cell::new(false)};
    static READY:Cell<bool>=const{Cell::new(false)};
    static TEAM:RefCell<String>=const{RefCell::new(String::new())};
    static TEAMS:RefCell<Vec<Value>>=const{RefCell::new(vec![])};
    static BOARD:RefCell<Value>=const{RefCell::new(Value::Null)};
    static EDIT:RefCell<Option<Value>>=const{RefCell::new(None)};
    static AI_GENERATION:Cell<u64>=const{Cell::new(0)};
    static AI_FOR:RefCell<String>=const{RefCell::new(String::new())};
    static AI_CONSENT_REQUIRED:Cell<bool>=const{Cell::new(false)};
}
fn generation() -> u64 {
    GENERATION.with(|g| {
        g.set(g.get() + 1);
        g.get()
    })
}
fn selected() -> String {
    TEAM.with(|t| t.borrow().clone())
}
fn active() -> bool {
    by_id("tab-collab").unwrap().class_list().contains("active")
}
fn controls() {
    let busy = BUSY.with(Cell::get);
    let ready = READY.with(Cell::get);
    for button in
        q_all("#collabBoard button, #collabForm input, #collabForm select, #collabForm button")
    {
        set_disabled(&button, busy || !ready);
    }
    for id in ["collabProjectNew", "collabTaskNew"] {
        set_disabled(&by_id(id).unwrap(), busy || !ready);
    }
    set_disabled(
        &by_id("collabRefresh").unwrap(),
        busy || selected().is_empty(),
    );
    set_disabled(&by_id("collabTeam").unwrap(), busy);
    set_disabled(&by_id("collabFormCancel").unwrap(), busy);
    set_disabled(&by_id("teamAiAnalyze").unwrap(), selected().is_empty());
    for button in q_all("#collabBoard [data-calendar-task]") {
        set_disabled(
            &button,
            busy || BOARD.with(|v| v.borrow().is_null() || !s(&v.borrow(), "error").is_empty()),
        );
    }
}
fn reset_form() {
    EDIT.with(|e| *e.borrow_mut() = None);
    toggle(&by_id("collabForm").unwrap(), "hidden", true);
}
fn reset_ai() {
    AI_GENERATION.with(|g| g.set(g.get() + 1));
    AI_FOR.with(|f| f.borrow_mut().clear());
    AI_CONSENT_REQUIRED.with(|v| v.set(false));
    set_text("teamAiMessage", "팀 Claude 연결을 확인할 수 있어요.");
    set_text("teamAiRefresh", "연결 확인 ↻");
    set_disabled(&by_id("teamAiRefresh").unwrap(), selected().is_empty());
}
async fn ai_status(connect: bool) {
    let folder = selected();
    if folder.is_empty() {
        return;
    }
    let epoch = EPOCH.with(Cell::get);
    let gen = AI_GENERATION.with(|g| {
        g.set(g.get() + 1);
        g.get()
    });
    AI_FOR.with(|f| *f.borrow_mut() = folder.clone());
    set_disabled(&by_id("teamAiRefresh").unwrap(), true);
    set_text("teamAiMessage", "팀 Claude 연결을 확인하고 있어요…");
    let response = call(
        if connect {
            "team-ai:connect"
        } else {
            "team-ai:status"
        },
        json!({"epoch":epoch,"folderId":folder}),
    )
    .await;
    if gen != AI_GENERATION.with(Cell::get)
        || epoch != EPOCH.with(Cell::get)
        || folder != selected()
    {
        return;
    }
    set_disabled(&by_id("teamAiRefresh").unwrap(), false);
    let message = match response {
        Ok(v) if v["epoch"] == epoch && v["folderId"] == folder => {
            let consent = b(&v, "requiresConsent");
            AI_CONSENT_REQUIRED.with(|r| r.set(consent));
            set_text(
                "teamAiRefresh",
                if consent {
                    "팀 AI 연결 →"
                } else {
                    "연결 확인 ↻"
                },
            );
            if consent {
                "팀 AI를 처음 연결할 때 Google 권한 전달 범위를 확인해 주세요.".into()
            } else if !b(&v, "configured") || !b(&v, "ready") {
                "팀 Claude를 준비하고 있어요. 개인 AI 대화는 계속 사용할 수 있어요.".into()
            } else {
                format!(
                    "팀 Claude 사용 설정을 확인했어요. 일일 한도 {} / {}회 요청했어요.",
                    v["used"].as_u64().unwrap_or(0),
                    v["dailyRequests"].as_u64().unwrap_or(0)
                )
            }
        }
        Ok(_) => return,
        Err(e) => e,
    };
    set_text("teamAiMessage", &message);
}
pub fn account(v: &Value) {
    let epoch = v["epoch"].as_u64().unwrap_or(0);
    if epoch < EPOCH.with(Cell::get) {
        return;
    }
    if EPOCH.with(|e| e.replace(epoch)) != epoch || !b(v, "signedIn") {
        generation();
        TEAM.with(|t| t.borrow_mut().clear());
        TEAMS.with(|t| t.borrow_mut().clear());
        BOARD.with(|v| *v.borrow_mut() = Value::Null);
        BUSY.with(|b| b.set(false));
        READY.with(|r| r.set(false));
        reset_form();
        reset_ai();
        clear(&by_id("collabBoard").unwrap());
        clear(&by_id("collabTeam").unwrap());
        set_text(
            "collabMessage",
            "파일 화면에서 Google로 로그인하고 팀을 연결해 주세요.",
        );
        controls();
    }
}
pub fn teams(rows: &[Value]) {
    TEAMS.with(|t| *t.borrow_mut() = rows.to_vec());
    let before = selected();
    let usable = rows
        .iter()
        .filter(|t| b(t, "accessVerified"))
        .collect::<Vec<_>>();
    let next = if usable.iter().any(|t| s(t, "folderId") == before) {
        before.clone()
    } else {
        usable.first().map(|t| s(t, "folderId")).unwrap_or_default()
    };
    let input = by_id("collabTeam").unwrap();
    clear(&input);
    let _ = input.append_child(
        &el("option")
            .attr("value", "")
            .text(if usable.is_empty() {
                "연결된 팀이 없어요"
            } else {
                "팀 선택"
            })
            .build(),
    );
    for team in usable {
        let _ = input.append_child(
            &el("option")
                .attr("value", &s(team, "folderId"))
                .text(&s(team, "name"))
                .build(),
        );
    }
    set_val(&input, &next);
    if next != before {
        switch(next);
    } else {
        controls();
    }
}
fn switch(folder: String) {
    generation();
    TEAM.with(|t| *t.borrow_mut() = folder);
    BOARD.with(|b| *b.borrow_mut() = Value::Null);
    READY.with(|r| r.set(false));
    BUSY.with(|b| b.set(false));
    reset_form();
    reset_ai();
    clear(&by_id("collabBoard").unwrap());
    controls();
    if selected().is_empty() {
        set_text(
            "collabMessage",
            "파일 화면에서 팀을 연결하면 프로젝트와 할 일을 함께 정리할 수 있어요.",
        );
    } else if active() {
        spawn(load(false));
    }
}
pub fn select(folder: &str) {
    if selected() != folder
        && TEAMS.with(|t| {
            t.borrow()
                .iter()
                .any(|t| s(t, "folderId") == folder && b(t, "accessVerified"))
        })
    {
        if EDIT.with(|e| e.borrow().is_some()) {
            return;
        }
        set_val(&by_id("collabTeam").unwrap(), folder);
        switch(folder.into());
    }
}
fn status(value: &str) -> &'static str {
    match value {
        "doing" => "진행 중",
        "done" => "완료",
        _ => "시작 전",
    }
}
fn project_name(id: &str) -> String {
    BOARD.with(|v| {
        v.borrow()["items"]
            .as_array()
            .and_then(|items| items.iter().find(|i| i["id"] == id))
            .map(|i| s(&i["variants"][0]["entity"], "title"))
            .unwrap_or_default()
    })
}
fn describe(e: &Value) -> String {
    let mut parts = vec![status(&s(e, "status")).into()];
    for value in [
        project_name(&s(e, "projectId")),
        s(e, "assignee"),
        s(e, "dueDate"),
    ] {
        if !value.is_empty() {
            parts.push(value);
        }
    }
    parts.join(" · ")
}
fn editor(entity: Value, heads: Value, conflict: bool) {
    if !READY.with(Cell::get) || BUSY.with(Cell::get) {
        return;
    }
    if EDIT.with(|e| e.borrow().is_some()) && !confirm("작성 중인 내용을 닫고 다른 항목을 열까요?")
    {
        return;
    }
    let kind = s(&entity, "kind");
    set_text(
        "collabFormTitle",
        if conflict {
            "동시 변경을 확인하고 정리"
        } else if kind == "project" {
            "프로젝트 정리"
        } else {
            "할 일 정리"
        },
    );
    set_text(
        "collabFormHint",
        if conflict {
            "다른 내용도 아래 목록에 남아 있어요. 모두 확인한 뒤 이 내용으로 정리해 저장해 주세요."
        } else {
            "팀원에게 공유할 내용이에요. 담당자 표시는 Google 공유 권한이나 캘린더를 변경하지 않아요."
        },
    );
    for (id, key) in [
        ("collabTitle", "title"),
        ("collabAssignee", "assignee"),
        ("collabDue", "dueDate"),
        ("collabStatus", "status"),
    ] {
        set_val(&by_id(id).unwrap(), &s(&entity, key));
    }
    let projects = by_id("collabProject").unwrap();
    clear(&projects);
    let _ = projects.append_child(&el("option").attr("value", "").text("프로젝트 없음").build());
    BOARD.with(|v| {
        if let Some(items) = v.borrow()["items"].as_array() {
            for item in items.iter().filter(|i| i["kind"] == "project") {
                let title = s(&item["variants"][0]["entity"], "title");
                let label = if b(item, "conflict") {
                    format!("{title} · 동시 변경 확인 필요")
                } else {
                    title
                };
                let _ = projects.append_child(
                    &el("option")
                        .attr("value", &s(item, "id"))
                        .text(&label)
                        .build(),
                );
            }
        }
    });
    set_val(&projects, &s(&entity, "projectId"));
    EDIT.with(|e| *e.borrow_mut() = Some(json!({"entity":entity,"heads":heads})));
    toggle(
        &by_id("collabTaskFields").unwrap(),
        "hidden",
        kind == "project",
    );
    toggle(&by_id("collabForm").unwrap(), "hidden", false);
    controls();
    by_id("collabForm").unwrap().scroll_into_view();
    let _ = by_id("collabTitle").unwrap().focus();
}
fn render(v: &Value) {
    let items = v["items"].as_array().cloned().unwrap_or_default();
    let root = by_id("collabBoard").unwrap();
    clear(&root);
    for kind in ["project", "task"] {
        let entries = items
            .iter()
            .filter(|i| i["kind"] == kind)
            .collect::<Vec<_>>();
        let section = el("section").cls("work-group").kid(el("h2").text(&format!(
            "{} · {}",
            if kind == "project" {
                "프로젝트"
            } else {
                "할 일"
            },
            entries.len()
        )));
        let mut section = section;
        for item in entries {
            let conflict = b(item, "conflict");
            let heads = item["heads"].clone();
            let mut card = el("article").cls(if conflict {
                "work-card work-conflict"
            } else {
                "work-card"
            });
            if conflict {
                card = card.kid(
                    el("p")
                        .cls("work-conflict-label")
                        .text("같은 항목에 동시 변경이 있어요. 두 내용을 확인해 주세요."),
                );
            }
            for variant in item["variants"].as_array().into_iter().flatten() {
                let entity = variant["entity"].clone();
                let edit_entity = entity.clone();
                let edit_heads = heads.clone();
                let mut row = el("div")
                    .cls("work-row")
                    .kid(
                        el("div")
                            .cls("work-info")
                            .kid(el("strong").text(&s(&entity, "title")))
                            .kid(el("small").text(&describe(&entity))),
                    )
                    .kid(
                        el("button")
                            .cls("text-button")
                            .attr("type", "button")
                            .flag("disabled", !READY.with(Cell::get))
                            .text(if conflict {
                                "이 내용으로 정리 →"
                            } else {
                                "수정 →"
                            })
                            .click(move |_| {
                                editor(edit_entity.clone(), edit_heads.clone(), conflict)
                            }),
                    );
                if !conflict && kind == "task" {
                    let task = s(&entity, "id");
                    let folder = selected();
                    let epoch = EPOCH.with(Cell::get);
                    row = row.kid(
                        el("button")
                            .cls("text-button")
                            .attr("type", "button")
                            .attr("data-calendar-task", "")
                            .text("내 일정 ＋")
                            .click(move |_| {
                                super::agenda::open(epoch, folder.clone(), task.clone())
                            }),
                    );
                }
                card = card.kid(row);
            }
            section = section.kid(card);
        }
        let _ = root.append_child(&section.build());
    }
    if items.is_empty() {
        let _ = root.append_child(
            &el("p")
                .cls("work-empty")
                .text("아직 정리한 내용이 없어요. 작은 프로젝트나 할 일부터 추가해 보세요.")
                .build(),
        );
    }
    controls();
}
fn accept(v: Value) {
    let error = s(&v, "error");
    let pending = v["pending"].as_u64().unwrap_or(0);
    READY.with(|r| r.set(error.is_empty() && b(&v["team"], "canUpload")));
    set_text(
        "collabMessage",
        &if !error.is_empty() {
            format!("{error} · 이 PC의 공유 대기 {pending}개")
        } else if pending > 0 {
            format!("이 PC에 공유 대기 {pending}개가 있어요. 동기화를 눌러 이어 보내 주세요.")
        } else if !b(&v["team"], "canUpload") {
            "최신 내용을 확인했어요. 이 팀은 보기만 할 수 있어요.".into()
        } else {
            "Drive의 최신 내용을 확인했어요. 저장한 변경은 팀원과 공유돼요.".into()
        },
    );
    let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(
        v["lastSync"].as_f64().unwrap_or(0.),
    ));
    let checked = format!("{:02}:{:02} 확인", date.get_hours(), date.get_minutes());
    set_text(
        "collabSyncNote",
        if pending > 0 {
            "공유 대기 있음"
        } else {
            &checked
        },
    );
    BOARD.with(|b| *b.borrow_mut() = v.clone());
    render(&v);
    if AI_FOR.with(|f| f.borrow().as_str() != selected()) {
        spawn(ai_status(false));
    }
}
pub async fn load(sync: bool) {
    let folder = selected();
    if folder.is_empty() || BUSY.with(|b| b.replace(true)) {
        return;
    }
    let epoch = EPOCH.with(Cell::get);
    let generation = generation();
    controls();
    set_text(
        "collabMessage",
        "팀 권한과 최신 협업 내용을 확인하고 있어요…",
    );
    let result = call(
        if sync { "collab:sync" } else { "collab:load" },
        json!({"epoch":epoch,"folderId":folder}),
    )
    .await;
    if generation != GENERATION.with(Cell::get) || epoch != EPOCH.with(Cell::get) {
        return;
    }
    BUSY.with(|b| b.set(false));
    match result {
        Ok(v) if v["epoch"] == epoch && v["folderId"] == folder => accept(v),
        Ok(_) => (),
        Err(e) => {
            READY.with(|r| r.set(false));
            clear(&by_id("collabBoard").unwrap());
            set_text(
                "collabMessage",
                &format!("{e} 저장한 내용과 작성 중인 입력은 유지했어요."),
            );
            controls();
        }
    }
}
async fn save() {
    let Some(mut edit) = EDIT.with(|e| e.borrow().clone()) else {
        return;
    };
    if !READY.with(Cell::get) || BUSY.with(|b| b.replace(true)) {
        return;
    }
    for (id, key) in [
        ("collabTitle", "title"),
        ("collabStatus", "status"),
        ("collabProject", "projectId"),
        ("collabAssignee", "assignee"),
        ("collabDue", "dueDate"),
    ] {
        edit["entity"][key] = json!(val(&by_id(id).unwrap()).trim());
    }
    if edit["entity"]["kind"] == "project" {
        for key in ["projectId", "assignee", "dueDate"] {
            edit["entity"][key] = json!("");
        }
    }
    let epoch = EPOCH.with(Cell::get);
    let folder = selected();
    let generation = generation();
    controls();
    set_text(
        "collabMessage",
        "내용을 보관하고 Drive 저장 결과를 확인하고 있어요…",
    );
    let result = call(
        "collab:save",
        json!({"epoch":epoch,"folderId":folder,"edit":edit}),
    )
    .await;
    if generation != GENERATION.with(Cell::get) || epoch != EPOCH.with(Cell::get) {
        return;
    }
    BUSY.with(|b| b.set(false));
    match result {
        Ok(v) if v["epoch"] == epoch && v["folderId"] == folder => {
            reset_form();
            accept(v);
        }
        Ok(_) => (),
        Err(e) => {
            set_text("collabMessage", &e);
            controls();
        }
    }
}
pub fn bind() {
    on(&by_id("teamAiAnalyze").unwrap(), "click", |_| {
        let folder = selected();
        if let Some(team) =
            TEAMS.with(|t| t.borrow().iter().find(|t| t["folderId"] == folder).cloned())
        {
            super::analysis::open(team, None);
        }
    });
    on(&by_id("teamAiRefresh").unwrap(), "click", |_| {
        let connect = AI_CONSENT_REQUIRED.with(Cell::get);
        if connect && !super::confirm_team_ai_connection() {
            return;
        }
        if connect {
            AI_CONSENT_REQUIRED.with(|r| r.set(false));
            set_text("teamAiRefresh", "연결 확인 ↻");
        }
        spawn(ai_status(connect));
    });
    on(&by_id("collabTeam").unwrap(), "change", |_| {
        let next = val(&by_id("collabTeam").unwrap());
        if EDIT.with(|e| e.borrow().is_some()) && !confirm("작성 중인 내용을 닫고 팀을 바꿀까요?")
        {
            set_val(&by_id("collabTeam").unwrap(), &selected());
            return;
        }
        switch(next);
    });
    on(&by_id("collabRefresh").unwrap(), "click", |_| {
        spawn(load(true))
    });
    for (id, kind) in [("collabProjectNew", "project"), ("collabTaskNew", "task")] {
        on(&by_id(id).unwrap(), "click", move |_| {
            editor(
                json!({"id":"","kind":kind,"title":"","projectId":"","assignee":"","dueDate":"","status":"todo"}),
                json!([]),
                false,
            )
        });
    }
    on(&by_id("collabForm").unwrap(), "submit", |e| {
        e.prevent_default();
        spawn(save());
    });
    on(&by_id("collabFormCancel").unwrap(), "click", |_| {
        reset_form()
    });
    every(30000, || {
        if active() && !BUSY.with(Cell::get) && EDIT.with(|e| e.borrow().is_none()) {
            spawn(load(false));
        }
    });
    controls();
}
pub fn apply_review(v: &Value) {
    if v["epoch"].as_u64() == Some(EPOCH.with(Cell::get)) && v["folderId"] == selected() {
        accept(v.clone());
    }
}
