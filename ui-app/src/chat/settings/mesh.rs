use super::{click, s, v};
use crate::api::call;
use crate::dom::*;
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};

thread_local! {
    static STATE: RefCell<Value> = const { RefCell::new(Value::Null) };
    static BUSY: Cell<bool> = const { Cell::new(false) };
    static POLLING: Cell<bool> = const { Cell::new(false) };
    static GENERATION: Cell<u64> = const { Cell::new(0) };
}

fn session() -> String {
    STATE.with(|state| s(&state.borrow(), "session"))
}

fn buttons() {
    let running = STATE.with(|state| state.borrow()["running"].as_bool().unwrap_or(false));
    let busy = BUSY.with(Cell::get);
    let consent = by_id("meshProbeConsent").is_some_and(|e| checked(&e));
    for (id, disabled) in [
        (
            "meshProbeStart",
            busy || running || !consent || v("meshProbeAddress").is_empty(),
        ),
        ("meshProbeStop", !running),
        ("meshProbeAdd", busy || !running),
        ("meshProbeAddress", running || busy),
        ("meshProbeRefresh", busy),
    ] {
        if let Some(el) = by_id(id) {
            set_disabled(&el, disabled);
        }
    }
    if let Some(peers) = by_id("meshProbePeers") {
        for el in q_all_in(&peers, "button") {
            set_disabled(&el, busy || !running);
        }
    }
}

fn render(info: Value) {
    let running = info["running"].as_bool().unwrap_or(false);
    let changed = session() != s(&info, "session");
    STATE.with(|state| state.replace(info.clone()));
    if let Some(area) = by_id("meshProbeRunning") {
        toggle(&area, "hidden", !running);
    }
    if let Some(card) = by_id("meshProbeCard") {
        if val(&card) != s(&info, "card") {
            set_val(&card, &s(&info, "card"));
        }
    }
    if changed {
        if let Some(input) = by_id("meshProbePeerCard") {
            set_val(&input, "");
        }
    }
    set_text("meshProbeFingerprint", &s(&info, "fingerprint"));
    let state = if running {
        let expires = info["expiresAt"].as_f64().unwrap_or(0.0);
        format!(
            "실험 실행 중 · {}까지 · 종료하거나 다시 시작하면 양쪽에서 새 정보를 등록하세요.",
            super::super::fmt_clock(expires)
        )
    } else {
        "실험이 꺼져 있어요. 실행하려면 LAN 주소를 고르고 동의 후 시작하세요.".into()
    };
    set_text("meshProbeState", &state);
    if let Some(list) = by_id("meshProbePeers") {
        clear(&list);
        for peer in info["peers"].as_array().into_iter().flatten() {
            let id = s(peer, "session");
            let error = s(peer, "lastError");
            let checked = peer["lastChecked"].as_f64().unwrap_or(0.0);
            let text = if !error.is_empty() {
                error
            } else if checked > 0.0 {
                format!(
                    "{}에 내 PC에서 연결 확인 성공",
                    super::super::fmt_clock(checked)
                )
            } else {
                "상대 PC에서도 내 정보를 등록한 뒤 연결 확인을 눌러 주세요.".into()
            };
            let label = format!("장치 {}", id.chars().take(8).collect::<String>());
            let item = el("div")
                .cls("item")
                .kid(
                    el("div")
                        .cls("main")
                        .kid(el("b").text(&label))
                        .kid(
                            el("p")
                                .cls("mesh-fingerprint")
                                .text(&s(peer, "fingerprint")),
                        )
                        .kid(el("p").cls("hint").text(&text)),
                )
                .kid(
                    el("button")
                        .attr("type", "button")
                        .text("연결 확인")
                        .click(move |_| {
                            run(
                                "mesh-probe:check",
                                json!({"session":session(), "peer":id}),
                                "연결 확인에 성공했어요.",
                            );
                        }),
                );
            let _ = list.append_child(&item.0);
        }
    }
    let discovered = info["discovered"].as_array().map(Vec::len).unwrap_or(0);
    let message = if s(&info, "discoveryError").is_empty() {
        format!("LAN에서 발견한 실험 장치 {discovered}대. 발견만으로 연결을 허용하지 않아요.")
    } else {
        s(&info, "discoveryError")
    };
    set_text("meshProbeDiscovered", &message);
    buttons();
}

async fn refresh() {
    if POLLING.with(|p| p.replace(true)) {
        return;
    }
    let generation = GENERATION.with(Cell::get);
    let result = call("mesh-probe:status", Value::Null).await;
    POLLING.with(|p| p.set(false));
    if GENERATION.with(Cell::get) != generation {
        return;
    }
    if let Ok(info) = result {
        render(info);
    }
}

pub(crate) async fn load() {
    if let Ok(addresses) = call("mesh-probe:interfaces", Value::Null).await {
        if let Some(select) = by_id("meshProbeAddress") {
            let selected = val(&select);
            clear(&select);
            let _ = select.append_child(
                &el("option")
                    .attr("value", "")
                    .text("사용할 Wi-Fi 또는 유선 LAN을 골라 주세요")
                    .flag("selected", selected.is_empty())
                    .0,
            );
            for network in addresses.as_array().into_iter().flatten() {
                let Some(ip) = network["address"].as_str() else {
                    continue;
                };
                let name = s(network, "name");
                let label = if name.is_empty() {
                    ip.to_string()
                } else {
                    format!("{name} · {ip}")
                };
                let _ = select.append_child(
                    &el("option")
                        .attr("value", ip)
                        .text(&label)
                        .flag("selected", ip == selected)
                        .0,
                );
            }
            if addresses.as_array().is_none_or(|a| a.is_empty()) {
                clear(&select);
                let _ = select.append_child(
                    &el("option")
                        .attr("value", "")
                        .text("사용할 사설 IPv4 LAN 주소가 없어요")
                        .0,
                );
            }
        }
    }
    refresh().await;
}

fn run(channel: &'static str, args: Value, success: &'static str) {
    if BUSY.with(|busy| busy.replace(true)) {
        return;
    }
    let generation = GENERATION.with(|g| {
        g.set(g.get() + 1);
        g.get()
    });
    buttons();
    set_text("meshProbeMessage", "처리하는 중…");
    spawn(async move {
        let result = call(channel, args).await;
        if GENERATION.with(Cell::get) != generation {
            BUSY.with(|busy| busy.set(false));
            buttons();
            return;
        }
        match result {
            Ok(info) => {
                render(info);
                set_text("meshProbeMessage", success);
            }
            Err(e) => {
                set_text("meshProbeMessage", &e);
                refresh().await;
            }
        }
        BUSY.with(|busy| busy.set(false));
        buttons();
    });
}

pub(crate) fn bind(standalone: bool) {
    if let Some(consent) = by_id("meshProbeConsent") {
        on(&consent, "change", |_| buttons());
    }
    if let Some(address) = by_id("meshProbeAddress") {
        on(&address, "change", |_| buttons());
    }
    click("meshProbeStart", |_| {
        let consent = by_id("meshProbeConsent").is_some_and(|e| checked(&e));
        run(
            "mesh-probe:start",
            json!({"address":v("meshProbeAddress"), "consent":consent}),
            "실험을 시작했어요. 연결 정보를 상대와 교환하세요.",
        );
    });
    click("meshProbeAdd", |_| {
        run(
            "mesh-probe:add",
            json!({"session":session(), "card":v("meshProbePeerCard")}),
            "상대 정보를 등록했어요. 상대 PC에서도 내 정보를 등록한 뒤 연결 확인을 누르세요.",
        )
    });
    click("meshProbeStop", |_| {
        let generation = GENERATION.with(|g| {
            g.set(g.get() + 1);
            g.get()
        });
        let args = json!({"session":session()});
        spawn(async move {
            let result = call("mesh-probe:stop", args).await;
            if GENERATION.with(Cell::get) != generation {
                return;
            }
            match result {
                Ok(info) => {
                    render(info);
                    set_text("meshProbeMessage", "연결 실험을 중지했어요.");
                }
                Err(e) => set_text("meshProbeMessage", &e),
            }
        });
    });
    click("meshProbeRefresh", |_| spawn(load()));
    every(3000, move || {
        if (standalone || super::super::current_tab() == "settings") && !BUSY.with(Cell::get) {
            spawn(refresh());
        }
    });
}
