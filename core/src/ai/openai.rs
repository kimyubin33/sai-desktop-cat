//! 프로젝트 업무 배분용 OpenAI Responses API. 키는 화면과 팀 서버에 돌려주지 않는다.
use super::{call_text, Ai};
use crate::http::{client, json_body};
use crate::team::Team;
use crate::util::J;
use crate::{err, Result};
use serde_json::{json, Value};
use std::time::Duration;

const URL: &str = "https://api.openai.com/v1/responses";
const DEFAULT_MODEL: &str = "gpt-5.6-terra";

impl Ai {
    pub fn openai_model(&self) -> String {
        self.settings.read(|s| s.os("openAiModel").filter(|m| !m.is_empty()).unwrap_or_else(|| DEFAULT_MODEL.into()))
    }

    fn openai_key(&self) -> Option<String> {
        #[cfg(windows)]
        {
            self.settings.read(|s| crate::secret::decrypt_field(s, "openAiKeyEnc")).filter(|k| !k.is_empty())
        }
        #[cfg(not(windows))]
        {
            self.openai_session_key.lock().unwrap().clone()
        }
    }

    pub fn has_openai_key(&self) -> bool {
        self.openai_key().is_some()
    }

    /// 생략한 항목은 그대로 두고 빈 키는 삭제한다. 설정 오류가 있으면 기존 연결을 유지한다.
    pub fn configure_openai(&self, key: Option<&str>, model: Option<&str>) -> Result<()> {
        let model = model.map(str::trim);
        if let Some(m) = model {
            if m.is_empty() || m.len() > 120 || m.starts_with("sk-") || !m.bytes().all(|b| b.is_ascii_alphanumeric() || b".-_:/".contains(&b)) {
                return err("GPT 모델 이름을 확인해 주세요.");
            }
        }
        let key = key.map(str::trim);
        if let Some(k) = key {
            if !k.is_empty() && (!k.starts_with("sk-") || k.starts_with("sk-ant-") || k.len() <= 3 || k.len() > 2048 || !k.bytes().all(|b| b.is_ascii_graphic())) {
                return err("OpenAI API 키를 확인해 주세요.");
            }
        }
        #[cfg(windows)]
        let encrypted = match key {
            Some(k) if !k.is_empty() => Some(crate::secret::encrypt(k).filter(|s| s.starts_with("dpapi:"))
                .ok_or_else(|| "Windows에서 GPT 키를 보호하지 못했어요. 기존 설정은 유지했어요.".to_string())?),
            _ => None,
        };
        self.settings.update(|s| {
            if let Some(model) = model {
                s["openAiModel"] = json!(model);
            }
            #[cfg(windows)]
            if key.is_some() {
                s["openAiKeyEnc"] = json!(encrypted);
            }
            #[cfg(not(windows))]
            if let Some(k) = key {
                *self.openai_session_key.lock().unwrap() = (!k.is_empty()).then(|| k.to_owned());
            }
        });
        Ok(())
    }

    pub async fn test_openai(&self) -> Value {
        match self.openai_text("짧게 답하세요.", "연결됐어요 라고만 답해 주세요.", 1024).await {
            Ok(_) => json!({ "ok": true, "text": "GPT에 연결됐어요." }),
            Err(e) => json!({ "ok": false, "error": e }),
        }
    }

    async fn openai_text(&self, system: &str, input: &str, max_tokens: u32) -> Result<String> {
        let key = self.openai_key().ok_or_else(|| "설정에서 GPT API 키를 연결해 주세요.".to_string())?;
        responses(URL, &key, &self.openai_model(), system, input, max_tokens).await
    }

    pub fn has_project_claude_key(&self, team: &Team) -> bool {
        self.own_key().is_some() || team.project_claude_key().is_some()
    }

    /// 프로젝트에서 명시적으로 누른 계산에는 선택한 제공자의 키만 사용한다.
    pub async fn project_text(&self, team: &Team, provider: &str, system: &str, input: &str, max_tokens: u32) -> Result<String> {
        match provider {
            "claude" => {
                let key = self.own_key().or_else(|| team.project_claude_key()).ok_or_else(|| "설정에서 Claude API 키를 연결하거나 이 팀의 공유 키를 사용해 주세요.".to_string())?;
                call_text(&key, &self.claude_model(), system, input, max_tokens).await
            }
            "openai" => self.openai_text(system, input, max_tokens).await,
            _ => err("업무를 계산할 AI를 선택해 주세요."),
        }
    }
}

async fn responses(url: &str, key: &str, model: &str, instructions: &str, input: &str, max_tokens: u32) -> Result<String> {
    let res = client().post(url).bearer_auth(key).timeout(Duration::from_secs(120))
        .json(&json!({ "model": model, "instructions": instructions, "input": input, "max_output_tokens": max_tokens, "store": false }))
        .send().await.map_err(|e| if e.is_timeout() { "GPT 응답 시간이 초과됐어요." } else { "OpenAI에 연결하지 못했어요." }.to_string())?;
    let (status, data) = json_body(res).await;
    if !(200..300).contains(&status) {
        // 공급자의 원문 오류는 입력 내용이나 키 일부를 포함할 수 있다.
        let message = match status {
            401 | 403 => "API 키와 프로젝트 권한을 확인해 주세요.",
            404 => "모델 이름과 해당 모델 사용 권한을 확인해 주세요.",
            429 if data["error"]["code"] == "insufficient_quota" => "OpenAI 프로젝트의 결제 설정과 사용 한도를 확인해 주세요.",
            429 => "요청이 많아요. 잠시 뒤 다시 시도해 주세요.",
            400 => "모델 이름과 입력 크기를 확인해 주세요.",
            _ => "잠시 뒤 다시 시도해 주세요.",
        };
        return err(format!("OpenAI {status} · {message}"));
    }
    response_text(&data)
}

fn response_text(data: &Value) -> Result<String> {
    match data["status"].as_str() {
        Some("completed") => {}
        Some("incomplete") if data["incomplete_details"]["reason"] == "max_output_tokens" => return err("GPT 응답이 출력 한도에서 끊겼어요. 기획서를 나눠 다시 시도해 주세요."),
        _ => return err("GPT가 응답을 완료하지 못했어요."),
    }
    let mut text = String::new();
    for item in data.arr("output") {
        if item["type"] != "message" || item["role"] != "assistant" {
            continue;
        }
        for part in item.arr("content") {
            match part["type"].as_str() {
                Some("output_text") => text.push_str(part["text"].as_str().unwrap_or("")),
                Some("refusal") => return err("GPT가 이 요청에 대한 답변을 제공하지 않았어요."),
                _ => {}
            }
        }
    }
    if text.trim().is_empty() { return err("GPT가 빈 응답을 보냈어요."); }
    Ok(text.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Stores;
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

    #[test]
    fn openai_settings_preserve_other_keys_and_never_return_plaintext_to_storage() {
        let stores = Stores::memory();
        let ai = Ai::new(stores.settings.clone());
        ai.set_shared(Some("shared-test-key".into()));
        assert_eq!(ai.openai_model(), DEFAULT_MODEL);
        ai.configure_openai(Some("sk-unit-private-test"), Some("my-gpt-model")).unwrap();
        assert_eq!(ai.openai_key().as_deref(), Some("sk-unit-private-test"));
        assert!(!stores.settings.get().to_string().contains("sk-unit-private-test"));
        ai.configure_openai(None, Some("second-model")).unwrap();
        assert!(ai.configure_openai(Some("sk-unit-new-test"), Some("sk-test-key")).is_err());
        assert!(ai.configure_openai(Some("wrong key"), None).is_err());
        assert_eq!(ai.openai_model(), "second-model");
        assert_eq!(ai.openai_key().as_deref(), Some("sk-unit-private-test"));
        assert_eq!(ai.config().api_key.as_deref(), Some("shared-test-key"));
        let restarted = Ai::new(stores.settings);
        assert_eq!(restarted.has_openai_key(), cfg!(windows));
        ai.configure_openai(Some(""), None).unwrap();
        assert!(!ai.has_openai_key());
    }

    #[tokio::test]
    async fn responses_posts_private_input_and_extracts_only_assistant_text() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/v1/responses", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut reader = BufReader::new(&mut sock);
            let mut header = String::new();
            loop {
                let mut line = String::new();
                assert!(reader.read_line(&mut line).await.unwrap() > 0);
                if line == "\r\n" { break; }
                header.push_str(&line.to_ascii_lowercase());
            }
            assert!(header.starts_with("post /v1/responses http/1.1"));
            assert!(header.lines().any(|l| l == "authorization: bearer unit-test-key"));
            assert!(!header.contains("x-api-key:"));
            let len = header.lines().find_map(|l| l.strip_prefix("content-length: ")).unwrap().parse().unwrap();
            let mut bytes = vec![0; len];
            reader.read_exact(&mut bytes).await.unwrap();
            let body: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(body, json!({"model":"test-model","instructions":"instructions","input":"팀 기획서","store":false,"max_output_tokens":4000}));
            let response = json!({"status":"completed","output":[
                {"type":"reasoning","summary":[]},
                {"type":"message","role":"assistant","content":[{"type":"output_text","text":"업무 "},{"type":"output_text","text":"배분"}]}
            ]}).to_string();
            let packet = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{response}", response.len());
            sock.write_all(packet.as_bytes()).await.unwrap();
        });
        let result = responses(&url, "unit-test-key", "test-model", "instructions", "팀 기획서", 4000).await;
        server.await.unwrap();
        assert_eq!(result.unwrap(), "업무 배분");
    }

    #[test]
    fn incomplete_refused_and_empty_responses_are_not_plans() {
        assert!(response_text(&json!({"status":"incomplete","incomplete_details":{"reason":"max_output_tokens"}})).is_err());
        assert!(response_text(&json!({"status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"refusal","refusal":"Sorry"}]}]})).is_err());
        assert!(response_text(&json!({"status":"completed","output":[]})).is_err());
    }
}
