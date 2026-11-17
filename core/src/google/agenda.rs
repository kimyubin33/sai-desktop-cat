//! Reviewed, conditional changes to this app's all-day events on the primary calendar.
use super::{collab::Collaboration, oauth::GoogleAuth, profile::Profiles, teams::Teams};
use crate::{err, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, sync::Mutex};
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Add {
    pub task_id: String,
    pub heads: Vec<String>,
    pub date: String,
    pub confirmed: bool,
}
fn key(subject: &str, folder: &str, task: &str) -> String {
    format!(
        "{:x}",
        Sha256::digest(format!("sai-calendar-v1\n{subject}\n{folder}\n{task}").as_bytes())
    )
}
fn end(date: &str) -> Result<String> {
    let day = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .map_err(|_| "추가할 날짜를 확인해 주세요.")?;
    let next = day.succ_opt().ok_or("추가할 날짜를 확인해 주세요.")?;
    if date.len() != 10
        || day.format("%Y-%m-%d").to_string() != date
        || date < "0001-01-01"
        || date > "9998-12-31"
    {
        return err("1~9998년의 정확한 날짜를 선택해 주세요.");
    }
    Ok(next.format("%Y-%m-%d").to_string())
}
fn manageable(value: &Value) -> bool {
    let start = value["start"]["date"].as_str().unwrap_or("");
    end(start).is_ok_and(|end| value["end"]["date"] == end)
        && value["status"] == "confirmed"
        && value["start"]["dateTime"].is_null()
        && value["end"]["dateTime"].is_null()
        && ["attendees", "recurrence", "attachments"]
            .iter()
            .all(|k| value[k].is_null() || value[k].as_array().is_some_and(|v| v.is_empty()))
        && ["description", "location", "recurringEventId", "hangoutLink"]
            .iter()
            .all(|k| value[k].is_null() || value[k] == "")
        && ["conferenceData", "gadget", "originalStartTime"]
            .iter()
            .all(|k| value[k].is_null())
        && value["attendeesOmitted"] != true
        && value["locked"] != true
        && (value["organizer"].is_null() || value["organizer"]["self"] == true)
        && (value["eventType"].is_null() || value["eventType"] == "default")
        && value["etag"]
            .as_str()
            .is_some_and(super::oauth::agenda::event_revision)
}
fn event_view(value: &Value, id: &str, source: &str) -> Result<Value> {
    if value["id"] != id {
        return err("Calendar의 일정 번호를 확인하지 못했어요.");
    }
    if value["status"] == "cancelled" {
        return Ok(json!({"state":"deleted"}));
    }
    if value["extendedProperties"]["private"]["saiSource"] != source
        || value["extendedProperties"]["private"]["saiVersion"] != "1"
    {
        return err("같은 번호의 다른 일정이 있어요. 기존 일정은 변경하지 않았어요.");
    }
    let summary = value["summary"].as_str().unwrap_or("");
    if summary.len() > 4096 {
        return err("Calendar 일정 제목의 크기를 확인하지 못했어요.");
    }
    Ok(
        json!({"state":"existing","title":summary,"date":value["start"]["date"].as_str().unwrap_or(""),"endDate":value["end"]["date"].as_str().unwrap_or(""),"dateTime":value["start"]["dateTime"].as_str().unwrap_or(""),"endDateTime":value["end"]["dateTime"].as_str().unwrap_or(""),"timeZone":value["start"]["timeZone"].as_str().unwrap_or(""),"manageable":manageable(value)}),
    )
}
async fn task_preview(
    auth: &GoogleAuth,
    profiles: &Profiles,
    teams: &Teams,
    collab: &Collaboration,
    epoch: u64,
    folder: &str,
    task: &str,
) -> Result<Value> {
    let lease = auth.full_drive_lease(epoch).await?;
    let mut view = collab
        .calendar_task(auth, profiles, teams, epoch, folder, task, None)
        .await?;
    auth.with_lease(&lease, || {
        view["email"] = json!(lease.email());
        view["connected"] = json!(auth.calendar_connected());
        Ok(view)
    })
    .await
}
pub(super) async fn add(
    auth: &GoogleAuth,
    profiles: &Profiles,
    teams: &Teams,
    collab: &Collaboration,
    epoch: u64,
    folder: &str,
    add: Add,
) -> Result<Value> {
    if !add.confirmed {
        return err("제목·날짜·내 기본 캘린더에 추가하는 내용을 확인해 주세요.");
    }
    let next = end(&add.date)?;
    let task = collab
        .calendar_task(
            auth,
            profiles,
            teams,
            epoch,
            folder,
            &add.task_id,
            Some(&add.heads),
        )
        .await?;
    if task["entity"]["status"] == "done" {
        return err("완료한 할 일이에요. 새 일정은 추가하지 않았어요.");
    }
    let lease = auth.calendar_lease(epoch).await?;
    let source = key(lease.subject(), folder, &add.task_id);
    let id = format!("5a1{source}");
    let path = format!("/calendar/v3/calendars/primary/events/{id}");
    let result = match auth.calendar_request(&lease, "GET", &path, None).await {
        Ok(v) => event_view(&v, &id, &source)?,
        Err((410, _)) => json!({"state":"deleted"}),
        Err((404, _)) => {
            // Recheck Drive immediately before the external write, never infer an assignee's identity.
            collab
                .calendar_task(
                    auth,
                    profiles,
                    teams,
                    epoch,
                    folder,
                    &add.task_id,
                    Some(&add.heads),
                )
                .await?;
            let body = json!({"id":id,"summary":task["entity"]["title"],"start":{"date":add.date},"end":{"date":next},"visibility":"private","transparency":"transparent","reminders":{"useDefault":false},"extendedProperties":{"private":{"saiVersion":"1","saiSource":source}}});
            let created = match auth
                .calendar_request(
                    &lease,
                    "POST",
                    "/calendar/v3/calendars/primary/events?sendUpdates=none",
                    Some(body),
                )
                .await
            {
                Ok(_) => true,
                Err((409, _)) => false,
                Err((410, _)) => {
                    return Ok(
                        json!({"epoch":epoch,"folderId":folder,"taskId":add.task_id,"state":"deleted"}),
                    )
                }
                Err((_, e)) => return Err(e),
            };
            let saved=auth.calendar_request(&lease,"GET",&path,None).await.map_err(|(_,_)|"등록 요청은 전송됐지만 결과를 확인하지 못했어요. 같은 할 일로 다시 확인하면 같은 일정 번호를 사용해요.")?;
            let mut view = event_view(&saved, &id, &source)?;
            if created && view["state"] == "existing" {
                view["state"] = json!("registered");
            }
            view
        }
        Err((_, e)) => return Err(e),
    };
    auth.with_lease(&lease, || {
        let mut v = result;
        v["epoch"] = json!(epoch);
        v["folderId"] = json!(folder);
        v["taskId"] = json!(add.task_id);
        Ok(v)
    })
    .await
}

#[derive(Clone, PartialEq, Eq, serde::Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub(super) enum Action {
    Update { date: String },
    Delete,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Change {
    pub review_id: String,
    pub action: Action,
    pub confirmed: bool,
}
#[derive(Clone)]
struct Attempt {
    action: Action,
    operation: String,
    title: String,
}
#[derive(Clone)]
struct Review {
    subject: String,
    epoch: u64,
    folder: String,
    task: String,
    heads: Vec<String>,
    title: String,
    revision: String,
    created: i64,
    attempt: Option<Attempt>,
}
#[derive(Default)]
pub(super) struct Agenda {
    reviews: Mutex<BTreeMap<String, Review>>,
    gate: tokio::sync::Mutex<()>,
}
impl Agenda {
    pub fn clear(&self) {
        self.reviews.lock().unwrap().clear();
    }
    pub fn forget(&self, epoch: u64, folder: &str, review: &str) {
        self.reviews
            .lock()
            .unwrap()
            .retain(|id, r| !(id == review && r.epoch == epoch && r.folder == folder));
    }
    fn review(&self, lease: &super::oauth::Lease, folder: &str, id: &str) -> Result<Review> {
        self.reviews
            .lock()
            .unwrap()
            .get(id)
            .filter(|r| {
                r.subject == lease.subject()
                    && r.epoch == lease.epoch()
                    && r.folder == folder
                    && crate::time::now_ms() - r.created < 15 * 60 * 1000
            })
            .cloned()
            .ok_or(
                "일정 확인이 만료됐거나 계정이 바뀌었어요. 최신 내용을 다시 확인해 주세요.".into(),
            )
    }
    pub async fn preview(
        &self,
        auth: &GoogleAuth,
        profiles: &Profiles,
        teams: &Teams,
        collab: &Collaboration,
        epoch: u64,
        folder: &str,
        task: &str,
    ) -> Result<Value> {
        let lease = auth.full_drive_lease(epoch).await?;
        let mut view = task_preview(auth, profiles, teams, collab, epoch, folder, task).await?;
        let mut raw = None;
        view["calendar"] = json!({"state":"unconnected"});
        if auth.calendar_connected() {
            let looked = async {
                let cal = auth.calendar_lease(epoch).await?;
                lookup(auth, &cal, folder, task).await
            }
            .await;
            match looked {
                Ok((event, value)) => {
                    view["calendar"] = event;
                    raw = value;
                }
                Err(e) => {
                    view["calendar"] = json!({"state":"unavailable"});
                    view["calendarError"] = json!(e);
                }
            }
        }
        auth.with_lease(&lease, || {
            view["connected"] = json!(auth.calendar_connected());
            let mut reviews = self.reviews.lock().unwrap();
            reviews.retain(|_, r| {
                r.subject == lease.subject()
                    && r.epoch == epoch
                    && crate::time::now_ms() - r.created < 15 * 60 * 1000
            });
            if let Some(value) = raw.filter(manageable) {
                if reviews.len() >= 8 {
                    let oldest = reviews
                        .iter()
                        .min_by_key(|(_, r)| r.created)
                        .map(|(k, _)| k.clone())
                        .unwrap();
                    reviews.remove(&oldest);
                }
                let id = super::oauth::random_token();
                reviews.insert(
                    id.clone(),
                    Review {
                        subject: lease.subject().into(),
                        epoch,
                        folder: folder.into(),
                        task: task.into(),
                        heads: serde_json::from_value(view["heads"].clone())
                            .map_err(|_| "업무 버전을 확인하지 못했어요.")?,
                        title: view["entity"]["title"].as_str().unwrap().into(),
                        revision: value["etag"].as_str().unwrap().into(),
                        created: crate::time::now_ms(),
                        attempt: None,
                    },
                );
                view["reviewId"] = json!(id);
            }
            Ok(view)
        })
        .await
    }
    pub async fn change(
        &self,
        auth: &GoogleAuth,
        profiles: &Profiles,
        teams: &Teams,
        collab: &Collaboration,
        epoch: u64,
        folder: &str,
        change: Change,
    ) -> Result<Value> {
        if !change.confirmed {
            return err("변경 전후 내용과 내 일정의 수정·삭제를 확인해 주세요.");
        }
        if let Action::Update { date } = &change.action {
            end(date)?;
        }
        let lease = auth.full_drive_lease(epoch).await?;
        let _gate = self
            .gate
            .try_lock()
            .map_err(|_| "일정 작업을 진행 중이에요. 결과를 먼저 확인해 주세요.")?;
        let review = self.review(&lease, folder, &change.review_id)?;
        teams.access(auth, profiles, epoch, folder).await?;
        let cal = auth.calendar_lease(epoch).await?;
        if let Some(attempt) = &review.attempt {
            if attempt.action != change.action {
                return err("이 확인창에서 이미 요청한 작업이 있어요. 결과를 먼저 확인해 주세요.");
            }
            return resolve(auth, &cal, &review, attempt).await;
        }
        let task = collab
            .calendar_task(
                auth,
                profiles,
                teams,
                epoch,
                folder,
                &review.task,
                Some(&review.heads),
            )
            .await?;
        if matches!(change.action, Action::Update { .. }) && task["entity"]["status"] == "done" {
            return err(
                "완료한 업무의 일정은 수정하지 않았어요. 연결된 일정은 확인 후 삭제할 수 있어요.",
            );
        }
        let (event, raw) = lookup(auth, &cal, folder, &review.task).await?;
        let Some(raw) = raw.filter(manageable) else {
            return guarded_reply(auth,&cal,&review,"conflict",event,"일정이 삭제됐거나 Calendar에서 직접 관리해야 하는 형식이에요. 최신 내용을 다시 확인해 주세요.").await;
        };
        if raw["etag"] != review.revision {
            return guarded_reply(
                auth,
                &cal,
                &review,
                "conflict",
                event,
                "다른 변경이 도착했어요. 현재 일정은 유지했어요. 최신 내용을 다시 확인해 주세요.",
            )
            .await;
        }
        let source = key(cal.subject(), folder, &review.task);
        let id = format!("5a1{source}");
        let operation = super::oauth::random_token();
        let (method, body) = match &change.action {
            Action::Delete => ("DELETE", None),
            Action::Update { date } => {
                let next = end(date)?;
                if raw["summary"] == review.title
                    && raw["start"]["date"] == *date
                    && raw["end"]["date"] == next
                {
                    return guarded_reply(
                        auth,
                        &cal,
                        &review,
                        "unchanged",
                        event,
                        "이미 같은 제목과 날짜예요. 수정 요청을 보내지 않았어요.",
                    )
                    .await;
                }
                let mut private = raw["extendedProperties"]["private"].clone();
                private["saiChange"] = json!(operation);
                (
                    "PATCH",
                    Some(
                        json!({"summary":review.title,"start":{"date":date},"end":{"date":next},"extendedProperties":{"private":private}}),
                    ),
                )
            }
        };
        collab
            .calendar_task(
                auth,
                profiles,
                teams,
                epoch,
                folder,
                &review.task,
                Some(&review.heads),
            )
            .await?;
        let attempt = Attempt {
            action: change.action,
            operation,
            title: review.title.clone(),
        };
        auth.with_lease(&cal, || {
            self.reviews
                .lock()
                .unwrap()
                .get_mut(&change.review_id)
                .ok_or("일정 확인창이 닫혔어요.")?
                .attempt = Some(attempt.clone());
            Ok(())
        })
        .await?;
        let sent = auth
            .calendar_change(
                &cal,
                method,
                &format!("/calendar/v3/calendars/primary/events/{id}?sendUpdates=none"),
                body,
                &review.revision,
            )
            .await;
        if matches!(sent, Err((412, _))) {
            return guarded_reply(
                auth,
                &cal,
                &review,
                "conflict",
                json!({"state":"unavailable"}),
                "확인 이후 일정이 변경돼 요청을 적용하지 않았어요. 최신 내용을 다시 확인해 주세요.",
            )
            .await;
        }
        // Read-only reconciliation after a write. Never repeat PATCH/DELETE on uncertain responses.
        resolve(auth, &cal, &review, &attempt).await
    }
    pub async fn check(
        &self,
        auth: &GoogleAuth,
        profiles: &Profiles,
        teams: &Teams,
        epoch: u64,
        folder: &str,
        id: &str,
    ) -> Result<Value> {
        let lease = auth.full_drive_lease(epoch).await?;
        let _gate = self
            .gate
            .try_lock()
            .map_err(|_| "일정 작업을 진행 중이에요. 잠시 뒤 결과를 확인해 주세요.")?;
        let review = self.review(&lease, folder, id)?;
        teams.access(auth, profiles, epoch, folder).await?;
        let Some(attempt) = &review.attempt else {
            return guarded_reply(
                auth,
                &lease,
                &review,
                "notSent",
                json!({"state":"unavailable"}),
                "변경 요청을 보내기 전에 멈췄어요. 최신 내용을 다시 확인한 뒤 실행해 주세요.",
            )
            .await;
        };
        let cal = auth.calendar_lease(epoch).await?;
        resolve(auth, &cal, &review, attempt).await
    }
}
async fn guarded_reply(
    auth: &GoogleAuth,
    lease: &super::oauth::Lease,
    review: &Review,
    state: &str,
    event: Value,
    message: &str,
) -> Result<Value> {
    auth.with_lease(lease, || Ok(reply(review, state, event, message)))
        .await
}
fn reply(review: &Review, state: &str, event: Value, message: &str) -> Value {
    json!({"epoch":review.epoch,"folderId":review.folder,"taskId":review.task,"state":state,"calendar":event,"message":message})
}
async fn lookup(
    auth: &GoogleAuth,
    lease: &super::oauth::Lease,
    folder: &str,
    task: &str,
) -> Result<(Value, Option<Value>)> {
    let source = key(lease.subject(), folder, task);
    let id = format!("5a1{source}");
    match auth
        .calendar_request(
            lease,
            "GET",
            &format!("/calendar/v3/calendars/primary/events/{id}"),
            None,
        )
        .await
    {
        Ok(value) => Ok((event_view(&value, &id, &source)?, Some(value))),
        Err((404, _)) => Ok((json!({"state":"missing"}), None)),
        Err((410, _)) => Ok((json!({"state":"deleted"}), None)),
        Err((_, e)) => Err(e),
    }
}
async fn resolve(
    auth: &GoogleAuth,
    lease: &super::oauth::Lease,
    review: &Review,
    attempt: &Attempt,
) -> Result<Value> {
    let looked = lookup(auth, lease, &review.folder, &review.task).await;
    let (event,raw)=match looked {Ok(v)=>v,Err(_)=>return auth.with_lease(lease,||Ok(reply(review,"uncertain",json!({"state":"unavailable"}),"전송한 변경의 결과를 확인하지 못했어요. 결과 다시 확인은 조회만 하고 수정·삭제를 재전송하지 않아요."))).await};
    let missing = event["state"] == "deleted" || event["state"] == "missing";
    let (state,message)=match &attempt.action {
        Action::Delete if missing=>("deleted","내 기본 캘린더에 일정이 없는 상태를 확인했어요. 팀의 할 일은 그대로 유지했어요."),
        Action::Update{date} if raw.as_ref().is_some_and(|r|r["extendedProperties"]["private"]["saiChange"]==attempt.operation&&r["summary"]==attempt.title&&r["start"]["date"]==*date&&end(date).is_ok_and(|end|r["end"]["date"]==end))=>("updated","현재 업무의 제목과 확인한 날짜로 일정이 수정된 것을 확인했어요."),
        _=>("conflict","요청한 결과를 확인하지 못했거나 다른 변경이 도착했어요. 현재 일정은 유지하며 최신 내용을 다시 확인해 주세요."),
    };
    auth.with_lease(lease, || Ok(reply(review, state, event, message)))
        .await
}
