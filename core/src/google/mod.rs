//! Google 캘린더·할 일 연결: 로그인 창 열기, 빈 시간 계산, 할 일 동기화, 시간표 일정 반영.
pub mod calendar;
pub mod oauth;
pub mod tasks;

use crate::schedule::time_plan::Interval;
use crate::store::Store;
use crate::team::Hub;
use crate::time::{now_ms, DAY};
use crate::util::J;
use crate::{err, Result};
use oauth::{auth_url, parse_google_client, pkce, GoogleAuth};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const LOGIN_TIMEOUT_SECS: u64 = 5 * 60;

pub type OpenUrl = Arc<dyn Fn(&str) + Send + Sync>;

#[derive(Clone)]
pub struct Google(Arc<Inner>);

struct Inner {
    settings: Store,
    team_cache: Store,
    hub: Hub,
    builtin: Option<Value>,
    auth: Mutex<Option<Arc<GoogleAuth>>>,
    open_url: OpenUrl,
}

impl Google {
    /// builtin: 설치 파일에 함께 들어간 OAuth 클라이언트 { clientId, clientSecret }
    pub fn new(settings: Store, team_cache: Store, hub: Hub, builtin: Option<Value>, open_url: OpenUrl) -> Google {
        // 앱에 들어 있던 연결 정보를 이 컴퓨터의 설정에도 남겨 둔다.
        // 나중에 연결 정보가 빠진 버전으로 업데이트되더라도 Google 연결이 끊기지 않게 하기 위해서다.
        if let Some(b) = &builtin {
            if settings.read(|s| s["googleClient"].is_null()) {
                let b = b.clone();
                settings.update(|s| s["googleClient"] = b);
            }
        }
        Google(Arc::new(Inner { settings, team_cache, hub, builtin, auth: Mutex::new(None), open_url }))
    }

    fn client(&self) -> Option<Value> {
        let c = self.0.settings.read(|s| s["googleClient"].clone());
        if c.is_object() && !c.s("clientId").is_empty() {
            Some(c)
        } else {
            self.0.builtin.clone()
        }
    }

    fn auth(&self) -> Result<Arc<GoogleAuth>> {
        let Some(c) = self.client() else { return err("Google 연결 정보(OAuth 클라이언트)가 아직 없어요. 설정 → 외부 연결에서 넣어 주세요.") };
        let mut g = self.0.auth.lock().unwrap();
        if g.as_ref().map(|a| a.client_id != c.s("clientId")).unwrap_or(true) {
            *g = Some(Arc::new(GoogleAuth::new(&c, self.0.settings.clone())));
        }
        Ok(g.clone().unwrap())
    }

    fn record(&self, patch: Value) {
        self.0.team_cache.update(|c| {
            if !c["google"].is_object() {
                c["google"] = json!({});
            }
            if let Some(o) = patch.as_object() {
                for (k, v) in o {
                    c["google"][k] = v.clone();
                }
            }
        });
    }

    fn fail<T>(&self, e: String) -> Result<T> {
        self.record(json!({ "lastError": e, "lastErrorAt": now_ms() }));
        Err(e)
    }

    pub fn is_ready(&self) -> bool {
        self.client().is_some() && self.auth().map(|a| a.is_signed_in()).unwrap_or(false)
    }

    // ---------- 로그인 ----------
    pub async fn login(&self) -> Result<Value> {
        let a = self.auth()?;
        let (verifier, challenge) = pkce();
        let state = crate::team::backend::random_code(16);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.map_err(|e| format!("로그인 창을 열지 못했어요: {e}"))?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        let redirect = format!("http://127.0.0.1:{port}");
        (self.0.open_url)(&auth_url(&a.client_id, &redirect, &challenge, &state));

        let wait = async {
            loop {
                let (mut sock, _) = listener.accept().await.map_err(|e| e.to_string())?;
                let mut buf = vec![0u8; 8192];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
                let url = url::Url::parse(&format!("http://127.0.0.1{path}")).map_err(|e| e.to_string())?;
                if url.path() != "/" {
                    let _ = sock.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n").await;
                    continue;
                }
                let q: std::collections::HashMap<String, String> = url.query_pairs().into_owned().collect();
                let page = |ok: bool, msg: &str| {
                    let body = format!(
                        "<!doctype html><meta charset=\"utf-8\"><title>사이</title><body style=\"font-family:sans-serif;padding:40px;text-align:center\"><h2>{}</h2><p>{}</p><p>이 창은 닫아도 돼요.</p></body>",
                        if ok { "사이와 Google 연결 완료" } else { "연결하지 못했어요" },
                        msg
                    );
                    format!("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body)
                };
                if q.get("state").map(|s| s.as_str()) != Some(state.as_str()) {
                    let _ = sock.write_all(page(false, "요청이 맞지 않아요. 사이에서 다시 시도해 주세요.").as_bytes()).await;
                    return Err("로그인 요청이 맞지 않아요.".to_string());
                }
                if let Some(e) = q.get("error") {
                    let _ = sock.write_all(page(false, "로그인을 취소했거나 권한을 허용하지 않았어요.").as_bytes()).await;
                    return Err(format!("Google 로그인 취소: {e}"));
                }
                let code = q.get("code").cloned().unwrap_or_default();
                return match a.exchange_code(&code, &verifier, &redirect).await {
                    Ok(email) => {
                        let msg = format!("{}으로 연결됐어요.", email.clone().unwrap_or_else(|| "Google 계정".into()));
                        let _ = sock.write_all(page(true, &msg).as_bytes()).await;
                        Ok(json!({ "email": email }))
                    }
                    Err(e) => {
                        let _ = sock.write_all(page(false, &e).as_bytes()).await;
                        Err(e)
                    }
                };
            }
        };
        match tokio::time::timeout(std::time::Duration::from_secs(LOGIN_TIMEOUT_SECS), wait).await {
            Ok(Ok(v)) => {
                self.record(json!({ "lastError": null }));
                Ok(v)
            }
            Ok(Err(e)) => Err(e),
            Err(_) => err("5분 안에 로그인을 마치지 못했어요. 다시 시도해 주세요."),
        }
    }

    pub async fn logout(&self) -> Result<()> {
        self.auth()?.sign_out().await;
        self.record(json!({ "lastSync": 0, "lastResult": null, "lastError": null }));
        Ok(())
    }

    // ---------- 캘린더 ----------
    /// 이번 7일, 그리고 마감까지의 빈 시간 (9~22시 기준). 이 기기에서만 계산·표시한다
    pub async fn availability(&self, project_id: Option<&str>) -> Result<Value> {
        let a = self.auth()?;
        let now = now_ms();
        let project = project_id
            .and_then(|pid| self.0.hub.all().into_iter().find_map(|t| t.project(pid)))
            .or_else(|| self.0.hub.active().active_project());
        let deadline = project.and_then(|p| crate::time::parse_deadline(&p.s("deadline")));
        let until = (now + 7 * DAY).max(deadline.unwrap_or(0));
        match calendar::query_busy(&a, now, until).await {
            Ok(busy) => {
                let week = calendar::free_hours(&busy, now, now + 7 * DAY, 9, 22);
                let to_dl = deadline.filter(|d| *d > now).map(|d| calendar::free_hours(&busy, now, d, 9, 22));
                self.record(json!({ "lastCalendar": now, "lastError": null }));
                Ok(json!({ "week": week, "toDeadline": to_dl, "window": "9시~22시" }))
            }
            Err(e) => self.fail(e),
        }
    }

    /// 시간표가 쓰는 바쁜 시간 (사이가 만든 일정 포함)
    pub async fn busy_between(&self, from: i64, to: i64) -> (Vec<Interval>, bool, Option<String>) {
        if !self.is_ready() {
            return (vec![], false, Some("Google 캘린더에 로그인하지 않았어요".into()));
        }
        let a = match self.auth() {
            Ok(a) => a,
            Err(e) => return (vec![], false, Some(e)),
        };
        match calendar::query_busy(&a, from, to).await {
            Ok(b) => {
                self.record(json!({ "lastCalendar": now_ms(), "lastError": null }));
                (b, true, None)
            }
            Err(e) => {
                self.record(json!({ "lastError": e, "lastErrorAt": now_ms() }));
                (vec![], false, Some(e))
            }
        }
    }

    fn describe(&self, project_id: &str) -> String {
        let p = self.0.hub.all().into_iter().find_map(|t| t.project(project_id));
        let mut lines = vec!["사이가 잡은 작업 시간이에요.".to_string()];
        if let Some(p) = p {
            lines.push(format!("프로젝트: {}", p.s("title")));
        }
        lines.push("시간을 바꾸려면 사이의 협업 탭이나 대화에서 고쳐 주세요.".into());
        lines.join("\n")
    }

    pub async fn create_block_event(&self, b: &Value) -> Result<String> {
        let a = self.auth()?;
        calendar::create_event(&a, &format!("사이: {}", b.s("title")), &self.describe(&b.s("projectId")), b.i("start"), b.i("end"), &b.s("taskId")).await
    }

    pub async fn patch_block_event(&self, event_id: &str, b: &Value) -> Result<()> {
        let a = self.auth()?;
        calendar::patch_event(&a, event_id, &format!("사이: {}", b.s("title")), b.i("start"), b.i("end")).await
    }

    pub async fn remove_block_event(&self, event_id: &str) -> Result<()> {
        let a = self.auth()?;
        calendar::delete_event(&a, event_id).await
    }

    // ---------- 할 일 ----------
    pub async fn sync_todo(&self) -> Result<Option<String>> {
        if self.client().is_none() || !self.is_ready() {
            return Ok(None);
        }
        let teams: Vec<_> = self.0.hub.all().into_iter().filter(|t| t.is_connected()).collect();
        if teams.is_empty() {
            return Ok(None);
        }
        let a = self.auth()?;
        let (mut created, mut marked, mut completed) = (0, 0, 0);
        // 모든 팀의 프로젝트를 돌며, 팀마다 내 uid 기준으로 수락한 작업을 올린다
        for t in teams {
            let me = t.my_uid();
            for p in t.collab_data().arr("projects") {
                let pid = p.s("id");
                let mine: Vec<Value> = t.tasks_of(&pid).into_iter().filter(|x| x.s("assignee") == me && x.b("accepted")).collect();
                let links = self.0.settings.read(|s| s["googleTaskLinks"].obj(&pid));
                if mine.is_empty() && links.is_empty() {
                    continue;
                }
                let r = async {
                    let list_id = tasks::ensure_list(&a, &format!("사이 · {}", p.s("title"))).await?;
                    tasks::sync_tasks(&a, &list_id, &mine, &links, &p.s("deadline"), &p.s("title")).await
                }
                .await;
                let r = match r {
                    Ok(r) => r,
                    Err(e) => return self.fail(e),
                };
                let links = r.links.clone();
                self.0.settings.update(|s| {
                    if !s["googleTaskLinks"].is_object() {
                        s["googleTaskLinks"] = json!({});
                    }
                    s["googleTaskLinks"][&pid] = Value::Object(links);
                });
                for id in &r.mark_done {
                    let _ = t.set_status(id, "done").await;
                }
                created += r.created;
                marked += r.mark_done.len();
                completed += r.completed_in_google;
            }
        }
        let text = format!("할 일 {created}개 추가, Google 에서 완료한 {marked}개 반영, 사이에서 완료한 {completed}개 반영");
        self.record(json!({ "lastSync": now_ms(), "lastResult": text, "lastError": null }));
        Ok(Some(text))
    }

    pub fn status(&self) -> Value {
        let c = self.client();
        let t = c.as_ref().and_then(|_| self.0.settings.read(|s| crate::secret::decrypt_json_field(s, "googleTokensEnc")));
        let s = self.0.team_cache.read(|c| c["google"].clone());
        json!({
            "configured": c.is_some(),
            "builtin": self.0.settings.read(|s| s["googleClient"].is_null()) && self.0.builtin.is_some(),
            "signedIn": t.as_ref().map(|t| !t.s("refreshToken").is_empty()).unwrap_or(false),
            "email": t.as_ref().and_then(|t| t.os("email")),
            "lastSync": s.i("lastSync"),
            "lastResult": s.get("lastResult").cloned().unwrap_or(Value::Null),
            "lastError": s.get("lastError").cloned().unwrap_or(Value::Null),
        })
    }

    pub fn configure(&self, text: &str) -> Result<Value> {
        let Some(c) = parse_google_client(text) else { return err("클라이언트 ID(…apps.googleusercontent.com)와 보안 비밀번호(GOCSPX-…)를 찾지 못했어요.") };
        let cc = c.clone();
        self.0.settings.update(|s| {
            s["googleClient"] = cc;
            s["googleTokensEnc"] = Value::Null;
        });
        *self.0.auth.lock().unwrap() = None;
        Ok(c)
    }
}
