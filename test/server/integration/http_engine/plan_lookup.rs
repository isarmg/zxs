//! Missing local plan preparations are not retryable storage failures.
use super::*;

#[tokio::test]
async fn absent_plan_returns_not_found_without_publishing_an_operation() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    let baseline = success(&app, &token, "GET", "/api/events", None).await;
    let plan = new_id("plan");
    for (method, suffix, body) in [
        ("GET", "", None),
        (
            "POST",
            "/cancel",
            Some(json!({"operation_id":new_id("op")})),
        ),
        (
            "POST",
            "/commit",
            Some(json!({"operation_id":new_id("op"),"plan_digest":"0".repeat(64)})),
        ),
    ] {
        let (status, rejected) = send(
            &app,
            Some(&token),
            method,
            &format!("/api/plans/{plan}{suffix}"),
            body,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{rejected}");
        assert_eq!(rejected["error"]["code"], "PLAN_NOT_FOUND");
        assert_eq!(rejected["error"]["retryable"], false);
    }
    assert_eq!(
        success(&app, &token, "GET", "/api/events", None).await,
        baseline
    );
}
