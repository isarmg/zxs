//! Online registration is an authenticated refusal, not a storage command.
use super::*;

#[tokio::test]
async fn online_disk_registration_retains_offline_refusal_and_body_boundary() {
    let root = tempfile::tempdir().unwrap();
    let app = application(root.path());
    let token = login(&app, "admin", "integration-test-password").await;
    let baseline = success(&app, &token, "GET", "/api/events", None).await;
    for body in ["", "{}", "null", "[]", "17", r#"{"ignored":true}"#] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/admin/disks")
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let bytes = http_routing::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let error: Value = json_codec::from_slice(&bytes).unwrap();
        assert_eq!(error["error"]["code"], "OFFLINE_CONFIGURATION_REQUIRED");
        assert_eq!(
            error["error"]["message"],
            "disk registration requires an explicit offline configuration and topology migration"
        );
        assert_eq!(error["error"]["retryable"], false);
    }
    for body in [
        "{".to_owned(),
        r#"{"x":1,"x":2}"#.to_owned(),
        " ".repeat(1024 * 1024 + 1),
    ] {
        for authenticated in [false, true] {
            let mut request = Request::builder().method("POST").uri("/api/admin/disks");
            if authenticated {
                request = request.header("authorization", format!("Bearer {token}"));
            }
            let response = app
                .clone()
                .oneshot(request.body(Body::from(body.clone())).unwrap())
                .await
                .unwrap();
            if authenticated {
                assert_eq!(
                    response.status(),
                    if body.len() > 1024 * 1024 {
                        StatusCode::PAYLOAD_TOO_LARGE
                    } else {
                        StatusCode::BAD_REQUEST
                    }
                );
            } else {
                assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
            }
        }
    }
    assert_eq!(
        success(&app, &token, "GET", "/api/events", None).await,
        baseline
    );
    success(&app, &token, "POST", "/api/auth/logout", Some(json!({}))).await;
    let (status, error) = send(
        &app,
        Some(&token),
        "POST",
        "/api/admin/disks",
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(error["error"]["code"], "UNAUTHORIZED");
}
