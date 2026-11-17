//! Runs against the binary-safe, credential-free Google fixture.
use super::*;
use crate::google::{
    agenda::{self, Action, Add, Agenda, Change},
    collab::Collaboration,
};
struct Flow {
    f: Fixture,
    collab: Collaboration,
    heads: Vec<String>,
    agenda: Agenda,
}
impl Flow {
    async fn new(connected: bool) -> Self {
        let f = Fixture::new().await;
        let collab = Collaboration::new(f.root.0.join("calendar-work"));
        let saved=collab.save(&f.auth,&f.profiles,&f.teams,f.auth.epoch(),"team-folder",json!({"heads":[],"entity":{"id":"calendar-task","kind":"task","title":"Review the brief","projectId":"","assignee":"Another member","dueDate":"2026-11-20","status":"todo"}})).await.unwrap();
        let heads = serde_json::from_value(saved["items"][0]["heads"].clone()).unwrap();
        if connected {
            let lease = f.auth.lease().await.unwrap();
            f.auth
                .finish_calendar(&lease, "calendar", "v", "http://127.0.0.1/", "attempt")
                .await
                .unwrap();
        }
        Self {
            f,
            collab,
            heads,
            agenda: Agenda::default(),
        }
    }
    async fn preview(&self) -> Result<Value> {
        self.agenda
            .preview(
                &self.f.auth,
                &self.f.profiles,
                &self.f.teams,
                &self.collab,
                self.f.auth.epoch(),
                "team-folder",
                "calendar-task",
            )
            .await
    }
    async fn add(&self, date: &str, confirmed: bool) -> Result<Value> {
        agenda::add(
            &self.f.auth,
            &self.f.profiles,
            &self.f.teams,
            &self.collab,
            self.f.auth.epoch(),
            "team-folder",
            Add {
                task_id: "calendar-task".into(),
                heads: self.heads.clone(),
                date: date.into(),
                confirmed,
            },
        )
        .await
    }
    async fn edit(&mut self, status: &str) {
        let saved=self.collab.save(&self.f.auth,&self.f.profiles,&self.f.teams,self.f.auth.epoch(),"team-folder",json!({"heads":self.heads,"entity":{"id":"calendar-task","kind":"task","title":"Changed title","projectId":"","assignee":"","dueDate":"2026-11-21","status":status}})).await.unwrap();
        self.heads = serde_json::from_value(saved["items"][0]["heads"].clone()).unwrap();
    }
    async fn change(&self, review: &str, action: Action, confirmed: bool) -> Result<Value> {
        self.agenda
            .change(
                &self.f.auth,
                &self.f.profiles,
                &self.f.teams,
                &self.collab,
                self.f.auth.epoch(),
                "team-folder",
                Change {
                    review_id: review.into(),
                    action,
                    confirmed,
                },
            )
            .await
    }
    async fn check(&self, review: &str) -> Result<Value> {
        self.agenda
            .check(
                &self.f.auth,
                &self.f.profiles,
                &self.f.teams,
                self.f.auth.epoch(),
                "team-folder",
                review,
            )
            .await
    }
    fn event(&self) -> Value {
        self.f
            .api
            .calendar_events
            .lock()
            .unwrap()
            .values()
            .next()
            .unwrap()
            .clone()
    }
    fn alter(&self, f: impl FnOnce(&mut Value)) {
        f(self
            .f
            .api
            .calendar_events
            .lock()
            .unwrap()
            .values_mut()
            .next()
            .unwrap());
    }
}
fn update(date: &str) -> Action {
    Action::Update { date: date.into() }
}
#[tokio::test]
async fn preview_reads_the_current_event_without_exposing_its_private_fields_or_changing_it() {
    let x = Flow::new(true).await;
    x.add("2026-11-20", true).await.unwrap();
    x.alter(|e| {
        e["summary"] = json!("Title edited in Calendar");
        e["etag"] = json!("\"manual\"");
    });
    let p = x.preview().await.unwrap();
    assert_eq!(p["calendar"]["title"], "Title edited in Calendar");
    assert_eq!(p["calendar"]["manageable"], true);
    assert!(!p["reviewId"].as_str().unwrap().is_empty());
    assert!(!p.to_string().contains("saiSource"));
    assert!(!p.to_string().contains("etag"));
    assert_eq!(x.f.api.calendar_attempts.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn reviewed_update_changes_only_title_date_and_its_private_operation_marker() {
    let mut x = Flow::new(true).await;
    x.add("2026-11-20", true).await.unwrap();
    x.edit("todo").await;
    x.alter(|e| {
        e["extendedProperties"]["private"]["custom"] = json!("keep me");
        e["reminders"] = json!({"useDefault":true});
        e["etag"] = json!("\"custom\"");
    });
    let p = x.preview().await.unwrap();
    let review = p["reviewId"].as_str().unwrap();
    let saved = x.change(review, update("2026-12-31"), true).await.unwrap();
    assert_eq!(saved["state"], "updated");
    let e = x.event();
    assert_eq!(e["summary"], "Changed title");
    assert_eq!(e["start"]["date"], "2026-12-31");
    assert_eq!(e["end"]["date"], "2027-01-01");
    assert_eq!(e["extendedProperties"]["private"]["custom"], "keep me");
    assert_eq!(e["reminders"]["useDefault"], true);
    assert_eq!(x.check(review).await.unwrap()["state"], "updated");
    assert_eq!(
        x.change(review, update("2026-12-31"), true).await.unwrap()["state"],
        "updated"
    );
    assert!(x.change(review, Action::Delete, true).await.is_err());
    assert_eq!(x.f.api.calendar_attempts.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn completed_tasks_can_have_their_linked_event_deleted_without_deleting_team_work() {
    let mut x = Flow::new(true).await;
    x.add("2026-11-20", true).await.unwrap();
    x.edit("done").await;
    let posts = x.f.api.posts.load(Ordering::SeqCst);
    let p = x.preview().await.unwrap();
    let review = p["reviewId"].as_str().unwrap();
    assert!(x.change(review, Action::Delete, false).await.is_err());
    assert_eq!(x.f.api.calendar_attempts.load(Ordering::SeqCst), 0);
    assert!(x.change(review, update("2026-11-21"), true).await.is_err());
    assert_eq!(
        x.change(review, Action::Delete, true).await.unwrap()["state"],
        "deleted"
    );
    assert_eq!(x.check(review).await.unwrap()["state"], "deleted");
    assert_eq!(x.f.api.calendar_deletes.load(Ordering::SeqCst), 1);
    assert_eq!(x.f.api.posts.load(Ordering::SeqCst), posts);
    let latest = x.preview().await.unwrap();
    assert_eq!(latest["entity"]["status"], "done");
    assert_eq!(latest["calendar"]["state"], "deleted");
    assert!(latest.get("reviewId").is_none());
}
#[tokio::test]
async fn an_unchanged_event_or_read_only_verification_never_sends_a_mutation() {
    let x = Flow::new(true).await;
    x.add("2026-11-20", true).await.unwrap();
    let p = x.preview().await.unwrap();
    let review = p["reviewId"].as_str().unwrap();
    assert_eq!(x.check(review).await.unwrap()["state"], "notSent");
    assert_eq!(
        x.change(review, update("2026-11-20"), true).await.unwrap()["state"],
        "unchanged"
    );
    assert_eq!(x.f.api.calendar_attempts.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn calendar_changes_after_preview_are_preserved_and_require_another_review() {
    let x = Flow::new(true).await;
    x.add("2026-11-20", true).await.unwrap();
    let p = x.preview().await.unwrap();
    let review = p["reviewId"].as_str().unwrap();
    x.alter(|e| {
        e["summary"] = json!("Keep this user edit");
        e["etag"] = json!("\"user-edit\"");
    });
    for action in [update("2026-11-21"), Action::Delete] {
        assert_eq!(
            x.change(review, action, true).await.unwrap()["state"],
            "conflict"
        );
    }
    assert_eq!(x.event()["summary"], "Keep this user edit");
    assert_eq!(x.f.api.calendar_attempts.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn if_match_prevents_an_edit_or_deletion_racing_between_get_and_write() {
    for action in [update("2026-11-21"), Action::Delete] {
        let x = Flow::new(true).await;
        x.add("2026-11-20", true).await.unwrap();
        let p = x.preview().await.unwrap();
        let review = p["reviewId"].as_str().unwrap();
        x.f.api.calendar_race.store(true, Ordering::SeqCst);
        assert_eq!(
            x.change(review, action.clone(), true).await.unwrap()["state"],
            "conflict"
        );
        assert_eq!(x.event()["summary"], "Changed during request");
        assert_eq!(x.f.api.calendar_changes.load(Ordering::SeqCst), 0);
        assert_eq!(x.f.api.calendar_deletes.load(Ordering::SeqCst), 0);
        x.check(review).await.unwrap();
        x.change(review, action, true).await.unwrap();
        assert_eq!(x.f.api.calendar_attempts.load(Ordering::SeqCst), 1);
    }
}
#[tokio::test]
async fn ambiguous_update_and_delete_responses_are_reconciled_without_repeating_writes() {
    for action in [update("2026-11-21"), Action::Delete] {
        let x = Flow::new(true).await;
        x.add("2026-11-20", true).await.unwrap();
        let p = x.preview().await.unwrap();
        let review = p["reviewId"].as_str().unwrap();
        x.f.api
            .calendar_change_ambiguous
            .store(true, Ordering::SeqCst);
        let expected = if action == Action::Delete {
            "deleted"
        } else {
            "updated"
        };
        assert_eq!(
            x.change(review, action.clone(), true).await.unwrap()["state"],
            expected
        );
        assert_eq!(x.check(review).await.unwrap()["state"], expected);
        assert_eq!(
            x.change(review, action, true).await.unwrap()["state"],
            expected
        );
        assert_eq!(x.f.api.calendar_attempts.load(Ordering::SeqCst), 1);
    }
}
#[tokio::test]
async fn a_lost_write_and_read_response_can_later_be_verified_with_get_only() {
    for action in [update("2026-11-21"), Action::Delete] {
        let x = Flow::new(true).await;
        x.add("2026-11-20", true).await.unwrap();
        let p = x.preview().await.unwrap();
        let review = p["reviewId"].as_str().unwrap();
        x.f.api
            .calendar_hide_after_change
            .store(true, Ordering::SeqCst);
        assert_eq!(
            x.change(review, action.clone(), true).await.unwrap()["state"],
            "uncertain"
        );
        assert_eq!(x.check(review).await.unwrap()["state"], "uncertain");
        x.f.api.calendar_reads_fail.store(false, Ordering::SeqCst);
        let expected = if action == Action::Delete {
            "deleted"
        } else {
            "updated"
        };
        assert_eq!(x.check(review).await.unwrap()["state"], expected);
        assert_eq!(x.f.api.calendar_attempts.load(Ordering::SeqCst), 1);
    }
}
#[tokio::test]
async fn two_pc_review_sessions_cannot_overwrite_each_others_newer_calendar_state() {
    let x = Flow::new(true).await;
    x.add("2026-11-20", true).await.unwrap();
    let p = x.preview().await.unwrap();
    let other = Agenda::default();
    let other_collab = Collaboration::new(x.f.root.0.join("another-pc-work"));
    let q = other
        .preview(
            &x.f.auth,
            &x.f.profiles,
            &x.f.teams,
            &other_collab,
            x.f.auth.epoch(),
            "team-folder",
            "calendar-task",
        )
        .await
        .unwrap();
    assert_eq!(
        x.change(p["reviewId"].as_str().unwrap(), update("2026-11-21"), true)
            .await
            .unwrap()["state"],
        "updated"
    );
    let r = other
        .change(
            &x.f.auth,
            &x.f.profiles,
            &x.f.teams,
            &other_collab,
            x.f.auth.epoch(),
            "team-folder",
            Change {
                review_id: q["reviewId"].as_str().unwrap().into(),
                action: Action::Delete,
                confirmed: true,
            },
        )
        .await
        .unwrap();
    assert_eq!(r["state"], "conflict");
    assert_eq!(x.event()["start"]["date"], "2026-11-21");
    assert_eq!(x.f.api.calendar_attempts.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn complex_or_unversioned_events_cannot_be_changed_even_using_an_older_review() {
    let variants = vec![
        json!({"attendees":[{"email":"guest@example.invalid"}]}),
        json!({"recurrence":["RRULE:FREQ=DAILY"]}),
        json!({"start":{"dateTime":"2026-11-20T10:00:00+09:00"}}),
        json!({"description":"Private notes in Calendar"}),
        json!({"end":{"date":"2026-11-25"}}),
        json!({"etag":"*"}),
    ];
    for changes in variants {
        let x = Flow::new(true).await;
        x.add("2026-11-20", true).await.unwrap();
        let before = x.preview().await.unwrap();
        x.alter(|e| {
            for (k, v) in changes.as_object().unwrap() {
                e[k] = v.clone();
            }
        });
        let p = x.preview().await.unwrap();
        assert_eq!(p["calendar"]["manageable"], false);
        assert!(p.get("reviewId").is_none());
        assert_eq!(
            x.change(before["reviewId"].as_str().unwrap(), Action::Delete, true)
                .await
                .unwrap()["state"],
            "conflict"
        );
        assert_eq!(x.f.api.calendar_attempts.load(Ordering::SeqCst), 0);
    }
}
#[tokio::test]
async fn source_permission_account_team_and_closed_review_checks_prevent_calendar_writes() {
    let mut x = Flow::new(true).await;
    x.add("2026-11-20", true).await.unwrap();
    let p = x.preview().await.unwrap();
    let review = p["reviewId"].as_str().unwrap();
    x.edit("todo").await;
    assert!(x.change(review, update("2026-11-21"), true).await.is_err());
    assert_eq!(x.check(review).await.unwrap()["state"], "notSent");
    let p = x.preview().await.unwrap();
    let review = p["reviewId"].as_str().unwrap();
    assert!(x
        .agenda
        .change(
            &x.f.auth,
            &x.f.profiles,
            &x.f.teams,
            &x.collab,
            x.f.auth.epoch(),
            "other-folder",
            Change {
                review_id: review.into(),
                action: Action::Delete,
                confirmed: true
            }
        )
        .await
        .is_err());
    x.agenda.forget(x.f.auth.epoch(), "team-folder", review);
    assert!(x.change(review, Action::Delete, true).await.is_err());
    let p = x.preview().await.unwrap();
    x.f.api.revoked.store(true, Ordering::SeqCst);
    assert!(x
        .change(p["reviewId"].as_str().unwrap(), Action::Delete, true)
        .await
        .is_err());
    x.f.api.revoked.store(false, Ordering::SeqCst);
    let p = x.preview().await.unwrap();
    x.f.api.second_account.store(true, Ordering::SeqCst);
    x.f.auth
        .exchange_code("switch", "v", "http://127.0.0.1/", "attempt")
        .await
        .unwrap();
    assert!(x
        .change(p["reviewId"].as_str().unwrap(), Action::Delete, true)
        .await
        .is_err());
    assert_eq!(x.f.api.calendar_attempts.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn unavailable_or_foreign_calendar_data_is_not_treated_as_a_missing_event() {
    let x = Flow::new(true).await;
    x.add("2026-11-20", true).await.unwrap();
    x.f.api.calendar_reads_fail.store(true, Ordering::SeqCst);
    let p = x.preview().await.unwrap();
    assert_eq!(p["calendar"]["state"], "unavailable");
    assert!(p.get("reviewId").is_none());
    x.f.api.calendar_reads_fail.store(false, Ordering::SeqCst);
    x.alter(|e| e["extendedProperties"] = json!({}));
    let p = x.preview().await.unwrap();
    assert_eq!(p["calendar"]["state"], "unavailable");
    assert!(p.get("reviewId").is_none());
    assert_eq!(x.f.api.calendar_attempts.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn preview_and_missing_consent_or_calendar_permission_never_create_an_event() {
    let x = Flow::new(false).await;
    let v = x.preview().await.unwrap();
    assert_eq!(v["entity"]["title"], "Review the brief");
    assert_eq!(v["connected"], false);
    assert_eq!(v["email"], "user@example.invalid");
    assert!(x.add("2026-11-20", true).await.is_err());
    assert!(x.add("2026-11-20", false).await.is_err());
    assert_eq!(x.f.api.calendar_posts.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn an_explicit_date_creates_a_private_all_day_event_without_inviting_the_assignee() {
    let x = Flow::new(true).await;
    let r = x.add("2026-12-31", true).await.unwrap();
    assert_eq!(r["state"], "registered");
    let events = x.f.api.calendar_events.lock().unwrap();
    assert_eq!(events.len(), 1);
    let e = events.values().next().unwrap();
    assert_eq!(e["start"]["date"], "2026-12-31");
    assert_eq!(e["end"]["date"], "2027-01-01");
    assert_eq!(e["summary"], "Review the brief");
    assert!(!e.to_string().contains("Another member"));
}
#[tokio::test]
async fn retry_after_a_lost_response_and_another_pc_reuses_one_event() {
    let x = Flow::new(true).await;
    x.f.api.calendar_ambiguous.store(true, Ordering::SeqCst);
    assert!(x.add("2026-11-20", true).await.is_err());
    assert_eq!(
        x.add("2026-11-20", true).await.unwrap()["state"],
        "existing"
    );
    let other = Collaboration::new(x.f.root.0.join("another-pc"));
    let r = agenda::add(
        &x.f.auth,
        &x.f.profiles,
        &x.f.teams,
        &other,
        x.f.auth.epoch(),
        "team-folder",
        Add {
            task_id: "calendar-task".into(),
            heads: x.heads.clone(),
            date: "2026-11-21".into(),
            confirmed: true,
        },
    )
    .await
    .unwrap();
    assert_eq!(r["state"], "existing");
    assert_eq!(r["date"], "2026-11-20");
    assert_eq!(x.f.api.calendar_posts.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn simultaneous_first_additions_use_one_id_and_never_overwrite() {
    let x = Flow::new(true).await;
    let (a, b) = tokio::join!(x.add("2026-11-20", true), x.add("2026-11-21", true));
    a.unwrap();
    b.unwrap();
    assert_eq!(x.f.api.calendar_events.lock().unwrap().len(), 1);
    assert_eq!(x.f.api.calendar_posts.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn user_edits_and_deleted_events_are_preserved_and_not_recreated() {
    let x = Flow::new(true).await;
    x.add("2026-11-20", true).await.unwrap();
    {
        let mut events = x.f.api.calendar_events.lock().unwrap();
        let e = events.values_mut().next().unwrap();
        e["summary"] = json!("User edited title");
        e["start"] = json!({"dateTime":"2026-11-22T12:00:00+09:00","timeZone":"Asia/Seoul"});
    }
    let r = x.add("2026-11-20", true).await.unwrap();
    assert_eq!(r["state"], "existing");
    assert_eq!(r["title"], "User edited title");
    assert_eq!(r["dateTime"], "2026-11-22T12:00:00+09:00");
    x.f.api
        .calendar_events
        .lock()
        .unwrap()
        .values_mut()
        .next()
        .unwrap()["status"] = json!("cancelled");
    assert_eq!(x.add("2026-11-20", true).await.unwrap()["state"], "deleted");
    assert_eq!(x.f.api.calendar_posts.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn stale_completed_cross_team_and_conflicting_tasks_cannot_create_events() {
    let mut x = Flow::new(true).await;
    let old = x.heads.clone();
    x.edit("todo").await;
    x.heads = old;
    assert!(x.add("2026-11-20", true).await.is_err());
    let view = x.preview().await.unwrap();
    x.heads = serde_json::from_value(view["heads"].clone()).unwrap();
    x.edit("done").await;
    assert_eq!(x.preview().await.unwrap()["entity"]["status"], "done");
    assert!(x.add("2026-11-20", true).await.is_err());
    assert!(x
        .agenda
        .preview(
            &x.f.auth,
            &x.f.profiles,
            &x.f.teams,
            &x.collab,
            x.f.auth.epoch(),
            "other-folder",
            "calendar-task"
        )
        .await
        .is_err());
    assert_eq!(x.f.api.calendar_posts.load(Ordering::SeqCst), 0);
    let x = Flow::new(true).await;
    let mut record: Value = {
        let files = x.f.api.files.lock().unwrap();
        let (_, bytes) = files
            .values()
            .find(|(m, _)| m["properties"]["saiCollab"] == "1")
            .unwrap();
        serde_json::from_slice(bytes).unwrap()
    };
    record["opId"] = json!("peer-fork");
    record["parents"] = json!([]);
    record["entity"]["title"] = json!("Peer edit");
    x.f.api
        .add("fork-file", "team-folder", record.to_string().as_bytes());
    {
        let mut files = x.f.api.files.lock().unwrap();
        let meta = &mut files.get_mut("fork-file").unwrap().0;
        meta["name"] = json!("sai-work-v1-peer-fork.json");
        meta["mimeType"] = json!("application/json");
        meta["properties"] = json!({"saiCollab":"1"});
    }
    assert!(x.preview().await.is_err());
    assert!(x.add("2026-11-20", true).await.is_err());
    assert_eq!(x.f.api.calendar_posts.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn permission_revoked_between_preflight_and_write_prevents_creation() {
    let x = Flow::new(true).await;
    x.f.api
        .calendar_revoke_before_write
        .store(true, Ordering::SeqCst);
    assert!(x.add("2026-11-20", true).await.is_err());
    assert_eq!(x.f.api.calendar_posts.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn calendar_denial_keeps_the_drive_login_and_does_not_expose_upstream_details() {
    let x = Flow::new(true).await;
    x.f.api.calendar_denied.store(true, Ordering::SeqCst);
    let e = x.add("2026-11-20", true).await.unwrap_err();
    assert!(!e.contains("private upstream"));
    assert!(!x.f.auth.calendar_connected());
    assert!(x.f.auth.has_full_drive());
    assert_eq!(x.f.api.calendar_posts.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn invalid_dates_forged_payloads_and_foreign_events_are_never_written_over() {
    let x = Flow::new(true).await;
    for date in ["2026-02-30", "2026-2-03", "0000-01-01", "9999-12-31", ""] {
        assert!(x.add(date, true).await.is_err());
    }
    assert!(serde_json::from_value::<Add>(json!({"taskId":"calendar-task","heads":x.heads,"date":"2026-11-20","confirmed":true,"calendarId":"someone-else"})).is_err());
    x.add("2026-11-20", true).await.unwrap();
    x.f.api
        .calendar_events
        .lock()
        .unwrap()
        .values_mut()
        .next()
        .unwrap()["extendedProperties"] = json!({});
    assert!(x.add("2026-11-20", true).await.is_err());
    assert_eq!(x.f.api.calendar_posts.load(Ordering::SeqCst), 1);
}
