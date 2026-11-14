//! 설정 탭: 팀 연결, 버전·업데이트, 말 걸기·기록·AI 설정, 팀에 Claude 키 공유.
use super::{arr, b, n, refresh_team_switch, s, st, update_engine};
use crate::api::call;
use crate::dom::*;
use serde_json::{json, Value};
use std::cell::Cell;
use std::future::Future;
use web_sys::Element;

fn form() -> Element {
    by_id("settingsForm").unwrap().into()
}

pub async fn load() {
    let Ok(sx) = call("settings:get", Value::Null).await else { return };
    st(|x| x.settings = sx.clone());
    let f = form();
    let freq = s(&sx, "frequency");
    set_val(&field(&f, "frequency"), if ["low", "normal", "high"].contains(&freq.as_str()) { &freq } else { "low" });
    let dnd = &sx["dnd"];
    set_checked(&field(&f, "dndOn"), dnd.is_object());
    if dnd.is_object() {
        set_val(&field(&f, "dndStart"), &s(dnd, "start"));
        set_val(&field(&f, "dndEnd"), &s(dnd, "end"));
    }
    set_val(&field(&f, "retentionDays"), &s(&sx, "retentionDays"));
    set_checked(&field(&f, "aiEnabled"), b(&sx["ai"], "enabled"));
    set_val(&field(&f, "model"), &s(&sx["ai"], "model"));
    set_val(&field(&f, "openAiModel"), &s(&sx, "openAiModel"));
    key_placeholder(&sx);
    update_engine(&sx);
}

fn key_placeholder(sx: &Value) {
    let f = form();
    let _ = field(&f, "apiKey").set_attribute("placeholder", if b(sx, "hasOwnKey") { "저장된 키가 있어요 (바꿀 때만 입력)" } else { "sk-ant-..." });
    let gpt_hint = if b(sx, "hasOpenAiKey") {
        if b(sx, "openAiKeyPersistent") { "저장된 GPT 키가 있어요 (바꿀 때만 입력)" } else { "현재 실행 중에 사용할 GPT 키가 있어요" }
    } else if b(sx, "openAiKeyPersistent") {
        "OpenAI API 키 입력"
    } else {
        "OpenAI API 키 입력 (앱을 닫으면 지워져요)"
    };
    let _ = field(&f, "openAiKey").set_attribute("placeholder", gpt_hint);
}

// ---------- 팀 연결 ----------
pub async fn render_team(info: Option<Value>) {
    let info = match info {
        Some(i) => i,
        None => match call("team:status", Value::Null).await {
            Ok(i) => i,
            Err(_) => return,
        },
    };
    refresh_team_switch(&info);
    let teams = arr(&info, "teams");
    let list = by_id("teamList").unwrap();
    clear(&list);
    for t in &teams {
        let active = b(t, "active");
        let tid = s(t, "teamId");
        let name = s(t, "name");
        let item = el("div")
            .cls(if active { "item agreed" } else { "item" })
            .kid(
                el("div")
                    .cls("main")
                    .kid(el("div").cls("title").text(&format!("{name} · 팀원 {}명", s(t, "memberCount"))))
                    .kid(el("div").cls("sub").text(&format!("참여 코드 {} · 내 이름 {}{}", s(t, "joinCode"), s(t, "displayName"), if s(t, "lastError").is_empty() { "" } else { " · 연결 문제 있음" }))),
            )
            .kid(if active {
                el("span").cls("status-pill sent").text("보는 중")
            } else {
                el("button").text("보기").click(move |_| {
                    let tid = tid.clone();
                    team_action(async move { call("team:switch", json!(tid)).await }, format!("{name}을 보고 있어요."));
                })
            });
        let _ = list.append_child(&item.0);
    }
    st(|x| x.active_team_name = teams.iter().find(|t| b(t, "active")).map(|t| s(t, "name")).unwrap_or_default());
    set_text("teamJoinTitle", if teams.is_empty() { "" } else { "다른 팀 추가하기 (만들기 또는 참여 코드)" });

    let stx = by_id("teamStatus").unwrap();
    clear(&stx);
    let fmt_time = |t: f64| if t > 0.0 { super::fmt_clock(t) } else { "아직 없음".into() };
    let configured = b(&info, "configured");
    let connected = b(&info, "connected");
    let local = b(&info, "localMode");
    if !configured {
        let _ = stx.append_child(&el("div").text("팀 서버가 아직 설정되지 않았어요.").0);
    } else if !connected {
        let _ = stx.append_child(&el("div").text(&format!("Firebase: {} · 팀에 아직 참여하지 않았어요.", s(&info, "projectId"))).0);
    } else {
        let _ = stx.append_child(&el("div").kid(el("b").text(&s(&info["team"], "name"))).txt(&format!(" · 팀원 {}명 · 마지막 동기화 {}", s(&info, "memberCount"), fmt_time(n(&info, "lastSync")))).0);
        let _ = stx.append_child(&el("div").txt("참여 코드 ").kid(el("span").cls("code-big").text(&s(&info["team"], "joinCode"))).txt(" — 팀원에게 알려 주세요").0);
        let _ = stx.append_child(&el("div").cls("hint").text(&if local { "로컬 시험 모드".into() } else { format!("Firebase: {}", s(&info, "projectId")) }).0);
    }
    if !s(&info, "lastError").is_empty() {
        let _ = stx.append_child(&el("div").cls("err").text(&format!("연결 문제: {}", s(&info, "lastError"))).0);
    }
    // 다른 컴퓨터로 옮기기 (Google 계정)
    let acc = &info["account"];
    if acc.is_object() && configured {
        let bx = el("div").cls("account-box").build();
        let line = |e: El| {
            let _ = bx.append_child(&e.0);
        };
        if !b(&info, "googleSignedIn") {
            line(el("div").cls("hint").txt("Google 로 로그인해 두면 다른 컴퓨터에서도 같은 Google 계정으로 로그인만 하면 팀이 그대로 돌아와요. ").kid(el("button").cls("link").text("외부 연결에서 로그인").click(|_| {
                if let Some(x) = by_id("extRoot") {
                    x.scroll_into_view();
                }
            })));
        } else if !b(acc, "ready") {
            line(el("div").cls("hint warn").text("다른 컴퓨터로 팀을 옮기려면 Google 로그인을 한 번 더 해 주세요 (외부 연결 → 로그아웃 → Google 로 로그인). Drive 권한이 새로 필요해요."));
        } else {
            let saved = n(acc, "savedAt");
            line(el("div").cls("hint").text(&if saved > 0.0 { format!("Google 계정에 팀 목록을 맡겨 뒀어요 ({}). 다른 컴퓨터에서 같은 Google 계정으로 로그인하면 팀·맡은 작업이 그대로 돌아와요.", super::fmt_date(saved)) } else { "Google 계정과 연결돼 있어요. 팀이 생기면 목록을 맡겨 둘게요.".into() }));
            if !s(acc, "error").is_empty() {
                line(el("div").cls("err").text(&s(acc, "error")));
            }
            line(el("div").cls("inline").kid(el("button").text("Google 계정에서 팀 불러오기").click(|_| {
                set_text("teamMsg", "Google 계정에서 팀을 찾는 중…");
                spawn(async {
                    match call("account:restore", Value::Null).await {
                        Ok(r) => {
                            set_text("teamMsg", &s(&r, "text"));
                            render_team(Some(r["team"].clone())).await;
                        }
                        Err(e) => set_text("teamMsg", &e),
                    }
                });
            })));
        }
        let _ = stx.append_child(&bx);
    }

    // 앱에 팀 서버 설정이 들어 있으면 설정 칸과 "바꾸기"는 아예 보이지 않는다
    let can_edit = b(&info, "canEditFirebase");
    let cfg = by_id("teamConfig").unwrap();
    let editing = cfg.get_attribute("data-editing").as_deref() == Some("1");
    toggle(&cfg, "hidden", !can_edit || (configured && !editing));
    // 팀이 있어도 다른 팀을 더 만들거나 참여할 수 있다
    toggle(&by_id("teamJoin").unwrap(), "hidden", !configured);
    toggle(&by_id("teamConnected").unwrap(), "hidden", !connected);
    toggle(&by_id("fbEdit").unwrap(), "hidden", !can_edit);

    let admin = &info["admin"];
    let i_am_admin = b(admin, "me");
    let ml = by_id("memberList").unwrap();
    clear(&ml);
    for m in arr(&info, "members") {
        let me = b(&m, "me");
        let name = s(&m, "displayName");
        let uid = s(&m, "uid");
        let item = el("div").cls("item").kid(el("div").cls("main").kid(el("div").cls("title").text(&format!("{name}{}{}", if me { " (나)" } else { "" }, if me && i_am_admin { " · 관리자" } else { "" })))).maybe(if i_am_admin && !me {
            Some(el("button").cls("link").text("내보내기").click(move |_| {
                if !confirm(&format!("\"{name}\"을(를) 팀에서 내보낼까요? 그 기기는 더 이상 이 팀을 볼 수 없고, 다시 들어오려면 참여 코드가 필요해요.")) {
                    return;
                }
                let uid = uid.clone();
                team_action(async move { call("team:remove-member", json!(uid)).await }, "팀에서 내보냈어요.".into());
            }))
        } else {
            None
        });
        let _ = ml.append_child(&item.0);
    }
    render_admin(admin);
}

/// 관리자 칸: 아직 없으면 정하기, 있으면 로그인, 관리자면 로그아웃
fn render_admin(admin: &Value) {
    let Some(bx) = by_id("adminBox") else { return };
    clear(&bx);
    let add = |e: El| {
        let _ = bx.append_child(&e.0);
    };
    if !b(admin, "supported") {
        add(el("div").cls("hint").text("관리자 기능은 팀 서버 규칙을 게시한 뒤에 쓸 수 있어요. (게시 후 \"지금 동기화\")"));
        return;
    }
    if b(admin, "me") {
        add(el("div").txt("관리자로 로그인돼 있어요. 팀원 이름 옆의 \"내보내기\"를 쓸 수 있어요. "));
        add(el("div").cls("inline").kid(el("button").attr("type", "button").cls("link").text("관리자 로그아웃").click(|_| team_action(async { call("team:admin-logout", Value::Null).await }, "관리자에서 로그아웃했어요.".into()))));
        return;
    }
    let exists = b(admin, "exists");
    let id_in = el("input").attr("type", "text").attr("maxlength", "30").attr("placeholder", "관리자 아이디").attr("autocomplete", "off").build();
    let pw_in = el("input").attr("type", "password").attr("maxlength", "100").attr("placeholder", if exists { "관리자 비밀번호" } else { "관리자 비밀번호 (6자 이상)" }).attr("autocomplete", "new-password").build();
    add(el("div").cls("hint").text(if exists {
        "팀원 내보내기는 관리자만 할 수 있어요. 관리자라면 아이디·비밀번호로 로그인해 주세요."
    } else {
        "이 팀에는 아직 관리자가 없어요. 팀을 만든 사람이 관리자 아이디·비밀번호를 정해 주세요. 한 번 정하면 바꿀 수 없고, 비밀번호는 다시 볼 수 없어요."
    }));
    let (i2, p2) = (id_in.clone(), pw_in.clone());
    add(el("div").cls("inline").child(id_in).child(pw_in).kid(el("button").attr("type", "button").text(if exists { "관리자 로그인" } else { "관리자 정하기" }).click(move |_| {
        let args = json!({ "adminId": val(&i2), "password": val(&p2) });
        set_val(&p2, "");
        if exists {
            team_action(async move { call("team:admin-login", args).await }, "관리자로 로그인했어요.".into());
        } else {
            if !confirm("이 아이디·비밀번호로 이 팀의 관리자를 정할까요? 한 번 정하면 바꿀 수 없어요.") {
                return;
            }
            team_action(async move { call("team:admin-setup", args).await }, "관리자를 정했어요. 다른 컴퓨터에서는 이 아이디·비밀번호로 로그인하면 돼요.".into());
        }
    })));
}

fn team_action<F: Future<Output = Result<Value, String>> + 'static>(fut: F, ok: String) {
    set_text("teamMsg", "처리하는 중…");
    spawn(async move {
        match fut.await {
            Ok(info) => {
                let notice = s(&info, "notice");
                set_text("teamMsg", if notice.is_empty() { &ok } else { &notice });
                render_team(Some(info)).await;
                for id in ["teamName", "joinCode", "adminId", "adminPw"] {
                    if let Some(e) = by_id(id) {
                        set_val(&e, "");
                    }
                }
            }
            Err(e) => set_text("teamMsg", &e),
        }
    });
}

fn v(id: &str) -> String {
    by_id(id).map(|e| val(&e)).unwrap_or_default()
}

// ---------- 버전 · 업데이트 ----------
fn update_text(status: &str) -> &'static str {
    match status {
        "idle" => "아직 확인하지 않았어요.",
        "checking" => "확인하는 중이에요…",
        "downloading" => "새 버전을 받는 중이에요",
        "ready" => "새 버전이 준비됐어요.",
        "latest" => "최신 버전이에요.",
        "error" => "확인하지 못했어요",
        _ => "",
    }
}

pub async fn render_update(stv: Option<Value>) {
    let stv = match stv {
        Some(x) => x,
        None => match call("update:status", Value::Null).await {
            Ok(x) => x,
            Err(_) => return,
        },
    };
    draw_update(&stv);
}

fn draw_update(stv: &Value) {
    let bx = by_id("updateInfo").unwrap();
    clear(&bx);
    let ver = if s(stv, "version").is_empty() { s(stv, "current") } else { s(stv, "version") };
    let build = s(stv, "build");
    let _ = bx.append_child(&el("div").txt("지금 버전 ").kid(el("b").text(&ver)).txt(&if build.is_empty() { String::new() } else { format!(" ({build})") }).0);
    if build == "x64" && b(stv, "armPc") {
        let _ = bx.append_child(&el("div").cls("hint").text("ARM 컴퓨터에서 x64 판을 쓰고 있어요. 다음 업데이트 때 더 빠른 ARM64 판으로 바뀌어요.").0);
    }
    let status = s(stv, "status");
    let mut line = update_text(&status).to_string();
    match status.as_str() {
        "downloading" => line += &format!(" ({}%) — {}", n(stv, "progress") as i64, s(stv, "latest")),
        "ready" => line = format!("새 버전 {}이 준비됐어요.", s(stv, "latest")),
        "error" | "unsupported" => line = format!("{}{}", if line.is_empty() { String::new() } else { format!("{line}: ") }, s(stv, "error")),
        _ => {}
    }
    if !line.is_empty() {
        let _ = bx.append_child(&el("div").cls(if status == "error" { "err" } else { "hint" }).text(&line).0);
    }
    toggle(&by_id("updateInstall").unwrap(), "hidden", status != "ready");
}

thread_local! {
    static UPDATE_POLL: Cell<i32> = Cell::new(0);
}

// 확인·내려받기는 앱이 뒤에서 하고, 화면은 0.5초마다 상태를 읽어 진행률을 보여 준다
fn poll_update() {
    UPDATE_POLL.with(|p| stop(p.get()));
    let inflight = std::rc::Rc::new(Cell::new(false));
    let id = every(500, move || {
        if inflight.get() {
            return;
        }
        inflight.set(true);
        let f = inflight.clone();
        spawn(async move {
            match call("update:status", Value::Null).await {
                Ok(stv) => {
                    draw_update(&stv);
                    if !["checking", "downloading"].contains(&s(&stv, "status").as_str()) {
                        UPDATE_POLL.with(|p| stop(p.get()));
                    }
                }
                Err(_) => UPDATE_POLL.with(|p| stop(p.get())),
            }
            f.set(false);
        });
    });
    UPDATE_POLL.with(|p| p.set(id));
}

// ---------- 팀에 Claude 키 공유 ----------
fn key_action<F: Future<Output = Result<Value, String>> + 'static>(fut: F, ok: &'static str) {
    set_text("teamKeyStatus", "처리하는 중…");
    spawn(async move {
        match fut.await {
            Ok(ns) => {
                st(|x| x.settings = ns.clone());
                update_engine(&ns);
                set_text("teamKeyStatus", ok);
            }
            Err(e) => set_text("teamKeyStatus", &e),
        }
    });
}

pub fn render_team_key(sx: &Value) {
    let Some(bx) = by_id("teamKey") else { return };
    clear(&bx);
    if !b(sx, "teamConnected") {
        let _ = bx.append_child(&el("p").cls("hint").text("팀에 연결하면 내 Claude 키를 팀원과 함께 쓸 수 있어요.").0);
        return;
    }
    let ts = &sx["teamShare"];
    let team_name = if s(sx, "teamName").is_empty() { "보고 있는 팀".into() } else { s(sx, "teamName") };
    let _ = bx.append_child(&el("strong").text(&format!("Claude 키 팀 공유 ({team_name})")).0);
    let has_own = b(sx, "hasOwnKey");
    if b(ts, "shared") && b(ts, "mine") {
        let _ = bx.append_child(&el("p").cls("hint").text("내 키를 이 팀에 공유하고 있어요. 키가 없는 팀원도 AI 기능을 쓰고, 요금은 내 계정에 나가요.").0);
        let _ = bx.append_child(&el("div").cls("inline").kid(el("button").attr("type", "button").cls("danger").text("공유 끄기").click(|_| key_action(async { call("ai:unshare-team", Value::Null).await }, "공유를 껐어요. 팀원 앱에서는 다음 동기화 때 사라져요."))).0);
    } else if b(ts, "shared") {
        let _ = bx.append_child(&el("p").cls("hint").text(&format!("{}님이 Claude 키를 팀에 공유했어요. {}", s(ts, "byName"), if has_own { "내 키가 있으니 내 키를 먼저 써요." } else { "내 키가 없어도 AI 기능을 쓸 수 있어요." })).0);
    } else {
        let _ = bx.append_child(&el("p").cls("hint warn").text("공유하면 키가 없는 팀원도 AI 기능을 쓸 수 있어요. 대신 팀원 사용량도 모두 내 계정으로 청구되고, 팀원은 누구든 키를 꺼내 볼 수 있어요. Console 에서 사용 한도를 정해 두고, 참여 코드는 팀원에게만 알려 주세요.").0);
        let _ = bx.append_child(
            &el("div")
                .cls("inline")
                .kid(el("button").attr("type", "button").text("이 팀에 내 Claude 키 공유하기").flag("disabled", !has_own).click(|_| {
                    if confirm("팀원 사용량이 내 계정으로 청구되고, 팀원은 키를 볼 수 있어요. 공유할까요?") {
                        key_action(async { call("ai:share-team", Value::Null).await }, "공유했어요. 팀원 앱은 다음 동기화(최대 5분) 때 키를 받아요.");
                    }
                }))
                .0,
        );
        if !has_own {
            let _ = bx.append_child(&el("p").cls("hint").text("먼저 위에 내 API 키를 저장해 주세요.").0);
        }
    }
    let _ = bx.append_child(&el("span").id("teamKeyStatus").cls("status").0);
}

async fn save_key_if_typed() -> Result<(), String> {
    let k = field(&form(), "apiKey");
    let key = val(&k).trim().to_string();
    if !key.is_empty() {
        call("ai:set-key", json!(key)).await?;
        if val(&k).trim() == key {
            set_val(&k, "");
        }
    }
    Ok(())
}

async fn save_openai_if_changed() -> Result<(), String> {
    let f = form();
    let input = field(&f, "openAiKey");
    let key = val(&input).trim().to_owned();
    let model = val(&field(&f, "openAiModel")).trim().to_owned();
    let previous_model = st(|state| s(&state.settings, "openAiModel"));
    if key.is_empty() && model == previous_model {
        return Ok(());
    }
    let mut args = json!({ "model": model });
    if !key.is_empty() {
        args["key"] = json!(key);
    }
    let sx = call("ai:set-openai", args).await?;
    if val(&input).trim() == key {
        set_val(&input, "");
    }
    let model_input = field(&f, "openAiModel");
    if val(&model_input).trim() == model {
        set_val(&model_input, &s(&sx, "openAiModel"));
    }
    st(|state| state.settings = sx.clone());
    update_engine(&sx);
    key_placeholder(&sx);
    Ok(())
}

fn click(id: &str, f: impl FnMut(web_sys::Event) + 'static) {
    if let Some(e) = by_id(id) {
        on(&e, "click", f);
    }
}

pub fn bind() {
    click("fbSave", |_| {
        let cfg = v("fbConfig");
        team_action(
            async move {
                let info = call("team:configure", json!(cfg)).await?;
                let _ = by_id("teamConfig").unwrap().set_attribute("data-editing", "0");
                set_val(&by_id("fbConfig").unwrap(), "");
                Ok(info)
            },
            "Firebase 설정을 저장했어요.".into(),
        )
    });
    click("fbEdit", |_| {
        let c = by_id("teamConfig").unwrap();
        let _ = c.set_attribute("data-editing", "1");
        toggle(&c, "hidden", false);
    });
    click("teamCreate", |_| {
        let args = json!({ "name": v("teamName"), "displayName": v("displayName"), "adminId": v("adminId"), "adminPassword": v("adminPw") });
        team_action(async move { call("team:create", args).await }, "팀을 만들었어요. 참여 코드를 팀원에게 알려 주세요. 관리자 아이디·비밀번호는 기억해 두세요.".into())
    });
    click("teamJoinBtn", |_| {
        let args = json!({ "code": v("joinCode"), "displayName": v("displayName") });
        team_action(async move { call("team:join", args).await }, "팀에 참여했어요.".into())
    });
    click("teamSyncBtn", |_| team_action(async { call("team:sync", Value::Null).await }, "동기화했어요.".into()));
    click("teamLeave", |_| {
        let name = st(|x| x.active_team_name.clone());
        let name = if name.is_empty() { "이 팀".into() } else { name };
        if !confirm(&format!("\"{name}\"에서 나갈까요? 이 팀으로 보낸 기록이 지워지고, 다시 참여하려면 참여 코드가 필요해요. 다른 팀은 그대로예요.")) {
            return;
        }
        team_action(async { call("team:leave", Value::Null).await }, "팀에서 나왔어요.".into());
    });

    click("updateCheck", |_| {
        set_text("updateInfo", "확인하는 중이에요…");
        spawn(async {
            match call("update:check", Value::Null).await {
                Ok(stv) => {
                    draw_update(&stv);
                    poll_update();
                }
                Err(e) => set_text("updateInfo", &e),
            }
        });
    });
    click("updateInstall", |_| {
        spawn(async {
            if let Err(e) = call("update:install", Value::Null).await {
                set_text("updateInfo", &e);
            }
        })
    });

    let f = form();
    on(&f, "submit", |e| {
        e.prevent_default();
        spawn(async {
            if let Err(e) = save_key_if_typed().await {
                set_text("saveStatus", &e);
                return;
            }
            if let Err(e) = save_openai_if_changed().await {
                set_text("saveStatus", &e);
                return;
            }
            let f = form();
            let dnd = if checked(&field(&f, "dndOn")) { json!({ "start": val(&field(&f, "dndStart")), "end": val(&field(&f, "dndEnd")) }) } else { Value::Null };
            let model = val(&field(&f, "model")).trim().to_string();
            let args = json!({
                "frequency": val(&field(&f, "frequency")),
                "dnd": dnd,
                "retentionDays": val(&field(&f, "retentionDays")).parse::<i64>().unwrap_or(30),
                "ai": { "enabled": checked(&field(&f, "aiEnabled")), "model": if model.is_empty() { "claude-sonnet-5-5".to_string() } else { model } },
            });
            match call("settings:set", args).await {
                Ok(sx) => {
                    st(|x| x.settings = sx.clone());
                    update_engine(&sx);
                    key_placeholder(&sx);
                    set_text("saveStatus", "저장했어요.");
                    after(2000, || set_text("saveStatus", ""));
                }
                Err(e) => set_text("saveStatus", &e),
            }
        });
    });
    click("testAi", |_| {
        spawn(async {
            if let Err(e) = save_key_if_typed().await {
                set_text("aiStatus", &e);
                return;
            }
            set_text("aiStatus", "확인하는 중…");
            let t = match call("ai:test", Value::Null).await {
                Ok(r) if b(&r, "ok") => "연결됐어요.".to_string(),
                Ok(r) => format!("연결하지 못했어요: {}", s(&r, "error")),
                Err(e) => format!("연결하지 못했어요: {e}"),
            };
            set_text("aiStatus", &t);
            // 키를 새로 넣었으면 머리말도 바꾼다
            if let Ok(sx) = call("settings:get", Value::Null).await {
                update_engine(&sx);
                key_placeholder(&sx);
            }
        });
    });
    click("testOpenAi", |_| {
        let button = by_id("testOpenAi").unwrap();
        set_disabled(&button, true);
        set_text("openAiStatus", "확인하는 중…");
        spawn(async move {
            let result = match save_openai_if_changed().await {
                Ok(()) => call("ai:test-openai", Value::Null).await,
                Err(e) => Err(e),
            };
            let msg = match result {
                Ok(r) if b(&r, "ok") => "GPT에 연결됐어요.".to_owned(),
                Ok(r) => format!("GPT 연결 실패 · {}", s(&r, "error")),
                Err(e) => format!("GPT 연결 실패 · {e}"),
            };
            set_text("openAiStatus", &msg);
            set_disabled(&button, false);
        });
    });
    click("deleteAll", |_| {
        if !confirm("모든 대화 기록을 지울까요? 공유 대기함은 그대로 남아요.") {
            return;
        }
        spawn(async {
            let _ = call("chat:delete-all", Value::Null).await;
            set_text("saveStatus", "대화 기록을 모두 지웠어요.");
        });
    });
}
