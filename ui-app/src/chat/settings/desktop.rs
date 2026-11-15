use super::{click, s, v};
use crate::api::call;
use crate::dom::*;
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};

thread_local! {
    static VERIFIED: RefCell<Option<Value>> = const { RefCell::new(None) };
    static BUSY: Cell<bool> = const { Cell::new(false) };
}

fn buttons() {
    let busy = BUSY.with(Cell::get);
    let allow_apply = VERIFIED.with(|v| v.borrow().is_some());
    for (id, disabled) in [("desktopCheck", busy), ("desktopApply", busy || !allow_apply), ("desktopRestore", busy)] {
        if let Some(el) = by_id(id) { set_disabled(&el, disabled); }
    }
}

pub(super) fn render(info: &Value, local: bool) -> String {
    let host = s(&info["teamServer"], "url");
    if let Some(box_) = by_id("desktopServer") { toggle(&box_, "hidden", local); }
    if let Some(el) = by_id("desktopRestore") { toggle(&el, "hidden", host.is_empty()); }
    if let Some(input) = by_id("desktopServerUrl") {
        if input.get_attribute("data-current").as_deref() != Some(&host) {
            set_val(&input, &host);
            let _ = input.set_attribute("data-current", &host);
            VERIFIED.with(|v| v.replace(None));
            set_text("desktopMsg", &s(info, "teamServerNotice"));
            buttons();
        }
    }
    if local { "로컬 시험 모드".into() }
    else if host.is_empty() { format!("Firebase · {}", s(info, "projectId")) }
    else { format!("데스크톱 서버 · {host}") }
}

async fn switch(target: Value) -> Result<(), String> {
    let name = target["url"].as_str().unwrap_or("Firebase");
    let prompt = if target.is_null() {
        "Firebase에서 이전에 사용한 팀 목록으로 돌아갈까요?".into()
    } else {
        format!("{name}을 팀 운영자가 제공한 주소로 신뢰하나요? 기존 팀 계정의 단기 로그인 토큰을 이 서버에 보내 팀원 자격을 확인합니다.")
    };
    if !confirm(&prompt) { return Ok(()); }
    set_text("desktopMsg", "이전된 팀과 계정을 확인하는 중…");
    let plan = call("team:host-plan", target.clone()).await?;
    if !target.is_null() && !VERIFIED.with(|v| v.borrow().as_ref() == Some(&target)) {
        return Err("입력한 주소가 바뀌었어요. 새 주소를 다시 확인해 주세요.".into());
    }
    let names = |key: &str| plan[key].as_array().into_iter().flatten().filter_map(|t| t.as_str()).collect::<Vec<_>>().join(", ");
    let teams = names("teams");
    let skipped = names("skipped");
    let mut summary = format!("{name}을 사용하도록 사이를 다시 시작할까요?");
    if !teams.is_empty() { summary.push_str(&format!("\n연결할 팀: {teams}")); }
    if !skipped.is_empty() { summary.push_str(&format!("\n이전 서버에만 남는 팀: {skipped}")); }
    if !confirm(&summary) {
        set_text("desktopMsg", "현재 서버를 계속 사용해요.");
        return Ok(());
    }
    set_text("desktopMsg", "설정을 저장하고 사이를 다시 시작해요…");
    call("team:host-apply", json!({"id":plan["id"]})).await?;
    Ok(())
}

pub(super) fn bind() {
    if let Some(input) = by_id("desktopServerUrl") {
        on(&input, "input", |_| {
            VERIFIED.with(|v| v.replace(None));
            set_text("desktopMsg", "");
            buttons();
        });
    }
    click("desktopCheck", |_| {
        if BUSY.with(|b| b.replace(true)) { return; }
        VERIFIED.with(|slot| slot.replace(None));
        buttons();
        let address = v("desktopServerUrl");
        set_text("desktopMsg", "서버에 연결하는 중…");
        spawn(async move {
            let result = call("team:host-check", json!({"url":address})).await;
            if v("desktopServerUrl") == address {
                match result {
                    Ok(host) => {
                        set_text("desktopMsg", &format!("{}에서 사이 팀 서버를 확인했어요.", s(&host,"url")));
                        VERIFIED.with(|v| v.replace(Some(host)));
                    }
                    Err(e) => set_text("desktopMsg", &e),
                }
            }
            BUSY.with(|b| b.set(false));
            buttons();
        });
    });
    for (id, cloud) in [("desktopRestore", true), ("desktopApply", false)] {
        click(id, move |_| {
            let target = if cloud { Some(Value::Null) } else { VERIFIED.with(|v| v.borrow().clone()) };
            let Some(target) = target else { return };
            if BUSY.with(|b| b.replace(true)) { return; }
            buttons();
            spawn(async move {
                if let Err(e) = switch(target).await { set_text("desktopMsg", &e); }
                BUSY.with(|b| b.set(false));
                buttons();
            });
        });
    }
    click("desktopMyUid", |_| {
        set_text("desktopMsg", "내 팀 계정을 확인하는 중…");
        spawn(async {
            match call("team:my-uid", Value::Null).await {
                Ok(uid) => {
                    if let Some(el) = by_id("desktopUid") {
                        set_val(&el, uid.as_str().unwrap_or_default());
                        toggle(&el, "hidden", false);
                    }
                    set_text("desktopMsg", "서버에서 새 팀을 만들 권한을 신청할 때 이 ID를 사용하세요.");
                }
                Err(e) => set_text("desktopMsg", &e)
            }
        });
    });
}
