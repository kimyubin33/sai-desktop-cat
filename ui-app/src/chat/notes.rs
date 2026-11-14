//! 정리함: 팀 약속, 공유 대기함, 저장된 대화.
use super::{arr, b, fmt_date, n, s};
use crate::api::call;
use crate::dom::*;
use serde_json::{json, Value};
use std::cell::Cell;

thread_local! {
    static RENDER_ID: Cell<u64> = const { Cell::new(0) };
}

fn status_text(st: &str) -> &'static str {
    match st {
        "review" => "확인 기다림",
        "needs-recipient" => "받을 팀원 고르기",
        "queued" => "곧 전달돼요",
        "sent" => "전달됨",
        "held" => "상대가 사정을 이야기함",
        _ => "",
    }
}

fn target_text(t: &str) -> &'static str {
    match t {
        "person" => "관련 팀원의 고양이",
        "team" => "팀 공통 제안",
        _ => "",
    }
}

/// 버튼 → 앱 호출 → 정리함 다시 그리기
fn redo(ch: &'static str, args: Value) {
    spawn(async move {
        let _ = call(ch, args).await;
        render().await;
    });
}

fn held_action(item: &Value, teams: &[Value], outbox: &[Value]) -> El {
    let team_id = s(item, "teamId");
    let Some(team) = teams.iter().find(|t| !team_id.is_empty() && t["teamId"] == team_id) else {
        return el("span").cls("hint").text("이 항목의 팀에 연결해야 약속을 제안할 수 있어요.");
    };
    let name = s(team, "name");
    if let Some(link) = item["heldProposalId"].as_str() {
        if let Some(proposal) = outbox.iter().find(|p| p["id"] == link && p["teamId"] == team_id && p["target"] == "team") {
            let status = status_text(&s(proposal, "status"));
            return el("span").cls("hint").attr("role", "status").text(&format!("‘{name}’ 팀 약속 제안 · {status}"));
        }
    }
    let seed = format!("{}에 대해 팀 약속을 만들고 싶어요.", s(item, "label"));
    el("button").text(&format!("‘{name}’ 팀 약속 잡기")).click(move |_| {
        spawn(super::talk::delivery::open("promise", "promise".into(), Some(team_id.clone()), Some(seed.clone())));
    })
}

/// 팀 전체에서 철회하는지, 내 기기에서만 지우는지 실행 전에 명확하게 표시한다.
fn remove_control(label: &str, prompt: &'static str, channel: &'static str, args: Value) -> El {
    let message = el("span").cls("hint err").attr("role", "status").build();
    let status = message.clone();
    let button = el("button").cls("link").text(label).click(move |e| {
        let Some(button) = event_target(&e) else { return };
        if button.has_attribute("disabled") || !confirm(prompt) { return; }
        set_disabled(&button, true);
        let status = status.clone();
        let args = args.clone();
        spawn(async move {
            match call(channel, args).await {
                Ok(_) => render().await,
                Err(e) => {
                    status.set_text_content(Some(&e));
                    set_disabled(&button, false);
                }
            }
        });
    });
    el("div").cls("controls").kid(button).child(message)
}

pub async fn render() {
    let request = RENDER_ID.with(|id| { id.set(id.get() + 1); id.get() });
    let outbox = call("share:outbox", Value::Null).await.ok().and_then(|v| v.as_array().cloned()).unwrap_or_default();
    let sessions = call("chat:list", Value::Null).await.ok().and_then(|v| v.as_array().cloned()).unwrap_or_default();
    let team = call("team:status", Value::Null).await.unwrap_or(Value::Null);
    if !RENDER_ID.with(|id| id.get() == request) { return; }
    let members = arr(&team, "members");
    let others: Vec<Value> = members.iter().filter(|m| !b(m, "me")).cloned().collect();
    let name_of = |uid: &str| members.iter().find(|m| s(m, "uid") == uid).map(|m| s(m, "displayName"));
    let connected = b(&team, "connected");
    let active_id = s(&team, "activeTeamId");
    let my_uid = members.iter().find(|m| b(m, "me")).map(|m| s(m, "uid"));

    // 팀 약속
    let pl = by_id("proposals").unwrap();
    clear(&pl);
    let proposals = arr(&team, "proposals");
    if !connected {
        let _ = pl.append_child(&el("div").cls("empty").text("팀에 연결하면 여기에 팀 약속 제안이 보여요.").0);
    } else if proposals.is_empty() {
        let _ = pl.append_child(&el("div").cls("empty").text("아직 팀 약속 제안이 없어요.").0);
    }
    for p in &proposals {
        let agreed = b(p, "agreedByAll");
        let mine = s(p, "mine");
        let id = s(p, "id");
        let title = format!("{} · {}", s(p, "label"), if agreed { "팀 약속".to_string() } else { format!("동의 {}/{}명", s(p, "agree"), s(p, "memberCount")) });
        let id2 = id.clone();
        let args = json!({"id": id, "teamId": active_id});
        let remove = if b(p, "custom") && my_uid.as_deref().is_some_and(|me| p["by"] == me) {
            remove_control("약속 삭제", "팀원 모두의 약속 목록과 팀 톡에서 이 제안을 삭제할까요?", "team:withdraw-promise", args)
        } else {
            remove_control("내 목록에서 삭제", "이 기기의 약속 목록과 고양이 알림에서 이 항목을 지울까요?", "team:hide-promise", args)
        };
        let item = el("div")
            .cls(if agreed { "item agreed" } else { "item" })
            .kid(el("div").cls("main").kid(el("div").cls("title").text(&title)).kid(el("div").cls("sub").text(&s(p, "text")).attr("title", &s(p, "text"))))
            .kid(if mine == "agree" { el("span").cls("status-pill sent").text("동의함") } else { el("button").cls("primary").text("좋아요").click(move |_| redo("team:respond", json!({ "proposalId": id, "response": "agree" }))) })
            .maybe(if mine.is_empty() { Some(el("button").cls("link").text("나중에").click(move |_| redo("team:respond", json!({ "proposalId": id2, "response": "later" })))) } else { None });
        let _ = pl.append_child(&item.kid(remove).0);
    }

    // 공유 대기함
    let ob = by_id("outbox").unwrap();
    clear(&ob);
    if outbox.is_empty() {
        let _ = ob.append_child(&el("div").cls("empty").text("아직 공유하기로 한 주제가 없어요.").0);
    }
    let teams = arr(&team, "teams");
    let many_teams = teams.len() > 1;
    for it in &outbox {
        let mut status = s(it, "status");
        if status.is_empty() {
            status = if s(it, "scope") == "review" { "review".into() } else { "queued".into() };
        }
        let id = s(it, "id");
        let target = s(it, "target");
        // 받을 팀원 고르기·보내기는 그 팀을 보고 있을 때만 (팀원 목록이 팀마다 다르므로)
        let here = connected && (s(it, "teamId").is_empty() || s(it, "teamId") == s(&team, "activeTeamId"));
        let controls = el("div").cls("controls").build();
        let recipient_select = || {
            let sel = el("select").attr("data-for", &id).kid(el("option").attr("value", "").text("받을 팀원")).build();
            for m in &others {
                let _ = sel.append_child(&el("option").attr("value", &s(m, "uid")).text(&s(m, "displayName")).flag("selected", s(m, "uid") == s(it, "toMember")).0);
            }
            sel
        };
        if here && status == "review" {
            let sel = if target == "person" { Some(recipient_select()) } else { None };
            if let Some(sl) = &sel {
                let _ = controls.append_child(sl);
            }
            let id = id.clone();
            let _ = controls.append_child(
                &el("button")
                    .cls("primary")
                    .text("이 문안으로 보내기")
                    .click(move |_| {
                        let to = sel.as_ref().map(|x| val(x)).filter(|v| !v.is_empty());
                        redo("share:confirm", json!({ "itemId": id, "toMember": to }));
                    })
                    .0,
            );
        } else if here && status == "needs-recipient" {
            let sel = recipient_select();
            let s2 = sel.clone();
            let id = id.clone();
            let _ = controls.append_child(&sel);
            let _ = controls.append_child(
                &el("button")
                    .text("정하기")
                    .click(move |_| {
                        let v = val(&s2);
                        if !v.is_empty() {
                            redo("share:set-recipient", json!({ "itemId": id, "toMember": v }));
                        }
                    })
                    .0,
            );
        } else if status == "held" && target == "person" {
            let _ = controls.append_child(&held_action(it, &teams, &outbox).0);
        }
        let to = if target == "person" && here { name_of(&s(it, "toMember")).filter(|_| !s(it, "toMember").is_empty()).map(|n| format!(" → {n}")).unwrap_or_default() } else { String::new() };
        let team_tag = if many_teams && !s(it, "teamName").is_empty() { format!("[{}] ", s(it, "teamName")) } else { String::new() };
        let has_controls = controls.child_element_count() > 0;
        let pill = if connected || status != "queued" { status_text(&status) } else { "팀 연결 전" };
        let delivered = matches!(status.as_str(), "sent" | "held");
        let remove = if delivered {
            remove_control("내 기록 삭제", "이 기기의 공유 기록을 삭제할까요? 전달된 내용은 상대에게 남습니다.", "share:remove",
                json!({"itemId":id,"teamId":it["teamId"],"delivered":true}))
        } else {
            remove_control("전송 취소", "이 항목의 전송을 취소하고 대기함에서 지울까요?", "share:remove",
                json!({"itemId":id,"teamId":it["teamId"],"delivered":false}))
        };
        let withdraw = if delivered && b(it, "custom") && target == "person" && teams.iter().any(|t| t["teamId"] == it["teamId"]) {
            Some(remove_control("전달 철회", "보낸 문안을 팀 서버에서 삭제할까요? 받는 사람의 앱은 다음 동기화 때 반영해요.", "share:withdraw-advice",
                json!({"id":id,"teamId":it["teamId"]})))
        } else { None };
        let last_error = s(it, "lastError");
        let item = el("div")
            .cls("item")
            .kid(
                el("div")
                    .cls("main")
                    .kid(el("div").cls("title").text(&format!("{team_tag}{} · {}{to}", s(it, "label"), target_text(&target))))
                    .kid(el("div").cls("sub").text(&if target == "team" { s(it, "teamProposal") } else { s(it, "advice") }).attr("title", &s(it, "advice")))
                    .maybe(if status == "held" { Some(el("div").cls("hint").text("상대가 자기 사정을 이야기해서 이 조언은 멈췄어요. 한쪽 이야기만으로 교정하지 않도록, 팀 약속으로 함께 맞춰 보는 걸 추천해요.")) } else { None })
                    .maybe(if b(it, "custom") && !delivered && !last_error.is_empty() {
                        Some(el("div").cls("hint err").text(&format!("전송 대기 · {last_error}")))
                    } else { None })
                    .maybe(if has_controls { Some(El(controls)) } else { None }),
            )
            .kid(el("span").cls(&format!("status-pill {status}")).text(pill))
            .maybe(withdraw).kid(remove);
        let _ = ob.append_child(&item.0);
    }

    if let (Some(section), Some(list)) = (by_id("receivedAdviceSection"), by_id("receivedAdvice")) {
        let advice = arr(&team, "receivedAdvice");
        clear(&list);
        toggle(&section, "hidden", advice.is_empty());
        for a in advice {
            let item = el("div").cls("item")
                .kid(el("div").cls("main")
                    .kid(el("div").cls("title").text(&s(&a, "title")))
                    .kid(el("div").cls("received-advice-text").text(&s(&a, "text"))))
                .kid(remove_control("내 목록에서 삭제", "이 기기의 받은 조언 목록과 고양이 알림에서 이 항목을 지울까요?",
                    "team:hide-advice", json!({"id":a["id"],"teamId":active_id})));
            let _ = list.append_child(&item.0);
        }
    }

    // 저장된 대화
    let list = by_id("sessions").unwrap();
    clear(&list);
    if sessions.is_empty() {
        let _ = list.append_child(&el("div").cls("empty").text("저장된 대화가 없어요.").0);
    }
    for x in &sessions {
        let id = s(x, "id");
        let id2 = id.clone();
        let title = format!("{} · {}{}", s(x, "modeLabel"), fmt_date(n(x, "startedAt")), if b(x, "hasSummary") { " · 정리됨" } else { "" });
        let item = el("div")
            .cls("item")
            .kid(el("div").cls("main").kid(el("div").cls("title").text(&title)).kid(el("div").cls("sub").text(&s(x, "preview"))))
            .kid(el("button").text("보기").click(move |_| spawn(view_session(id.clone()))))
            .kid(el("button").cls("link").text("삭제").click(move |_| {
                if !confirm("이 대화를 지울까요? 되돌릴 수 없어요.") {
                    return;
                }
                let id = id2.clone();
                spawn(async move {
                    let _ = call("chat:delete", json!(id)).await;
                    if let Some(v) = by_id("sessionView") {
                        toggle(&v, "hidden", true);
                    }
                    render().await;
                });
            }));
        let _ = list.append_child(&item.0);
    }
}

async fn view_session(id: String) {
    let Ok(sx) = call("chat:get", json!(id)).await else { return };
    let view = by_id("sessionView").unwrap();
    clear(&view);
    toggle(&view, "hidden", false);
    let bx = el("div").cls("messages").build();
    for t in arr(&sx, "turns") {
        let cls = format!("msg {} {}", s(&t, "role"), if b(&t, "safety") { "safety" } else { "" });
        let _ = bx.append_child(&el("div").cls(cls.trim()).text(&s(&t, "text")).0);
    }
    let _ = view.append_child(&el("h2").text(&fmt_date(n(&sx, "startedAt"))).0);
    let _ = view.append_child(&bx);
    if sx["summary"].is_object() {
        super::talk::render_summary(sx["summary"].clone(), &s(&sx, "id"), &view, false);
    } else if sx["mode"] == "complaint" && sx["safetyHold"].is_null() && arr(&sx, "turns").iter().any(|t| t["role"] == "user") {
        let _ = view.append_child(&el("button").cls("primary").text("AI 공유 문안 만들기").click(move |_| {
            spawn(super::talk::delivery::open("complaint", id.clone(), None, None));
        }).0);
    }
    scroll_into_view(&view, true);
}
