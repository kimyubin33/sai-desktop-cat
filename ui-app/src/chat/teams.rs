use super::{arr, b, profile, s};
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
        set_text("teamMessage", "");
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
    let rows = arr(v, "teams");
    super::collab::teams(rows);
    super::files::teams_changed(rows);
    for team in rows {
        let fid = s(team, "folderId");
        let epoch = EPOCH.with(Cell::get);
        let verified = b(team, "accessVerified");
        let subtitle = if !verified {
            s(team, "error")
        } else if b(team, "canInvite") {
            "소유자".into()
        } else if b(team, "canUpload") {
            "편집".into()
        } else {
            "보기".into()
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
                        .attr("data-icon", "arrow")
                        .attr("aria-label", "팀 파일 열기")
                        .attr("title", "팀 파일 열기")
                        .text("파일")
                        .click(move |_| {
                            super::files::show(file_team.clone());
                        }),
                ),
        );
        let details = el("details")
            .cls("team-options")
            .kid(
                el("summary")
                    .attr("aria-label", "팀 관리")
                    .attr("title", "팀 관리")
                    .text("···"),
            )
            .kid(
                el("button")
                    .cls("text-button")
                    .attr("type", "button")
                    .flag("disabled", !verified || busy)
                    .attr("data-icon", "external")
                    .attr("title", "Google Drive 폴더 열기")
                    .text("Google")
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
                    el("label").cls("field-label").text("이메일").kid(
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
                    el("button")
                        .cls("subtle")
                        .attr("type", "submit")
                        .flag("disabled", busy)
                        .text("초대"),
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
                .text("목록에서 빼기")
                .click(move |_| {
                    if confirm("목록에서 뺄까요? Drive 원본은 유지돼요.") {
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
                ""
            } else if !ready {
                "동기화 필요"
            } else if !connected {
                "Google 연결 갱신 필요"
            } else if rows.is_empty() {
                "팀 없음"
            } else if rows.iter().any(|row| !b(row, "accessVerified")) {
                "연결을 확인하지 못한 팀 폴더가 있어요. 공유 상태와 인터넷 연결을 확인해 주세요."
            } else if v["pending"].as_u64().unwrap_or(0) > 0 {
                "동기화 대기"
            } else {
                ""
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
            "teams:connect" => "연결 중…",
            "teams:invite" => "초대 중…",
            _ => "확인 중…",
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
                            "이미 초대된 계정"
                        } else {
                            "초대됨"
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
