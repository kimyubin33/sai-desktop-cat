//! Runs inside the binary-safe Google mock fixture in files::tests.
use super::*;
use crate::google::{
    collab::Collaboration,
    review::{Choice, Input, Reviews},
};
const SOURCE: &str = "Review the brief. Share notes by 2026-11-20.";
struct Flow {
    f: Fixture,
    reviews: Reviews,
    collab: Collaboration,
}
impl Flow {
    async fn new() -> Self {
        let f = Fixture::new().await;
        let reviews = Reviews::new(f.root.0.join("reviews"));
        let collab = Collaboration::new(f.root.0.join("work"));
        let lease = f.auth.lease().await.unwrap();
        f.auth
            .set_team_ai_consent(&lease, &f._server.url, true)
            .await
            .unwrap();
        Self { f, reviews, collab }
    }
    async fn prepare(&self, input: Input) -> Result<Value> {
        self.reviews
            .prepare(
                &self.f.auth,
                &self.f.profiles,
                &self.f.teams,
                &self.f.files,
                &self.collab,
                self.f.auth.epoch(),
                "team-folder",
                input,
            )
            .await
    }
    async fn text(&self) -> Value {
        self.prepare(Input::Text {
            text: SOURCE.into(),
        })
        .await
        .unwrap()
    }
    async fn run(&self, id: &str, confirmed: bool, configured: bool) -> Result<Value> {
        self.reviews
            .run(
                &self.f.auth,
                &self.f.profiles,
                &self.f.teams,
                &self.f.files,
                &self.collab,
                self.f.auth.epoch(),
                "team-folder",
                id,
                confirmed,
                configured.then_some(self.f._server.url.as_str()),
            )
            .await
    }
    async fn commit(&self, id: &str, value: Value, confirmed: bool) -> Result<Value> {
        let choices: Vec<Choice> = serde_json::from_value(value).unwrap();
        self.reviews
            .commit(
                &self.f.auth,
                &self.f.profiles,
                &self.f.teams,
                &self.f.files,
                &self.collab,
                self.f.auth.epoch(),
                "team-folder",
                id,
                choices,
                confirmed,
            )
            .await
    }
    fn source(&self, doc: bool) {
        self.f
            .api
            .add("source-file", "team-folder", SOURCE.as_bytes());
        let mut files = self.f.api.files.lock().unwrap();
        let m = &mut files.get_mut("source-file").unwrap().0;
        m["name"] = json!(if doc { "Brief" } else { "brief.txt" });
        m["mimeType"] = json!(if doc {
            "application/vnd.google-apps.document"
        } else {
            "text/plain"
        });
    }
}
fn choices() -> Value {
    json!([{"index":0,"title":"Reviewed title","assignee":"Reviewed member","dueDate":"2026-11-21","projectId":""},{"index":1,"title":"Share notes","assignee":"","dueDate":"","projectId":""}])
}
#[tokio::test]
async fn preview_is_exact_and_does_not_call_ai_or_write_source_to_disk() {
    let x = Flow::new().await;
    let p = x.text().await;
    assert_eq!(p["text"], SOURCE);
    assert_eq!(x.f.api.ai_calls.load(Ordering::SeqCst), 0);
    assert_eq!(x.f.api.posts.load(Ordering::SeqCst), 0);
    assert!(!x.f.root.0.join("reviews").exists());
    assert_eq!(p["existing"], json!([]));
}
#[tokio::test]
async fn file_and_google_document_previews_are_bounded_real_text() {
    for doc in [false, true] {
        let x = Flow::new().await;
        x.source(doc);
        let p = x
            .prepare(Input::Drive {
                file_id: "source-file".into(),
            })
            .await
            .unwrap();
        assert_eq!(p["text"], SOURCE);
        assert_eq!(x.f.api.ai_calls.load(Ordering::SeqCst), 0);
    }
}
#[tokio::test]
async fn exported_documents_are_never_truncated_and_unsupported_files_stay_private() {
    let x = Flow::new().await;
    x.source(true);
    x.f.api
        .files
        .lock()
        .unwrap()
        .get_mut("source-file")
        .unwrap()
        .1 = vec![b'a'; 24 * 1024 + 1];
    assert!(x
        .prepare(Input::Drive {
            file_id: "source-file".into()
        })
        .await
        .is_err());
    x.f.api.add("pdf", "team-folder", b"%PDF-private");
    {
        let mut files = x.f.api.files.lock().unwrap();
        let meta = &mut files.get_mut("pdf").unwrap().0;
        meta["name"] = json!("private.pdf");
        meta["mimeType"] = json!("application/pdf");
    }
    assert!(x
        .prepare(Input::Drive {
            file_id: "pdf".into()
        })
        .await
        .is_err());
    assert_eq!(x.f.api.ai_calls.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn a_source_modified_after_analysis_cannot_be_shared() {
    let x = Flow::new().await;
    x.source(false);
    let p = x
        .prepare(Input::Drive {
            file_id: "source-file".into(),
        })
        .await
        .unwrap();
    let id = p["reviewId"].as_str().unwrap();
    x.run(id, true, true).await.unwrap();
    x.f.api
        .files
        .lock()
        .unwrap()
        .get_mut("source-file")
        .unwrap()
        .0["version"] = json!("2");
    assert!(x.commit(id, choices(), true).await.is_err());
    assert_eq!(x.f.api.posts.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn missing_send_consent_or_deployment_never_calls_the_provider() {
    let x = Flow::new().await;
    let p = x.text().await;
    let id = p["reviewId"].as_str().unwrap();
    assert!(x.run(id, false, true).await.is_err());
    assert!(x.run(id, true, false).await.is_err());
    assert_eq!(x.f.api.ai_calls.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn a_successful_analysis_is_cached_and_only_reviewed_fields_are_shared() {
    let x = Flow::new().await;
    let p = x.text().await;
    let id = p["reviewId"].as_str().unwrap();
    let result = x.run(id, true, true).await.unwrap();
    assert_eq!(result["result"]["candidates"].as_array().unwrap().len(), 2);
    x.run(id, true, true).await.unwrap();
    assert_eq!(x.f.api.ai_calls.load(Ordering::SeqCst), 1);
    assert!(x.commit(id, choices(), false).await.is_err());
    assert_eq!(x.f.api.posts.load(Ordering::SeqCst), 0);
    let saved = x.commit(id, choices(), true).await.unwrap();
    assert_eq!(saved["added"], 2);
    assert_eq!(saved["state"]["pending"], 0);
    let all = x.f.api.files.lock().unwrap();
    let records = all
        .values()
        .map(|(_, bytes)| String::from_utf8_lossy(bytes))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(records.contains("Reviewed title"));
    assert!(!records.contains(SOURCE));
    assert!(!records.contains("A request to review"));
}
#[tokio::test]
async fn repeated_commit_and_another_pc_cannot_duplicate_source_tasks() {
    let x = Flow::new().await;
    let p = x.text().await;
    let id = p["reviewId"].as_str().unwrap();
    x.run(id, true, true).await.unwrap();
    x.commit(id, choices(), true).await.unwrap();
    let again = x.commit(id, choices(), true).await.unwrap();
    assert_eq!(again["alreadyImported"], true);
    assert_eq!(x.f.api.posts.load(Ordering::SeqCst), 2);
    let other = Reviews::new(x.f.root.0.join("second-pc"));
    let other_collab = Collaboration::new(x.f.root.0.join("second-pc-work"));
    let p = other
        .prepare(
            &x.f.auth,
            &x.f.profiles,
            &x.f.teams,
            &x.f.files,
            &other_collab,
            x.f.auth.epoch(),
            "team-folder",
            Input::Text {
                text: format!("  {SOURCE}  "),
            },
        )
        .await
        .unwrap();
    assert_eq!(p["existing"].as_array().unwrap().len(), 2);
    assert!(other
        .run(
            &x.f.auth,
            &x.f.profiles,
            &x.f.teams,
            &x.f.files,
            &other_collab,
            x.f.auth.epoch(),
            "team-folder",
            p["reviewId"].as_str().unwrap(),
            true,
            Some(&x.f._server.url)
        )
        .await
        .is_err());
    assert_eq!(x.f.api.ai_calls.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn unchecked_candidates_are_not_shared() {
    let x = Flow::new().await;
    let p = x.text().await;
    let id = p["reviewId"].as_str().unwrap();
    x.run(id, true, true).await.unwrap();
    let selected = json!([choices()[1]]);
    let saved = x.commit(id, selected, true).await.unwrap();
    assert_eq!(saved["added"], 1);
    let all = x.f.api.files.lock().unwrap();
    let body = all
        .values()
        .map(|(_, b)| String::from_utf8_lossy(b))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(body.contains("Share notes"));
    assert!(!body.contains("Reviewed title"));
}
#[tokio::test]
async fn changing_a_drive_file_cannot_bypass_its_existing_task_group() {
    let x = Flow::new().await;
    x.source(false);
    let p = x
        .prepare(Input::Drive {
            file_id: "source-file".into(),
        })
        .await
        .unwrap();
    let id = p["reviewId"].as_str().unwrap();
    x.run(id, true, true).await.unwrap();
    x.commit(id, choices(), true).await.unwrap();
    x.f.api
        .files
        .lock()
        .unwrap()
        .get_mut("source-file")
        .unwrap()
        .0["version"] = json!("2");
    let p = x
        .prepare(Input::Drive {
            file_id: "source-file".into(),
        })
        .await
        .unwrap();
    assert_eq!(p["existing"].as_array().unwrap().len(), 2);
    assert!(x
        .run(p["reviewId"].as_str().unwrap(), true, true)
        .await
        .is_err());
    assert_eq!(x.f.api.ai_calls.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn failed_or_lost_requests_are_not_automatically_retried_and_restart_warns() {
    let x = Flow::new().await;
    let p = x.text().await;
    let id = p["reviewId"].as_str().unwrap();
    x.f.api.ai_fail.store(true, Ordering::SeqCst);
    assert!(x.run(id, true, true).await.is_err());
    x.f.api.ai_fail.store(false, Ordering::SeqCst);
    assert!(x.run(id, true, true).await.is_err());
    assert_eq!(x.f.api.ai_calls.load(Ordering::SeqCst), 1);
    let reopened = Reviews::new(x.f.root.0.join("reviews"));
    let p = reopened
        .prepare(
            &x.f.auth,
            &x.f.profiles,
            &x.f.teams,
            &x.f.files,
            &x.collab,
            x.f.auth.epoch(),
            "team-folder",
            Input::Text {
                text: SOURCE.into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(p["priorAttempt"], true);
    assert_eq!(x.f.api.ai_calls.load(Ordering::SeqCst), 1);
    for entry in std::fs::read_dir(x.f.root.0.join("reviews")).unwrap() {
        let body = std::fs::read_to_string(entry.unwrap().path()).unwrap();
        assert!(!body.contains(SOURCE));
        assert!(!body.contains("A request to review"));
    }
}
#[tokio::test]
async fn invalid_or_forged_choices_have_no_partial_writes_and_can_be_corrected() {
    let x = Flow::new().await;
    let p = x.text().await;
    let id = p["reviewId"].as_str().unwrap();
    x.run(id, true, true).await.unwrap();
    let mut invalid = choices();
    invalid[1]["title"] = json!("");
    assert!(x.commit(id, invalid, true).await.is_err());
    let mut forged = choices();
    forged[1]["index"] = json!(8);
    assert!(x.commit(id, forged, true).await.is_err());
    let mut project = choices();
    project[0]["projectId"] = json!("missing-project");
    assert!(x.commit(id, project, true).await.is_err());
    assert_eq!(x.f.api.posts.load(Ordering::SeqCst), 0);
    assert_eq!(x.commit(id, choices(), true).await.unwrap()["added"], 2);
}
#[tokio::test]
async fn an_ambiguous_task_upload_is_retained_and_retry_never_creates_duplicate_tasks() {
    let x = Flow::new().await;
    let p = x.text().await;
    let id = p["reviewId"].as_str().unwrap();
    x.run(id, true, true).await.unwrap();
    x.f.api.ambiguous.store(true, Ordering::SeqCst);
    let saved = x.commit(id, choices(), true).await.unwrap();
    assert_eq!(saved["state"]["pending"], 2);
    let retried = x.commit(id, choices(), true).await.unwrap();
    assert_eq!(retried["state"]["pending"], 0);
    assert_eq!(x.f.api.posts.load(Ordering::SeqCst), 2);
}
#[tokio::test]
async fn stale_source_and_permissions_hide_results_and_prevent_sharing() {
    for revoked in [false, true] {
        let x = Flow::new().await;
        x.source(false);
        let p = x
            .prepare(Input::Drive {
                file_id: "source-file".into(),
            })
            .await
            .unwrap();
        if revoked {
            x.f.api.ai_revoked.store(true, Ordering::SeqCst);
        } else {
            x.f.api.ai_changed.store(true, Ordering::SeqCst);
        }
        assert!(x
            .run(p["reviewId"].as_str().unwrap(), true, true)
            .await
            .is_err());
        assert_eq!(x.f.api.ai_calls.load(Ordering::SeqCst), 1);
        assert!(x
            .commit(p["reviewId"].as_str().unwrap(), choices(), true)
            .await
            .is_err());
        assert_eq!(x.f.api.posts.load(Ordering::SeqCst), 0);
    }
}
#[tokio::test]
async fn changed_files_before_analysis_or_commit_and_read_only_teams_cannot_write() {
    let x = Flow::new().await;
    x.source(false);
    let p = x
        .prepare(Input::Drive {
            file_id: "source-file".into(),
        })
        .await
        .unwrap();
    let id = p["reviewId"].as_str().unwrap();
    x.f.api
        .files
        .lock()
        .unwrap()
        .get_mut("source-file")
        .unwrap()
        .0["version"] = json!("2");
    assert!(x.run(id, true, true).await.is_err());
    assert_eq!(x.f.api.ai_calls.load(Ordering::SeqCst), 0);
    let p = x.text().await;
    let id = p["reviewId"].as_str().unwrap();
    x.run(id, true, true).await.unwrap();
    x.f.api.read_only.store(true, Ordering::SeqCst);
    assert!(x.commit(id, choices(), true).await.is_err());
    assert_eq!(x.f.api.posts.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn a_review_cannot_cross_an_account_team_or_explicit_close() {
    let x = Flow::new().await;
    let p = x.text().await;
    let id = p["reviewId"].as_str().unwrap();
    assert!(x
        .reviews
        .run(
            &x.f.auth,
            &x.f.profiles,
            &x.f.teams,
            &x.f.files,
            &x.collab,
            x.f.auth.epoch(),
            "other-team",
            id,
            true,
            Some(&x.f._server.url)
        )
        .await
        .is_err());
    x.reviews.forget(x.f.auth.epoch(), "team-folder", id);
    assert!(x.run(id, true, true).await.is_err());
    let p = x.text().await;
    x.f.auth.sign_out().await.unwrap();
    assert!(x
        .run(p["reviewId"].as_str().unwrap(), true, true)
        .await
        .is_err());
    assert_eq!(x.f.api.ai_calls.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn malformed_utf8_oversize_and_corrupt_attempt_metadata_fail_without_paid_requests() {
    let x = Flow::new().await;
    assert!(x
        .prepare(Input::Text {
            text: "a".repeat(24577)
        })
        .await
        .is_err());
    x.f.api.add("bad", "team-folder", &[0xff, 0x00]);
    {
        let mut f = x.f.api.files.lock().unwrap();
        f.get_mut("bad").unwrap().0["name"] = json!("bad.txt");
    }
    assert!(x
        .prepare(Input::Drive {
            file_id: "bad".into()
        })
        .await
        .is_err());
    let p = x.text().await;
    x.f.api.ai_fail.store(true, Ordering::SeqCst);
    let _ = x.run(p["reviewId"].as_str().unwrap(), true, true).await;
    let path = std::fs::read_dir(x.f.root.0.join("reviews"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    std::fs::write(&path, b"{broken").unwrap();
    let reopened = Reviews::new(x.f.root.0.join("reviews"));
    assert!(reopened
        .prepare(
            &x.f.auth,
            &x.f.profiles,
            &x.f.teams,
            &x.f.files,
            &x.collab,
            x.f.auth.epoch(),
            "team-folder",
            Input::Text {
                text: SOURCE.into()
            }
        )
        .await
        .is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"{broken");
    assert_eq!(x.f.api.ai_calls.load(Ordering::SeqCst), 1);
}
