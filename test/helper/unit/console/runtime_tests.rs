use super::*;
use http_routing::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use json_codec::{Value, json};
use service_stack::ServiceExt;

fn state() -> State {
    let (stopping, _receiver) = watch::channel(false);
    State::new(SocketAddr::from(([0, 0, 0, 0], 9040)), "runtime-csrf".into(), stopping)
}

fn post(path: &str, body: Value) -> Request<Body> {
    Request::builder().method("POST").uri(path)
        .header("host", "127.0.0.1:9040")
        .header("origin", "http://127.0.0.1:9040")
        .header("x-zxs-helper-csrf", "runtime-csrf")
        .header("content-type", "application/json")
        .body(Body::from(json_codec::to_vec(&body).unwrap())).unwrap()
}

#[tokio::test]
async fn runtime_routes_keep_origin_csrf_and_single_work_owner_boundaries() {
    for path in ["/api/initialization/runtime-preview", "/api/initialization/runtime-prepare"] {
        let mut request = post(path, json!({}));
        request.headers_mut().remove("x-zxs-helper-csrf");
        assert_eq!(http::router(state()).oneshot(request).await.unwrap().status(), StatusCode::FORBIDDEN);
        let state = state();
        let _owner = Arc::clone(&state.jobs).try_acquire_owned().unwrap();
        assert_eq!(http::router(state).oneshot(post(path, json!({}))).await.unwrap().status(), StatusCode::CONFLICT);
    }
}

#[tokio::test]
async fn runtime_route_rejects_missing_confirmation_without_touching_host_paths() {
    let response = http::router(state()).oneshot(post(
        "/api/initialization/runtime-prepare",
        json!({"input":{"service_user":"deliberately invalid"},"request_digest":"unused",
            "confirmation":"prepare-empty-runtime-only","external_administration_stopped":false}),
    )).await.unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let value: Value = json_codec::from_slice(&to_bytes(response.into_body(), MAX_REQUEST_BYTES).await.unwrap()).unwrap();
    assert_eq!(value["error"]["code"], "empty_runtime_preparation");
    assert!(value["error"]["message"].as_str().unwrap().contains("before inspecting or writing"));
}

#[tokio::test]
async fn runtime_request_rejects_unknown_destination_before_account_lookup() {
    let response = http::router(state()).oneshot(post(
        "/api/initialization/runtime-preview",
        json!({"service_user":"deliberately invalid", "destination":"/tmp/elsewhere"}),
    )).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}
