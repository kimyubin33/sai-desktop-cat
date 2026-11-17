use super::{b, s, show_tab};
use crate::{api::call, dom::*};
use serde_json::Value;
use std::cell::Cell;
thread_local! {
    static BUSY:Cell<bool>=const{Cell::new(false)};
    static LOGIN:Cell<bool>=const{Cell::new(false)};
    static RESUMABLE:Cell<bool>=const{Cell::new(false)};
    static UPGRADE:Cell<bool>=const{Cell::new(false)};
    static EPOCH:Cell<u64>=const{Cell::new(0)};
    static LOAD:Cell<u64>=const{Cell::new(0)};
    static RESUME_FAILED:Cell<bool>=const{Cell::new(false)};
}
fn draw(value: &Value) {
    if !super::profile::draw(&value["profile"]) {
        return;
    }
    super::teams::account(&value["profile"]);
    let g = &value["google"];
    let signed = b(g, "signedIn");
    if signed && RESUME_FAILED.with(|f| f.replace(false)) {
        set_text(
            "googleMessage",
            "저장된 Google 연결을 확인했어요. 다시 로그인하지 않고 이어 쓸 수 있어요.",
        );
    }
    let upgrade = signed && !b(g, "driveFull");
    UPGRADE.with(|v| v.set(upgrade));
    EPOCH.with(|v| v.set(value["profile"]["epoch"].as_u64().unwrap_or(0)));
    let configured = b(g, "configured");
    let pending = b(g, "loginPending");
    let busy = BUSY.with(Cell::get) || pending;
    let checking = busy && !LOGIN.with(Cell::get) && !pending;
    let resumable = b(g, "hasSession") && !signed;
    RESUMABLE.with(|r| r.set(resumable));
    let email = s(g, "email");
    set_text(
        "accountLabel",
        if signed && !email.is_empty() {
            &email
        } else {
            "Google 계정 연결"
        },
    );
    set_text(
        "googleAccountText",
        if signed {
            if email.is_empty() {
                "Google 계정을 확인했어요."
            } else {
                &email
            }
        } else if checking {
            "Google 계정을 확인하고 있어요."
        } else if b(g, "needsReauth") {
            "Google로 다시 로그인해 연결을 확인해 주세요."
        } else if resumable {
            "이 기기의 Google 연결을 확인할 수 있어요."
        } else if configured {
            "Google 계정으로 연결할 수 있어요."
        } else {
            "Google 로그인을 준비하고 있어요."
        },
    );
    let label = if checking {
        "계정 확인 중…"
    } else if busy {
        "브라우저에서 로그인 중…"
    } else if upgrade {
        "Google 연결 한 번 갱신"
    } else if signed {
        "Google 연결됨"
    } else if resumable {
        "연결 다시 확인"
    } else if b(g, "needsReauth") {
        "Google로 다시 로그인"
    } else if configured {
        "Google로 시작하기"
    } else {
        "Google 로그인 준비 중"
    };
    for id in ["googleConnect", "settingsGoogleConnect"] {
        if let Some(button) = by_id(id) {
            if id == "googleConnect" {
                if let Some(span) = q_in(&button, "span") {
                    span.set_text_content(Some(label));
                }
            } else {
                button.set_text_content(Some(label));
            }
            set_disabled(&button, busy || signed && !upgrade || !configured);
        }
    }
    set_text(
        "accountHint",
        if upgrade {
            "Drive 전체 접근을 처음 한 번 허용해 주세요. 이후 파일 선택·전송·팀 연결은 저장된 로그인으로 이용해요."
        } else if signed {
            "Google 연결을 저장했어요. Drive 탐색과 파일 전송은 앱 안에서 바로 이용할 수 있어요."
        } else if configured {
            "첫 로그인에서 계정·앱 설정·Drive 전체 접근을 함께 연결해요. 캘린더 권한은 아직 요청하지 않아요."
        } else {
            "지금은 개인 AI를 사용할 수 있어요. Google 연결 정보가 포함된 앱을 준비하고 있어요."
        },
    );
    if let Some(button) = by_id("googleLogout") {
        toggle(&button, "hidden", !b(g, "hasCredentials"));
        set_disabled(&button, busy);
    }
    if let Some(button) = by_id("googleCancel") {
        toggle(
            &button,
            "hidden",
            !(pending || busy && LOGIN.with(Cell::get)),
        );
        set_disabled(&button, false);
    }
}
async fn perform(channel: &'static str) {
    RESUME_FAILED.with(|f| f.set(false));
    let request = LOAD.with(|n| {
        n.set(n.get() + 1);
        n.get()
    });
    LOGIN.with(|login| login.set(channel == "g:login" || channel == "g:upgrade"));
    for button in q_all("[data-google], #googleLogout") {
        set_disabled(&button, true);
    }
    let login = channel == "g:login" || channel == "g:upgrade";
    toggle(&by_id("googleCancel").unwrap(), "hidden", !login);
    set_text(
        "googleMessage",
        match channel {
            "g:login" => {
                "브라우저에서 Google 로그인을 완료해 주세요. 기다리지 않고 취소할 수도 있어요."
            }
            "g:upgrade"=>"브라우저에서 한 번만 Drive 전체 접근을 허용해 주세요. 이후에는 저장된 연결을 사용해요.",
            "g:resume" => "이 기기의 Google 계정을 확인하고 있어요.",
            _ => "이 기기에서 로그아웃하고 있어요.",
        },
    );
    let result = call(
        channel,
        if channel == "g:upgrade" {
            serde_json::json!({"epoch":EPOCH.with(Cell::get)})
        } else {
            Value::Null
        },
    )
    .await;
    if request != LOAD.with(Cell::get) {
        return;
    }
    BUSY.with(|busy| busy.set(false));
    LOGIN.with(|login| login.set(false));
    toggle(&by_id("googleCancel").unwrap(), "hidden", true);
    match result {
        Ok(v) => {
            draw(&v);
            set_text(
                "googleMessage",
                if channel == "g:logout" {
                    "이 기기에서 로그아웃했어요. 다른 PC의 연결은 유지돼요."
                } else {
                    "계정을 확인했어요. 아래에서 이름·설정 동기화 상태를 확인할 수 있어요."
                },
            );
            super::settings::load().await;
            super::teams::load().await;
        }
        Err(e) => {
            let status = call("workspace:status", Value::Null).await;
            if request != LOAD.with(Cell::get) {
                return;
            }
            let mut verified = false;
            if let Ok(v) = status {
                verified = b(&v["google"], "signedIn");
                draw(&v);
            }
            if channel == "g:resume" && verified {
                set_text(
                    "googleMessage",
                    "저장된 Google 연결을 확인했어요. 다시 로그인하지 않고 이어 쓸 수 있어요.",
                );
            } else {
                RESUME_FAILED.with(|f| f.set(channel == "g:resume"));
                set_text("googleMessage", &e);
            }
        }
    }
}
pub async fn load() {
    if BUSY.with(Cell::get) {
        return;
    }
    let request = LOAD.with(|n| {
        n.set(n.get() + 1);
        n.get()
    });
    let result = call("workspace:status", Value::Null).await;
    if request != LOAD.with(Cell::get) || BUSY.with(Cell::get) {
        return;
    }
    match result {
        Ok(v) => {
            draw(&v);
            if b(&v["google"], "hasSession")
                && !b(&v["google"], "signedIn")
                && !b(&v["google"], "loginPending")
            {
                if !BUSY.with(|busy| busy.replace(true)) {
                    perform("g:resume").await;
                }
            } else if b(&v["google"], "signedIn") {
                super::teams::load().await;
            }
        }
        Err(e) => {
            set_text("googleMessage", &e);
            set_text(
                "accountHint",
                "앱 연결을 확인하지 못했어요. 앱을 다시 열어 주세요.",
            );
            for button in q_all("[data-google]") {
                set_disabled(&button, true);
            }
        }
    }
}
fn action(channel: &'static str) {
    if BUSY.with(|busy| busy.replace(true)) {
        return;
    }
    spawn(perform(channel));
}
pub fn bind() {
    for button in q_all("[data-google]") {
        on(&button, "click", |_| {
            show_tab("settings");
            action(if RESUMABLE.with(Cell::get) {
                "g:resume"
            } else if UPGRADE.with(Cell::get) {
                "g:upgrade"
            } else {
                "g:login"
            });
        });
    }
    on(&by_id("googleCancel").unwrap(), "click", |_| {
        set_disabled(&by_id("googleCancel").unwrap(), true);
        spawn(async {
            match call("g:cancel-login", Value::Null).await {
                Ok(v) => {
                    BUSY.with(|busy| busy.set(false));
                    LOGIN.with(|login| login.set(false));
                    draw(&v);
                    set_text(
                        "googleMessage",
                        "로그인 창은 닫아도 돼요. 현재 연결 상태를 확인해 주세요.",
                    );
                }
                Err(e) => set_text("googleMessage", &e),
            }
        });
    });
    on(&by_id("googleLogout").unwrap(), "click", |_| {
        if confirm("이 기기에서만 로그아웃할까요? 다른 PC의 연결과 Drive 자료는 유지돼요.")
        {
            action("g:logout");
        }
    });
    for button in q_all("[data-open-drive]") {
        on(&button, "click", |_| {
            spawn(async {
                if let Err(e) = call("app:open-drive", Value::Null).await {
                    set_text("accountHint", &e);
                }
            })
        });
    }
}
