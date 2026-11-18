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
    static PAID:Cell<bool>=const{Cell::new(false)};
    static READY:Cell<bool>=const{Cell::new(false)};
    static EXISTING:Cell<bool>=const{Cell::new(false)};
    static ATTEMPTED:Cell<bool>=const{Cell::new(false)};
    static CAN_SHARE:Cell<bool>=const{Cell::new(false)};
    static DONE:Cell<bool>=const{Cell::new(false)};
    static TEAM:RefCell<Option<Value>>=const{RefCell::new(None)};
    static REVIEW:RefCell<String>=const{RefCell::new(String::new())};
}
fn dialog() -> HtmlDialogElement {
    by_id("analysisDialog").unwrap().unchecked_into()
}
fn bump() -> u64 {
    GEN.with(|g| {
        g.set(g.get() + 1);
        g.get()
    })
}
fn context() -> Option<Value> {
    TEAM.with(|t|t.borrow().as_ref().map(|t|json!({"epoch":t["epoch"],"folderId":t["folderId"],"reviewId":REVIEW.with(|r|r.borrow().clone())})))
}
fn reset() {
    let old = context();
    bump();
    TEAM.with(|t| *t.borrow_mut() = None);
    REVIEW.with(|r| r.borrow_mut().clear());
    BUSY.with(|v| v.set(false));
    PAID.with(|v| v.set(false));
    READY.with(|v| v.set(false));
    DONE.with(|v| v.set(false));
    ATTEMPTED.with(|v| v.set(false));
    set_val(&by_id("analysisInput").unwrap(), "");
    set_val(&by_id("analysisText").unwrap(), "");
    clear(&by_id("analysisCandidates").unwrap());
    set_text("analysisSummary", "");
    if let Some(args) = old {
        spawn(async move {
            let _ = call("analysis:forget", args).await;
        });
    }
}
fn close() {
    if PAID.with(Cell::get)&&!confirm("분석을 시작한 요청은 중계에서 완료될 수 있어요. 창을 닫으면 결과를 잃을 수 있고, 새 분석에는 비용이 다시 들 수 있어요. 닫을까요?"){return;}
    dialog().close();
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
fn controls() {
    let busy = BUSY.with(Cell::get);
    let count = q_all("#analysisCandidates input[data-choice]")
        .iter()
        .filter(|e| checked(e))
        .count();
    for e in q_all("#analysisDialog input, #analysisDialog select, #analysisDialog textarea, #analysisDialog button"){set_disabled(&e,busy);}
    set_disabled(&by_id("analysisClose").unwrap(), false);
    set_disabled(
        &by_id("analysisRun").unwrap(),
        busy || !READY.with(Cell::get)
            || EXISTING.with(Cell::get)
            || ATTEMPTED.with(Cell::get)
            || REVIEW.with(|r| r.borrow().is_empty())
            || !checked(&by_id("analysisSendAck").unwrap()),
    );
    set_disabled(
        &by_id("analysisSave").unwrap(),
        busy || DONE.with(Cell::get)
            || !CAN_SHARE.with(Cell::get)
            || count == 0
            || !checked(&by_id("analysisShareAck").unwrap()),
    );
    set_text("analysisSave", &format!("추가 {count}"));
}
pub fn open(team: Value, file: Option<String>) {
    if dialog().open() || team["epoch"].as_u64() != Some(EPOCH.with(Cell::get)) {
        return;
    }
    reset();
    TEAM.with(|t| *t.borrow_mut() = Some(team.clone()));
    EXISTING.with(|e| e.set(false));
    CAN_SHARE.with(|c| c.set(false));
    set_text("analysisTeam", &s(&team, "name"));
    set_text("analysisMessage", "");
    set_text("analysisConnection", "");
    for (id, hidden) in [
        ("analysisEntry", file.is_some()),
        ("analysisPreview", true),
        ("analysisResults", true),
        ("analysisConnect", true),
    ] {
        toggle(&by_id(id).unwrap(), "hidden", hidden);
    }
    for id in ["analysisSendAck", "analysisShareAck"] {
        set_checked(&by_id(id).unwrap(), false);
    }
    if dialog().show_modal().is_err() {
        reset();
        return;
    }
    controls();
    if let Some(file_id) = file {
        spawn(prepare(json!({"kind":"drive","fileId":file_id})));
    } else {
        let _ = by_id("analysisInput").unwrap().focus();
    }
}
async fn connection(connect: bool) {
    let Some(args) = context() else {
        return;
    };
    let gen = GEN.with(Cell::get);
    let result = call(
        if connect {
            "team-ai:connect"
        } else {
            "team-ai:status"
        },
        args,
    )
    .await;
    if gen != GEN.with(Cell::get) {
        return;
    }
    match result {
        Ok(v) => {
            READY.with(|r| r.set(b(&v, "ready")));
            let consent = b(&v, "requiresConsent");
            toggle(&by_id("analysisConnect").unwrap(), "hidden", !consent);
            set_text(
                "analysisConnection",
                if consent {
                    "연결 필요"
                } else if b(&v, "ready") {
                    ""
                } else {
                    "연결 대기"
                },
            );
        }
        Err(e) => {
            READY.with(|r| r.set(false));
            set_text("analysisConnection", &e);
        }
    }
    controls();
}
async fn prepare(source: Value) {
    if BUSY.with(|b| b.replace(true)) {
        return;
    }
    let Some(mut args) = context() else {
        BUSY.with(|b| b.set(false));
        return;
    };
    args["source"] = source;
    let gen = bump();
    controls();
    set_text("analysisMessage", "불러오는 중…");
    let result = call("analysis:prepare", args).await;
    if gen != GEN.with(Cell::get) {
        if let Ok(v) = result {
            let _ = call(
                "analysis:forget",
                json!({"epoch":v["epoch"],"folderId":v["folderId"],"reviewId":v["reviewId"]}),
            )
            .await;
        }
        return;
    }
    BUSY.with(|b| b.set(false));
    match result {
        Ok(v) => {
            REVIEW.with(|r| *r.borrow_mut() = s(&v, "reviewId"));
            CAN_SHARE.with(|c| c.set(b(&v, "canShare")));
            let existing = v["existing"].as_array().is_some_and(|a| !a.is_empty());
            EXISTING.with(|e| e.set(existing));
            set_val(&by_id("analysisText").unwrap(), &s(&v, "text"));
            set_text(
                "analysisSource",
                &format!("{} · {} B", s(&v, "name"), v["bytes"].as_u64().unwrap_or(0)),
            );
            let projects = by_id("analysisProject").unwrap();
            clear(&projects);
            let _ = projects.append_child(&el("option").attr("value", "").text("미지정").build());
            for p in v["projects"].as_array().into_iter().flatten() {
                let _ = projects.append_child(
                    &el("option")
                        .attr("value", &s(p, "id"))
                        .text(&s(p, "name"))
                        .build(),
                );
            }
            set_text(
                "analysisSendLabel",
                if b(&v, "priorAttempt") {
                    "사이 · Anthropic에 재전송 (추가 비용)"
                } else {
                    "사이 · Anthropic에 본문 전송"
                },
            );
            toggle(&by_id("analysisEntry").unwrap(), "hidden", true);
            toggle(&by_id("analysisPreview").unwrap(), "hidden", false);
            set_text(
                "analysisMessage",
                if existing {
                    "이미 등록한 자료"
                } else if !CAN_SHARE.with(Cell::get) {
                    "읽기 전용 · 팀 저장 불가"
                } else {
                    ""
                },
            );
            connection(false).await;
        }
        Err(e) => set_text("analysisMessage", &e),
    }
    controls();
}
fn render_result(result: &Value) {
    set_text("analysisSummary", &s(result, "summary"));
    let root = by_id("analysisCandidates").unwrap();
    clear(&root);
    for (index, c) in result["candidates"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        let card = el("article")
            .cls("analysis-candidate")
            .attr("data-index", &index.to_string())
            .kid(
                el("label")
                    .cls("analysis-choice")
                    .kid(
                        el("input")
                            .attr("type", "checkbox")
                            .attr("data-choice", "")
                            .attr(
                                "aria-label",
                                &format!("할 일 {} 검토 완료 후 선택", index + 1),
                            )
                            .on("change", |_| controls()),
                    )
                    .kid(el("strong").text(&format!("할 일 {}", index + 1))),
            )
            .kid(
                el("label").cls("field-label").text("제목").kid(
                    el("input")
                        .attr("data-title", "")
                        .attr("type", "text")
                        .attr("maxlength", "160")
                        .attr("value", &s(c, "title")),
                ),
            )
            .kid(
                el("div")
                    .cls("analysis-fields")
                    .kid(
                        el("label").cls("field-label").text("담당자").kid(
                            el("input")
                                .attr("data-assignee", "")
                                .attr("type", "text")
                                .attr("maxlength", "120")
                                .attr("value", &s(c, "assignee")),
                        ),
                    )
                    .kid(
                        el("label").cls("field-label").text("마감일").kid(
                            el("input")
                                .attr("data-due", "")
                                .attr("type", "date")
                                .attr("min", "0001-01-01")
                                .attr("max", "9999-12-31")
                                .attr("value", &s(c, "dueDate")),
                        ),
                    ),
            )
            .kid(el("p").cls("micro").text(if b(c, "uncertain") {
                "확인 필요 · 날짜와 담당자를 원문에서 확인하고 수정해 주세요."
            } else {
                "AI 제안 · 원문을 확인한 뒤 선택해 주세요."
            }))
            .kid(
                el("blockquote")
                    .cls("analysis-evidence")
                    .text(&s(c, "evidence")),
            );
        let _ = root.append_child(&card.build());
    }
    toggle(&by_id("analysisResults").unwrap(), "hidden", false);
    toggle(&by_id("analysisPreview").unwrap(), "hidden", true);
    set_text(
        "analysisMessage",
        if !CAN_SHARE.with(Cell::get) {
            "읽기 전용 팀이에요. 결과는 여기에서 확인할 수 있지만 할 일을 팀에 추가할 수는 없어요."
        } else if result["candidates"]
            .as_array()
            .is_some_and(|a| a.is_empty())
        {
            "추출한 할 일 후보가 없어요. 요약과 원문을 확인해 주세요."
        } else {
            "제목·담당자·마감일·근거를 검토한 할 일만 선택해 주세요. 선택은 기본으로 꺼져 있어요."
        },
    );
    controls();
}
async fn run() {
    if !READY.with(Cell::get)
        || ATTEMPTED.with(Cell::get)
        || !checked(&by_id("analysisSendAck").unwrap())
        || BUSY.with(|b| b.replace(true))
    {
        return;
    }
    let Some(mut args) = context() else {
        return;
    };
    args["confirmed"] = json!(true);
    let gen = GEN.with(Cell::get);
    ATTEMPTED.with(|a| a.set(true));
    PAID.with(|a| a.set(true));
    controls();
    set_text(
        "analysisMessage",
        "확인한 내용을 분석하고 있어요. 같은 요청을 자동으로 다시 보내지 않아요…",
    );
    let result = call("analysis:run", args).await;
    if gen != GEN.with(Cell::get) {
        return;
    }
    BUSY.with(|b| b.set(false));
    PAID.with(|b| b.set(false));
    match result {
        Ok(v) => {
            render_result(&v["result"]);
            if !s(&v, "warning").is_empty() {
                set_text("analysisMessage", &s(&v, "warning"));
            }
        }
        Err(e) => set_text("analysisMessage", &e),
    }
    controls();
}
async fn save() {
    if DONE.with(Cell::get)
        || !CAN_SHARE.with(Cell::get)
        || !checked(&by_id("analysisShareAck").unwrap())
    {
        return;
    }
    let mut choices = vec![];
    let project = val(&by_id("analysisProject").unwrap());
    for row in q_all("#analysisCandidates .analysis-candidate") {
        if !q_in(&row, "[data-choice]").is_some_and(|c| checked(&c)) {
            continue;
        }
        let index = row
            .get_attribute("data-index")
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(u64::MAX);
        choices.push(json!({"index":index,"title":val(&q_in(&row,"[data-title]").unwrap()),"assignee":val(&q_in(&row,"[data-assignee]").unwrap()),"dueDate":val(&q_in(&row,"[data-due]").unwrap()),"projectId":project}));
    }
    if choices.is_empty() || BUSY.with(|b| b.replace(true)) {
        return;
    }
    let Some(mut args) = context() else {
        return;
    };
    args["confirmed"] = json!(true);
    args["choices"] = json!(choices);
    let gen = GEN.with(Cell::get);
    controls();
    set_text(
        "analysisMessage",
        "검토한 할 일을 보관하고 팀 Drive 저장 결과를 확인하고 있어요…",
    );
    let result = call("analysis:commit", args).await;
    if gen != GEN.with(Cell::get) {
        return;
    }
    BUSY.with(|b| b.set(false));
    match result {
        Ok(v) => {
            DONE.with(|d| d.set(true));
            super::collab::apply_review(&v["state"]);
            let pending = v["state"]["pending"].as_u64().unwrap_or(0);
            let error = s(&v["state"], "error");
            let message = if !error.is_empty() || pending > 0 {
                format!("할 일을 이 PC에 보관했어요. 공유 대기 {pending}개는 협업에서 동기화해 주세요. {error}")
            } else if b(&v, "alreadyImported") {
                "이미 이 자료에서 등록한 할 일을 확인했어요. 중복으로 추가하지 않았어요.".into()
            } else {
                format!(
                    "선택한 할 일 {}개를 팀에 추가했어요.",
                    v["added"].as_u64().unwrap_or(0)
                )
            };
            set_text("analysisMessage", &message);
        }
        Err(e) => set_text("analysisMessage", &e),
    }
    controls();
}
pub fn bind() {
    on(&by_id("analysisClose").unwrap(), "click", |_| close());
    on(&dialog(), "cancel", |e| {
        e.prevent_default();
        close();
    });
    on(&dialog(), "close", |_| reset());
    on(&by_id("analysisPrepare").unwrap(), "click", |_| {
        spawn(prepare(
            json!({"kind":"text","text":val(&by_id("analysisInput").unwrap())}),
        ))
    });
    on(&by_id("analysisRun").unwrap(), "click", |_| spawn(run()));
    on(&by_id("analysisSave").unwrap(), "click", |_| spawn(save()));
    for id in ["analysisSendAck", "analysisShareAck"] {
        on(&by_id(id).unwrap(), "change", |_| controls());
    }
    on(&by_id("analysisConnect").unwrap(), "click", |_| {
        if super::confirm_team_ai_connection() {
            spawn(connection(true));
        }
    });
    on(&by_id("analysisOpenCollab").unwrap(), "click", |_| {
        close();
        super::show_tab("collab");
    });
}
