//! Google 캘린더: 바쁜 시간(free/busy)만 읽어 빈 시간을 계산하고, 작업 시간을 일정으로 잡는다.
//! 일정 제목·내용·참석자는 읽지 않는다 (freebusy 범위만 사용).
use super::oauth::GoogleAuth;
use crate::schedule::time_plan::{gaps, merge, Interval};
use crate::time::{add_days, start_of_day, to_rfc3339, HOUR};
use crate::util::J;
use crate::Result;
use serde_json::json;

const CAL: &str = "https://www.googleapis.com/calendar/v3";
const TZ: &str = "Asia/Seoul";

pub async fn query_busy(auth: &GoogleAuth, from: i64, to: i64) -> Result<Vec<Interval>> {
    let d = auth
        .api("POST", &format!("{CAL}/freeBusy"), Some(json!({ "timeMin": to_rfc3339(from), "timeMax": to_rfc3339(to), "timeZone": TZ, "items": [{ "id": "primary" }] })))
        .await
        .map_err(|e| e.1)?;
    let cal = &d["calendars"]["primary"];
    if let Some(e) = cal.arr("errors").first() {
        return Err(format!("캘린더를 읽지 못했어요: {}", e.s("reason")));
    }
    Ok(cal
        .arr("busy")
        .iter()
        .filter_map(|b| Some(Interval { start: crate::time::parse_rfc3339(&b.s("start"))?, end: crate::time::parse_rfc3339(&b.s("end"))? }))
        .collect())
}

/// 하루 중 일할 수 있는 시간대(예: 9~22시)에서 바쁜 시간을 뺀 빈 시간 (시간 단위)
pub fn free_hours(busy: &[Interval], from: i64, to: i64, day_start: u32, day_end: u32) -> f64 {
    let merged = merge(busy.to_vec());
    let mut total = 0i64;
    let mut day = start_of_day(from);
    while day < to {
        let ws = from.max(day + day_start as i64 * HOUR);
        let we = to.min(day + day_end as i64 * HOUR);
        if we > ws {
            total += gaps(ws, we, &merged).iter().map(|g| g.end - g.start).sum::<i64>();
        }
        day = add_days(day, 1);
    }
    crate::util::round1(total as f64 / HOUR as f64)
}

pub async fn create_event(auth: &GoogleAuth, summary: &str, description: &str, start: i64, end: i64, sai_task_id: &str) -> Result<String> {
    let d = auth
        .api(
            "POST",
            &format!("{CAL}/calendars/primary/events"),
            Some(json!({
                "summary": summary,
                "description": description,
                "start": { "dateTime": to_rfc3339(start), "timeZone": TZ },
                "end": { "dateTime": to_rfc3339(end), "timeZone": TZ },
                "reminders": { "useDefault": true },
                "extendedProperties": { "private": { "saiTaskId": sai_task_id } },
            })),
        )
        .await
        .map_err(|e| e.1)?;
    Ok(d.s("id"))
}

/// 사용자가 캘린더에서 지운 일정이면 Err
pub async fn patch_event(auth: &GoogleAuth, event_id: &str, summary: &str, start: i64, end: i64) -> Result<()> {
    let r = auth
        .api(
            "PATCH",
            &format!("{CAL}/calendars/primary/events/{}", urlencoding::encode(event_id)),
            Some(json!({
                "summary": summary,
                "start": { "dateTime": to_rfc3339(start), "timeZone": TZ },
                "end": { "dateTime": to_rfc3339(end), "timeZone": TZ },
            })),
        )
        .await
        .map_err(|e| e.1)?;
    if r.s("status") == "cancelled" {
        return Err("지워진 일정이에요.".into());
    }
    Ok(())
}

pub async fn delete_event(auth: &GoogleAuth, event_id: &str) -> Result<()> {
    match auth.api("DELETE", &format!("{CAL}/calendars/primary/events/{}", urlencoding::encode(event_id)), None).await {
        Ok(_) => Ok(()),
        Err((404, _)) | Err((410, _)) => Ok(()),
        Err((_, e)) => Err(e),
    }
}
