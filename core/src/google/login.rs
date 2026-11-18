//! 일시적인 루프백 로그인. 임의 요청은 무시하고 해당 로그인 시도만 취소한다.
use super::{
    oauth::{self, GoogleAuth},
    OpenUrl, ReturnToApp,
};
use crate::{err, Result};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::AsyncReadExt,
    net::{TcpListener, TcpStream},
    sync::{watch, Mutex as AsyncMutex, MutexGuard},
    task::AbortHandle,
};

mod return_page;

#[derive(Default)]
pub(super) struct Login {
    gate: AsyncMutex<()>,
    cancel: Mutex<Option<watch::Sender<bool>>>,
    completion: Mutex<Option<AbortHandle>>,
    return_to_app: Option<ReturnToApp>,
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

struct LocalRequest<'a> {
    method: &'a str,
    path: &'a str,
    headers: &'a str,
}
impl<'a> LocalRequest<'a> {
    fn parse(request: &'a str, host: &str) -> Option<Self> {
        let (line, headers) = request.split_once("\r\n")?;
        let (headers, _) = headers.split_once("\r\n\r\n")?;
        let mut parts = line.split_whitespace();
        let (method, path, version) = (parts.next()?, parts.next()?, parts.next()?);
        if parts.next().is_some()
            || !["HTTP/1.0", "HTTP/1.1"].contains(&version)
            || !path.starts_with('/')
        {
            return None;
        }
        let parsed = Self {
            method,
            path,
            headers,
        };
        (parsed.header("host") == Some(host)).then_some(parsed)
    }
    fn header(&self, name: &str) -> Option<&'a str> {
        let mut values = self
            .headers
            .split("\r\n")
            .filter_map(|line| line.split_once(':'))
            .filter(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.trim());
        let v = values.next()?;
        values.next().is_none().then_some(v)
    }
}

fn callback(request: &str, state: &str, host: &str) -> Callback {
    let Some(request) = LocalRequest::parse(request, host) else {
        return Callback::Ignore;
    };
    if request.method != "GET" {
        return Callback::Ignore;
    }
    let Ok(url) = url::Url::parse(&format!("http://{host}{}", request.path)) else {
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
async fn read_headers(socket: &mut TcpStream) -> Option<String> {
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut bytes = Vec::new();
        let mut chunk = [0u8; 1024];
        loop {
            let n = socket.read(&mut chunk).await.ok()?;
            if n == 0 || bytes.len() + n > 8192 {
                return None;
            }
            bytes.extend_from_slice(&chunk[..n]);
            if let Some(end) = bytes.windows(4).position(|p| p == b"\r\n\r\n") {
                bytes.truncate(end + 4);
                return String::from_utf8(bytes).ok();
            }
        }
    })
    .await
    .ok()
    .flatten()
}
impl Login {
    pub fn with_return_to_app(return_to_app: ReturnToApp) -> Self {
        Self {
            return_to_app: Some(return_to_app),
            ..Self::default()
        }
    }

    fn close_completion(&self) {
        if let Some(task) = self.completion.lock().unwrap().take() {
            task.abort();
        }
    }

    pub fn cancel(&self) {
        if let Some(sender) = &*self.cancel.lock().unwrap() {
            let _ = sender.send(true);
        }
        self.close_completion();
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
        self.close_completion();
        let (cancel, mut cancelled) = watch::channel(false);
        *self.cancel.lock().unwrap() = Some(cancel);
        let attempt = async {
            let (verifier, challenge) = oauth::pkce();
            let state = oauth::random_token();
            let nonce = oauth::random_token();
            let listener = TcpListener::bind("127.0.0.1:0")
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
                let result = match decision {
                    Callback::Ignore => {
                        return_page::write_response(&mut socket, return_page::Response::BadRequest)
                            .await;
                        continue;
                    }
                    Callback::Denied => {
                        err("Google 로그인을 취소했거나 필요한 권한을 허용하지 않았어요.")
                    }
                    Callback::Code(code) => {
                        if purpose.picker() {
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
                        }
                    }
                };
                let ok = result.is_ok();
                let return_to_app = self.return_to_app.clone();
                // The completion page stays available without keeping the login gate locked.
                let task = tokio::spawn(return_page::serve(
                    listener,
                    socket,
                    host,
                    purpose,
                    ok,
                    return_to_app,
                ));
                *self.completion.lock().unwrap() = Some(task.abort_handle());
                return result;
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
