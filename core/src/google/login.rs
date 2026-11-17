//! 일시적인 루프백 로그인. 임의 요청은 무시하고 해당 로그인 시도만 취소한다.
use super::{
    oauth::{self, GoogleAuth},
    OpenUrl,
};
use crate::{err, Result};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::{watch, Mutex as AsyncMutex, MutexGuard},
};

#[derive(Default)]
pub(super) struct Login {
    gate: AsyncMutex<()>,
    cancel: Mutex<Option<watch::Sender<bool>>>,
}
enum Callback {
    Ignore,
    Denied,
    Code(String),
}
#[cfg(test)]
fn selected_folder(request: &str, host: &str) -> Result<String> {
    Ok(selected_files(request, host, false)?.remove(0))
}
fn selected_files(request: &str, host: &str, multiple: bool) -> Result<Vec<String>> {
    let path = request
        .split_whitespace()
        .nth(1)
        .ok_or("선택한 폴더를 확인하지 못했어요.")?;
    let url = url::Url::parse(&format!("http://{host}{path}"))
        .map_err(|_| "폴더 선택 응답이 올바르지 않아요.")?;
    let ids: Vec<_> = url
        .query_pairs()
        .filter(|(k, _)| k == "picked_file_ids")
        .map(|(_, v)| v.into_owned())
        .collect();
    if ids.len() != 1 {
        return err("선택한 자료의 응답을 확인하지 못했어요.");
    }
    let picked: Vec<String> = ids[0].split(',').map(str::to_owned).collect();
    let unique: std::collections::BTreeSet<_> = picked.iter().collect();
    if picked.is_empty()
        || picked.len() > if multiple { 10 } else { 1 }
        || unique.len() != picked.len()
        || !picked.iter().all(|s| super::profile::model::id(s))
    {
        return err(if multiple {
            "파일은 한 번에 10개까지 선택해 주세요."
        } else {
            "팀 폴더 하나를 선택해 주세요."
        });
    }
    Ok(picked)
}

fn callback(request: &str, state: &str, host: &str) -> Callback {
    let mut lines = request.split("\r\n");
    let line = lines
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .collect::<Vec<_>>();
    if line.len() != 3
        || line[0] != "GET"
        || !["HTTP/1.0", "HTTP/1.1"].contains(&line[2])
        || !line[1].starts_with('/')
    {
        return Callback::Ignore;
    }
    let hosts: Vec<_> = lines
        .filter_map(|line| line.split_once(':'))
        .filter(|(name, _)| name.eq_ignore_ascii_case("host"))
        .map(|(_, value)| value.trim())
        .collect();
    if hosts != [host] {
        return Callback::Ignore;
    }
    let Ok(url) = url::Url::parse(&format!("http://{host}{}", line[1])) else {
        return Callback::Ignore;
    };
    if url.path() != "/" || url.fragment().is_some() {
        return Callback::Ignore;
    }
    let mut query = HashMap::new();
    for (key, value) in url.query_pairs() {
        if query.insert(key.into_owned(), value.into_owned()).is_some() {
            return Callback::Ignore;
        }
    }
    if query.get("state").map(String::as_str) != Some(state) {
        return Callback::Ignore;
    }
    if query
        .get("iss")
        .is_some_and(|issuer| issuer != "https://accounts.google.com")
    {
        return Callback::Ignore;
    }
    if query.contains_key("error") {
        return Callback::Denied;
    }
    match query
        .get("code")
        .filter(|c| !c.is_empty() && c.len() <= 4096)
    {
        Some(code) => Callback::Code(code.clone()),
        None => Callback::Ignore,
    }
}
fn response(ok: bool, message: &str) -> String {
    let title = if ok {
        "Google 계정을 확인했어요."
    } else {
        "앱으로 돌아가 주세요."
    };
    let body = format!("<!doctype html><html lang=\"ko\"><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width\"><title>사이</title><body style=\"background:#171817;color:#eee;padding:12vh 8vw;font:16px/1.8 sans-serif\"><p>사이</p><h1 style=\"font-size:28px;font-weight:400\">{title}</h1><p>{message}</p><p style=\"color:#aaa\">이 창은 닫아도 돼요.</p></body></html>");
    format!("HTTP/1.1 {}\r\nContent-Type: text/html; charset=utf-8\r\nCache-Control: no-store\r\nContent-Security-Policy: default-src 'none'; style-src 'unsafe-inline'\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",if ok {"200 OK"} else {"400 Bad Request"},body.len())
}
async fn read_headers(socket: &mut tokio::net::TcpStream) -> Option<String> {
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut bytes = Vec::new();
        let mut chunk = [0u8; 1024];
        loop {
            let n = socket.read(&mut chunk).await.ok()?;
            if n == 0 || bytes.len() + n > 8192 {
                return None;
            }
            bytes.extend_from_slice(&chunk[..n]);
            if bytes.windows(4).any(|p| p == b"\r\n\r\n") {
                return String::from_utf8(bytes).ok();
            }
        }
    })
    .await
    .ok()
    .flatten()
}
impl Login {
    pub fn cancel(&self) {
        if let Some(sender) = &*self.cancel.lock().unwrap() {
            let _ = sender.send(true);
        }
    }
    pub fn busy(&self) -> bool {
        self.gate.try_lock().is_err()
    }
    pub async fn idle(&self) -> MutexGuard<'_, ()> {
        self.gate.lock().await
    }
    pub async fn run(&self, auth: Arc<GoogleAuth>, open: OpenUrl) -> Result<Value> {
        self.run_for(auth, open, oauth::Purpose::Identity, None)
            .await
    }
    pub async fn run_for(
        &self,
        auth: Arc<GoogleAuth>,
        open: OpenUrl,
        purpose: oauth::Purpose,
        epoch: Option<u64>,
    ) -> Result<Value> {
        let _guard = self.gate.try_lock().map_err(|_| {
            "Google 로그인을 진행하고 있어요. 브라우저에서 완료하거나 취소해 주세요."
        })?;
        let expected = if purpose == oauth::Purpose::Identity {
            None
        } else {
            Some(auth.lease().await?)
        };
        if epoch.is_some_and(|e| expected.as_ref().is_none_or(|lease| lease.epoch() != e)) {
            return err("계정 연결이 바뀌었어요. 다시 시도해 주세요.");
        }
        if purpose == oauth::Purpose::Files && auth.has_full_drive() {
            return Ok(json!({"email":auth.verified_identity().await?.email}));
        }
        if purpose == oauth::Purpose::Calendar && auth.calendar_connected() {
            match auth
                .calendar_lease(expected.as_ref().unwrap().epoch())
                .await
            {
                Ok(lease) => return Ok(json!({"email":lease.email()})),
                Err(e) if auth.calendar_connected() => return Err(e),
                Err(_) => {}
            }
        }
        if purpose.picker() && !auth.has_file_scope() {
            return err("먼저 Drive 연결을 허용해 주세요.");
        }
        let (cancel, mut cancelled) = watch::channel(false);
        *self.cancel.lock().unwrap() = Some(cancel);
        let attempt = async {
            let (verifier, challenge) = oauth::pkce();
            let state = oauth::random_token();
            let nonce = oauth::random_token();
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .map_err(|_| "Google 로그인 창을 준비하지 못했어요.")?;
            let host = format!(
                "127.0.0.1:{}",
                listener
                    .local_addr()
                    .map_err(|_| "로그인 주소를 확인하지 못했어요.")?
                    .port()
            );
            let redirect = format!("http://{host}");
            open(&oauth::auth_url_for(
                &auth.client_id,
                &redirect,
                &challenge,
                &state,
                &nonce,
                purpose,
                expected.as_ref().map(|e| e.email()),
            ));
            loop {
                let (mut socket, _) = listener
                    .accept()
                    .await
                    .map_err(|_| "로그인 응답을 받지 못했어요.")?;
                let request = read_headers(&mut socket).await;
                let decision = match &request {
                    Some(request) => callback(request, &state, &host),
                    None => Callback::Ignore,
                };
                match decision {
                    Callback::Ignore => {
                        let _ = socket
                            .write_all(
                                response(false, "사이 앱에서 연 로그인 창을 사용해 주세요.")
                                    .as_bytes(),
                            )
                            .await;
                    }
                    Callback::Denied => {
                        let _ = socket
                            .write_all(
                                response(
                                    false,
                                    "Google 로그인을 취소했거나 필요한 권한을 허용하지 않았어요.",
                                )
                                .as_bytes(),
                            )
                            .await;
                        return err("Google 로그인을 취소했거나 필요한 권한을 허용하지 않았어요.");
                    }
                    Callback::Code(code) => {
                        let result = if purpose.picker() {
                            match selected_files(request.as_deref().unwrap_or(""), &host, purpose == oauth::Purpose::SelectedFiles) {
                                Ok(ids) => auth.finish_picker(expected.as_ref().unwrap(), &code, &verifier, &redirect).await.map(|_| if purpose == oauth::Purpose::Folder { json!({"folderId":ids[0],"epoch":expected.as_ref().unwrap().epoch()}) } else { json!({"fileIds":ids,"epoch":expected.as_ref().unwrap().epoch()}) }),
                                Err(e) => Err(e),
                            }
                        } else if purpose == oauth::Purpose::Calendar {
                            auth.finish_calendar(
                                expected.as_ref().unwrap(),
                                &code,
                                &verifier,
                                &redirect,
                                &nonce,
                            )
                            .await
                            .map(|i| json!({"email":i.email}))
                        } else {
                            auth.exchange_identity(
                                &code,
                                &verifier,
                                &redirect,
                                &nonce,
                                expected.as_ref(),
                                oauth::DRIVE_SCOPE,
                            )
                            .await
                            .map(|i| json!({"email":i.email}))
                        };
                        let page = if result.is_ok() {
                            response(true, "사이 앱에서 이어서 사용해 주세요.")
                        } else {
                            response(false, "사이 앱에서 안내를 확인한 뒤 다시 시도해 주세요.")
                        };
                        // 저장 완료 뒤 브라우저 응답 지연을 로그인 취소로 오인하지 않는다.
                        tokio::spawn(async move {
                            let _ = tokio::time::timeout(
                                Duration::from_secs(2),
                                socket.write_all(page.as_bytes()),
                            )
                            .await;
                        });
                        return result;
                    }
                }
            }
        };
        let result = tokio::select! {
            biased;
            _ = cancelled.changed() => err("Google 로그인을 취소했어요. 기존 연결은 유지돼요."),
            result = tokio::time::timeout(Duration::from_secs(300),attempt) => result.unwrap_or_else(|_|err("5분 안에 로그인을 마치지 못했어요. 다시 시도해 주세요.")),
        };
        *self.cancel.lock().unwrap() = None;
        result
    }
}

#[cfg(test)]
mod tests;
