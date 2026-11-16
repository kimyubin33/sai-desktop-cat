use super::{click, s, v};
use crate::{api::call, dom::*};
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;

thread_local! {
    static NET: RefCell<Value> = const { RefCell::new(Value::Null) };
    static NET_BUSY: Cell<u32> = const { Cell::new(0) };
    static NET_GEN: Cell<u64> = const { Cell::new(0) };
    static POLLING: Cell<bool> = const { Cell::new(false) };
    static LOG_BUSY: Cell<bool> = const { Cell::new(false) };
    static LOG_WRITE: Cell<bool> = const { Cell::new(false) };
    static LOG_GEN: Cell<u64> = const { Cell::new(0) };
    static AUTO_LOG_LOADING: Cell<bool> = const { Cell::new(false) };
    static EDITING_TEAM: RefCell<String> = const { RefCell::new(String::new()) };
    // 저장 오류 후 같은 내용을 재시도하면 같은 요청 ID를 사용한다.
    static DRAFTS: RefCell<BTreeMap<String,(String,Option<String>)>> = const { RefCell::new(BTreeMap::new()) };
}

fn team() -> String {
    super::identity::team_id()
}
fn session() -> String {
    NET.with(|n| s(&n.borrow(), "session"))
}

fn controls() {
    let busy = NET_BUSY.with(|n| n.get() > 0);
    let running = NET.with(|n| n.borrow()["running"] == true);
    let consent = by_id("meshSessionConsent").is_some_and(|e| checked(&e));
    for (id, disabled) in [
        (
            "meshSessionStart",
            busy || running || !consent || team().is_empty() || v("meshSessionAddress").is_empty(),
        ),
        ("meshSessionStop", false),
        ("meshSessionAdd", busy || !running),
        ("meshSessionAddress", busy || running),
        ("meshSessionConsent", busy || running),
        ("meshExchangeConsent", busy || running),
        (
            "meshRestoreConsent",
            busy || running || !by_id("meshAutomaticConsent").is_some_and(|e| checked(&e)),
        ),
        (
            "meshCourierConsent",
            busy || running || !by_id("meshAutomaticConsent").is_some_and(|e| checked(&e)),
        ),
        (
            "meshAutomaticConsent",
            busy || running || !by_id("meshExchangeConsent").is_some_and(|e| checked(&e)),
        ),
        (
            "meshLocalSave",
            LOG_BUSY.with(Cell::get) || !LOG_WRITE.with(Cell::get),
        ),
    ] {
        if let Some(e) = by_id(id) {
            set_disabled(&e, disabled);
        }
    }
    if let Some(list) = by_id("meshSessionPeers") {
        for b in q_all_in(&list, "button") {
            let exchange = b.get_attribute("data-exchange").is_some();
            set_disabled(
                &b,
                busy || !running
                    || (exchange && !NET.with(|n| n.borrow()["exchangeEnabled"] == true)),
            );
        }
    }
}

fn render_net(info: Value) {
    let changed = session() != s(&info, "session");
    NET.with(|n| n.replace(info.clone()));
    let life = &info["lifecycle"];
    let phase = match s(life, "phase").as_str() {
        "running" => "연결 실행 중",
        "suspended" => "절전으로 연결 중지",
        "waiting-network" => "동의한 Windows 네트워크 대기",
        "expired" => "이번 실행 시간 만료",
        "needs-consent" => "다시 동의 필요",
        "exited" => "앱 종료 중",
        _ => "연결 중지",
    };
    set_text("meshLifecycleStatus",&format!("{phase} · 이번 실행 복구 {} · 다음 앱 시작 자동 연결 {} · PC 팀 {} · 남은 시간 약 {}초{}",
        if life["restore"]==true {"허용"} else {"현재 실행에서 꺼짐"},
        if life["startupEnabled"]==true {"켜짐"} else {"꺼짐 또는 다시 동의 필요"},
        s(life,"teamId").chars().take(12).collect::<String>(),s(life,"remainingSeconds"),
        if s(life,"notice").is_empty(){String::new()}else{format!(" · {}",s(life,"notice"))}));
    if let Some(card) = by_id("meshSessionCard") {
        set_val(&card, &s(&info, "card"));
    }
    if changed {
        if let Some(input) = by_id("meshSessionInput") {
            set_val(&input, "");
        }
    }
    set_text(
        "meshSessionStatus",
        &if info["running"] == true {
            format!("승인 장치 연결 중 · {} · PC 팀 {} · {}까지. 선택한 팀을 바꿔도 연결의 팀은 유지돼요.",if info["exchangeEnabled"]==true { "암호화된 준비 기록 교환 허용" } else { "연결 확인만 허용" },s(&info,"teamId").chars().take(12).collect::<String>(),super::super::fmt_clock(info["expiresAt"].as_f64().unwrap_or(0.0)))
        } else {
            info["notice"]
                .as_str()
                .unwrap_or(
                    "연결 확인이 꺼져 있어요. 최신 승인 정보가 같은 장치끼리만 확인할 수 있어요.",
                )
                .to_string()
        },
    );
    let auto = &info["automatic"];
    let host = &info["host"];
    let host_text = if host["enabled"] == true {
        let role = match s(host, "phase").as_str() {
            "alone" => "현재 확인한 후보는 이 PC뿐이에요.".to_string(),
            "representative" => "이 PC가 현재 연결 범위의 대표 중계를 맡아요.".to_string(),
            "peer" => format!(
                "이 PC에서 확인한 대표는 {}예요. 응답 확인 유효 시간은 약 {}초 남았어요.",
                s(host, "deviceId").chars().take(12).collect::<String>(),
                s(host, "confirmedForSeconds")
            ),
            _ => "인증된 연결을 확인하며 대표 PC를 선정하고 있어요.".to_string(),
        };
        let error = s(host, "lastError");
        format!("{role} 확인한 후보 {}대 · 대표 연결을 우선 교환하며 다른 연결도 유지해요.{} 연결이 나뉘면 대표가 다를 수 있고, 관리자 권한은 바뀌지 않아요.",s(host,"candidates"),if error.is_empty(){String::new()}else{format!(" 일부 후보 확인: {error}")})
    } else {
        "대표 PC 선정은 꺼져 있어요. 중계와 대표 후보 참여에 동의한 실행 동안에만 동작해요.".into()
    };
    set_text("meshHostStatus", &host_text);
    set_text(
        "meshCourierStatus",
        if info["courierEnabled"] == true {
            "암호문 중계 참여 중 · 상대도 중계에 동의해야 해요. 중계 보관 응답은 최종 수신 확인과 달라요."
        } else {
            "암호문 중계는 꺼져 있어요. 최종 수신 확인이 돌아오기 전까지 원본은 대기로 남아요."
        },
    );
    set_text(
        "meshAutomaticStatus",
        &if auto["enabled"] == true {
            let phase = match s(auto, "phase").as_str() {
                "no-peers" => "다른 승인 장치 없음",
                "connecting" => "장치 정보 확인 중",
                "exchanging" => "암호화 기록 교환 중",
                "waiting" => "다음 교환 대기",
                _ => "승인 PC 찾는 중",
            };
            let error = s(auto, "lastError");
            format!(
                "자동 연결 · {phase} · 발견 후보 {}대 · 확인한 장치 {}대 · 다음 시도 {}초{}",
                s(auto, "discovered"),
                s(auto, "paired"),
                s(auto, "retryInSeconds"),
                if error.is_empty() {
                    String::new()
                } else {
                    format!(" · {error}")
                }
            )
        } else {
            "자동 연결은 꺼져 있어요. 동의한 실행 동안에만 동작해요.".into()
        },
    );
    if let Some(list) = by_id("meshSessionPeers") {
        clear(&list);
        for peer in info["peers"].as_array().into_iter().flatten() {
            let id = s(peer, "session");
            let error = s(peer, "lastError");
            let status = if !error.is_empty() {
                error
            } else if peer["lastSuccess"].as_f64().unwrap_or(0.0) > 0.0 {
                "최근 인증된 연결 작업 성공 · 본문 보관 여부는 기록별 상태를 확인하세요.".into()
            } else {
                "상대에서도 내 연결 정보를 등록한 뒤 확인하세요.".into()
            };
            let item = el("div")
                .cls("item")
                .kid(
                    el("div")
                        .cls("main")
                        .kid(
                            el("b")
                                .text(&format!("연결 {}", id.chars().take(12).collect::<String>())),
                        )
                        .kid(el("p").cls("hint").text(&format!(
                            "{} · {status}",
                            if peer["hostEnabled"] == true {
                                "상대 중계·대표 후보 참여"
                            } else if peer["courierEnabled"] == true {
                                "상대 암호문 중계 허용"
                            } else {
                                "상대 직접 교환만 허용"
                            }
                        ))),
                )
                .kid(
                    el("button")
                        .attr("type", "button")
                        .text("연결 확인")
                        .click(move |_| {
                            net_run(
                                "mesh-session:check",
                                json!({"session":session(),"peer":id}),
                                "승인된 장치와 연결을 확인했어요.",
                            )
                        }),
                );
            let item = if peer["exchangeEnabled"] == true {
                let id = s(peer, "session");
                item.kid(
                    el("button")
                        .attr("type", "button")
                        .attr("data-exchange", "true")
                        .text("암호화 기록 교환")
                        .click(move |_| {
                            net_run(
                                "mesh-session:exchange",
                                json!({"session":session(),"peer":id}),
                                "암호화된 기록과 저장 확인을 교환했어요.",
                            );
                        }),
                )
            } else {
                item
            };
            let _ = list.append_child(&item.0);
        }
    }
    controls();
}

fn render_log(info: Value) {
    LOG_WRITE.with(|n| n.set(info["canWrite"] == true));
    set_text(
        "meshLocalStatus",
        &format!(
            "내 원본 {}개 · 받은 본문 {}개 · 발신 대기/보류 {}개 · 충돌 {}건 · 내 원본과 수신 본문 각 최근 50개",
            s(&info, "stored"),
            s(&info,"received"),s(&info, "pending"),s(&info,"conflicts")
        ),
    );
    if let Some(list) = by_id("meshLocalMessages") {
        clear(&list);
        for message in info["messages"].as_array().into_iter().flatten() {
            let item = el("div").cls("item").kid(
                el("div")
                    .cls("main")
                    .kid(el("b").text(&format!(
                        "{} #{} · {}",
                        if message["incoming"] == true {
                            format!(
                                "보낸 장치 {}",
                                s(message, "author").chars().take(12).collect::<String>()
                            )
                        } else {
                            "내 원본".into()
                        },
                        s(message, "sequence"),
                        match s(message, "state").as_str() {
                            "peer-stored" => "수신 대상 장치들의 보관 확인 완료",
                            "local-only" => "이 PC에만 저장 · 다른 수신 장치 없음",
                            "approval-held" => "승인 이력 변경으로 전송 보류",
                            "legacy-held" => "이전 로컬 전용 기록 · 자동 전송 제외",
                            "received" => "받은 본문을 이 PC에 보관",
                            _ => "상대 장치의 저장 확인 대기",
                        }
                    )))
                    .kid(el("p").cls("mesh-local-text").text(&s(message, "text"))),
            );
            let _ = list.append_child(&item.0);
        }
    }
    controls();
}

pub(super) async fn load() {
    let selected = team();
    let changed = EDITING_TEAM.with(|id| {
        if *id.borrow() == selected {
            return false;
        }
        *id.borrow_mut() = selected.clone();
        true
    });
    if changed {
        let body = DRAFTS.with(|d| {
            d.borrow()
                .get(&selected)
                .map(|(body, _)| body.clone())
                .unwrap_or_default()
        });
        if let Some(input) = by_id("meshLocalText") {
            set_val(&input, &body);
        }
        set_text("meshLocalMessage", "");
        if let Some(list) = by_id("meshLocalMessages") {
            clear(&list);
        }
    }
    let generation = LOG_GEN.with(|n| {
        n.set(n.get() + 1);
        n.get()
    });
    LOG_WRITE.with(|n| n.set(false));
    controls();
    if selected.is_empty() {
        set_text("meshLocalStatus", "PC 팀을 먼저 준비하거나 등록해 주세요.");
        if let Some(list) = by_id("meshLocalMessages") {
            clear(&list);
        }
    } else if !LOG_BUSY.with(Cell::get) {
        match call("mesh-journal:status", json!({"teamId":selected})).await {
            Ok(info) if team() == selected && generation == LOG_GEN.with(Cell::get) => {
                render_log(info)
            }
            Err(error) if team() == selected && generation == LOG_GEN.with(Cell::get) => {
                set_text("meshLocalStatus", &error);
                if let Some(list) = by_id("meshLocalMessages") {
                    clear(&list);
                }
            }
            _ => {}
        }
    }
    if by_id("meshSessionAddress").is_some_and(|e| val(&e).is_empty()) {
        if let Ok(items) = call("mesh-session:interfaces", Value::Null).await {
            if let Some(select) = by_id("meshSessionAddress").filter(|select| {
                val(select).is_empty() && !NET.with(|n| n.borrow()["running"] == true)
            }) {
                clear(&select);
                let _ =
                    select.append_child(&el("option").attr("value", "").text("이 PC의 LAN 선택").0);
                for item in items.as_array().into_iter().flatten() {
                    let ip = s(item, "address");
                    let _ = select.append_child(
                        &el("option")
                            .attr("value", &ip)
                            .text(&format!("{} · {}", s(item, "name"), ip))
                            .0,
                    );
                }
            }
        }
    }
    poll_net().await;
    controls();
}

async fn poll_net() {
    if NET_BUSY.with(|n| n.get() > 0) || POLLING.with(|n| n.replace(true)) {
        return;
    }
    let generation = NET_GEN.with(Cell::get);
    let result = call("mesh-session:status", Value::Null).await;
    if generation == NET_GEN.with(Cell::get) {
        if let Ok(info) = result {
            render_net(info);
        }
    }
    POLLING.with(|n| n.set(false));
}

async fn refresh_auto_log() {
    let selected = team();
    if selected.is_empty()
        || LOG_BUSY.with(Cell::get)
        || !NET.with(|n| {
            n.borrow()["running"] == true
                && n.borrow()["automatic"]["enabled"] == true
                && s(&n.borrow(), "teamId") == selected
        })
        || AUTO_LOG_LOADING.with(|n| n.replace(true))
    {
        return;
    }
    let generation = LOG_GEN.with(Cell::get);
    let result = call("mesh-journal:status", json!({"teamId":selected})).await;
    if team() == selected && generation == LOG_GEN.with(Cell::get) && !LOG_BUSY.with(Cell::get) {
        match result {
            Ok(info) => render_log(info),
            Err(error) => set_text("meshLocalStatus", &error),
        }
    }
    AUTO_LOG_LOADING.with(|n| n.set(false));
}

fn net_run(channel: &'static str, args: Value, success: &'static str) {
    if NET_BUSY.with(|n| n.get() > 0) && channel != "mesh-session:stop" {
        return;
    }
    NET_BUSY.with(|n| n.set(n.get() + 1));
    let generation = NET_GEN.with(|n| {
        n.set(n.get() + 1);
        n.get()
    });
    controls();
    set_text("meshSessionMessage", "처리하는 중…");
    spawn(async move {
        let result = call(channel, args).await;
        if generation == NET_GEN.with(Cell::get) {
            match result {
                Ok(info) => {
                    if channel == "mesh-session:stop" {
                        if let Some(restore) = by_id("meshRestoreConsent") {
                            set_checked(&restore, false);
                        }
                    }
                    if info["report"].is_object() {
                        let message = format!(
                            "상대 본문 보관 확인 {}건 · 내 본문 보관 확인 {}건 · 중계 자료 전송 {}개/수신 {}개{}",
                            s(&info["report"], "sent"),
                            s(&info["report"], "received"),
                            s(&info["report"], "relaySent"),
                            s(&info["report"], "relayReceived"),
                            if info["report"]["more"] == true {
                                " · 남은 기록이 있어요. 다시 교환하세요."
                            } else {
                                " · 이번 교환을 마쳤어요."
                            }
                        );
                        render_net(info["session"].clone());
                        set_text("meshSessionMessage", &message);
                    } else {
                        render_net(info);
                        set_text("meshSessionMessage", success);
                    }
                }
                Err(error) => set_text("meshSessionMessage", &error),
            }
        }
        NET_BUSY.with(|n| n.set(n.get().saturating_sub(1)));
        controls();
        if channel == "mesh-session:exchange" {
            load().await;
        }
    });
}

fn save_local() {
    if LOG_BUSY.with(|n| n.replace(true)) {
        return;
    }
    let selected = team();
    let text = v("meshLocalText");
    LOG_GEN.with(|n| n.set(n.get() + 1));
    controls();
    set_text("meshLocalMessage", "이 PC에 저장하는 중…");
    spawn(async move {
        let old = DRAFTS.with(|d| d.borrow().get(&selected).cloned());
        let token = match old {
            Some((body, Some(token))) if body == text => Ok(token),
            _ => call("mesh-journal:draft-id", Value::Null)
                .await
                .and_then(|value| {
                    value
                        .as_str()
                        .map(str::to_string)
                        .ok_or_else(|| "저장 요청을 만들지 못했어요.".into())
                }),
        };
        let result = match token {
            Ok(token) => {
                DRAFTS.with(|d| {
                    let mut drafts = d.borrow_mut();
                    let draft = drafts
                        .entry(selected.clone())
                        .or_insert((text.clone(), None));
                    if draft.0 == text {
                        draft.1 = Some(token.clone());
                    }
                });
                call(
                    "mesh-journal:append",
                    json!({"teamId":selected,"requestId":token,"text":text}),
                )
                .await
            }
            Err(error) => Err(error),
        };
        if result.is_ok() {
            DRAFTS.with(|d| {
                let mut drafts = d.borrow_mut();
                if drafts.get(&selected).is_some_and(|(body, _)| body == &text) {
                    drafts.remove(&selected);
                }
            });
        }
        if team() == selected {
            match result {
                Ok(info) => {
                    render_log(info["status"].clone());
                    if v("meshLocalText") == text {
                        if let Some(input) = by_id("meshLocalText") {
                            set_val(&input, "");
                        }
                    }
                    set_text(
                        "meshLocalMessage",
                        "이 PC에 저장했어요. 동의한 연결에서 암호화 기록 교환을 누르면 발신 대기를 처리해요.",
                    );
                }
                Err(error) => set_text("meshLocalMessage", &error),
            }
        }
        LOG_BUSY.with(|n| n.set(false));
        controls();
        load().await;
    });
}

pub(super) fn bind() {
    if let Some(input) = by_id("meshLocalText") {
        on(&input, "input", |_| {
            let selected = team();
            let text = v("meshLocalText");
            DRAFTS.with(|d| {
                let mut drafts = d.borrow_mut();
                if !drafts.get(&selected).is_some_and(|(body, _)| body == &text) {
                    drafts.insert(selected, (text, None));
                }
            });
        });
    }
    for id in [
        "meshSessionAddress",
        "meshSessionConsent",
        "meshExchangeConsent",
        "meshAutomaticConsent",
        "meshCourierConsent",
        "meshRestoreConsent",
    ] {
        if let Some(e) = by_id(id) {
            on(&e, "change", |_| {
                if !by_id("meshExchangeConsent").is_some_and(|e| checked(&e)) {
                    if let Some(auto) = by_id("meshAutomaticConsent") {
                        set_checked(&auto, false);
                    }
                }
                if !by_id("meshAutomaticConsent").is_some_and(|e| checked(&e)) {
                    if let Some(courier) = by_id("meshCourierConsent") {
                        set_checked(&courier, false);
                    }
                    if let Some(restore) = by_id("meshRestoreConsent") {
                        set_checked(&restore, false);
                    }
                }
                controls();
            });
        }
    }
    click("meshSessionRefresh", |_| spawn(load()));
    click("meshLocalRefresh", |_| spawn(load()));
    click("meshLocalSave", |_| save_local());
    click("meshSessionStart", |_| {
        net_run(
            "mesh-session:start",
            json!({"teamId":team(),"address":v("meshSessionAddress"),"consent":by_id("meshSessionConsent").is_some_and(|e|checked(&e)),"exchangeConsent":by_id("meshExchangeConsent").is_some_and(|e|checked(&e)),"autoConsent":by_id("meshAutomaticConsent").is_some_and(|e|checked(&e)),"courierConsent":by_id("meshCourierConsent").is_some_and(|e|checked(&e)),"restoreConsent":by_id("meshRestoreConsent").is_some_and(|e|checked(&e))}),
            "승인 장치 연결 확인을 시작했어요.",
        )
    });
    click("meshSessionStop", |_| {
        net_run(
            "mesh-session:stop",
            json!({"session":session()}),
            "연결 확인을 중지했어요.",
        )
    });
    click("meshSessionAdd", |_| {
        net_run(
            "mesh-session:add",
            json!({"session":session(),"card":v("meshSessionInput")}),
            "승인된 상대 장치를 등록했어요.",
        )
    });
    every(2000, || {
        spawn(poll_net());
    });
    every(5000, || spawn(refresh_auto_log()));
    controls();
}
