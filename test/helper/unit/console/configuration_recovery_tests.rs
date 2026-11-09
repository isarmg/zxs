use super::*;
use http_routing::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use json_codec::{Value, json};
use service_stack::ServiceExt;
fn state() -> State {
    let (stopping, _receiver) = watch::channel(false);
    State::new(
        SocketAddr::from(([0, 0, 0, 0], 9040)),
        "configuration-csrf".into(),
        stopping,
    )
}
fn post(path: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(path)
        .header("host", "127.0.0.1:9040")
        .header("origin", "http://127.0.0.1:9040")
        .header("x-zxs-helper-csrf", "configuration-csrf")
        .header("content-type", "application/json")
        .body(Body::from(json_codec::to_vec(&body).unwrap()))
        .unwrap()
}
#[tokio::test]
async fn both_original_configuration_routes_keep_csrf_and_one_owner() {
    for path in [
        "/api/deployment/install-recover-preview",
        "/api/deployment/install-recover-files",
    ] {
        let mut request = post(path, json!({}));
        request.headers_mut().remove("x-zxs-helper-csrf");
        assert_eq!(
            http::router(state())
                .oneshot(request)
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        let state = state();
        let _owner = Arc::clone(&state.jobs).try_acquire_owned().unwrap();
        assert_eq!(
            http::router(state)
                .oneshot(post(path, json!({})))
                .await
                .unwrap()
                .status(),
            StatusCode::CONFLICT
        );
    }
}
#[tokio::test]
async fn original_configuration_request_cannot_supply_new_settings() {
    for key in ["input", "destination", "activate"] {
        let mut body = json!({"request_digest":"a".repeat(64)});
        body[key] = true.into();
        assert_eq!(
            http::router(state())
                .oneshot(post("/api/deployment/install-recover-preview", body))
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
}
#[tokio::test]
async fn invalid_digest_and_unacknowledged_continuation_fail_before_host_access() {
    for (path, body, message) in [
        (
            "/api/deployment/install-recover-preview",
            json!({"request_digest":"bad"}),
            "before host access",
        ),
        (
            "/api/deployment/install-recover-files",
            json!({"request_digest":"bad","confirmation":"resume-original-configuration-files-only","external_administration_stopped":false}),
            "before any host access",
        ),
    ] {
        let response = http::router(state())
            .oneshot(post(path, body))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let body: Value = json_codec::from_slice(
            &to_bytes(response.into_body(), MAX_REQUEST_BYTES)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(body["error"]["code"], "configuration_file_installation");
        assert!(body["error"]["message"].as_str().unwrap().contains(message));
    }
}
