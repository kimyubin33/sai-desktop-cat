//! Developer-bundled HTTPS gateway only. No provider key or refresh token leaves this module.
use super::{
    oauth::{GoogleAuth, Lease},
    profile::{model::id, Profiles},
    teams::Teams,
};
use crate::{err, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
pub(super) struct Client {
    endpoint: Option<String>,
}
impl Client {
    pub async fn connect(
        &self,
        auth: &GoogleAuth,
        profiles: &Profiles,
        teams: &Teams,
        epoch: u64,
        folder: &str,
    ) -> Result<Value> {
        let lease = auth.full_drive_lease(epoch).await?;
        teams.access(auth, profiles, epoch, folder).await?;
        let endpoint = self.endpoint.as_deref().ok_or("팀 AI를 준비하고 있어요.")?;
        auth.set_team_ai_consent(&lease, endpoint, true).await?;
        self.status(auth, profiles, teams, epoch, folder).await
    }
    pub fn new(endpoint: Option<&str>) -> Result<Self> {
        let endpoint = endpoint.filter(|s| !s.trim().is_empty());
        if let Some(raw) = endpoint {
            let u = url::Url::parse(raw).map_err(|_| "팀 AI 연결 정보가 올바르지 않아요.")?;
            let secure = u.scheme() == "https"
                && u.host_str().is_some_and(|h| h != "localhost")
                && u.host().is_some_and(|h| matches!(h, url::Host::Domain(_)));
            let test_loopback =
                cfg!(test) && u.scheme() == "http" && u.host_str() == Some("127.0.0.1");
            if (!secure && !test_loopback)
                || !u.username().is_empty()
                || u.password().is_some()
                || u.query().is_some()
                || u.fragment().is_some()
                || u.path() != "/"
                || raw.len() > 2048
            {
                return err("팀 AI의 배포 연결 정보를 확인해 주세요.");
            }
            return Ok(Self {
                endpoint: Some(u.origin().ascii_serialization()),
            });
        }
        Ok(Self { endpoint: None })
    }
    pub async fn status(
        &self,
        auth: &GoogleAuth,
        profiles: &Profiles,
        teams: &Teams,
        epoch: u64,
        folder: &str,
    ) -> Result<Value> {
        let lease = auth.full_drive_lease(epoch).await?;
        teams.access(auth, profiles, epoch, folder).await?;
        let Some(endpoint) = &self.endpoint else {
            return auth.with_lease(&lease,||Ok(json!({"epoch":epoch,"folderId":folder,"configured":false,"ready":false,"provider":"claude"}))).await;
        };
        if !auth.team_ai_consented(&lease, endpoint) {
            return auth.with_lease(&lease,||Ok(json!({"epoch":epoch,"folderId":folder,"configured":true,"ready":false,"provider":"claude","requiresConsent":true}))).await;
        }
        let v = auth
            .team_ai_request(&lease, endpoint, "/v1/status", json!({"folderId":folder}))
            .await?;
        let ready = v["ready"]
            .as_bool()
            .ok_or("팀 AI 연결 상태를 확인하지 못했어요.")?;
        let owner = v["owner"]
            .as_bool()
            .ok_or("팀 AI의 사용 역할을 확인하지 못했어요.")?;
        let limit = v["dailyRequests"]
            .as_u64()
            .filter(|n| *n >= 1 && *n <= 100)
            .ok_or("팀 AI의 사용 한도를 확인하지 못했어요.")?;
        let used = v["used"]
            .as_u64()
            .filter(|n| *n <= 100)
            .ok_or("팀 AI의 사용량을 확인하지 못했어요.")?;
        if v["provider"] != "claude" {
            return err("지원하지 않는 팀 AI 연결이에요.");
        }
        auth.with_lease(&lease,||Ok(json!({"epoch":epoch,"folderId":folder,"configured":true,"ready":ready,"provider":"claude","owner":owner,"dailyRequests":limit,"used":used}))).await
    }
    // Called by the reviewed native flow; never accepts an endpoint from frontend IPC.
    pub async fn analyze(
        &self,
        auth: &GoogleAuth,
        profiles: &Profiles,
        teams: &Teams,
        epoch: u64,
        folder: &str,
        attempt: &Attempt,
    ) -> Result<Value> {
        let lease = auth.full_drive_lease(epoch).await?;
        teams.access(auth, profiles, epoch, folder).await?;
        let endpoint = self.endpoint.as_deref().ok_or("팀 AI를 준비하고 있어요.")?;
        if !auth.team_ai_consented(&lease, endpoint) {
            return err("팀 AI 연결의 권한 전달 범위를 먼저 확인해 주세요.");
        }
        if !id(folder)
            || !id(&attempt.request_id)
            || attempt.request_id.len() < 32
            || attempt.request_id.len() > 80
            || attempt.created_at <= 0
            || attempt.text.trim().is_empty()
            || attempt.text.len() > 24 * 1024
            || attempt.text.contains('\0')
        {
            return err("AI에 보낼 내용과 요청 정보를 확인해 주세요.");
        }
        let result=auth.team_ai_request(&lease,endpoint,"/v1/analyze",json!({"folderId":folder,"requestId":attempt.request_id,"createdAt":attempt.created_at,"text":attempt.text})).await?;
        let result = validate_result(result, attempt)?;
        teams.access(auth, profiles, epoch, folder).await?;
        auth.with_lease(&lease, || {
            Ok(json!({"epoch":epoch,"folderId":folder,"result":result}))
        })
        .await
    }
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Attempt {
    pub request_id: String,
    pub created_at: i64,
    pub text: String,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Candidate {
    title: String,
    assignee: String,
    due_date: String,
    evidence: String,
    uncertain: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Analysis {
    request_id: String,
    summary: String,
    candidates: Vec<Candidate>,
}
fn text(s: &str, max: usize, empty: bool) -> bool {
    (empty || !s.trim().is_empty()) && s.chars().count() <= max && !s.chars().any(char::is_control)
}
fn validate_result(value: Value, attempt: &Attempt) -> Result<Value> {
    let result: Analysis =
        serde_json::from_value(value).map_err(|_| "AI 결과의 형식을 확인하지 못했어요.")?;
    if result.request_id != attempt.request_id
        || result.summary.trim().is_empty()
        || result.summary.chars().count() > 2000
        || result
            .summary
            .chars()
            .any(|c| c.is_control() && !['\n', '\t', '\r'].contains(&c))
        || result.candidates.len() > 12
        || result.candidates.iter().any(|c| {
            !text(&c.title, 160, false)
                || !text(&c.assignee, 120, true)
                || c.evidence.trim().is_empty()
                || c.evidence.chars().count() > 500
                || c.evidence
                    .chars()
                    .any(|c| c.is_control() && !['\n', '\t', '\r'].contains(&c))
                || !attempt.text.contains(&c.evidence)
                || !c.due_date.is_empty()
                    && (c.due_date.len() != 10
                        || c.due_date.starts_with("0000")
                        || chrono::NaiveDate::parse_from_str(&c.due_date, "%Y-%m-%d").is_err())
        })
    {
        return err("AI 결과의 요청 번호·근거·날짜를 확인하지 못했어요. 업무를 저장하지 않았어요.");
    }
    serde_json::to_value(result).map_err(|_| "AI 결과를 준비하지 못했어요.".into())
}
pub(super) async fn request(
    auth: &GoogleAuth,
    lease: &Lease,
    http: &reqwest::Client,
    token: &str,
    endpoint: &str,
    path: &str,
    body: Value,
) -> Result<Value> {
    // Called only by GoogleAuth, with a captured verified lease and a validated bundled origin.
    if !["/v1/status", "/v1/analyze"].contains(&path) {
        return err("허용되지 않은 팀 AI 요청이에요.");
    }
    auth.with_lease(lease, || Ok(())).await?;
    let response = http
        .post(format!("{endpoint}{path}"))
        .bearer_auth(token)
        .timeout(std::time::Duration::from_secs(75))
        .json(&body)
        .send()
        .await
        .map_err(|_| {
            "팀 AI 응답을 확인하지 못했어요. 같은 분석을 자동으로 다시 실행하지 않았어요."
        })?;
    let status = response.status().as_u16();
    let mut response = response;
    let mut bytes = vec![];
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "팀 AI 응답을 끝까지 받지 못했어요.")?
    {
        if bytes.len() + chunk.len() > 64 * 1024 {
            return err("팀 AI 응답이 허용 크기를 넘었어요.");
        }
        bytes.extend_from_slice(&chunk);
    }
    let value: Value =
        serde_json::from_slice(&bytes).map_err(|_| "팀 AI 응답의 형식을 확인하지 못했어요.")?;
    auth.with_lease(lease, || Ok(())).await?;
    if !(200..300).contains(&status) {
        return err(match value["error"].as_str().unwrap_or("") {
            "request_already_attempted" | "request_conflict" => {
                "이미 시도한 분석이에요. 중복 비용을 막기 위해 다시 실행하지 않았어요."
            }
            "daily_limit" => "팀 AI의 오늘 사용 한도에 도달했어요.",
            "membership_required"
            | "team_access_denied"
            | "team_not_enabled"
            | "google_access_denied" => "현재 계정의 팀 AI 사용 권한을 확인하지 못했어요.",
            "not_configured" => "팀 AI를 준비하고 있어요.",
            _ => "팀 AI 작업을 완료하지 못했어요. 내용과 분석 요청은 자동으로 재전송하지 않았어요.",
        });
    }
    Ok(value)
}
#[cfg(test)]
mod tests;
