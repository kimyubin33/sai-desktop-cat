//! Google OAuth 2.0 — 데스크톱 앱용 (루프백 주소 + PKCE)
//! https://developers.google.com/identity/protocols/oauth2/native-app
use crate::http::{client, form, json_body};
use crate::store::Store;
use crate::util::J;
use crate::{err, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD as B64URL, Engine};
use rand::RngCore;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::Mutex;

const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const REVOKE_URL: &str = "https://oauth2.googleapis.com/revoke";

/// 필요한 만큼만: 바쁜 시간(제목·내용은 못 봄), 사이가 만든 일정 추가, 할 일, 계정 이메일 표시
pub const SCOPES: &[&str] = &[
    "openid",
    "email",
    "https://www.googleapis.com/auth/calendar.freebusy",
    "https://www.googleapis.com/auth/calendar.events",
    "https://www.googleapis.com/auth/tasks",
    // 사이 전용 숨김 폴더 (다른 컴퓨터에서 내 팀을 되찾기), 사이로 올린 파일만
    "https://www.googleapis.com/auth/drive.appdata",
    "https://www.googleapis.com/auth/drive.file",
];

pub fn pkce() -> (String, String) {
    let mut b = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut b);
    let verifier = B64URL.encode(b);
    let challenge = B64URL.encode(Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

pub fn auth_url(client_id: &str, redirect_uri: &str, challenge: &str, state: &str) -> String {
    let scope = SCOPES.join(" ");
    let q = form(&[
        ("client_id", client_id),
        ("redirect_uri", redirect_uri),
        ("response_type", "code"),
        ("scope", &scope),
        ("code_challenge", challenge),
        ("code_challenge_method", "S256"),
        ("access_type", "offline"),
        ("prompt", "consent"),
        ("state", state),
    ]);
    format!("{AUTH_URL}?{q}")
}

/// id_token 의 이메일만 꺼낸다 (화면 표시용 — 서명 검증은 하지 않으므로 권한 판단에 쓰지 않는다)
pub fn email_from_id_token(id_token: &str) -> Option<String> {
    let payload = id_token.split('.').nth(1)?;
    let bytes = B64URL.decode(payload.trim_end_matches('=')).ok()?;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    v.os("email")
}

/// Google Cloud 콘솔에서 받은 클라이언트 JSON 또는 "ID / 비밀번호" 텍스트에서 값을 뽑는다
pub fn parse_google_client(text: &str) -> Option<Value> {
    if let Ok(j) = serde_json::from_str::<Value>(text) {
        let c = if j.get("installed").is_some() { &j["installed"] } else if j.get("web").is_some() { &j["web"] } else { &j };
        let id = c.os("client_id").or_else(|| c.os("clientId"));
        let secret = c.os("client_secret").or_else(|| c.os("clientSecret"));
        if let (Some(i), Some(s)) = (id, secret) {
            return Some(json!({ "clientId": i, "clientSecret": s }));
        }
    }
    let id = regex::Regex::new(r"([0-9]+-[a-z0-9]+\.apps\.googleusercontent\.com)").ok()?.captures(text)?[1].to_string();
    let secret = regex::Regex::new(r"(GOCSPX-[A-Za-z0-9_-]+)").ok()?.captures(text)?[1].to_string();
    Some(json!({ "clientId": id, "clientSecret": secret }))
}

async fn post_form(params: &[(&str, &str)]) -> Result<Value> {
    let res = client()
        .post(TOKEN_URL)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(form(params))
        .send()
        .await
        .map_err(|e| format!("Google 에 연결하지 못했어요: {e}"))?;
    let (status, d) = json_body(res).await;
    if !(200..300).contains(&status) {
        let code = d.s("error");
        let msg = d.os("error_description").unwrap_or_else(|| if code.is_empty() { status.to_string() } else { code.clone() });
        return Err(if code == "invalid_grant" { "__invalid_grant".into() } else { format!("Google 로그인 실패: {msg}") });
    }
    Ok(d)
}

/// 로그인 정보(refresh token)는 설정에 암호화해 둔다
pub struct GoogleAuth {
    pub client_id: String,
    client_secret: String,
    settings: Store,
    access: Mutex<Option<(String, i64)>>,
}

impl GoogleAuth {
    pub fn new(client: &Value, settings: Store) -> GoogleAuth {
        GoogleAuth { client_id: client.s("clientId"), client_secret: client.s("clientSecret"), settings, access: Mutex::new(None) }
    }

    pub fn tokens(&self) -> Option<Value> {
        self.settings.read(|s| crate::secret::decrypt_json_field(s, "googleTokensEnc"))
    }

    fn save_tokens(&self, t: Option<Value>) {
        let enc = t.as_ref().and_then(crate::secret::encrypt_json);
        self.settings.update(|s| s["googleTokensEnc"] = json!(enc));
    }

    pub fn is_signed_in(&self) -> bool {
        self.tokens().map(|t| !t.s("refreshToken").is_empty()).unwrap_or(false)
    }

    pub async fn exchange_code(&self, code: &str, verifier: &str, redirect_uri: &str) -> Result<Option<String>> {
        let d = post_form(&[
            ("code", code),
            ("client_id", &self.client_id),
            ("client_secret", &self.client_secret),
            ("redirect_uri", redirect_uri),
            ("grant_type", "authorization_code"),
            ("code_verifier", verifier),
        ])
        .await
        .map_err(|e| if e == "__invalid_grant" { "Google 로그인 실패: 코드가 만료됐어요. 다시 시도해 주세요.".into() } else { e })?;
        let refresh = d.s("refresh_token");
        if refresh.is_empty() {
            return err("Google 이 오래 쓰는 로그인 정보를 주지 않았어요. 다시 로그인해 주세요.");
        }
        let email = email_from_id_token(&d.s("id_token"));
        self.save_tokens(Some(json!({ "refreshToken": refresh, "email": email, "scope": d.s("scope") })));
        let exp = crate::time::now_ms() + (if d.n("expires_in") > 0.0 { d.n("expires_in") } else { 3600.0 } * 1000.0) as i64;
        *self.access.lock().unwrap() = Some((d.s("access_token"), exp));
        Ok(email)
    }

    pub async fn access_token(&self) -> Result<String> {
        if let Some((t, exp)) = self.access.lock().unwrap().clone() {
            if crate::time::now_ms() < exp - 60_000 {
                return Ok(t);
            }
        }
        let Some(t) = self.tokens().filter(|t| !t.s("refreshToken").is_empty()) else { return err("Google 에 로그인되어 있지 않아요.") };
        match post_form(&[("client_id", &self.client_id), ("client_secret", &self.client_secret), ("refresh_token", &t.s("refreshToken")), ("grant_type", "refresh_token")]).await {
            Ok(d) => {
                let exp = crate::time::now_ms() + (if d.n("expires_in") > 0.0 { d.n("expires_in") } else { 3600.0 } * 1000.0) as i64;
                *self.access.lock().unwrap() = Some((d.s("access_token"), exp));
                Ok(d.s("access_token"))
            }
            Err(e) if e == "__invalid_grant" => {
                // 테스트 상태의 앱은 약 7일 뒤 로그인이 만료된다
                self.save_tokens(None);
                err("Google 로그인이 만료됐어요. 설정 → 외부 연결에서 다시 로그인해 주세요.")
            }
            Err(e) => Err(e),
        }
    }

    /// Google API 호출. 오류는 (상태 코드, 문장)
    pub async fn api(&self, method: &str, url: &str, body: Option<Value>) -> std::result::Result<Value, (u16, String)> {
        let token = self.access_token().await.map_err(|e| (0, e))?;
        let m = reqwest::Method::from_bytes(method.as_bytes()).unwrap();
        let mut rb = client().request(m, url).bearer_auth(token);
        if let Some(b) = body {
            rb = rb.json(&b);
        }
        let res = rb.send().await.map_err(|e| (0, format!("Google 에 연결하지 못했어요: {e}")))?;
        let (status, d) = json_body(res).await;
        if status == 204 {
            return Ok(Value::Null);
        }
        if !(200..300).contains(&status) {
            let msg = d.get("error").map(|e| e.os("message").unwrap_or_else(|| e.to_string())).unwrap_or_else(|| status.to_string());
            return Err((status, format!("Google API 실패 ({status}): {msg}")));
        }
        Ok(d)
    }

    pub async fn sign_out(&self) {
        let t = self.tokens();
        self.save_tokens(None);
        *self.access.lock().unwrap() = None;
        if let Some(rt) = t.map(|t| t.s("refreshToken")).filter(|r| !r.is_empty()) {
            let _ = client().post(format!("{REVOKE_URL}?token={}", urlencoding::encode(&rt))).send().await;
        }
    }
}
