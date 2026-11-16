use super::{click, s, v};
use crate::api::call;
use crate::dom::*;
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};

thread_local! {
    static STATE: RefCell<Value> = const { RefCell::new(Value::Null) };
    static BUSY: Cell<bool> = const { Cell::new(false) };
    static GENERATION: Cell<u64> = const { Cell::new(0) };
}

pub(super) fn team_id() -> String {
    v("meshIdentityTeam")
}
pub(super) fn admin_here() -> bool {
    selected_team()["adminHere"] == true
}

fn selected_team() -> Value {
    STATE.with(|state| {
        state.borrow()["teams"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|team| s(team, "teamId") == team_id())
            .cloned()
            .unwrap_or(Value::Null)
    })
}

fn controls() {
    let busy = BUSY.with(Cell::get);
    let has_device = STATE.with(|state| state.borrow()["device"].is_object());
    let may_create_team = STATE.with(|state| {
        state.borrow()["teams"].as_array().is_some_and(|teams| {
            teams.is_empty() || teams.iter().any(|team| team["adminHere"] == true)
        })
    });
    let team = selected_team();
    let has_team = team.is_object();
    let admin = team["adminHere"].as_bool().unwrap_or(false);
    for (id, disabled) in [
        ("meshIdentityCreate", busy || has_device),
        (
            "meshIdentityCreateTeam",
            busy || !has_device || !may_create_team,
        ),
        ("meshIdentityRequest", busy || !has_device || !has_team),
        ("meshIdentityExport", busy || !has_team),
        ("meshIdentityImport", busy),
        ("meshIdentityApprove", busy || !admin),
        ("meshIdentityTeam", busy),
        ("meshIdentityTeamName", busy),
    ] {
        if let Some(element) = by_id(id) {
            set_disabled(&element, disabled);
        }
    }
    if let Some(list) = by_id("meshIdentityDevices") {
        for button in q_all_in(&list, "button") {
            set_disabled(&button, busy || !admin);
        }
    }
}

fn render_team() {
    let team = selected_team();
    let admin = team["adminHere"].as_bool().unwrap_or(false);
    let own = STATE.with(|state| s(&state.borrow()["device"], "device_id"));
    let membership = team["devices"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|device| s(&device["device"], "device_id") == own);
    let own_status = match membership {
        Some(member) if member["active"] == true => "이 장치가 승인되어 있어요.",
        Some(_) => "이 장치의 등록이 해제되어 있어요.",
        None => "이 장치는 아직 승인되지 않았어요.",
    };
    let line = if team.is_object() {
        format!(
            "{} · 승인 이력 {} · {} {}",
            s(&team, "name"),
            s(&team, "revision"),
            if admin {
                "이 PC에 운영 관리자 키가 있어요."
            } else {
                "관리자 서명을 검증해 등록한 팀이에요."
            },
            own_status
        )
    } else {
        "관리자는 새 PC 팀을 준비하고, 팀원은 받은 승인 정보를 먼저 등록하세요.".into()
    };
    set_text("meshIdentityTeamInfo", &line);
    set_text("meshIdentityFingerprint", &s(&team, "authorityFingerprint"));
    if let Some(list) = by_id("meshIdentityDevices") {
        clear(&list);
        for member in team["devices"].as_array().into_iter().flatten() {
            let id = s(&member["device"], "device_id");
            let active = member["active"] == true;
            let label = format!(
                "장치 {}{} · {}",
                id.chars().take(12).collect::<String>(),
                if id == own { " (이 PC)" } else { "" },
                if active {
                    "승인됨"
                } else {
                    "등록 해제됨"
                }
            );
            let mut item = el("div").cls("item").kid(
                el("div")
                    .cls("main")
                    .kid(el("b").text(&label))
                    .kid(el("p").cls("mesh-fingerprint").text(&id)),
            );
            if admin && active {
                item = item.kid(el("button").attr("type","button").text("등록 해제").click(move |_| {
                    if confirm(&format!("장치 {}의 PC 팀 등록을 해제할까요? 갱신된 승인 자료를 다른 장치에 전달해야 반영돼요.", id.chars().take(12).collect::<String>())) {
                        run("mesh-identity:revoke", json!({"teamId":team_id(),"deviceId":id}), "등록을 해제했어요. 갱신된 공개 승인 정보를 전달하세요.");
                    }
                }));
            }
            let _ = list.append_child(&item.0);
        }
    }
    controls();
    spawn(super::workbench::load());
    super::migration::selected_changed();
}

fn render(response: Value) {
    let status = response["status"].clone();
    let previous = team_id();
    let output = response["output"].as_str();
    let from_output = output
        .and_then(|text| serde_json::from_str::<Value>(text).ok())
        .map(|value| s(&value["manifest"]["body"], "team_id"))
        .filter(|id| !id.is_empty())
        .unwrap_or(previous);
    let preferred = response["selectedTeam"]
        .as_str()
        .filter(|id| !id.is_empty())
        .unwrap_or(&from_output);
    let own = s(&status["device"], "device_id");
    set_text(
        "meshIdentityStatus",
        if own.is_empty() {
            "내 장치를 아직 만들지 않았어요."
        } else {
            "내 장치 키를 Windows 사용자 계정으로 보호해 보관하고 있어요."
        },
    );
    set_text("meshIdentityDevice", &own);
    STATE.with(|state| state.replace(status.clone()));
    if let Some(select) = by_id("meshIdentityTeam") {
        clear(&select);
        let teams = status["teams"].as_array();
        for team in teams.into_iter().flatten() {
            let id = s(team, "teamId");
            let _ = select.append_child(
                &el("option")
                    .attr("value", &id)
                    .text(&s(team, "name"))
                    .flag("selected", id == preferred)
                    .0,
            );
        }
        if teams.is_none_or(|teams| teams.is_empty()) {
            let _ = select.append_child(
                &el("option")
                    .attr("value", "")
                    .text("아직 등록한 PC 팀이 없어요")
                    .0,
            );
        }
    }
    if let Some(output) = output {
        if let Some(area) = by_id("meshIdentityOutput") {
            set_val(&area, output);
        }
    }
    render_team();
}

pub(super) async fn load() {
    if BUSY.with(Cell::get) {
        return;
    }
    let generation = GENERATION.with(Cell::get);
    let result = call("mesh-identity:status", Value::Null).await;
    if GENERATION.with(Cell::get) != generation {
        return;
    }
    match result {
        Ok(info) => render(info),
        Err(error) => set_text("meshIdentityMessage", &error),
    }
}

fn run(channel: &'static str, args: Value, success: &'static str) {
    if BUSY.with(|busy| busy.replace(true)) {
        return;
    }
    GENERATION.with(|value| value.set(value.get() + 1));
    controls();
    set_text("meshIdentityMessage", "처리하는 중…");
    spawn(async move {
        match call(channel, args).await {
            Ok(info) => {
                render(info);
                set_text("meshIdentityMessage", success);
            }
            Err(error) => set_text("meshIdentityMessage", &error),
        }
        BUSY.with(|busy| busy.set(false));
        controls();
    });
}

pub(super) fn bind() {
    if let Some(select) = by_id("meshIdentityTeam") {
        on(&select, "change", |_| {
            if let Some(area) = by_id("meshIdentityOutput") {
                set_val(&area, "");
            }
            render_team();
        });
    }
    click("meshIdentityCreate", |_| {
        run(
            "mesh-identity:create-device",
            Value::Null,
            "내 장치를 만들었어요. 재시작해도 같은 장치 신원을 사용해요.",
        )
    });
    click("meshIdentityCreateTeam", |_| {
        run(
            "mesh-identity:create-team",
            json!({"name":v("meshIdentityTeamName")}),
            "PC 팀을 준비했어요. 운영 관리자 지문과 공개 승인 정보를 전달하세요.",
        )
    });
    click("meshIdentityRequest", |_| {
        run(
            "mesh-identity:request",
            json!({"teamId":team_id()}),
            "공개 등록 요청을 만들었어요. 운영 관리자에게 전달하세요.",
        )
    });
    click("meshIdentityExport", |_| {
        run(
            "mesh-identity:export",
            json!({"teamId":team_id()}),
            "공개 승인 정보를 만들었어요.",
        )
    });
    click("meshIdentityApprove", |_| {
        run(
            "mesh-identity:approve",
            json!({"teamId":team_id(),"input":v("meshIdentityInput")}),
            "장치를 승인했어요. 갱신된 공개 승인 정보를 요청한 장치에 전달하세요.",
        )
    });
    click("meshIdentityImport", |_| {
        run(
            "mesh-identity:import",
            json!({"input":v("meshIdentityInput"),"fingerprint":v("meshIdentityPin")}),
            "서명을 검증해 승인 정보를 등록했어요. 오래된 자료는 현재 상태를 되돌리지 않아요.",
        )
    });
}
