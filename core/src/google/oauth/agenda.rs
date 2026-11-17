//! Separate Calendar consent and credentials, bound to the active Google subject.
use super::*;
fn scopes(value: &str) -> bool {
    let values = value.split_whitespace().collect::<Vec<_>>();
    values.contains(&"openid")
        && (values.contains(&"email")
            || values.contains(&"https://www.googleapis.com/auth/userinfo.email"))
        && values.contains(&CALENDAR_SCOPE)
}
pub(in crate::google) fn event_revision(value: &str) -> bool {
    value.len() >= 3
        && value.len() <= 256
        && value.starts_with('"')
        && value.ends_with('"')
        && value.as_bytes()[1..value.len() - 1]
            .iter()
            .all(|b| *b == 0x21 || (0x23..=0x7e).contains(b))
}
fn event_path(path: &str, suffix: &str) -> bool {
    path.strip_prefix("/calendar/v3/calendars/primary/events/")
        .and_then(|s| s.strip_suffix(suffix))
        .is_some_and(|id| {
            id.len() >= 5
                && id.len() <= 1024
                && id
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'v').contains(&b))
        })
}
impl GoogleAuth {
    fn calendar_tokens(&self) -> Option<Value> {
        #[cfg(windows)]
        if !self.settings.read(|s| {
            s["googleCalendarTokensEnc"]
                .as_str()
                .is_some_and(|v| v.starts_with("dpapi:"))
        }) {
            return None;
        }
        let t = self
            .settings
            .read(|s| crate::secret::decrypt_json_field(s, "googleCalendarTokensEnc"))?;
        (t["version"] == 1
            && t["clientId"] == self.client_id
            && identity::valid_subject(&t.s("subject"))
            && !t.s("refreshToken").is_empty()
            && scopes(&t.s("scope")))
        .then_some(t)
    }
    pub(in crate::google) fn calendar_connected(&self) -> bool {
        self.has_session()
            && !self.calendar_reauth.load(Ordering::SeqCst)
            && self.stored_session().is_some_and(|main| {
                self.calendar_tokens()
                    .is_some_and(|cal| cal["subject"] == main["subject"])
            })
    }
    fn save_calendar(&self, value: Value) -> Result<()> {
        use base64::engine::general_purpose::STANDARD;
        use zeroize::Zeroize;
        let mut bytes =
            serde_json::to_vec(&value).map_err(|_| "Calendar 연결 정보를 준비하지 못했어요.")?;
        let protected = crate::secret::protect_device_secret(&bytes);
        bytes.zeroize();
        let protected =
            protected.map_err(|_| "Calendar 연결 정보를 Windows에서 보호하지 못했어요.")?;
        self.settings
            .commit(|s| {
                s["googleCalendarTokensEnc"] =
                    json!(format!("dpapi:{}", STANDARD.encode(protected)))
            })
            .map_err(|_| "Calendar 연결 정보를 저장하지 못했어요.".into())
    }
    pub(in crate::google) async fn finish_calendar(
        &self,
        expected: &Lease,
        code: &str,
        verifier: &str,
        redirect: &str,
        nonce: &str,
    ) -> Result<Identity> {
        let _guard = self.operation.lock().await;
        if !self.lease_current(expected) {
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
                    "Calendar 연결 코드가 만료됐어요. 다시 연결해 주세요.".into()
                } else {
                    e
                }
            })?;
        let (token, expires) = Self::access_fields(&d)?;
        if !scopes(&d.s("scope")) || d.s("refresh_token").is_empty() {
            return err("Calendar 일정 권한을 허용해 주세요. 기존 Google 로그인은 유지했어요.");
        }
        let sub = identity::claims_from_google_response(
            &d.s("id_token"),
            &self.client_id,
            Some(nonce),
            &token,
            crate::time::now_ms() / 1000,
        )?;
        let identity = identity::user_identity(&self.transport.userinfo(&token).await?, &sub)?;
        if identity.subject != expected.subject() {
            return err(
                "앱에 로그인한 Google 계정으로 Calendar를 연결해 주세요. 기존 연결은 유지했어요.",
            );
        }
        self.save_calendar(json!({"version":1,"clientId":self.client_id,"subject":identity.subject,"scope":d.s("scope"),"refreshToken":d.s("refresh_token")}))?;
        *self.calendar_access.lock().unwrap() = Some(Access {
            token,
            expires,
            identity: identity.clone(),
            epoch: expected.epoch(),
        });
        self.calendar_reauth.store(false, Ordering::SeqCst);
        Ok(identity)
    }
    pub(in crate::google) async fn calendar_lease(&self, epoch: u64) -> Result<Lease> {
        let main = self.lease().await?;
        let _guard = self.operation.lock().await;
        if epoch != main.epoch() || !self.lease_current(&main) {
            return err("계정 연결이 바뀌었어요.");
        }
        if !self.calendar_connected() {
            return err("내 Calendar 일정 권한을 먼저 연결해 주세요.");
        }
        let stored = self
            .calendar_tokens()
            .ok_or("Calendar 연결을 다시 확인해 주세요.")?;
        if let Some(access) = self
            .calendar_access
            .lock()
            .unwrap()
            .as_ref()
            .filter(|a| {
                a.epoch == epoch
                    && a.identity.subject == main.subject()
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
                ("refresh_token", &stored.s("refreshToken")),
                ("grant_type", "refresh_token"),
            ])
            .await
        {
            Ok(d) => d,
            Err(e) if e == "__invalid_grant" => {
                self.calendar_reauth.store(true, Ordering::SeqCst);
                return err("Calendar 연결이 만료됐어요. 일정 권한만 다시 연결해 주세요.");
            }
            Err(e) => return Err(e),
        };
        let (token, expires) = Self::access_fields(&d)?;
        let scope = d.os("scope").unwrap_or_else(|| stored.s("scope"));
        if !scopes(&scope) {
            self.calendar_reauth.store(true, Ordering::SeqCst);
            return err("Calendar 일정 권한을 다시 허용해 주세요.");
        }
        if let Some(id_token) = d.os("id_token") {
            let sub = identity::claims_from_google_response(
                &id_token,
                &self.client_id,
                None,
                &token,
                crate::time::now_ms() / 1000,
            )?;
            if sub != main.subject() {
                return err("Calendar 계정 정보가 달라요.");
            }
        }
        let identity =
            identity::user_identity(&self.transport.userinfo(&token).await?, main.subject())?;
        let mut next = stored;
        next["scope"] = json!(scope);
        if let Some(refresh) = d.os("refresh_token").filter(|s| !s.is_empty()) {
            next["refreshToken"] = json!(refresh);
        }
        self.save_calendar(next)?;
        let access = Access {
            token,
            expires,
            identity,
            epoch,
        };
        *self.calendar_access.lock().unwrap() = Some(access.clone());
        Ok(Lease { access })
    }
    pub(in crate::google) async fn calendar_request(
        &self,
        lease: &Lease,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> std::result::Result<Value, (u16, String)> {
        self.calendar_send(lease, method, path, body, None).await
    }
    pub(in crate::google) async fn calendar_change(
        &self,
        lease: &Lease,
        method: &str,
        path: &str,
        body: Option<Value>,
        revision: &str,
    ) -> std::result::Result<Value, (u16, String)> {
        if !event_revision(revision) || !["PATCH", "DELETE"].contains(&method) {
            return Err((0, "확인한 일정의 버전이 올바르지 않아요.".into()));
        }
        self.calendar_send(lease, method, path, body, Some(revision))
            .await
    }
    async fn calendar_send(
        &self,
        lease: &Lease,
        method: &str,
        path: &str,
        body: Option<Value>,
        revision: Option<&str>,
    ) -> std::result::Result<Value, (u16, String)> {
        let allowed = match method {
            "GET" => revision.is_none() && body.is_none() && event_path(path, ""),
            "POST" => {
                revision.is_none()
                    && body.is_some()
                    && path == "/calendar/v3/calendars/primary/events?sendUpdates=none"
            }
            "PATCH" => {
                revision.is_some() && body.is_some() && event_path(path, "?sendUpdates=none")
            }
            "DELETE" => {
                revision.is_some() && body.is_none() && event_path(path, "?sendUpdates=none")
            }
            _ => false,
        };
        if !self.lease_current(lease) || !self.calendar_connected() || !allowed {
            return Err((0, "Calendar 연결과 계정을 다시 확인해 주세요.".into()));
        }
        let mut request = self
            .transport
            .http
            .request(
                reqwest::Method::from_bytes(method.as_bytes()).unwrap(),
                format!("{}{path}", self.transport.api),
            )
            .bearer_auth(&lease.access.token);
        if let Some(body) = body {
            request = request.json(&body);
        }
        if let Some(revision) = revision {
            request = request.header(reqwest::header::IF_MATCH, revision);
        }
        let response = request.send().await.map_err(|_| {
            (
                0,
                "Calendar 요청 결과를 확인하지 못했어요. 결과를 다시 확인해 주세요.".into(),
            )
        })?;
        let status = response.status().as_u16();
        if !self.lease_current(lease) {
            return Err((0,"계정 연결이 바뀌었어요. 이미 전송한 일정은 이전 계정의 Calendar에서 확인해 주세요.".into()));
        }
        if !(200..300).contains(&status) {
            if status == 401 || status == 403 {
                self.calendar_reauth.store(true, Ordering::SeqCst);
                *self.calendar_access.lock().unwrap() = None;
            }
            return Err((
                status,
                format!(
                    "Calendar 작업을 마치지 못했어요 ({status}). 일정 권한과 연결을 확인해 주세요."
                ),
            ));
        }
        if method == "DELETE" {
            return Ok(Value::Null);
        }
        let (_, value) = Transport::body(response).await.map_err(|e| (status, e))?;
        if !self.lease_current(lease) {
            return Err((
                0,
                "계정 연결이 바뀌었어요. 현재 계정에 결과를 표시하지 않았어요.".into(),
            ));
        }
        Ok(value)
    }
}
#[cfg(test)]
mod tests;
