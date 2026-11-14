//! 개인 상담에서 공유할 문안, 또는 대화를 통해 팀에 제안할 약속을 작성하고 승인한다.
use super::super::{arr, b, refresh_team_switch, s, show_tab};
use crate::api::call;
use crate::dom::*;
use serde_json::{json, Value};
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::JsCast;
use web_sys::KeyboardEvent;

struct Editor {
    kind: &'static str,
    session: String,
    team: String,
    view: Value,
    provider: String,
    input: String,
    recipient: String,
    busy: bool,
    error: String,
    from_tab: String,
}
type Active = Rc<RefCell<Editor>>;
thread_local! { static EDITOR: RefCell<Option<Active>> = const { RefCell::new(None) }; }

fn current(editor: &Active) -> bool {
    EDITOR.with(|slot| slot.borrow().as_ref().is_some_and(|e| Rc::ptr_eq(e, editor)))
}

pub(super) fn close(return_to_source: bool) {
    let Some(editor) = EDITOR.with(|slot| slot.borrow_mut().take()) else {
        return;
    };
    let panel = by_id("deliveryPanel").unwrap();
    clear(&panel);
    toggle(&panel, "hidden", true);
    toggle(&by_id("messages").unwrap(), "hidden", false);
    let in_chat = super::st(|s| s.session.is_some());
    toggle(&by_id("composer").unwrap(), "hidden", !in_chat);
    toggle(&by_id("modePicker").unwrap(), "hidden", in_chat);
    if return_to_source {
        show_tab(&editor.borrow().from_tab);
    }
}

pub(in crate::chat) fn switch_team(team: &str) {
    let changed = EDITOR.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|e| !e.borrow().team.is_empty() && e.borrow().team != team)
    });
    if changed {
        close(false);
    }
}

fn scope(editor: &Active) -> Value {
    let e = editor.borrow();
    json!({"kind":e.kind,"sessionId":e.session,"teamId":e.team})
}

fn apply_view(editor: &Active, view: Value) -> Result<(), String> {
    let mut e = editor.borrow_mut();
    if view["kind"].as_str().is_some_and(|kind| kind != e.kind) || view["sessionId"].as_str().is_some_and(|session| session != e.session) {
        return Err("대화가 변경됐어요. 원래 대화를 다시 열어 주세요.".into());
    }
    let team = s(&view, "teamId");
    if !e.team.is_empty() && team != e.team {
        return Err("팀이 변경됐어요. 보고 있는 팀에서 다시 열어 주세요.".into());
    }
    let claude = b(&view, "hasClaudeKey");
    let gpt = b(&view, "hasOpenaiKey");
    if !(e.provider == "claude" && claude || e.provider == "openai" && gpt) {
        e.provider = if claude {
            "claude"
        } else if gpt {
            "openai"
        } else {
            ""
        }
        .into();
    }
    if e.view["draft"]["id"] != view["draft"]["id"] {
        e.recipient.clear();
    }
    e.team = team;
    e.view = view;
    Ok(())
}

async fn read(editor: &Active) -> Result<(), String> {
    let response = call("delivery:view", scope(editor)).await?;
    if current(editor) {
        apply_view(editor, response)?;
    }
    Ok(())
}

pub(in crate::chat) async fn open(kind: &'static str, session: String, source_team: Option<String>, seed: Option<String>) {
    close(false);
    let editor = Rc::new(RefCell::new(Editor {
        kind,
        session,
        team: source_team.clone().unwrap_or_default(),
        provider: String::new(),
        view: Value::Null,
        input: seed.unwrap_or_default(),
        recipient: String::new(),
        busy: true,
        error: String::new(),
        from_tab: super::super::current_tab(),
    }));
    EDITOR.with(|slot| *slot.borrow_mut() = Some(editor.clone()));
    show_tab("chat");
    for id in ["modePicker", "composer", "messages"] {
        toggle(&by_id(id).unwrap(), "hidden", true);
    }
    toggle(&by_id("deliveryPanel").unwrap(), "hidden", false);
    draw(&editor);
    let result = async {
        let info = call("team:status", Value::Null).await?;
        if !current(&editor) {
            return Ok(());
        }
        if let Some(team) = source_team {
            if !arr(&info, "teams").iter().any(|t| t["teamId"] == team) {
                return Err("이 항목의 팀에 다시 연결해 주세요.".into());
            }
            if info["activeTeamId"] != team {
                let switched = call("team:switch", json!(team)).await?;
                if !current(&editor) {
                    return Ok(());
                }
                if switched["activeTeamId"] != team {
                    return Err("이 항목의 팀을 열지 못했어요.".into());
                }
                refresh_team_switch(&switched);
            }
        } else {
            editor.borrow_mut().team = s(&info, "activeTeamId");
        }
        read(&editor).await
    }
    .await;
    finish(&editor, result);
}

fn finish(editor: &Active, result: Result<(), String>) {
    if !current(editor) {
        return;
    }
    {
        let mut e = editor.borrow_mut();
        e.busy = false;
        e.error = result.err().unwrap_or_default();
    }
    draw(editor);
}

fn run(editor: &Active) -> bool {
    if !current(editor) || editor.borrow().busy {
        return false;
    }
    {
        let mut e = editor.borrow_mut();
        e.busy = true;
        e.error.clear();
    }
    draw(editor);
    true
}

fn generate(editor: Active, retry: bool) {
    if !current(&editor) {
        return;
    }
    let mut payload = scope(&editor);
    let text = {
        let e = editor.borrow();
        if e.provider.is_empty() || e.team.is_empty() {
            return;
        }
        payload["provider"] = json!(e.provider);
        if retry {
            String::new()
        } else {
            e.input.trim().to_owned()
        }
    };
    if !text.is_empty() {
        payload["text"] = json!(text);
    }
    if !run(&editor) {
        return;
    }
    spawn(async move {
        let result = call("delivery:generate", payload).await;
        if !current(&editor) {
            return;
        }
        let result = match result {
            Ok(view) => {
                editor.borrow_mut().input.clear();
                apply_view(&editor, view)
            }
            Err(error) => {
                // 새 입력이 이미 저장됐다면 원문을 다시 보내지 않고 재시도한다.
                if read(&editor).await.is_ok() && !text.is_empty() {
                    let last_user = arr(&editor.borrow().view, "turns").into_iter().rev().find(|t| t["role"] == "user");
                    if last_user.is_some_and(|t| t["text"] == text) {
                        editor.borrow_mut().input.clear();
                    }
                }
                Err(error)
            }
        };
        finish(&editor, result);
    });
}

fn approve(editor: Active) {
    if !current(&editor) {
        return;
    }
    let mut payload = scope(&editor);
    {
        let e = editor.borrow();
        payload["draftId"] = e.view["draft"]["id"].clone();
        if e.view["draft"]["target"] == "person" {
            if e.recipient.is_empty() {
                return;
            }
            payload["toMember"] = json!(e.recipient);
        }
    }
    if !run(&editor) {
        return;
    }
    spawn(async move {
        let result = call("delivery:approve", payload).await;
        if !current(&editor) {
            return;
        }
        let result = result.and_then(|view| apply_view(&editor, view));
        finish(&editor, result);
    });
}

fn discard(editor: Active) {
    if !current(&editor) || !confirm("이 기기에 저장된 AI 작성 대화와 초안을 지울까요?") || !run(&editor) {
        return;
    }
    spawn(async move {
        let result = call("delivery:discard", scope(&editor)).await;
        if !current(&editor) {
            return;
        }
        let result = result.and_then(|view| {
            editor.borrow_mut().input.clear();
            apply_view(&editor, view)
        });
        finish(&editor, result);
    });
}

fn draw(editor: &Active) {
    let e = editor.borrow();
    let panel = by_id("deliveryPanel").unwrap();
    clear(&panel);
    let add = |x: El| {
        let _ = panel.append_child(&x.0);
    };
    let promise = e.kind == "promise";
    let team = s(&e.view, "teamName");
    add(el("div")
        .cls("delivery-head")
        .kid(el("h2").text(if promise { "팀 약속 잡기" } else { "AI 공유 문안" }))
        .kid(el("button").id("deliveryClose").cls("link").text("돌아가기").click(|_| close(true))));
    if !team.is_empty() {
        add(el("p").cls("delivery-team").text(&format!("{team} 팀")));
    }
    if !e.error.is_empty() {
        add(el("p").cls("err").attr("role", "alert").text(&e.error));
    }
    if e.view.is_null() {
        if !e.busy {
            let ed = editor.clone();
            add(el("button").text("다시 불러오기").click(move |_| {
                let e = ed.clone();
                if run(&e) {
                    spawn(async move {
                        let r = read(&e).await;
                        finish(&e, r);
                    });
                }
            }));
        } else {
            add(el("p").attr("role", "status").text("불러오는 중…"));
        }
        return;
    }
    let turns = arr(&e.view, "turns");
    if !turns.is_empty() {
        let mut log = el("div").cls("delivery-turns").attr("aria-label", "작성 대화");
        for turn in &turns {
            let you = turn["role"] == "user";
            log = log.kid(el("div").cls(if you { "msg user" } else { "msg cat" }).text(&s(turn, "text")));
        }
        add(log);
    }
    let saved = b(&e.view, "approved");
    let question = s(&e.view, "question");
    let recommendation = s(&e.view, "recommendation");
    for text in [question, recommendation] {
        if !text.is_empty() && !turns.last().is_some_and(|t| t["text"] == text) {
            add(el("p").cls("delivery-reply").text(&text));
        }
    }
    if saved {
        add(el("p")
            .attr("role", "status")
            .text("공유 대기함에 등록했어요. 정리함에서 전송 상태를 확인할 수 있어요."));
        add(el("button").text("공유 대기함 보기").click(|_| {
            close(false);
            show_tab("notes");
        }));
    } else {
        if e.view["draft"].is_object() {
            add(preview(editor, &e));
        }
        add(el("p").cls("hint").text(if promise {
            "팀원과 정하고 싶은 약속을 적어 주세요. 작성할 때 이 대화를 선택한 AI에 보냅니다."
        } else {
            "선택한 AI가 이번 상담 내용을 읽고 공유할 내용과 대상을 제안합니다. 추가로 반영할 내용을 적을 수 있어요."
        }));
        add(controls(editor, &e));
    }
    if !turns.is_empty() || e.view["draft"].is_object() || saved || b(&e.view, "canRetry") {
        let ed = editor.clone();
        add(el("button")
            .id("deliveryDiscard")
            .cls("link")
            .flag("disabled", e.busy)
            .text(if saved && promise {
                "작성 기록 지우고 다음 팀 약속 잡기"
            } else {
                "작성 대화·초안 삭제"
            })
            .click(move |_| discard(ed.clone())));
    }
}

fn preview(editor: &Active, e: &Editor) -> El {
    let draft = &e.view["draft"];
    let person = draft["target"] == "person";
    let mut card = el("section")
        .cls("delivery-preview")
        .kid(el("h3").text("전달할 문안"))
        .kid(el("strong").text(&s(draft, "title")))
        .kid(el("p").text(&s(draft, "text")));
    if person {
        let select = el("select")
            .id("deliveryRecipient")
            .attr("aria-label", "받을 팀원")
            .flag("disabled", e.busy)
            .kid(el("option").attr("value", "").text("받을 팀원 선택"));
        let mut select = select;
        for m in arr(&e.view, "members").iter().filter(|m| !b(m, "me")) {
            select = select.kid(
                el("option")
                    .attr("value", &s(m, "uid"))
                    .flag("selected", m["uid"] == e.recipient)
                    .text(&s(m, "displayName")),
            );
        }
        let ed = editor.clone();
        card = card.kid(el("label").text("받는 사람").kid(select.on("change", move |ev| {
            if let Some(target) = event_target(&ev) {
                ed.borrow_mut().recipient = val(&target);
                draw(&ed);
            }
        })));
        card = card.kid(el("p").cls("hint").text("승인하면 선택한 팀원에게 이 문안을 전달합니다."));
    } else {
        card = card.kid(
            el("p")
                .cls("hint")
                .text("승인하면 내 이름으로 팀 톡에 이 문안이 게시되고, 팀원들은 정리함에서 약속에 동의할 수 있어요."),
        );
    }
    let ed = editor.clone();
    card.kid(
        el("button")
            .id("deliveryApprove")
            .cls("primary")
            .flag("disabled", e.busy || person && e.recipient.is_empty() || e.team.is_empty())
            .text("이 문안 승인하고 대기함에 넣기")
            .click(move |_| approve(ed.clone())),
    )
}

fn controls(editor: &Active, e: &Editor) -> El {
    let connected = !e.team.is_empty() && e.view["isConnected"] != false;
    let available = !e.provider.is_empty() && connected;
    let mut part = el("div").cls("delivery-controls");
    if !available {
        let text = if !connected {
            "설정에서 팀에 연결하면 문안을 작성할 수 있어요."
        } else {
            "새 문안을 작성하려면 설정에서 Claude 또는 GPT API 키를 연결해 주세요."
        };
        part = part
            .kid(el("p").cls("hint").text(text))
            .kid(el("button").cls("link").text("연결 설정").click(|_| show_tab("settings")));
        let ed = editor.clone();
        part = part.kid(el("button").cls("link").text("연결 다시 확인").flag("disabled", e.busy).click(move |_| {
            let e = ed.clone();
            if run(&e) {
                spawn(async move {
                    let r = read(&e).await;
                    finish(&e, r);
                });
            }
        }));
    } else {
        let ed = editor.clone();
        let mut select = el("select")
            .id("deliveryProvider")
            .attr("aria-label", "작성에 사용할 AI")
            .flag("disabled", e.busy);
        for (key, label, flag) in [("claude", "Claude", "hasClaudeKey"), ("openai", "GPT", "hasOpenaiKey")] {
            if b(&e.view, flag) {
                select = select.kid(el("option").attr("value", key).flag("selected", e.provider == key).text(label));
            }
        }
        part = part.kid(el("label").text("작성에 사용할 AI").kid(select.on("change", move |ev| {
            if let Some(target) = event_target(&ev) {
                ed.borrow_mut().provider = val(&target);
            }
        })));
    }
    let is_promise = e.kind == "promise";
    let ed = editor.clone();
    let input = el("textarea")
        .id("deliveryInput")
        .attr("rows", "2")
        .attr("maxlength", "6000")
        .flag("disabled", !available || e.busy)
        .attr("aria-label", "AI에게 전할 말")
        .attr(
            "placeholder",
            if is_promise {
                "예: 회의 자료를 언제까지 올릴지 정하고 싶어요"
            } else {
                "추가하고 싶은 내용 (선택)"
            },
        )
        .on("input", move |ev| {
            if let Some(t) = event_target(&ev) {
                let value = val(&t);
                ed.borrow_mut().input = value.clone();
                if is_promise {
                    if let Some(button) = by_id("deliveryGenerate") {
                        set_disabled(&button, value.trim().is_empty());
                    }
                }
            }
        })
        .build();
    set_val(&input, &e.input);
    let key_editor = editor.clone();
    on(&input, "keydown", move |ev| {
        let key: &KeyboardEvent = ev.unchecked_ref();
        if key.key() == "Enter" && !key.shift_key() && !key.is_composing() {
            ev.prevent_default();
            if !is_promise || !key_editor.borrow().input.trim().is_empty() {
                generate(key_editor.clone(), false);
            }
        }
    });
    let ed = editor.clone();
    part = part.child(input).kid(
        el("button")
            .id("deliveryGenerate")
            .cls("primary")
            .flag("disabled", e.busy || !available || is_promise && e.input.trim().is_empty())
            .text(if e.busy {
                "처리 중…"
            } else if is_promise {
                "AI에게 보내기"
            } else {
                "AI 공유 문안 작성"
            })
            .click(move |_| generate(ed.clone(), false)),
    );
    if b(&e.view, "canRetry") {
        let ed = editor.clone();
        part = part.kid(
            el("button")
                .id("deliveryRetry")
                .flag("disabled", e.busy || !available)
                .text("저장한 내용으로 다시 시도")
                .click(move |_| generate(ed.clone(), true)),
        );
    }
    part
}
