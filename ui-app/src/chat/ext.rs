//! 설정 → 외부 연결: Telegram 알림, Google 캘린더·할 일. 각 연결의 실제 상태와 마지막 결과를 그대로 보여 준다.
use super::{b, fmt_date, n, s};
use crate::api::call;
use crate::dom::*;
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::future::Future;
use web_sys::HtmlElement;

thread_local! {
    static VIEW: RefCell<Option<Value>> = RefCell::new(None);
    static LINK_TIMER: Cell<i32> = Cell::new(0);
}

fn when(t: f64) -> String {
    if t > 0.0 {
        fmt_date(t)
    } else {
        "아직 없음".into()
    }
}

fn set_msg(t: &str, bad: bool) {
    if let Some(m) = by_id("extMsg") {
        m.set_text_content(Some(t));
        toggle(&m, "bad", bad);
    }
}

fn act<F: Future<Output = Result<Value, String>> + 'static>(fut: F, ok: &'static str) {
    set_msg("처리하는 중…", false);
    spawn(async move {
        match fut.await {
            Ok(r) => {
                if r.get("teamConnected").is_some() {
                    VIEW.with(|v| *v.borrow_mut() = Some(r.clone()));
                } else if r["view"].is_object() {
                    VIEW.with(|v| *v.borrow_mut() = Some(r["view"].clone()));
                }
                draw();
                // 로그인처럼 결과 문장을 돌려주는 요청은 그 문장을 보여 준다
                let text = s(&r, "text");
                set_msg(if text.is_empty() { ok } else { &text }, false);
                if !text.is_empty() {
                    spawn(super::settings::render_team(None));
                }
            }
            Err(e) => {
                draw();
                set_msg(&e, true);
            }
        }
    });
}

fn c(ch: &'static str, args: Value) -> impl Future<Output = Result<Value, String>> {
    async move { call(ch, args).await }
}

pub async fn render() {
    let v = call("ext:status", Value::Null).await.ok();
    VIEW.with(|x| *x.borrow_mut() = v);
    draw();
}

fn state_line(ok: bool, text: &str) -> El {
    el("div").cls(if ok { "ext-state on" } else { "ext-state off" }).kid(el("span").cls("dot")).txt(text)
}

fn telegram_box(view: &Value) -> HtmlElement {
    let t = &view["telegram"];
    let bx = el("div").cls("ext-item").kid(el("div").cls("ext-title").text("Telegram 알림")).kid(el("p").cls("hint").text("배정 제안, 도움 요청, 팀 약속 제안, 그리고 자리를 비웠을 때의 내 진행 확인만 보내요. 개인 대화나 고민 내용은 보내지 않아요.")).build();
    let add = |e: El| {
        let _ = bx.append_child(&e.0);
    };
    if !b(view, "teamConnected") {
        add(state_line(false, "먼저 위에서 팀에 연결해 주세요."));
        return bx;
    }
    if !t["bot"].is_object() {
        let input = el("input").attr("type", "password").attr("placeholder", "123456789:AA…").attr("autocomplete", "off").build();
        let i2 = input.clone();
        add(state_line(false, "팀 봇이 아직 없어요. 팀에서 한 명만 넣으면 돼요."));
        add(el("p").cls("hint").text("Telegram 의 @BotFather 에서 /newbot 으로 만든 봇의 토큰을 넣어 주세요. 토큰은 팀원만 볼 수 있게 저장돼요."));
        add(el("div").cls("inline").child(input).kid(el("button").text("봇 저장").click(move |_| act(c("tg:set-bot", json!(val(&i2))), "봇을 저장했어요."))));
        return bx;
    }
    let linked = b(t, "linked");
    let user = s(&t["bot"], "username");
    add(state_line(linked, &if linked { format!("@{user} 와 연결됨 · 팀에서 {}명 연결", s(t, "linkedCount")) } else { format!("팀 봇 @{user} · 아직 내 Telegram 과 연결 안 됨") }));
    if !linked {
        let pending = b(t, "pending");
        add(el("div").cls("inline").kid(el("button").cls("primary").text(if pending { "Telegram 다시 열기" } else { "Telegram 연결하기" }).click(|_| start_link())));
        if pending {
            add(el("p").cls("hint").text("Telegram 에서 봇 대화가 열리면 아래쪽 \"시작(Start)\"을 눌러 주세요. 눌렀다면 자동으로 연결돼요."));
        }
    } else {
        let cb1 = el("input").attr("type", "checkbox").flag("checked", b(t, "teamAlerts")).build();
        let c1 = cb1.clone();
        on(&cb1, "change", move |_| act(c("tg:team-alerts", json!(checked(&c1))), "저장했어요."));
        let cb2 = el("input").attr("type", "checkbox").flag("checked", b(view, "telegramAway")).build();
        let c2 = cb2.clone();
        on(&cb2, "change", move |_| act(c("tg:away", json!(checked(&c2))), "저장했어요."));
        add(el("label").cls("check").child(cb1).txt(" 팀 알림 받기 (배정·도움 요청·팀 약속)"));
        add(el("label").cls("check").child(cb2).txt(" 자리를 비웠을 때(10분 이상 입력 없음) 진행 확인을 Telegram 으로"));
        add(el("div")
            .cls("inline")
            .kid(el("button").text("시험 메시지 보내기").click(|_| act(c("tg:test", Value::Null), "보냈어요. Telegram 을 확인해 주세요.")))
            .kid(el("button").cls("link").text("연결 끊기").click(|_| act(c("tg:unlink", Value::Null), "연결을 끊었어요."))));
    }
    let lr = &t["lastResult"];
    if lr.is_object() {
        add(el("div").cls(if b(lr, "ok") { "hint" } else { "hint err" }).text(&format!("마지막 결과 ({}): {}", when(n(lr, "at")), s(lr, "text"))));
    }
    bx
}

fn start_link() {
    act(c("tg:start-link", Value::Null), "Telegram 이 열렸어요. 봇 대화에서 \"시작\"을 눌러 주세요.");
    LINK_TIMER.with(|t| stop(t.get()));
    let tries = std::rc::Rc::new(Cell::new(0));
    let id = every(4000, move || {
        tries.set(tries.get() + 1);
        let n = tries.get();
        spawn(async move {
            match call("tg:check-link", Value::Null).await {
                Ok(r) if b(&r, "linked") => {
                    LINK_TIMER.with(|t| stop(t.get()));
                    VIEW.with(|v| *v.borrow_mut() = Some(r["view"].clone()));
                    draw();
                    set_msg("연결됐어요. Telegram 에 확인 메시지를 보냈어요.", false);
                }
                Ok(_) => {}
                Err(e) => {
                    LINK_TIMER.with(|t| stop(t.get()));
                    set_msg(&e, true);
                }
            }
        });
        if n > 60 {
            LINK_TIMER.with(|t| stop(t.get()));
        }
    });
    LINK_TIMER.with(|t| t.set(id));
}

fn google_box(view: &Value) -> HtmlElement {
    let g = &view["google"];
    let bx = el("div").cls("ext-item").kid(el("div").cls("ext-title").text("Google 캘린더 · 할 일")).kid(el("p").cls("hint").text("캘린더는 바쁜 시간만 읽어요 (일정 제목·내용은 못 봐요). 계산한 빈 시간은 이 컴퓨터에서만 쓰고, 팀에는 내가 저장한 \"주간 가능 시간\"만 보여요. 할 일에는 내가 수락한 작업만 올라가요. Drive 는 사이 전용 숨김 폴더(팀 목록 맡기기)와 사이로 올린 파일만 다뤄요 — 내 Drive 의 다른 파일은 못 봐요.")).build();
    let add = |e: El| {
        let _ = bx.append_child(&e.0);
    };
    if !b(g, "configured") {
        let ta = el("textarea").attr("rows", "3").attr("placeholder", "클라이언트 ID(…apps.googleusercontent.com)와 보안 비밀번호(GOCSPX-…), 또는 내려받은 JSON").build();
        let t2 = ta.clone();
        add(state_line(false, "Google 연결 정보가 아직 없어요."));
        let _ = bx.append_child(&ta);
        add(el("div").cls("inline").kid(el("button").text("연결 정보 저장").click(move |_| act(c("g:configure", json!(val(&t2))), "저장했어요. 이제 로그인해 주세요."))));
        return bx;
    }
    if !b(g, "signedIn") {
        add(state_line(false, "로그인 안 됨"));
        add(el("div").cls("inline").kid(el("button").cls("primary").text("Google 로 로그인").click(|_| act(c("g:login", Value::Null), "Google 과 연결됐어요."))));
        add(el("p").cls("hint").text("브라우저에서 Google 로그인 화면이 열려요. 로그인 후 권한을 허용하면 자동으로 돌아와요. 테스트 중인 앱이라 약 7일마다 다시 로그인해야 해요."));
    } else {
        let email = s(g, "email");
        add(state_line(true, &format!("{} 연결됨", if email.is_empty() { "Google 계정".into() } else { email })));
        if !b(g, "drive") {
            add(el("div").cls("hint warn").text("새 기능(다른 컴퓨터로 팀 옮기기, 팀 자료 Drive 올리기)을 쓰려면 아래 \"로그아웃\" 후 다시 로그인해 주세요. Drive 권한이 새로 필요해요."));
        }
        let lr = s(g, "lastResult");
        add(el("div").cls("hint").text(&format!("마지막 할 일 동기화: {}{}", when(n(g, "lastSync")), if lr.is_empty() { String::new() } else { format!(" — {lr}") })));
        add(el("div")
            .cls("inline")
            .kid(el("button").text("할 일 지금 동기화").click(|_| act(c("g:sync", Value::Null), "동기화했어요.")))
            .kid(el("button").cls("link").text("로그아웃").click(|_| act(c("g:logout", Value::Null), "로그아웃했어요."))));
    }
    if !s(g, "lastError").is_empty() {
        add(el("div").cls("hint err").text(&format!("문제: {}", s(g, "lastError"))));
    }
    bx
}

fn draw() {
    let Some(root) = by_id("extRoot") else { return };
    clear(&root);
    let Some(view) = VIEW.with(|v| v.borrow().clone()) else {
        let _ = root.append_child(&el("p").cls("hint").text("외부 연결 상태를 불러오지 못했어요.").0);
        return;
    };
    let _ = root.append_child(&telegram_box(&view));
    let _ = root.append_child(&google_box(&view));
    let _ = root.append_child(&el("div").id("extMsg").cls("status").0);
}
