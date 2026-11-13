//! Claude API.
//!  - "그냥 이야기할래요": 일반 Claude 대화처럼 전체 대화를 보고 길이 제한 없이 (스트리밍)
//!  - 상담 흐름: 단계별 의도는 앱의 규칙이 정하고, AI는 문장과 정리 품질만 돕는다
//!  - 시간표 대화: 도구(tool use)로 시간표를 직접 고친다
//! API 키가 없거나 호출이 실패하면 규칙 기반 흐름이 그대로 동작한다.
use crate::counseling::Session;
use crate::http::{client, json_body, short};
use crate::store::Store;
use crate::topics::{topic, TOPICS};
use crate::util::J;
use crate::{err, Result};
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::sync::Mutex;
use std::time::Duration;

const API_URL: &str = "https://api.anthropic.com/v1/messages";

/// 시험할 때만 다른 주소로 (SAI_ANTHROPIC_URL)
fn api_url() -> String {
    std::env::var("SAI_ANTHROPIC_URL").unwrap_or_else(|_| API_URL.to_string())
}

const PERSONA: &str = "당신은 팀원 각자의 바탕화면에 사는 고양이 \"{name}\"입니다. 지금은 한 사용자와 단둘이 비공개로 이야기하고 있습니다.
지켜야 할 원칙:
- 항상 부드러운 존댓말로 말하고, 질문은 한 번에 하나만 합니다. 보통은 짧게 말하되 필요하면 충분히 설명해도 됩니다.
- 사용자가 겪은 불편과 감정은 인정하되, 사용자 편만 들거나 상대를 비난하지 않습니다.
- 확인되지 않은 추측을 사실처럼 말하지 않고, 상대의 잘못을 확정하지 않습니다.
- 사람에 대한 평가(무능하다, 게으르다 등)는 관찰 가능한 행동과 필요한 변화로 바꿔 말합니다.
- 사용자의 말을 다른 사람에게 그대로 전하겠다고 약속하지 않습니다. 원문과 이름은 다른 팀원에게 가지 않습니다.
- 심리 치료 효과를 약속하지 않고, 진단하지 않습니다.
- 이모지, 마크다운, 목록 기호를 쓰지 않습니다.";

const FREE_PERSONA: &str = "당신은 사용자의 바탕화면에 사는 고양이 \"{name}\"입니다. Anthropic의 Claude를 바탕으로 동작하며, 사용자와 자유롭게 대화합니다.
- 일반 대화, 질문, 공부, 글쓰기, 고민 상담 등 무엇이든 성실하게 돕습니다. 필요한 만큼 충분히 길게 답해도 됩니다.
- 부드러운 존댓말을 쓰고, 고양이다운 따뜻함은 살짝만 드러냅니다.
- 팀원에 대한 이야기가 나오면 사용자 편만 들거나 상대를 단정해 비난하지 않고, 관찰 가능한 행동과 필요한 변화로 바꿔 생각하도록 돕습니다.
- 심리 치료 효과를 약속하거나 진단하지 않습니다. 위급해 보이면 전문 도움을 권합니다.
- 화면이 마크다운을 그리지 않으므로 **, #, 표 같은 기호 대신 줄바꿈과 \"1.\" 같은 번호로 정리합니다.";

pub struct Config {
    pub enabled: bool,
    pub api_key: Option<String>,
    pub model: String,
    pub name: String,
}

pub struct Ai {
    settings: Store,
    pub last_error: Mutex<Option<String>>,
}

async fn post(api_key: &str, body: &Value, timeout: Duration) -> Result<reqwest::Response> {
    let res = client()
        .post(api_url())
        .timeout(timeout)
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .json(body)
        .send()
        .await
        .map_err(|e| if e.is_timeout() { "AI가 제때 응답하지 않았어요.".to_string() } else { format!("AI에 연결하지 못했어요: {e}") })?;
    if !res.status().is_success() {
        let (status, d) = json_body(res).await;
        return err(format!("API {status}: {}", short(&d)));
    }
    Ok(res)
}

/// 한 번에 받는 호출. max_tokens 에서 끊기면 실패로 본다 (끊긴 문장을 쓰지 않도록)
pub async fn call_text(api_key: &str, model: &str, system: &str, user: &str, max_tokens: u32) -> Result<String> {
    let body = json!({ "model": model, "max_tokens": max_tokens, "system": system, "messages": [{ "role": "user", "content": user }] });
    let (_, d) = json_body(post(api_key, &body, Duration::from_secs(60)).await?).await;
    if d.s("stop_reason") == "max_tokens" {
        return err("답이 너무 길어 중간에 끊겼어요.");
    }
    Ok(d.arr("content").iter().filter(|b| b.s("type") == "text").map(|b| b.s("text")).collect::<String>().trim().to_string())
}

/// 도구(tool use)를 쓰는 호출. 응답 전체를 돌려준다
pub async fn call_messages(api_key: &str, model: &str, system: &str, messages: &Value, tools: &Value) -> Result<Value> {
    let body = json!({ "model": model, "max_tokens": 2000, "system": system, "messages": messages, "tools": tools });
    Ok(json_body(post(api_key, &body, Duration::from_secs(90)).await?).await.1)
}

/// 스트리밍 호출: 글자가 오는 대로 on_text(지금까지의 전체 글) 를 부른다.
/// 응답이 max_tokens 에서 끊기면 이어서 쓰게 해서 끝까지 받는다. 60초 동안 아무것도 오지 않을 때만 멈춘다.
pub async fn stream(api_key: &str, model: &str, system: &str, messages: &[Value], on_text: &(dyn Fn(&str) + Send + Sync)) -> Result<String> {
    let mut full = String::new();
    for _round in 0..5 {
        let mut msgs = messages.to_vec();
        if !full.is_empty() {
            msgs.push(json!({ "role": "assistant", "content": full }));
        }
        let body = json!({ "model": model, "max_tokens": 8192, "system": system, "messages": msgs, "stream": true });
        let res = post(api_key, &body, Duration::from_secs(60 * 30)).await?;
        let mut s = res.bytes_stream();
        // 글자(UTF-8)가 조각 사이에서 잘릴 수 있으므로, 줄이 끝날 때까지 바이트로 모은다
        let mut buf: Vec<u8> = Vec::new();
        let mut stop: Option<String> = None;
        loop {
            let next = tokio::time::timeout(Duration::from_secs(60), s.next()).await;
            let chunk = match next {
                Err(_) => {
                    if full.is_empty() {
                        return err("AI가 60초 동안 응답하지 않았어요.");
                    }
                    return Ok(full.trim().to_string());
                }
                Ok(None) => break,
                Ok(Some(Err(e))) => return err(format!("AI 응답을 받다가 끊겼어요: {e}")),
                Ok(Some(Ok(c))) => c,
            };
            buf.extend_from_slice(&chunk);
            while let Some(i) = buf.iter().position(|b| *b == b'\n') {
                let line_bytes: Vec<u8> = buf.drain(..=i).collect();
                let line = String::from_utf8_lossy(&line_bytes).trim().to_string();
                let Some(data) = line.strip_prefix("data:") else { continue };
                let Ok(ev) = serde_json::from_str::<Value>(data.trim()) else { continue };
                match ev.s("type").as_str() {
                    "content_block_delta" if ev["delta"].s("type") == "text_delta" => {
                        full.push_str(&ev["delta"].s("text"));
                        on_text(&full);
                    }
                    "message_delta" => {
                        if let Some(r) = ev["delta"].os("stop_reason") {
                            stop = Some(r);
                        }
                    }
                    "error" => return err(format!("API 오류: {}", ev["error"].s("message"))),
                    _ => {}
                }
            }
        }
        if stop.as_deref() != Some("max_tokens") {
            return Ok(full.trim().to_string());
        }
        // 이어 쓰기: 끝의 공백은 API 가 받지 않는다
        full = full.trim_end().to_string();
    }
    Ok(full.trim().to_string())
}

fn transcript(s: &Session, limit: usize) -> String {
    let turns: Vec<_> = s.turns.iter().filter(|t| !t.safety).collect();
    let start = turns.len().saturating_sub(limit);
    turns[start..].iter().map(|t| format!("{}: {}", if t.role == "user" { "사용자" } else { "고양이" }, t.text)).collect::<Vec<_>>().join("\n")
}

impl Ai {
    pub fn new(settings: Store) -> Ai {
        Ai { settings, last_error: Mutex::new(None) }
    }

    pub fn config(&self) -> Config {
        let s = self.settings.get();
        let key = crate::secret::decrypt_field(&s, "apiKeyEnc").filter(|k| !k.is_empty());
        let model = s["ai"].os("model").filter(|m| !m.is_empty()).unwrap_or_else(|| "claude-sonnet-5-5".into());
        Config { enabled: s["ai"].b("enabled") && key.is_some(), api_key: key, model, name: s.s("catName") }
    }

    pub fn set_key(&self, key: &str) {
        let enc = crate::secret::encrypt(key.trim());
        self.settings.update(|s| s["apiKeyEnc"] = json!(enc));
    }

    pub fn has_key(&self) -> bool {
        self.config().api_key.is_some()
    }

    fn set_err(&self, e: Option<String>) {
        *self.last_error.lock().unwrap() = e;
    }

    pub fn last_error(&self) -> Option<String> {
        self.last_error.lock().unwrap().clone()
    }

    pub async fn test(&self) -> Value {
        let c = self.config();
        let Some(k) = c.api_key else { return json!({ "ok": false, "error": "API 키가 없어요." }) };
        match call_text(&k, &c.model, "짧게 답하세요.", "\"연결됐어요\"라고만 답해 주세요.", 30).await {
            Ok(t) => json!({ "ok": true, "text": t }),
            Err(e) => json!({ "ok": false, "error": e }),
        }
    }

    /// "그냥 이야기할래요": 전체 대화를 보고 길이 제한 없이 답한다 (스트리밍). 실패하면 None
    pub async fn free_chat(&self, s: &Session, on_text: &(dyn Fn(&str) + Send + Sync)) -> Option<String> {
        let c = self.config();
        let k = c.api_key?;
        let mut msgs: Vec<Value> = vec![];
        let turns: Vec<_> = s.turns.iter().filter(|t| !t.safety).collect();
        for t in turns.iter().skip(turns.len().saturating_sub(200)) {
            let role = if t.role == "user" { "user" } else { "assistant" };
            match msgs.last_mut() {
                Some(last) if last["role"] == role => {
                    let joined = format!("{}\n\n{}", last.s("content"), t.text);
                    last["content"] = json!(joined);
                }
                _ => msgs.push(json!({ "role": role, "content": t.text })),
            }
        }
        while msgs.first().map(|m| m["role"] != "user").unwrap_or(false) {
            msgs.remove(0);
        }
        if msgs.last().map(|m| m["role"] != "user").unwrap_or(true) {
            return None;
        }
        match stream(&k, &c.model, &FREE_PERSONA.replace("{name}", &c.name), &msgs, on_text).await {
            Ok(t) if !t.is_empty() => {
                self.set_err(None);
                Some(t)
            }
            Ok(_) => None,
            Err(e) => {
                self.set_err(Some(e));
                None
            }
        }
    }

    /// 단계별 의도는 그대로 두고 문장만 자연스럽게 다듬는다. 실패하면 None
    pub async fn polish_reply(&self, s: &Session, intent: &str, fallback: &str) -> Option<String> {
        let c = self.config();
        if !c.enabled {
            return None;
        }
        let user = format!("지금까지의 대화:\n{}\n\n이번에 고양이가 할 일: {intent}\n참고 문장(이 뜻을 유지하되 대화에 맞게 자연스럽게): {fallback}\n\n고양이의 다음 말만 출력하세요.", transcript(s, 12));
        match call_text(c.api_key.as_deref()?, &c.model, &PERSONA.replace("{name}", &c.name), &user, 1500).await {
            Ok(t) if !t.is_empty() => {
                self.set_err(None);
                Some(t)
            }
            Ok(_) => None,
            Err(e) => {
                self.set_err(Some(e));
                None
            }
        }
    }

    /// 규칙 기반 정리를 AI가 보강한다. 주제 id 는 정해진 목록 안에서만 받는다.
    pub async fn enrich_summary(&self, s: &Session, base: Value) -> Value {
        let c = self.config();
        if !c.enabled || base.s("mode") != "complaint" {
            return base;
        }
        let topic_list = TOPICS.iter().map(|t| format!("{}: {}", t.id, t.label)).collect::<Vec<_>>().join("\n");
        let user = format!(
            "다음 비공개 대화를 정리해 JSON 하나만 출력하세요. 다른 말은 쓰지 마세요.\n\n{}\n\n형식:\n{{\"reportedEvent\": \"사용자가 보고한 사건(사용자 관점이라는 것이 드러나게)\", \"emotions\": [\"감정\"], \"impact\": \"실제 영향\", \"needs\": \"기대·필요\", \"actionRequests\": [\"상대에게 요청할 수 있는 구체적 행동\"], \"reframes\": [{{\"from\": \"사람에 대한 평가\", \"to\": \"행동과 필요한 변화로 바꾼 표현\"}}], \"toConfirm\": [\"아직 확인되지 않은 점\"], \"topics\": [\"아래 목록의 id\"]}}\n\n규칙: 추측을 사실로 쓰지 말 것. 상대의 잘못을 확정하지 말 것. 욕설·이름은 옮기지 말 것.\n주제 목록:\n{topic_list}",
            transcript(s, 20)
        );
        let Some(k) = c.api_key else { return base };
        let raw = match call_text(&k, &c.model, &PERSONA.replace("{name}", &c.name), &user, 3000).await {
            Ok(r) => r,
            Err(e) => {
                self.set_err(Some(e));
                return base;
            }
        };
        let (Some(a), Some(b)) = (raw.find('{'), raw.rfind('}')) else { return base };
        let Ok(j) = serde_json::from_str::<Value>(&raw[a..=b]) else { return base };
        let mut m = base.clone();
        m["aiAssisted"] = json!(true);
        for k in ["reportedEvent", "impact", "needs"] {
            if let Some(v) = j.os(k).filter(|v| !v.trim().is_empty()) {
                m[k] = json!(v.trim());
            }
        }
        for k in ["emotions", "actionRequests", "toConfirm"] {
            let v: Vec<String> = j.strs(k).into_iter().take(6).collect();
            if !v.is_empty() {
                m[k] = json!(v);
            }
        }
        let rf: Vec<Value> = j.arr("reframes").into_iter().filter(|r| !r.s("from").is_empty() && !r.s("to").is_empty()).take(4).collect();
        if j.get("reframes").map(|v| v.is_array()).unwrap_or(false) {
            m["reframes"] = json!(rf);
        }
        let mut topics = base.strs("topics");
        for t in j.strs("topics") {
            if topic(&t).is_some() && !topics.contains(&t) {
                topics.push(t);
            }
        }
        m["topics"] = json!(topics);
        self.set_err(None);
        m
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// SSE 를 조각내 보내고, 첫 응답은 max_tokens 에서 끊는다 → 이어 쓰기까지 합쳐져야 한다
    #[tokio::test]
    async fn stream_joins_continuation() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        std::env::set_var("SAI_ANTHROPIC_URL", format!("http://127.0.0.1:{port}/v1/messages"));
        let server = tokio::spawn(async move {
            for round in 0..2 {
                let (mut sock, _) = listener.accept().await.unwrap();
                let mut buf = vec![0u8; 65536];
                let n = sock.read(&mut buf).await.unwrap();
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                if round == 1 {
                    assert!(req.contains("\"assistant\""), "이어 쓰기 요청에 앞부분이 있어야 해요");
                }
                let ev = |t: &str| format!("event: x\ndata: {}\n\n", serde_json::json!({ "type": "content_block_delta", "delta": { "type": "text_delta", "text": t } }));
                let (body, stop) = if round == 0 { (ev("안녕하세요, ") + &ev("긴 답의 앞부분"), "max_tokens") } else { (ev(" 이어지는 뒷부분이에요."), "end_turn") };
                let body = body + &format!("data: {}\n\n", serde_json::json!({ "type": "message_delta", "delta": { "stop_reason": stop } }));
                let head = format!("HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n", body.len());
                sock.write_all(head.as_bytes()).await.unwrap();
                let bytes = body.as_bytes();
                // 일부러 잘게 나눠 보낸다
                for chunk in bytes.chunks(37) {
                    sock.write_all(chunk).await.unwrap();
                    tokio::time::sleep(std::time::Duration::from_millis(2)).await;
                }
            }
        });
        let seen = std::sync::Mutex::new(0usize);
        let cb = |_: &str| *seen.lock().unwrap() += 1;
        let out = stream("k", "m", "s", &[json!({ "role": "user", "content": "hi" })], &cb).await.unwrap();
        server.await.unwrap();
        assert_eq!(out, "안녕하세요, 긴 답의 앞부분 이어지는 뒷부분이에요.");
        assert!(*seen.lock().unwrap() >= 3);
    }
}
