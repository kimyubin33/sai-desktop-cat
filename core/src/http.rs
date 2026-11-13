//! 모든 네트워크 호출이 함께 쓰는 HTTP 클라이언트와 도우미.
use once_cell::sync::Lazy;
use serde_json::Value;
use std::time::Duration;

pub static CLIENT: Lazy<reqwest::Client> = Lazy::new(|| {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .user_agent(concat!("sai-desktop-cat/", env!("CARGO_PKG_VERSION")))
        .build()
        .expect("http client")
});

pub fn client() -> &'static reqwest::Client {
    &CLIENT
}

/// 응답 본문을 JSON 으로 (비어 있거나 JSON 이 아니면 Null)
pub async fn json_body(res: reqwest::Response) -> (u16, Value) {
    let status = res.status().as_u16();
    let text = res.text().await.unwrap_or_default();
    let v = if text.trim().is_empty() { Value::Null } else { serde_json::from_str(&text).unwrap_or(Value::String(text)) };
    (status, v)
}

/// 오류 메시지용 짧은 본문
pub fn short(v: &Value) -> String {
    let s = match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    s.chars().take(300).collect()
}

pub fn form(params: &[(&str, &str)]) -> String {
    params.iter().map(|(k, v)| format!("{}={}", urlencoding::encode(k), urlencoding::encode(v))).collect::<Vec<_>>().join("&")
}
