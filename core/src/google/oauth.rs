//! Google 데스크톱 OAuth. 고정된 Google HTTPS 응답과 UserInfo로 신원을 확인한다.
pub(super) mod agenda;
mod identity;
pub(super) mod media;
use crate::{err, http::form, store::Store, util::J, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD as B64URL, Engine};
pub use identity::Identity;
use rand::RngCore;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Mutex,
};
use std::time::Duration;

const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const USERINFO_URL: &str = "https://openidconnect.googleapis.com/v1/userinfo";
pub const APPDATA_SCOPE: &str = "https://www.googleapis.com/auth/drive.appdata";
pub const FILE_SCOPE: &str = "https://www.googleapis.com/auth/drive.file";
pub const DRIVE_SCOPE: &str = "https://www.googleapis.com/auth/drive";
pub const CALENDAR_SCOPE: &str = "https://www.googleapis.com/auth/calendar.events.owned";
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Purpose {
    Identity,
    Files,
    Folder,
    SelectedFiles,
    Calendar,
}
impl Purpose {
    pub fn picker(self) -> bool {
        matches!(self, Self::Folder | Self::SelectedFiles)
    }
}
// 기본 로그인은 신원 + 앱 전용 숨김 폴더(drive.appdata)만 요청한다.
// Drive 전체(팀 폴더)·선택 파일·Calendar 권한은 사용자가 그 기능을 켤 때 별도 동의 흐름으로 요청한다.
pub const SCOPES: &[&str] = &["openid", "email", APPDATA_SCOPE];

pub fn random_token() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    B64URL.encode(bytes)
}
pub fn pkce() -> (String, String) {
    let verifier = random_token();
    let challenge = B64URL.encode(Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}
pub fn auth_url(
    client_id: &str,
    redirect: &str,
    challenge: &str,
    state: &str,
    nonce: &str,
) -> String {
    auth_url_for(
        client_id,
        redirect,
        challenge,
        state,
        nonce,
        Purpose::Identity,
        None,
    )
}
pub(super) fn auth_url_for(
    client_id: &str,
    redirect: &str,
    challenge: &str,
    state: &str,
    nonce: &str,
    purpose: Purpose,
    hint: Option<&str>,
) -> String {
    let scopes = match purpose {
        Purpose::Identity => SCOPES.to_vec(),
        Purpose::Files => [SCOPES, &[DRIVE_SCOPE]].concat(),
        Purpose::Calendar => vec!["openid", "email", CALENDAR_SCOPE],
        Purpose::Folder | Purpose::SelectedFiles => vec![FILE_SCOPE],
    };
    let scope = scopes.join(" ");
    let mut params = vec![
        ("client_id", client_id),
        ("redirect_uri", redirect),
        ("response_type", "code"),
        ("scope", scope.as_str()),
        ("code_challenge", challenge),
        ("code_challenge_method", "S256"),
        ("access_type", "offline"),
        (
            "prompt",
            if purpose.picker() {
                "consent"
            } else {
                "consent select_account"
            },
        ),
        ("state", state),
    ];
    if !purpose.picker() {
        params.push(("nonce", nonce));
    }
    if let Some(hint) = hint {
        params.push(("login_hint", hint));
    }
    if purpose == Purpose::Calendar {
        params.push(("include_granted_scopes", "false"));
    }
    if purpose.picker() {
        params.extend([
            ("trigger_onepick", "true"),
            (
                "allow_multiple",
                if purpose == Purpose::Folder {
                    "false"
                } else {
                    "true"
                },
            ),
            (
                "allow_folder_selection",
                if purpose == Purpose::Folder {
                    "true"
                } else {
                    "false"
                },
            ),
            ("include_granted_scopes", "false"),
        ]);
        if purpose == Purpose::Folder {
            params.push(("mimetypes", "application/vnd.google-apps.folder"));
        }
    }
    format!("{AUTH_URL}?{}", form(&params))
}
pub fn parse_google_client(text: &str) -> Option<Value> {
    let j: Value = serde_json::from_str(text).ok()?;
    let c = if j.get("installed").is_some() {
        &j["installed"]
    } else {
        &j
    };
    let id = c.os("clientId").or_else(|| c.os("client_id"))?;
    let secret = c.os("clientSecret").or_else(|| c.os("client_secret"))?;
    if !id.ends_with(".apps.googleusercontent.com") || secret.is_empty() {
        return None;
    }
    Some(json!({"clientId":id,"clientSecret":secret}))
}
pub fn account_scopes(scope: &str) -> bool {
    let scopes: Vec<_> = scope.split_whitespace().collect();
    scopes.contains(&"openid")
        && scopes.contains(&APPDATA_SCOPE)
        && (scopes.contains(&"email")
            || scopes.contains(&"https://www.googleapis.com/auth/userinfo.email"))
}
struct Transport {
    http: reqwest::Client,
    token: String,
    userinfo: String,
    api: String,
}
impl Transport {
    fn google() -> Self {
        let http = reqwest::Client::builder()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(30))
            .user_agent(concat!("sai-desktop-cat/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("Google OAuth client");
        Self {
            http,
            token: TOKEN_URL.into(),
            userinfo: USERINFO_URL.into(),
            api: "https://www.googleapis.com".into(),
        }
    }
    async fn body(mut response: reqwest::Response) -> Result<(u16, Value)> {
        let status = response.status().as_u16();
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| "Google 응답을 읽지 못했어요.")?
        {
            if bytes.len().saturating_add(chunk.len()) > 65536 {
                return err("Google 응답 크기가 올바르지 않아요.");
            }
            bytes.extend_from_slice(&chunk);
        }
        let body =
            serde_json::from_slice(&bytes).map_err(|_| "Google 응답 형식을 확인하지 못했어요.")?;
        Ok((status, body))
    }
    async fn token(&self, params: &[(&str, &str)]) -> Result<Value> {
        let response = self
            .http
            .post(&self.token)
            .header("content-type", "application/x-www-form-urlencoded")
            .body(form(params))
            .send()
            .await
            .map_err(|_| "Google에 연결하지 못했어요. 인터넷 연결을 확인해 주세요.")?;
        let (status, body) = Self::body(response).await?;
        if status == 400 && body["error"] == "invalid_grant" {
            return err("__invalid_grant");
        }
        if status != 200 {
            return err("Google 로그인을 확인하지 못했어요. 잠시 후 다시 시도해 주세요.");
        }
        Ok(body)
    }
    async fn userinfo(&self, access: &str) -> Result<Value> {
        let response = self
            .http
            .get(&self.userinfo)
            .bearer_auth(access)
            .send()
            .await
            .map_err(|_| "Google 계정을 확인하지 못했어요. 인터넷 연결을 확인해 주세요.")?;
        let (status, body) = Self::body(response).await?;
        if status != 200 {
            return err("Google 계정 확인이 만료됐거나 권한이 부족해요. 다시 로그인해 주세요.");
        }
        Ok(body)
    }
}
#[derive(Clone)]
struct Access {
    token: String,
    expires: i64,
    identity: Identity,
    epoch: u64,
}
/// Captured verified account and token. Never reselect an account during a Drive operation.
#[derive(Clone)]
pub(super) struct Lease {
    access: Access,
}
impl Lease {
    pub fn email(&self) -> &str {
        &self.access.identity.email
    }
    pub fn subject(&self) -> &str {
        &self.access.identity.subject
    }
    pub fn epoch(&self) -> u64 {
        self.access.epoch
    }
}
pub struct GoogleAuth {
    pub client_id: String,
    client_secret: String,
    settings: Store,
    transport: Transport,
    access: Mutex<Option<Access>>,
    drive_access: Mutex<Option<Access>>,
    drive_reauth: AtomicBool,
    calendar_access: Mutex<Option<Access>>,
    calendar_reauth: AtomicBool,
    operation: tokio::sync::Mutex<()>,
    reauth: AtomicBool,
    epoch: AtomicU64,
}
impl GoogleAuth {
    pub(super) fn team_ai_consented(&self, lease: &Lease, endpoint: &str) -> bool {
        let key = format!(
            "{:x}",
            Sha256::digest(format!("team-ai-v1\n{}\n{endpoint}", lease.subject()).as_bytes())
        );
        self.settings.read(|s| s["teamAiConsents"][key] == true)
    }
    pub(super) async fn set_team_ai_consent(
        &self,
        lease: &Lease,
        endpoint: &str,
        allowed: bool,
    ) -> Result<()> {
        let key = format!(
            "{:x}",
            Sha256::digest(format!("team-ai-v1\n{}\n{endpoint}", lease.subject()).as_bytes())
        );
        self.with_lease(lease, || {
            self.settings
                .commit(|s| {
                    if !s["teamAiConsents"].is_object() {
                        s["teamAiConsents"] = json!({});
                    }
                    s["teamAiConsents"][&key] = json!(allowed);
                })
                .map_err(|_| "팀 AI 연결 동의를 이 PC에 저장하지 못했어요.".into())
        })
        .await
    }
    pub(super) async fn team_ai_request(
        &self,
        lease: &Lease,
        endpoint: &str,
        path: &str,
        body: Value,
    ) -> Result<Value> {
        super::team_ai::Client::new(Some(endpoint))?;
        if !self.team_ai_consented(lease, endpoint) {
            return err("팀 AI 연결의 권한 전달 범위를 먼저 확인해 주세요.");
        }
        if self.tokens().is_some_and(|t| {
            t.s("scope")
                .split_whitespace()
                .any(|scope| scope.starts_with("https://www.googleapis.com/auth/calendar"))
        }) {
            return err("Google 기본 연결에 Calendar 권한이 함께 포함돼 있어 팀 AI로 전달하지 않았어요. Drive 연결을 갱신해 주세요.");
        }
        super::team_ai::request(
            self,
            lease,
            &self.transport.http,
            &lease.access.token,
            endpoint,
            path,
            body,
        )
        .await
    }
    pub fn new(client: &Value, settings: Store) -> Self {
        Self {
            client_id: client.s("clientId"),
            client_secret: client.s("clientSecret"),
            settings,
            transport: Transport::google(),
            access: Mutex::new(None),
            drive_access: Mutex::new(None),
            drive_reauth: AtomicBool::new(false),
            calendar_access: Mutex::new(None),
            calendar_reauth: AtomicBool::new(false),
            operation: tokio::sync::Mutex::new(()),
            reauth: AtomicBool::new(false),
            epoch: AtomicU64::new(1),
        }
    }
    pub fn tokens(&self) -> Option<Value> {
        self.settings
            .read(|s| crate::secret::decrypt_json_field(s, "googleTokensEnc"))
    }
    pub fn has_credentials(&self) -> bool {
        self.settings.read(|s| !s["googleTokensEnc"].is_null())
    }
    pub fn has_session(&self) -> bool {
        !self.reauth.load(Ordering::SeqCst) && self.stored_session().is_some()
    }
    fn stored_session(&self) -> Option<Value> {
        #[cfg(windows)]
        if !self.settings.read(|s| {
            s["googleTokensEnc"]
                .as_str()
                .is_some_and(|s| s.starts_with("dpapi:"))
        }) {
            return None;
        }
        let t = self.tokens()?;
        (t["version"] == 2
            && t["clientId"] == self.client_id
            && identity::valid_subject(&t.s("subject"))
            && !t.s("refreshToken").is_empty()
            && account_scopes(&t.s("scope")))
        .then_some(t)
    }
    pub fn needs_reauth(&self) -> bool {
        self.has_credentials() && !self.has_session()
    }
    pub fn active_identity(&self) -> Option<Identity> {
        self.active_context().1
    }
    pub(super) fn active_context(&self) -> (u64, Option<Identity>) {
        let stored = self.stored_session();
        let access = self.access.lock().unwrap();
        let active = access.as_ref().filter(|a| {
            !self.reauth.load(Ordering::SeqCst)
                && a.expires > crate::time::now_ms() + 60000
                && stored
                    .as_ref()
                    .is_some_and(|s| s["subject"] == a.identity.subject)
        });
        match active {
            Some(a) => (a.epoch, Some(a.identity.clone())),
            None => (self.epoch(), None),
        }
    }
    pub fn is_signed_in(&self) -> bool {
        self.active_identity().is_some()
    }
    fn save_tokens(&self, value: Option<Value>) -> Result<()> {
        use base64::engine::general_purpose::STANDARD;
        use zeroize::Zeroize;
        let encoded = match value {
            None => Value::Null,
            Some(value) => {
                let mut bytes = serde_json::to_vec(&value)
                    .map_err(|_| "Google 로그인 정보를 준비하지 못했어요.")?;
                let protected = crate::secret::protect_device_secret(&bytes);
                bytes.zeroize();
                let protected = protected.map_err(|_| {
                    "Windows에서 Google 로그인 정보를 보호하지 못했어요. 기존 연결은 유지했어요."
                })?;
                json!(format!("dpapi:{}", STANDARD.encode(protected)))
            }
        };
        self.settings
            .commit(|s| {
                s["googleTokensEnc"] = encoded;
                if s["googleTokensEnc"].is_null() {
                    s["googleCalendarTokensEnc"] = Value::Null;
                }
            })
            .map_err(|_| "Google 로그인 정보를 저장하지 못했어요. 기존 연결은 유지했어요.".into())
    }
    fn access_fields(d: &Value) -> Result<(String, i64)> {
        let access = d.s("access_token");
        let expires = d["expires_in"].as_i64().filter(|n| *n > 0 && *n <= 86400);
        if access.is_empty() || d.s("token_type").to_lowercase() != "bearer" || expires.is_none() {
            return err("Google 로그인 정보의 형식이 올바르지 않아요.");
        }
        Ok((access, crate::time::now_ms() + expires.unwrap() * 1000))
    }
    pub async fn exchange_code(
        &self,
        code: &str,
        verifier: &str,
        redirect: &str,
        nonce: &str,
    ) -> Result<Identity> {
        self.exchange_identity(code, verifier, redirect, nonce, None, APPDATA_SCOPE)
            .await
    }
    pub(super) async fn exchange_identity(
        &self,
        code: &str,
        verifier: &str,
        redirect: &str,
        nonce: &str,
        expected: Option<&Lease>,
        required_scope: &str,
    ) -> Result<Identity> {
        let _guard = self.operation.lock().await;
        if expected.is_some_and(|e| !self.lease_current(e)) {
            return err("계정 연결이 바뀌었어요. 다시 시도해 주세요.");
        }
        let d = self
            .transport
            .token(&[
                ("code", code),
                ("client_id", &self.client_id),
                ("client_secret", &self.client_secret),
                ("redirect_uri", redirect),
                ("grant_type", "authorization_code"),
                ("code_verifier", verifier),
            ])
            .await
            .map_err(|e| {
                if e == "__invalid_grant" {
                    "로그인 코드가 만료됐어요. 다시 시도해 주세요.".into()
                } else {
                    e
                }
            })?;
        let (token, expires) = Self::access_fields(&d)?;
        if d.s("refresh_token").is_empty() || !account_scopes(&d.s("scope")) {
            return err(
                "Google 로그인과 앱 설정 보관 권한을 허용해 주세요. 기존 연결은 유지했어요.",
            );
        }
        if !d.s("scope").split_whitespace().any(|s| s == required_scope) {
            return err("Google Drive 접근 권한을 함께 허용해 주세요. 기존 연결은 유지했어요.");
        }
        let sub = identity::claims_from_google_response(
            &d.s("id_token"),
            &self.client_id,
            Some(nonce),
            &token,
            crate::time::now_ms() / 1000,
        )?;
        let info = self.transport.userinfo(&token).await?;
        let identity = identity::user_identity(&info, &sub)?;
        if expected.is_some_and(|e| e.subject() != identity.subject) {
            return err(
                "앱에 연결된 Google 계정으로 Drive 권한을 허용해 주세요. 기존 연결은 유지했어요.",
            );
        }
        self.save_tokens(Some(json!({"version":2,"clientId":self.client_id,"subject":identity.subject,"email":identity.email,
            "refreshToken":d.s("refresh_token"),"scope":d.s("scope")})))?;
        *self.access.lock().unwrap() = Some(Access {
            token,
            expires,
            identity: identity.clone(),
            epoch: self.epoch.fetch_add(1, Ordering::SeqCst) + 1,
        });
        self.settings.set_overlay(None);
        *self.drive_access.lock().unwrap() = None;
        *self.calendar_access.lock().unwrap() = None;
        self.calendar_reauth.store(false, Ordering::SeqCst);
        self.drive_reauth.store(false, Ordering::SeqCst);
        self.reauth.store(false, Ordering::SeqCst);
        Ok(identity)
    }
    pub(super) async fn finish_picker(
        &self,
        lease: &Lease,
        code: &str,
        verifier: &str,
        redirect: &str,
    ) -> Result<()> {
        let _guard = self.operation.lock().await;
        if !self.lease_current(lease) {
            return err("계정 연결이 바뀌었어요.");
        }
        let d = self
            .transport
            .token(&[
                ("code", code),
                ("client_id", &self.client_id),
                ("client_secret", &self.client_secret),
                ("redirect_uri", redirect),
                ("grant_type", "authorization_code"),
                ("code_verifier", verifier),
            ])
            .await
            .map_err(|_| "폴더 선택을 확인하지 못했어요. 다시 시도해 주세요.")?;
        let (token, expires) = Self::access_fields(&d)?;
        if !d.s("scope").split_whitespace().any(|s| s == FILE_SCOPE) {
            return err("선택한 폴더의 앱 접근을 허용해 주세요.");
        }
        if d.s("refresh_token").is_empty() {
            return err("폴더 연결을 이어 쓰려면 Google 접근을 다시 허용해 주세요.");
        }
        // Picker allows drive.file only, hence no OIDC identity. Bind its grant by comparing
        // Google's immutable Drive permission IDs against the already verified app session.
        let base_id = self.drive_identity(&lease.access.token).await?;
        let picked_id = self.drive_identity(&token).await?;
        if base_id != picked_id {
            return err(
                "앱에 연결된 Google 계정으로 폴더를 선택해 주세요. 기존 연결은 유지했어요.",
            );
        }
        let mut stored = self
            .stored_session()
            .ok_or("Google 계정을 다시 확인해 주세요.")?;
        stored["driveRefreshToken"] = json!(d.s("refresh_token"));
        stored["drivePermissionId"] = json!(base_id);
        self.save_tokens(Some(stored))?;
        *self.drive_access.lock().unwrap() = Some(Access {
            token,
            expires,
            identity: lease.access.identity.clone(),
            epoch: lease.epoch(),
        });
        self.drive_reauth.store(false, Ordering::SeqCst);
        Ok(())
    }
    pub fn has_file_scope(&self) -> bool {
        self.has_full_drive()
            || !self.drive_reauth.load(Ordering::SeqCst)
                && self
                    .stored_session()
                    .is_some_and(|s| s.s("scope").split_whitespace().any(|s| s == FILE_SCOPE))
    }
    pub fn has_full_drive(&self) -> bool {
        self.stored_session()
            .is_some_and(|s| s.s("scope").split_whitespace().any(|s| s == DRIVE_SCOPE))
    }
    pub(super) async fn full_drive_lease(&self, epoch: u64) -> Result<Lease> {
        let lease = self.lease().await?;
        if lease.epoch() != epoch {
            return err("계정 연결이 바뀌었어요. 현재 계정에서 다시 시도해 주세요.");
        }
        if !self.has_full_drive() {
            return err("처음 한 번 Google 연결을 갱신해 Drive 전체 접근을 허용해 주세요. 이후에는 저장된 연결을 사용해요.");
        }
        Ok(lease)
    }
    async fn drive_identity(&self, token: &str) -> Result<String> {
        let response = self
            .transport
            .http
            .get(format!(
                "{}/drive/v3/about?fields=user(permissionId)",
                self.transport.api
            ))
            .bearer_auth(token)
            .send()
            .await
            .map_err(|_| "Drive 계정을 확인하지 못했어요.")?;
        let (status, value) = Transport::body(response).await?;
        if status != 200 {
            return err("Drive 계정을 확인하지 못했어요. 기존 연결은 유지했어요.");
        }
        value["user"]["permissionId"]
            .as_str()
            .filter(|s| super::profile::model::id(s))
            .map(str::to_owned)
            .ok_or_else(|| "Drive 계정의 고유 정보를 확인하지 못했어요.".into())
    }
    pub(super) async fn file_lease(&self) -> Result<Lease> {
        let base = self.session().await?;
        let _guard = self.operation.lock().await;
        let lease = Lease {
            access: base.clone(),
        };
        if !self.lease_current(&lease) || !self.has_file_scope() {
            return err("Drive 연결을 다시 허용해 주세요.");
        }
        let stored = self
            .stored_session()
            .ok_or("Google 계정을 다시 확인해 주세요.")?;
        if self.has_full_drive() {
            return Ok(lease);
        }
        let Some(refresh) = stored["driveRefreshToken"].as_str() else {
            return Ok(lease);
        };
        if let Some(access) = self
            .drive_access
            .lock()
            .unwrap()
            .as_ref()
            .filter(|a| {
                a.epoch == base.epoch
                    && a.identity.subject == base.identity.subject
                    && a.expires > crate::time::now_ms() + 60000
            })
            .cloned()
        {
            return Ok(Lease { access });
        }
        let d = match self
            .transport
            .token(&[
                ("client_id", &self.client_id),
                ("client_secret", &self.client_secret),
                ("refresh_token", refresh),
                ("grant_type", "refresh_token"),
            ])
            .await
        {
            Ok(d) => d,
            Err(e) if e == "__invalid_grant" => {
                self.drive_reauth.store(true, Ordering::SeqCst);
                return err("Drive 연결이 만료됐어요. 팀 Drive 연결을 다시 허용해 주세요.");
            }
            Err(e) => return Err(e),
        };
        let (token, expires) = Self::access_fields(&d)?;
        if d.get("scope").is_some() && !d.s("scope").split_whitespace().any(|s| s == FILE_SCOPE) {
            self.drive_reauth.store(true, Ordering::SeqCst);
            return err("Drive 자료 접근 권한을 다시 허용해 주세요.");
        }
        if self.drive_identity(&token).await? != stored.s("drivePermissionId") {
            self.drive_reauth.store(true, Ordering::SeqCst);
            return err("Drive 계정이 달라요. 기존 연결은 유지했어요.");
        }
        if let Some(refresh) = d["refresh_token"].as_str() {
            if refresh.is_empty() {
                return err("Drive 연결 정보를 확인하지 못했어요.");
            }
            let mut next = stored.clone();
            next["driveRefreshToken"] = json!(refresh);
            self.save_tokens(Some(next))?;
        }
        let access = Access {
            token,
            expires,
            identity: base.identity,
            epoch: base.epoch,
        };
        *self.drive_access.lock().unwrap() = Some(access.clone());
        Ok(Lease { access })
    }
    async fn session(&self) -> Result<Access> {
        let _guard = self.operation.lock().await;
        if !self.has_session() {
            return err("Google 계정 연결을 다시 확인해 주세요. Google로 로그인하면 이어서 사용할 수 있어요.");
        }
        let stored = self
            .stored_session()
            .ok_or("Google 계정에 다시 로그인해 주세요.")?;
        if let Some(access) = self
            .access
            .lock()
            .unwrap()
            .as_ref()
            .filter(|a| {
                a.expires > crate::time::now_ms() + 60000 && stored["subject"] == a.identity.subject
            })
            .cloned()
        {
            return Ok(access);
        }
        let response = self
            .transport
            .token(&[
                ("client_id", &self.client_id),
                ("client_secret", &self.client_secret),
                ("refresh_token", &stored.s("refreshToken")),
                ("grant_type", "refresh_token"),
            ])
            .await;
        let d = match response {
            Ok(d) => d,
            Err(error) if error == "__invalid_grant" => {
                self.reauth.store(true, Ordering::SeqCst);
                *self.access.lock().unwrap() = None;
                return err("Google 로그인이 만료됐어요. Google로 다시 로그인해 주세요.");
            }
            Err(error) => return Err(error),
        };
        let (token, expires) = Self::access_fields(&d)?;
        let scope = d.os("scope").unwrap_or_else(|| stored.s("scope"));
        if !account_scopes(&scope) {
            self.reauth.store(true, Ordering::SeqCst);
            return err("Google 앱 설정 보관 권한을 다시 허용해 주세요.");
        }
        if let Some(id_token) = d.os("id_token") {
            let sub = identity::claims_from_google_response(
                &id_token,
                &self.client_id,
                None,
                &token,
                crate::time::now_ms() / 1000,
            )?;
            if sub != stored.s("subject") {
                self.reauth.store(true, Ordering::SeqCst);
                return err("연결된 Google 계정 정보가 달라요. 다시 로그인해 주세요.");
            }
        }
        let info = self.transport.userinfo(&token).await?;
        let identity = match identity::user_identity(&info, &stored.s("subject")) {
            Ok(identity) => identity,
            Err(error) => {
                *self.access.lock().unwrap() = None;
                self.reauth.store(true, Ordering::SeqCst);
                return Err(error);
            }
        };
        let mut next = stored;
        next["email"] = json!(identity.email);
        next["scope"] = json!(scope);
        if let Some(refresh) = d.os("refresh_token").filter(|s| !s.is_empty()) {
            next["refreshToken"] = json!(refresh);
        }
        self.save_tokens(Some(next))?;
        let access = Access {
            token,
            expires,
            identity,
            epoch: self.epoch.load(Ordering::SeqCst),
        };
        *self.access.lock().unwrap() = Some(access.clone());
        Ok(access)
    }
    pub async fn access_token(&self) -> Result<String> {
        Ok(self.session().await?.token)
    }
    pub async fn verified_identity(&self) -> Result<Identity> {
        Ok(self.session().await?.identity)
    }
    pub fn epoch(&self) -> u64 {
        self.epoch.load(Ordering::SeqCst)
    }
    pub(super) async fn lease(&self) -> Result<Lease> {
        Ok(Lease {
            access: self.session().await?,
        })
    }
    pub(super) fn lease_current(&self, lease: &Lease) -> bool {
        self.epoch() == lease.epoch()
            && self.has_session()
            && self
                .stored_session()
                .is_some_and(|s| s["subject"] == lease.subject())
    }
    pub(super) async fn with_lease<T>(
        &self,
        lease: &Lease,
        f: impl FnOnce() -> Result<T>,
    ) -> Result<T> {
        let _guard = self.operation.lock().await;
        if !self.lease_current(lease) {
            return err("계정 연결이 바뀌었어요. 현재 계정에서 다시 시도해 주세요.");
        }
        f()
    }
    pub(super) async fn profile_request(
        &self,
        lease: &Lease,
        method: &str,
        path: &str,
        body: Option<(String, Vec<u8>)>,
    ) -> std::result::Result<Value, (u16, String)> {
        if !self.lease_current(lease) {
            return Err((0, "계정 연결이 바뀌었어요.".into()));
        }
        // All paths are constructed internally. A remote file can never supply a destination URL.
        let url = format!("{}{path}", self.transport.api);
        let method = reqwest::Method::from_bytes(method.as_bytes())
            .map_err(|_| (0, "요청 방식이 올바르지 않아요.".into()))?;
        let mut request = self
            .transport
            .http
            .request(method, &url)
            .bearer_auth(&lease.access.token);
        if let Some((mime, body)) = body {
            request = request.header("content-type", mime).body(body);
        }
        let response = request.send().await.map_err(|_| {
            (
                0,
                "Google Drive에 연결하지 못했어요. 인터넷 연결을 확인해 주세요.".into(),
            )
        })?;
        let status = response.status().as_u16();
        let (_, value) = Transport::body(response).await.map_err(|e| (status, e))?;
        if !self.lease_current(lease) {
            return Err((0, "계정 연결이 바뀌었어요.".into()));
        }
        if !(200..300).contains(&status) {
            return Err((
                status,
                format!("Google Drive 동기화를 마치지 못했어요 ({status})."),
            ));
        }
        Ok(value)
    }
    pub async fn api(
        &self,
        method: &str,
        url: &str,
        body: Option<Value>,
    ) -> std::result::Result<Value, (u16, String)> {
        let parsed =
            url::Url::parse(url).map_err(|_| (0, "Google API 주소가 올바르지 않아요.".into()))?;
        if parsed.scheme() != "https"
            || !parsed
                .host_str()
                .is_some_and(|h| h == "www.googleapis.com" || h.ends_with(".googleapis.com"))
            || !parsed.username().is_empty()
            || parsed.password().is_some()
        {
            return Err((0, "허용되지 않은 Google API 주소예요.".into()));
        }
        let token = self.access_token().await.map_err(|e| (0, e))?;
        let method = reqwest::Method::from_bytes(method.as_bytes())
            .map_err(|_| (0, "요청 방식이 올바르지 않아요.".into()))?;
        let mut request = self.transport.http.request(method, url).bearer_auth(token);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request
            .send()
            .await
            .map_err(|_| (0, "Google에 연결하지 못했어요.".into()))?;
        let status = response.status().as_u16();
        if status == 204 {
            return Ok(Value::Null);
        }
        let (_, value) = Transport::body(response).await.map_err(|e| (status, e))?;
        if !(200..300).contains(&status) {
            return Err((
                status,
                format!("Google 요청을 완료하지 못했어요 ({status})."),
            ));
        }
        Ok(value)
    }
    /// 이 PC의 정보만 지운다. Google 계정 전체의 권한·다른 PC의 토큰은 취소하지 않는다.
    pub async fn sign_out(&self) -> Result<()> {
        let _guard = self.operation.lock().await;
        self.save_tokens(None)?;
        self.epoch.fetch_add(1, Ordering::SeqCst);
        self.settings.set_overlay(None);
        *self.access.lock().unwrap() = None;
        *self.drive_access.lock().unwrap() = None;
        *self.calendar_access.lock().unwrap() = None;
        self.calendar_reauth.store(false, Ordering::SeqCst);
        self.drive_reauth.store(false, Ordering::SeqCst);
        self.reauth.store(false, Ordering::SeqCst);
        Ok(())
    }
}
#[cfg(test)]
pub(super) mod tests;
