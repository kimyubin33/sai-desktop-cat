//! 설정의 "캐릭터 AI" 카드: 이 PC에서 도는 모델을 받고, 켜고, 지운다.
use super::{b, s};
use crate::{
    api::{call, listen},
    dom::*,
};
use serde_json::{json, Value};

fn gb(bytes: u64) -> String {
    format!("{:.1}GB", bytes as f64 / 1_073_741_824.0)
}

fn draw(v: &Value) {
    let supported = b(v, "supported");
    let installed = b(v, "installed");
    let installing = b(v, "installing");
    let enabled = b(v, "enabled");
    let total = v["downloadBytes"].as_u64().unwrap_or(0);
    let mut text = if !supported {
        "이 PC에서는 아직 쓸 수 없어요.".to_owned()
    } else if installed {
        format!("{} · 이 PC 안에서만 실행돼요. 캐릭터가 하는 말은 PC 밖으로 나가지 않아요.", s(v, "model"))
    } else {
        format!(
            "캐릭터가 상황에 맞춰 먼저 말을 걸어요. 처음 한 번 약 {}를 받고, 이후에는 인터넷 없이 이 PC에서 돌아가요.",
            gb(total)
        )
    };
    if b(v, "lowMemory") {
        text.push_str(&format!(
            " 이 PC의 메모리는 {}GB라 다른 프로그램이 느려질 수 있어요.",
            v["memoryGb"]
        ));
    }
    set_text("localAiText", &text);

    let bar = by_id("localAiProgress").unwrap();
    toggle(&bar, "hidden", !installing);
    let p = &v["progress"];
    let (done, all) = (p["done"].as_u64().unwrap_or(0), p["total"].as_u64().unwrap_or(0));
    if all > 0 {
        let _ = bar.set_attribute("value", &((done * 100 / all).to_string()));
    } else {
        let _ = bar.remove_attribute("value");
    }
    let message = if installing {
        match s(p, "phase").as_str() {
            "runtime" => format!("실행 파일 받는 중… {} / {}", gb(done), gb(all)),
            "extract" => "실행 파일 푸는 중…".to_owned(),
            "model" => format!("모델 받는 중… {} / {} · 멈춰도 다음에 이어서 받아요", gb(done), gb(all)),
            _ => "준비 중…".to_owned(),
        }
    } else {
        s(v, "error")
    };
    set_text("localAiMessage", &message);

    let install = by_id("localAiInstall").unwrap();
    toggle(&install, "hidden", installed || !supported);
    set_disabled(&install, installing);
    set_text("localAiInstall", if v["progress"].is_null() && !s(v, "error").is_empty() { "이어 받기" } else { "받기" });
    toggle(&by_id("localAiCancel").unwrap(), "hidden", !installing);
    let enable = by_id("localAiEnable").unwrap();
    set_checked(&enable, enabled);
    set_disabled(&enable, !installed || installing);
    set_disabled(&by_id("localAiTry").unwrap(), !installed || !enabled || installing);
    toggle(&by_id("localAiRemove").unwrap(), "hidden", !installed);
}

async fn run(channel: &str, args: Value) {
    match call(channel, args).await {
        Ok(v) if v.is_object() => draw(&v),
        Ok(_) => load().await,
        Err(e) => {
            load().await;
            set_text("localAiMessage", &e);
        }
    }
}

pub async fn load() {
    if let Ok(v) = call("local-ai:status", Value::Null).await {
        draw(&v);
    }
}

pub fn bind() {
    listen("local-ai:progress", |v| draw(&v));
    on(&by_id("localAiInstall").unwrap(), "click", |_| {
        spawn(run("local-ai:install", Value::Null))
    });
    on(&by_id("localAiCancel").unwrap(), "click", |_| {
        spawn(run("local-ai:cancel", Value::Null))
    });
    on(&by_id("localAiEnable").unwrap(), "change", |_| {
        let on = checked(&by_id("localAiEnable").unwrap());
        spawn(run("local-ai:enable", json!({ "on": on })))
    });
    on(&by_id("localAiTry").unwrap(), "click", |_| {
        set_text("localAiMessage", "캐릭터가 할 말을 고르는 중이에요. 처음에는 30초쯤 걸릴 수 있어요.");
        spawn(async {
            let _ = call("local-ai:try", Value::Null).await;
        })
    });
    on(&by_id("localAiRemove").unwrap(), "click", |_| {
        if confirm("캐릭터 AI 파일을 이 PC에서 지울까요? 다시 쓰려면 처음부터 받아야 해요.") {
            spawn(run("local-ai:remove", Value::Null))
        }
    });
    spawn(load());
}
