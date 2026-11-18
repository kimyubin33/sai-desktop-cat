use super::{arr, b, profile, s};
use crate::{api::call, dom::*};
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
thread_local! {
    static EPOCH:Cell<u64> = const{Cell::new(0)};
    static BUSY:Cell<bool> = const{Cell::new(false)};
    static GENERATION:Cell<u64> = const{Cell::new(0)};
    static LOAD:Cell<u64> = const{Cell::new(0)};
    /// Team whose management panel stays open across a redraw (after invite / member / policy changes).
    static OPEN:RefCell<String> = const{RefCell::new(String::new())};
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
        OPEN.with(|o| o.borrow_mut().clear());
        clear(&by_id("teamList").unwrap());
        toggle(&by_id("teamCreateForm").unwrap(), "hidden", true);
        toggle(&by_id("teamResume").unwrap(), "hidden", true);
    }
    if !b(v, "signedIn") {
        set_text("teamMessage", "");
        for id in ["teamEnable", "teamNew", "teamPick", "teamRefresh", "teamSync"] {
            set_disabled(&by_id(id).unwrap(), true);
            toggle(&by_id(id).unwrap(), "hidden", true);
        }
    }
}
fn role_label(role: &str) -> &'static str {
    match role {
        "owner" => "소유자",
        "writer" | "fileOrganizer" | "organizer" => "편집",
        "commenter" => "댓글",
        _ => "보기",
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
    // Drive consent does not depend on the settings sync; only creating or joining a team does.
    set_disabled(&by_id("teamEnable").unwrap(), busy || !signed);
    for id in ["teamNew", "teamPick"] {
        toggle(&by_id(id).unwrap(), "hidden", !connected);
        set_disabled(&by_id(id).unwrap(), busy || !signed || !connected || !ready);
    }
    toggle(&by_id("teamSync").unwrap(), "hidden", !signed || ready);
    set_disabled(&by_id("teamSync").unwrap(), busy);
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
    let open_folder = OPEN.with(|o| o.borrow().clone());
    clear(&list);
    let rows = arr(v, "teams");
    super::collab::teams(rows);
    super::files::teams_changed(rows);
    for team in rows {
        let fid = s(team, "folderId");
        let epoch = EPOCH.with(Cell::get);
        let verified = b(team, "accessVerified");
        let manage = verified && b(team, "canManage");
        let subtitle = if !verified {
            s(team, "error")
        } else if manage {
            if b(team, "membersCanInvite") {
                "소유자 · 멤버도 초대 가능".into()
            } else {
                "소유자".into()
            }
        } else {
            let access = if b(team, "canUpload") { "편집" } else { "보기" };
            match s(team, "ownerEmail") {
                owner if owner.is_empty() => access.into(),
                owner => format!("{access} · 소유자 {owner}"),
            }
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
        let mut details = el("details")
            .cls("team-options")
            .flag("open", open_folder == fid)
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
        if b(team, "canInvite") && verified {
            details = details.kid(invite_form(team, &fid, epoch, busy, &drafts, manage));
        }
        if manage {
            details = details.kid(owner_tools(team, &fid, epoch, busy));
        }
        let remove_id = fid.clone();
        let details = details.kid(
            el("button")
                .cls("text-button")
                .attr("type", "button")
                .flag("disabled", busy)
                .text("목록에서 빼기")
                .click(move |_| {
                    if confirm("목록에서 뺄까요? Drive 원본과 공유 상태는 그대로예요.") {
                        action("teams:remove", json!({"folderId":remove_id,"epoch":epoch}));
                    }
                }),
        );
        let _ = list.append_child(&card.kid(details).build());
    }
    if !busy {
        let sync_error = s(v, "syncError");
        let message = if !signed {
            String::new()
        } else if !ready {
            // Joining and creating teams are recorded in the account's settings, so they wait for the sync.
            if sync_error.is_empty() {
                "계정 설정을 동기화하는 중이에요. 끝나면 팀을 만들거나 참여할 수 있어요.".into()
            } else {
                format!("계정 설정 동기화가 끝나지 않아 팀 만들기·참여를 잠시 막았어요. {sync_error}")
            }
        } else if !connected {
            "팀 폴더를 쓰려면 ‘Google’ 버튼으로 Drive 연결을 허용해 주세요. 로그인할 때는 Drive 권한을 받지 않아요.".into()
        } else if rows.is_empty() {
            "아직 팀이 없어요. 팀을 만들거나, 초대받았다면 ‘참여’를 눌러 주세요.".into()
        } else if rows.iter().any(|row| !b(row, "accessVerified")) {
            "연결을 확인하지 못한 팀 폴더가 있어요. 공유 상태와 인터넷 연결을 확인해 주세요.".into()
        } else if v["pending"].as_u64().unwrap_or(0) > 0 {
            "동기화 대기".into()
        } else {
            String::new()
        };
        set_text("teamMessage", &message);
    }
}
fn invite_form(
    team: &Value,
    fid: &str,
    epoch: u64,
    busy: bool,
    drafts: &std::collections::BTreeMap<String, String>,
    manage: bool,
) -> El {
    let invite_id = fid.to_owned();
    let team_name = s(team, "name");
    let form = el("form")
        .cls("team-invite-form")
        .attr("data-folder", fid)
        .kid(
            el("label").cls("field-label").text("초대할 Google 이메일").kid(
                el("input")
                    .attr("name", "email")
                    .attr("type", "email")
                    .attr("maxlength", "254")
                    .attr("autocomplete", "off")
                    .attr("value", drafts.get(fid).map(String::as_str).unwrap_or(""))
                    .flag("required", true)
                    .flag("disabled", busy),
            ),
        )
        .kid(
            el("label").cls("field-label").text("권한").kid(
                el("select")
                    .attr("name", "role")
                    .flag("disabled", busy)
                    .kid(el("option").attr("value", "writer").text("편집 — 올리기·고치기·지우기"))
                    .kid(el("option").attr("value", "reader").text("보기 — 내려받기만")),
            ),
        )
        .kid(el("p").cls("micro").text(if manage {
            "초대한 사람은 이 팀 폴더의 모든 파일에 접근해요. Google이 초대 메일을 보내요."
        } else {
            "소유자가 멤버 초대를 허용한 팀이에요. 초대한 사람은 이 팀 폴더의 모든 파일에 접근해요."
        }))
        .kid(
            el("button")
                .cls("subtle")
                .attr("type", "submit")
                .flag("disabled", busy)
                .text("초대"),
        );
    on(form.node(), "submit", move |e| {
        e.prevent_default();
        let Some(target) = e.current_target() else {
            return;
        };
        use wasm_bindgen::JsCast;
        let form: web_sys::Element = target.unchecked_into();
        let (Some(input), Some(select)) = (
            q_in(&form, "input[name=email]"),
            q_in(&form, "select[name=role]"),
        ) else {
            return;
        };
        let email = val(&input).trim().to_owned();
        let role = val(&select);
        let reach = if role == "reader" {
            "모든 파일을 보고 내려받을 수 있어요"
        } else {
            "모든 파일을 보고, 올리고, 고치고, 지울 수 있어요"
        };
        // An e-mail typo hands the whole folder to a stranger, so the owner confirms the exact address.
        if confirm(&format!(
            "{email} 계정을 ‘{team_name}’ 팀에 {} 권한으로 초대할까요?\n\n이 계정은 팀 폴더의 {reach}.\n주소를 한 글자씩 다시 확인해 주세요.",
            role_label(&role)
        )) {
            OPEN.with(|o| *o.borrow_mut() = invite_id.clone());
            action(
                "teams:invite",
                json!({"folderId":invite_id,"email":email,"role":role,"epoch":epoch}),
            );
        }
    });
    form
}
fn owner_tools(team: &Value, fid: &str, epoch: u64, busy: bool) -> El {
    let policy_id = fid.to_owned();
    let members_id = fid.to_owned();
    let open = b(team, "membersCanInvite");
    let policy = el("label")
        .cls("check-row")
        .kid(
            el("input")
                .attr("type", "checkbox")
                .flag("checked", open)
                .flag("disabled", busy)
                .on("change", move |e| {
                    let Some(target) = event_target(&e) else {
                        return;
                    };
                    let next = checked(&target);
                    if next
                        && !confirm("편집 권한 멤버도 다른 사람을 초대할 수 있게 할까요?\n공유 서버처럼 자유로워지지만, 누가 들어오는지는 소유자가 직접 확인해야 해요.")
                    {
                        set_checked(&target, false);
                        return;
                    }
                    OPEN.with(|o| *o.borrow_mut() = policy_id.clone());
                    action(
                        "teams:policy",
                        json!({"folderId":policy_id,"membersCanInvite":next,"epoch":epoch}),
                    );
                }),
        )
        .kid(el("span").text("편집 멤버도 초대 가능"));
    let panel = el("div")
        .cls("team-members")
        .attr("data-members", fid);
    el("div")
        .cls("team-owner-tools")
        .kid(policy)
        .kid(
            el("button")
                .cls("text-button")
                .attr("type", "button")
                .flag("disabled", busy)
                .text("팀원 관리")
                .click(move |_| spawn(members(members_id.clone(), epoch))),
        )
        .kid(panel)
}
fn members_panel(fid: &str) -> Option<web_sys::HtmlElement> {
    q_all(".team-members")
        .into_iter()
        .find(|p| p.get_attribute("data-members").as_deref() == Some(fid))
}
async fn members(fid: String, epoch: u64) {
    let Some(panel) = members_panel(&fid) else {
        return;
    };
    clear(&panel);
    let _ = panel.append_child(&el("p").cls("micro").text("팀원 확인 중…").build());
    let result = call("teams:members", json!({"folderId":fid,"epoch":epoch})).await;
    // The card may have been redrawn while waiting; look the panel up again.
    let Some(panel) = members_panel(&fid) else {
        return;
    };
    clear(&panel);
    if epoch != EPOCH.with(Cell::get) {
        return;
    }
    let rows = match result {
        Ok(v) => v["members"].as_array().cloned().unwrap_or_default(),
        Err(e) => {
            let _ = panel.append_child(&el("p").cls("micro").text(&e).build());
            return;
        }
    };
    if rows.iter().any(|m| b(m, "public")) {
        let _ = panel.append_child(
            &el("p")
                .cls("micro")
                .text("⚠ 이 폴더에 ‘링크가 있는 사용자’ 같은 공개 권한이 있어요. 팀원이 아니어도 열 수 있으니 내보내기를 권해요.")
                .build(),
        );
    }
    for m in rows {
        let pid = s(&m, "id");
        let role = s(&m, "role");
        let who = if b(&m, "public") {
            if s(&m, "type") == "domain" {
                format!("{} 도메인 전체", s(&m, "domain"))
            } else {
                "링크가 있는 모든 사용자".into()
            }
        } else {
            match (s(&m, "name"), s(&m, "email")) {
                (n, e) if n.is_empty() => e,
                (n, e) if e.is_empty() => n,
                (n, e) => format!("{n} · {e}"),
            }
        };
        let mut row = el("div")
            .cls("team-member")
            .kid(el("span").text(&who))
            .kid(el("small").text(role_label(&role)));
        if role != "owner" {
            if !b(&m, "public") {
                let (role_fid, role_pid) = (fid.clone(), pid.clone());
                let current = role.clone();
                row = row.kid(
                    el("select")
                        .attr("aria-label", "권한 바꾸기")
                        .kid(el("option").attr("value", "writer").flag("selected", role == "writer").text("편집"))
                        .kid(el("option").attr("value", "reader").flag("selected", role != "writer").text("보기"))
                        .on("change", move |e| {
                            let Some(target) = event_target(&e) else {
                                return;
                            };
                            let next = val(&target);
                            if next == current {
                                return;
                            }
                            OPEN.with(|o| *o.borrow_mut() = role_fid.clone());
                            action(
                                "teams:member",
                                json!({"folderId":role_fid,"memberId":role_pid,"role":next,"epoch":epoch}),
                            );
                        }),
                );
            }
            let (kick_fid, kick_pid, name) = (fid.clone(), pid.clone(), who.clone());
            row = row.kid(
                el("button")
                    .cls("text-button")
                    .attr("type", "button")
                    .text("내보내기")
                    .click(move |_| {
                        if confirm(&format!(
                            "{name}을(를) 팀에서 내보낼까요?\n팀 폴더 접근 권한이 바로 사라져요. 이미 내려받은 파일은 회수되지 않아요."
                        )) {
                            OPEN.with(|o| *o.borrow_mut() = kick_fid.clone());
                            action(
                                "teams:member",
                                json!({"folderId":kick_fid,"memberId":kick_pid,"role":"remove","epoch":epoch}),
                            );
                        }
                    }),
            );
        }
        let _ = panel.append_child(&row.build());
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
            let editing = document().active_element().is_some_and(|e| {
                e.closest(".team-invite-form, .team-members")
                    .ok()
                    .flatten()
                    .is_some()
            });
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
    for control in q_all("#teamSpace button, #teamSpace input, #teamSpace select") {
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
            "teams:connect" => "팀 폴더 권한을 확인하고 연결하는 중…",
            "teams:invite" => "초대 중…",
            "teams:member" => "팀원 권한을 바꾸는 중…",
            "teams:policy" => "초대 설정을 바꾸는 중…",
            _ => "확인 중…",
        },
    );
    spawn(async move {
        let result = call(channel, args).await;
        BUSY.with(|b| b.set(false));
        toggle(&by_id("teamCancel").unwrap(), "hidden", true);
        for control in q_all("#teamSpace button, #teamSpace input, #teamSpace select") {
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
                if channel == "teams:connect" && request_epoch == EPOCH.with(Cell::get) {
                    set_text("teamMessage", "팀에 참여했어요.");
                }
                if channel == "teams:member" && request_epoch == EPOCH.with(Cell::get) {
                    set_text("teamMessage", "팀원 권한을 바꿨어요.");
                    spawn(members(requested_folder.clone(), request_epoch));
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
                            "이미 같은 권한으로 초대된 계정이에요."
                        } else {
                            "초대했어요. 상대가 같은 Google 계정으로 로그인해 ‘참여’를 누르면 돼요."
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
    on(&by_id("teamSync").unwrap(), "click", |_| {
        if BUSY.with(Cell::get) {
            return;
        }
        set_disabled(&by_id("teamSync").unwrap(), true);
        set_text("teamMessage", "계정 설정을 다시 동기화하는 중…");
        spawn(async {
            let _ = call("profile:sync", Value::Null).await;
            load().await;
        })
    });
    on(&by_id("teamCancel").unwrap(), "click", |_| {
        spawn(async {
            let _ = call("g:cancel-login", Value::Null).await;
        })
    });
}
/// `folder` is a folder ID from the team list or a pasted Drive folder link.
pub fn connect(folder: String, epoch: u64) {
    if epoch == EPOCH.with(Cell::get) {
        action("teams:connect", json!({"folderId":folder,"epoch":epoch}));
    }
}
