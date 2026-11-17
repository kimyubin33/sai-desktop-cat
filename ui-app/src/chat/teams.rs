use super::{b, profile, s};
use crate::{api::call, dom::*};
use serde_json::{json, Value};
use std::cell::Cell;
thread_local! {
    static EPOCH:Cell<u64> = const{Cell::new(0)};
    static BUSY:Cell<bool> = const{Cell::new(false)};
    static GENERATION:Cell<u64> = const{Cell::new(0)};
    static LOAD:Cell<u64> = const{Cell::new(0)};
}
pub fn account(v: &Value) {
    let epoch = v["epoch"].as_u64().unwrap_or(0);
    if epoch < EPOCH.with(Cell::get) {
        return;
    }
    super::files::account(v);
    super::drive_picker::account(v);
    super::collab::account(v);
    super::analysis::account(v);
    super::agenda::account(v);
    if EPOCH.with(|e| e.replace(epoch)) != epoch || !b(v, "signedIn") {
        GENERATION.with(|g| g.set(g.get() + 1));
        clear(&by_id("teamList").unwrap());
        toggle(&by_id("teamCreateForm").unwrap(), "hidden", true);
        toggle(&by_id("teamResume").unwrap(), "hidden", true);
    }
    if !b(v, "signedIn") {
        set_text(
            "teamMessage",
            "Google 로그인 후 팀 폴더를 연결할 수 있어요.",
        );
        for id in ["teamEnable", "teamNew", "teamPick", "teamRefresh"] {
            set_disabled(&by_id(id).unwrap(), true);
            toggle(&by_id(id).unwrap(), "hidden", true);
        }
    }
}
fn draw(v: &Value) {
    if !profile::accept(v) {
        return;
    }
    account(v);
    let signed = b(v, "signedIn");
    let connected = b(v, "driveFull");
    let ready = b(v, "ready");
    let busy = BUSY.with(Cell::get);
    toggle(
        &by_id("teamEnable").unwrap(),
        "hidden",
        connected || !signed,
    );
    toggle(&by_id("teamRefresh").unwrap(), "hidden", !signed);
    set_disabled(&by_id("teamEnable").unwrap(), busy || !signed || !ready);
    for id in ["teamNew", "teamPick"] {
        toggle(&by_id(id).unwrap(), "hidden", !connected);
        set_disabled(&by_id(id).unwrap(), busy || !signed || !connected || !ready);
    }
    set_disabled(&by_id("teamRefresh").unwrap(), busy || !signed);
    let draft = v["draft"].is_object();
    toggle(&by_id("teamResume").unwrap(), "hidden", !draft);
    if draft {
        set_text(
            "teamResume",
            &format!("‘{}’ 이어 만들기 →", s(&v["draft"], "name")),
        );
    }
    set_disabled(&by_id("teamResume").unwrap(), busy || !connected || !ready);
    let list = by_id("teamList").unwrap();
    // Preserve an unfinished invite while a periodic permission check refreshes the cards.
    let drafts: std::collections::BTreeMap<_, _> = q_all(".team-invite-form")
        .iter()
        .filter_map(|form| {
            let folder = form.get_attribute("data-folder")?;
            let input = q_in(form, "input[name=email]")?;
            Some((folder, val(&input)))
        })
        .collect();
    clear(&list);
    let rows = v["teams"].as_array().cloned().unwrap_or_default();
    super::collab::teams(&rows);
    super::files::teams_changed(&rows);
    for team in &rows {
        let fid = s(team, "folderId");
        let epoch = EPOCH.with(Cell::get);
        let verified = b(team, "accessVerified");
        let subtitle = if !verified {
            s(team, "error")
        } else if b(team, "canInvite") {
            "내가 만든 팀 · 소유자".into()
        } else if b(team, "canUpload") {
            "함께 쓰는 팀 · 편집 가능".into()
        } else {
            "함께 쓰는 팀 · 보기 가능".into()
        };
        let open_id = fid.clone();
        let file_team = team.clone();
        let card = el("article").cls("team-card").kid(
            el("div")
                .cls("team-card-heading")
                .kid(
                    el("div")
                        .kid(el("h3").text(&s(team, "name")))
                        .kid(el("p").text(&subtitle)),
                )
                .kid(
                    el("button")
                        .cls("subtle")
                        .attr("type", "button")
                        .flag("disabled", busy)
                        .text("파일 보기 →")
                        .click(move |_| {
                            super::files::show(file_team.clone());
                        }),
                ),
        );
        let details = el("details")
            .cls("team-options")
            .kid(el("summary").text("팀 관리 ＋"))
            .kid(
                el("button")
                    .cls("text-button")
                    .attr("type", "button")
                    .flag("disabled", !verified || busy)
                    .text("Drive 폴더 열기 ↗")
                    .click(move |_| {
                        action("teams:open", json!({"folderId":open_id,"epoch":epoch}))
                    }),
            );
        let details = if b(team, "canInvite") && verified {
            let invite_id = fid.clone();
            let form = el("form")
                .cls("team-invite-form")
                .attr("data-folder", &fid)
                .kid(
                    el("label")
                        .cls("field-label")
                        .text("팀원의 Google 이메일")
                        .kid(
                            el("input")
                                .attr("name", "email")
                                .attr("type", "email")
                                .attr("maxlength", "254")
                                .attr("autocomplete", "off")
                                .attr("value", drafts.get(&fid).map(String::as_str).unwrap_or(""))
                                .flag("required", true)
                                .flag("disabled", busy),
                        ),
                )
                .kid(
                    el("p")
                        .cls("micro")
                        .text("이 계정에 팀 폴더 편집 권한을 추가하고 Google 알림 메일을 보내요."),
                )
                .kid(
                    el("button")
                        .cls("subtle")
                        .attr("type", "submit")
                        .flag("disabled", busy)
                        .text("초대 보내기"),
                );
            on(form.node(), "submit", move |e| {
                e.prevent_default();
                if let Some(target) = e.current_target() {
                    use wasm_bindgen::JsCast;
                    let form: web_sys::Element = target.unchecked_into();
                    if let Some(input) = q_in(&form, "input[name=email]") {
                        action(
                            "teams:invite",
                            json!({"folderId":invite_id,"email":val(&input),"epoch":epoch}),
                        );
                    }
                }
            });
            details.kid(form)
        } else {
            details
        };
        let remove_id = fid.clone();
        let details = details.kid(
            el("button")
                .cls("text-button")
                .attr("type", "button")
                .flag("disabled", busy)
                .text("내 목록에서 빼기")
                .click(move |_| {
                    if confirm(
                        "내 팀 목록에서 뺄까요? Drive 폴더·파일과 Google 공유 권한은 유지돼요.",
                    ) {
                        action("teams:remove", json!({"folderId":remove_id,"epoch":epoch}));
                    }
                }),
        );
        let _ = list.append_child(&card.kid(details).build());
    }
    if !busy {
        set_text(
            "teamMessage",
            if !signed {
                "Google 로그인 후 팀 폴더를 연결할 수 있어요."
            } else if !ready {
                "먼저 계정의 기존 설정을 동기화해 주세요."
            } else if !connected {
                "Google 연결을 한 번 갱신하면 앱 안에서 파일과 초대받은 팀을 고를 수 있어요."
            } else if rows.is_empty() {
                "새 팀을 만들거나 초대받은 폴더를 선택해 주세요."
            } else if rows.iter().any(|row| !b(row, "accessVerified")) {
                "연결을 확인하지 못한 팀 폴더가 있어요. 공유 상태와 인터넷 연결을 확인해 주세요."
            } else if v["pending"].as_u64().unwrap_or(0) > 0 {
                "계정 변경을 이 기기에 보관했어요. 다른 PC에 반영하려면 동기화를 마쳐야 해요."
            } else {
                "현재 Drive 권한을 확인했어요. 팀의 파일 보기에서 자료를 올리고 받을 수 있어요."
            },
        );
    }
}
pub async fn load() {
    if BUSY.with(Cell::get) {
        return;
    }
    let generation = GENERATION.with(Cell::get);
    let request = LOAD.with(|n| {
        n.set(n.get() + 1);
        n.get()
    });
    let result = call("teams:status", Value::Null).await;
    if generation != GENERATION.with(Cell::get)
        || request != LOAD.with(Cell::get)
        || BUSY.with(Cell::get)
    {
        return;
    }
    match result {
        Ok(v) => {
            let editing = document()
                .active_element()
                .is_some_and(|e| e.closest(".team-invite-form").ok().flatten().is_some());
            if editing && v["epoch"].as_u64() == Some(EPOCH.with(Cell::get)) && b(&v, "signedIn") {
                return;
            }
            draw(&v);
        }
        Err(e) => set_text("teamMessage", &e),
    }
}
fn action(channel: &'static str, mut args: Value) {
    if BUSY.with(|b| b.replace(true)) {
        return;
    }
    if args.get("epoch").is_none() {
        args["epoch"] = json!(EPOCH.with(Cell::get));
    }
    let request_epoch = EPOCH.with(Cell::get);
    GENERATION.with(|g| g.set(g.get() + 1));
    let requested_folder = s(&args, "folderId");
    for control in q_all("#teamSpace button, #teamSpace input") {
        set_disabled(&control, true);
    }
    toggle(
        &by_id("teamCancel").unwrap(),
        "hidden",
        channel != "teams:enable",
    );
    set_disabled(&by_id("teamCancel").unwrap(), false);
    set_text(
        "teamMessage",
        match channel {
            "teams:enable" => "브라우저에서 앱과 같은 Google 계정으로 Drive 연결을 허용해 주세요.",
            "teams:connect" => "선택한 팀의 현재 공유 권한을 확인하고 있어요…",
            "teams:invite" => "Google 공유 권한과 초대 결과를 확인하고 있어요…",
            _ => "팀 폴더를 확인하고 있어요…",
        },
    );
    spawn(async move {
        let result = call(channel, args).await;
        BUSY.with(|b| b.set(false));
        toggle(&by_id("teamCancel").unwrap(), "hidden", true);
        for control in q_all("#teamSpace button, #teamSpace input") {
            set_disabled(&control, false);
        }
        match result {
            Ok(v) => {
                if v.get("state").is_some() {
                    draw(&v["state"]);
                } else {
                    load().await;
                }
                if channel == "teams:create" {
                    toggle(&by_id("teamCreateForm").unwrap(), "hidden", true);
                    set_val(&by_id("teamName").unwrap(), "");
                }
                if channel == "teams:invite" && request_epoch == EPOCH.with(Cell::get) {
                    for form in q_all(".team-invite-form") {
                        if form.get_attribute("data-folder").as_deref()
                            == Some(requested_folder.as_str())
                        {
                            if let Some(input) = q_in(&form, "input[name=email]") {
                                set_val(&input, "");
                            }
                        }
                    }
                    set_text(
                        "teamMessage",
                        if b(&v["result"], "alreadyShared") {
                            "이미 편집 권한이 있는 팀원이에요. 초대받은 팀 연결에서 폴더를 선택하면 돼요."
                        } else {
                            "편집 권한을 확인했어요. 팀원은 Google 알림 메일을 확인한 뒤 초대받은 팀 연결을 눌러 주세요."
                        },
                    );
                }
                super::workspace::load().await;
            }
            Err(e) => {
                load().await;
                if request_epoch == EPOCH.with(Cell::get) {
                    set_text("teamMessage", &e);
                }
            }
        }
    });
}
pub fn bind() {
    on(&by_id("teamEnable").unwrap(), "click", |_| {
        action("teams:enable", json!({}))
    });
    on(&by_id("teamPick").unwrap(), "click", |_| {
        super::drive_picker::teams(EPOCH.with(Cell::get));
    });
    on(&by_id("teamNew").unwrap(), "click", |_| {
        toggle(&by_id("teamCreateForm").unwrap(), "hidden", false);
        let _ = by_id("teamName").unwrap().focus();
    });
    on(&by_id("teamCreateCancel").unwrap(), "click", |_| {
        toggle(&by_id("teamCreateForm").unwrap(), "hidden", true)
    });
    on(&by_id("teamCreateForm").unwrap(), "submit", |e| {
        e.prevent_default();
        action(
            "teams:create",
            json!({"name":val(&by_id("teamName").unwrap())}),
        );
    });
    on(&by_id("teamResume").unwrap(), "click", |_| {
        action("teams:create", json!({"name":""}))
    });
    on(&by_id("teamRefresh").unwrap(), "click", |_| spawn(load()));
    on(&by_id("teamCancel").unwrap(), "click", |_| {
        spawn(async {
            let _ = call("g:cancel-login", Value::Null).await;
        })
    });
}
pub fn connect(folder: String, epoch: u64) {
    if epoch == EPOCH.with(Cell::get) {
        action("teams:connect", json!({"folderId":folder,"epoch":epoch}));
    }
}
