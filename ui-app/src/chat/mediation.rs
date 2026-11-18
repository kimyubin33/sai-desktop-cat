//! "팀원에게 전해 줘": 상담 대화를 바탕으로 사이가 다듬은 부탁을 팀원 한 명에게 전하고, 받은 말에 답한다.
use super::{arr, s, talk};
use crate::{api::call, dom::*};
use serde_json::{json, Value};
use std::cell::RefCell;
use wasm_bindgen::JsCast;
use web_sys::HtmlDialogElement;

thread_local! {
    static TARGETS: RefCell<Value> = const { RefCell::new(Value::Null) };
}

const REACTIONS: &[(&str, &str)] = &[("ok", "알겠어"), ("talk", "이야기해 보고 싶어"), ("unsure", "잘 모르겠어")];

fn reaction_label(id: &str) -> &'static str {
    REACTIONS.iter().find(|(k, _)| *k == id).map(|(_, l)| *l).unwrap_or("")
}
fn dialog() -> HtmlDialogElement {
    by_id("mediateDialog").unwrap().unchecked_into()
}
fn message(text: &str) {
    set_text("mediateMessage", text);
}
fn who(name: &str, email: &str) -> String {
    if name.trim().is_empty() {
        email.to_owned()
    } else {
        format!("{} ({email})", name.trim())
    }
}
fn selected_team() -> Option<Value> {
    let fid = val(&by_id("mediateTeam").unwrap());
    TARGETS.with(|t| arr(&t.borrow(), "teams").iter().find(|r| s(r, "folderId") == fid).cloned())
}
fn busy(on: bool) {
    for id in ["mediateDraft", "mediateTeam", "mediateTo"] {
        set_disabled(&by_id(id).unwrap(), on);
    }
    controls(on);
}
fn controls(busy: bool) {
    let preview_open = !has_class(&by_id("mediatePreview").unwrap(), "hidden");
    let ready = preview_open && checked(&by_id("mediateAck").unwrap()) && !val(&by_id("mediateText").unwrap()).trim().is_empty();
    set_disabled(&by_id("mediateSend").unwrap(), busy || !ready);
}
fn hide_draft() {
    toggle(&by_id("mediatePreview").unwrap(), "hidden", true);
    toggle(&by_id("mediateReply").unwrap(), "hidden", true);
    set_checked(&by_id("mediateAck").unwrap(), false);
    controls(false);
}

fn fill_members() {
    let to = by_id("mediateTo").unwrap();
    clear(&to);
    hide_draft();
    let Some(team) = selected_team() else { return };
    let members = arr(&team, "members");
    for m in members {
        let _ = to.append_child(&el("option").attr("value", &s(m, "email")).text(&who(&s(m, "name"), &s(m, "email"))).build());
    }
    if !team["error"].is_null() {
        message(&s(&team, "error"));
    } else if team["rosterMissing"] == true {
        message("팀 소유자가 사이에서 팀원 목록을 한 번 열면 이 팀에 말을 전할 수 있어요.");
    } else if members.is_empty() {
        message("이 팀에는 아직 말을 전할 다른 팀원이 없어요.");
    } else {
        message("");
    }
    set_disabled(&by_id("mediateDraft").unwrap(), members.is_empty());
}

async fn load_targets() {
    let team = by_id("mediateTeam").unwrap();
    match call("mediation:targets", Value::Null).await {
        Ok(v) => {
            clear(&team);
            let teams = arr(&v, "teams");
            if teams.is_empty() {
                let _ = team.append_child(&el("option").attr("value", "").text("연결된 팀이 없어요").build());
            }
            for t in teams {
                let _ = team.append_child(&el("option").attr("value", &s(t, "folderId")).text(&s(t, "name")).build());
            }
            TARGETS.with(|t| *t.borrow_mut() = v);
            fill_members();
        }
        Err(e) => {
            clear(&team);
            let _ = team.append_child(&el("option").attr("value", "").text("팀을 불러오지 못했어요").build());
            message(&e);
            set_disabled(&by_id("mediateDraft").unwrap(), true);
        }
    }
}

fn context() -> Option<Value> {
    let session = talk::session_id();
    let email = val(&by_id("mediateTo").unwrap());
    let team = selected_team()?;
    if session.is_empty() || email.is_empty() {
        return None;
    }
    let epoch = TARGETS.with(|t| t.borrow()["epoch"].clone());
    Some(json!({"epoch":epoch,"folderId":team["folderId"],"email":email,"sessionId":session,"note":val(&by_id("mediateNote").unwrap())}))
}

async fn draft() {
    let Some(args) = context() else {
        message("먼저 AI 탭에서 사이와 이야기를 나눠 주세요. 그 대화를 바탕으로 문안을 써요.");
        return;
    };
    busy(true);
    hide_draft();
    message("사이가 대화를 살펴보고 전할 말을 다듬는 중이에요…");
    let result = call("mediation:draft", args).await;
    busy(false);
    let v = match result {
        Ok(v) => v,
        Err(e) => return message(&e),
    };
    message("");
    let reply = by_id("mediateReply").unwrap();
    match s(&v, "kind").as_str() {
        "draft" => {
            let to = s(&v, "toName");
            let to = if to.trim().is_empty() { "받는 사람".to_owned() } else { format!("{}님", to.trim()) };
            set_text(
                "mediateFrame",
                &format!("{to}에게 “내가 사이에게 부탁한 말”이라고 밝히고 이렇게 전해요. 고쳐 써도 보내기 전에 한 번 더 검사해요."),
            );
            set_val(&by_id("mediateText").unwrap(), &s(&v, "text"));
            toggle(&by_id("mediatePreview").unwrap(), "hidden", false);
        }
        "question" => {
            reply.set_text_content(Some(&format!("사이가 물어봐요: {}\n요점 칸에 답을 적고 다시 다듬어 주세요.", s(&v, "text"))));
            toggle(&reply, "hidden", false);
        }
        _ => {
            reply.set_text_content(Some(&format!("사이 생각에는 지금은 전하지 않는 게 좋겠어요. {}", s(&v, "text"))));
            toggle(&reply, "hidden", false);
        }
    }
    controls(false);
}

async fn send() {
    let Some(mut args) = context() else { return };
    let text = val(&by_id("mediateText").unwrap()).trim().to_owned();
    if !confirm("이 말을 보낼까요? 받는 사람은 내가 보낸 것을 알 수 있고, 보낸 뒤에는 취소할 수 없어요.") {
        return;
    }
    args["text"] = json!(text);
    busy(true);
    message("보내기 전에 한 번 더 검토하는 중이에요…");
    let result = call("mediation:send", args).await;
    busy(false);
    match result {
        Ok(v) => {
            hide_draft();
            set_val(&by_id("mediateNote").unwrap(), "");
            message("보냈어요. 사이가 받는 사람에게 한가한 때 전할 거예요.");
            draw_book(&v);
        }
        Err(e) => message(&e),
    }
}

fn row(title: &str, text: &str) -> El {
    el("article").cls("mediate-item").kid(el("strong").text(title)).kid(el("p").text(text))
}

fn draw_book(v: &Value) {
    let inbox = by_id("mediateInbox").unwrap();
    clear(&inbox);
    if arr(v, "inbox").is_empty() {
        let _ = inbox.append_child(&el("p").cls("micro").text("아직 받은 말이 없어요.").build());
    }
    for r in arr(v, "inbox") {
        let id = s(r, "id");
        let from = s(r, "from");
        let mut item = row(&format!("{} · {}", who(&s(r, "fromName"), &from), s(r, "teamName")), &s(r, "text"));
        let actions = el("div").cls("inline");
        let actions = if r["reaction"].is_string() {
            actions.kid(el("span").cls("status-tag").text(&format!("내 답: {}", reaction_label(&s(r, "reaction")))))
        } else {
            let mut a = actions;
            for (key, label) in REACTIONS {
                let (id, key) = (id.clone(), key.to_string());
                a = a.kid(el("button").cls("subtle").attr("type", "button").text(label).click(move |_| {
                    let (id, key) = (id.clone(), key.clone());
                    spawn(async move {
                        match call("mediation:react", json!({"id":id,"reaction":key})).await {
                            Ok(v) => draw_book(&v),
                            Err(e) => message(&e),
                        }
                    })
                }));
            }
            a
        };
        let mute_from = from.clone();
        let actions = actions.kid(el("button").cls("text-button").attr("type", "button").text("이 사람 말 끄기").click(move |_| {
            if confirm("이 사람이 사이를 통해 보내는 말을 더 받지 않을까요? 나중에 다시 켤 수 있어요.") {
                let value = mute_from.clone();
                spawn(mute("person", value, true));
            }
        }));
        item = item.kid(actions);
        let _ = inbox.append_child(&item.build());
    }
    for email in v["muted"]["people"].as_array().into_iter().flatten().filter_map(|e| e.as_str()) {
        let value = email.to_owned();
        let line = el("p")
            .cls("micro")
            .text(&format!("{email}의 말은 받지 않고 있어요. "))
            .kid(el("button").cls("text-button").attr("type", "button").text("다시 받기").click(move |_| spawn(mute("person", value.clone(), false))));
        let _ = inbox.append_child(&line.build());
    }

    let sent = by_id("mediateSent").unwrap();
    clear(&sent);
    if arr(v, "sent").is_empty() {
        let _ = sent.append_child(&el("p").cls("micro").text("아직 보낸 말이 없어요.").build());
    }
    for r in arr(v, "sent") {
        let status = if r["reaction"].is_string() {
            format!("답: {}", reaction_label(&s(r, "reaction")))
        } else {
            "답을 기다리는 중".to_owned()
        };
        let item = row(&format!("{} · {}", who(&s(r, "toName"), &s(r, "to")), s(r, "teamName")), &s(r, "text"))
            .kid(el("span").cls("status-tag").text(&status));
        let _ = sent.append_child(&item.build());
    }
}

async fn mute(kind: &'static str, value: String, on: bool) {
    match call("mediation:mute", json!({"kind":kind,"value":value,"on":on})).await {
        Ok(v) => draw_book(&v),
        Err(e) => message(&e),
    }
}

async fn load_book(check: bool) {
    let channel = if check { "mediation:check" } else { "mediation:book" };
    match call(channel, Value::Null).await {
        Ok(v) => {
            draw_book(&v);
            if check {
                let c = &v["checked"];
                let (new, held) = (c["requests"].as_u64().unwrap_or(0) + c["reactions"].as_u64().unwrap_or(0), c["held"].as_u64().unwrap_or(0));
                let mut text = if new == 0 { "새로 받은 말이 없어요.".to_owned() } else { format!("새로 {new}개를 받았어요.") };
                if held > 0 {
                    text.push_str(&format!(" 한도를 넘었거나 꺼 둔 사람의 말 {held}개는 받지 않았어요."));
                }
                message(&text);
            }
        }
        Err(e) => message(&e),
    }
}

fn open() {
    hide_draft();
    message(if talk::session_id().is_empty() {
        "먼저 AI 탭에서 사이와 이야기를 나눠 주세요. 그 대화를 바탕으로 문안을 써요. 받은 말은 아래에서 볼 수 있어요."
    } else {
        ""
    });
    if dialog().show_modal().is_err() {
        return;
    }
    spawn(load_targets());
    spawn(load_book(false));
}

pub fn bind() {
    on(&by_id("mediateOpen").unwrap(), "click", |_| open());
    on(&by_id("mediateClose").unwrap(), "click", |_| dialog().close());
    on(&by_id("mediateTeam").unwrap(), "change", |_| fill_members());
    on(&by_id("mediateTo").unwrap(), "change", |_| hide_draft());
    on(&by_id("mediateDraft").unwrap(), "click", |_| spawn(draft()));
    on(&by_id("mediateSend").unwrap(), "click", |_| spawn(send()));
    on(&by_id("mediateCheck").unwrap(), "click", |_| spawn(load_book(true)));
    on(&by_id("mediateAck").unwrap(), "change", |_| controls(false));
    on(&by_id("mediateText").unwrap(), "input", |_| controls(false));
}
