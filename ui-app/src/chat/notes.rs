//! 정리함: 팀 약속, 공유 대기함, 저장된 대화.
use super::{arr, b, fmt_date, n, s};
use crate::api::call;
use crate::dom::*;
use serde_json::{json, Value};

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

pub async fn render() {
    let outbox = call("share:outbox", Value::Null).await.ok().and_then(|v| v.as_array().cloned()).unwrap_or_default();
    let sessions = call("chat:list", Value::Null).await.ok().and_then(|v| v.as_array().cloned()).unwrap_or_default();
    let team = call("team:status", Value::Null).await.unwrap_or(Value::Null);
    let members = arr(&team, "members");
    let others: Vec<Value> = members.iter().filter(|m| !b(m, "me")).cloned().collect();
    let name_of = |uid: &str| members.iter().find(|m| s(m, "uid") == uid).map(|m| s(m, "displayName"));
    let connected = b(&team, "connected");

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
        let item = el("div")
            .cls(if agreed { "item agreed" } else { "item" })
            .kid(el("div").cls("main").kid(el("div").cls("title").text(&title)).kid(el("div").cls("sub").text(&s(p, "text")).attr("title", &s(p, "text"))))
            .kid(if mine == "agree" { el("span").cls("status-pill sent").text("동의함") } else { el("button").cls("primary").text("좋아요").click(move |_| redo("team:respond", json!({ "proposalId": id, "response": "agree" }))) })
            .maybe(if mine.is_empty() { Some(el("button").cls("link").text("나중에").click(move |_| redo("team:respond", json!({ "proposalId": id2, "response": "later" })))) } else { None });
        let _ = pl.append_child(&item.0);
    }

    // 공유 대기함
    let ob = by_id("outbox").unwrap();
    clear(&ob);
    if outbox.is_empty() {
        let _ = ob.append_child(&el("div").cls("empty").text("아직 공유하기로 한 주제가 없어요.").0);
    }
    let many_teams = arr(&team, "teams").len() > 1;
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
        } else if status == "held" {
            let topic = s(it, "topic");
            let _ = controls.append_child(&el("button").text("팀 약속으로 맞춰 보자고 제안").click(move |_| redo("team:propose", json!(topic))).0);
        }
        let to = if target == "person" { name_of(&s(it, "toMember")).filter(|_| !s(it, "toMember").is_empty()).map(|n| format!(" → {n}")).unwrap_or_default() } else { String::new() };
        let team_tag = if many_teams && !s(it, "teamName").is_empty() { format!("[{}] ", s(it, "teamName")) } else { String::new() };
        let has_controls = controls.child_element_count() > 0;
        let pill = if connected || status != "queued" { status_text(&status) } else { "팀 연결 전" };
        let id2 = id.clone();
        let item = el("div")
            .cls("item")
            .kid(
                el("div")
                    .cls("main")
                    .kid(el("div").cls("title").text(&format!("{team_tag}{} · {}{to}", s(it, "label"), target_text(&target))))
                    .kid(el("div").cls("sub").text(&if target == "team" { s(it, "teamProposal") } else { s(it, "advice") }).attr("title", &s(it, "advice")))
                    .maybe(if status == "held" { Some(el("div").cls("hint").text("상대가 자기 사정을 이야기해서 이 조언은 멈췄어요. 한쪽 이야기만으로 교정하지 않도록, 팀 약속으로 함께 맞춰 보는 걸 추천해요.")) } else { None })
                    .maybe(if has_controls { Some(El(controls)) } else { None }),
            )
            .kid(el("span").cls(&format!("status-pill {status}")).text(pill))
            .maybe(if status == "sent" || status == "held" { None } else { Some(el("button").cls("link").text("빼기").click(move |_| redo("share:remove", json!(id2)))) });
        let _ = ob.append_child(&item.0);
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
    }
    scroll_into_view(&view, true);
}
