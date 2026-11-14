//! 앱(Rust, Tauri)과 이야기하기. 화면은 call(채널, 값) 하나로 모든 기능을 부르고, 앱이 보내는 이벤트를 듣는다.
use serde::Serialize;
use serde_json::Value;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

fn tauri(path: &[&str]) -> Option<js_sys::Function> {
    let mut cur: JsValue = crate::dom::window().into();
    for p in ["__TAURI__"].iter().chain(path.iter()) {
        cur = js_sys::Reflect::get(&cur, &JsValue::from_str(p)).ok()?;
        if cur.is_undefined() {
            return None;
        }
    }
    cur.dyn_into::<js_sys::Function>().ok()
}

fn to_js(v: &Value) -> JsValue {
    v.serialize(&serde_wasm_bindgen::Serializer::json_compatible()).unwrap_or(JsValue::NULL)
}

fn from_js(v: JsValue) -> Value {
    if v.is_undefined() || v.is_null() {
        return Value::Null;
    }
    serde_wasm_bindgen::from_value(v).unwrap_or(Value::Null)
}

fn err_text(e: JsValue) -> String {
    e.as_string()
        .or_else(|| js_sys::Reflect::get(&e, &JsValue::from_str("message")).ok().and_then(|m| m.as_string()))
        .unwrap_or_else(|| "알 수 없는 오류".into())
}

async fn invoke(cmd: &str, payload: Value) -> Result<Value, String> {
    let f = tauri(&["core", "invoke"]).ok_or("앱과 연결되지 않았어요.")?;
    let p = f.call2(&JsValue::NULL, &JsValue::from_str(cmd), &to_js(&payload)).map_err(err_text)?;
    let r = wasm_bindgen_futures::JsFuture::from(js_sys::Promise::from(p)).await.map_err(err_text)?;
    Ok(from_js(r))
}

/// 대화창 → 앱
pub async fn call(ch: &str, args: Value) -> Result<Value, String> {
    invoke("call", serde_json::json!({ "ch": ch, "args": args })).await
}

/// 고양이 창 → 앱 (결과는 쓰지 않는다)
pub fn cat_event(ev: &str, args: Value) {
    let ev = ev.to_string();
    crate::dom::spawn(async move {
        let _ = invoke("cat_event", serde_json::json!({ "ev": ev, "args": args })).await;
    });
}

/// 앱이 보내는 이벤트 듣기
pub fn listen(name: &str, f: impl Fn(Value) + 'static) {
    let _ = register_listener(name, f);
}

/// 첫 알림을 요청하기 전에 실제 Tauri 구독이 끝났는지 기다릴 때 사용한다.
pub fn register_listener(name: &str, f: impl Fn(Value) + 'static) -> Option<js_sys::Promise> {
    let l = tauri(&["event", "listen"])?;
    let cb = Closure::<dyn Fn(JsValue)>::new(move |e: JsValue| {
        let payload = js_sys::Reflect::get(&e, &JsValue::from_str("payload")).unwrap_or(JsValue::NULL);
        f(from_js(payload));
    });
    let registered = l.call2(&JsValue::NULL, &JsValue::from_str(name), cb.as_ref().unchecked_ref()).ok()?;
    cb.forget();
    Some(js_sys::Promise::from(registered))
}
