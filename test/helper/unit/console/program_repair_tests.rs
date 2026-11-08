use super::*;
use http_routing::{body::{Body, to_bytes}, http::{Request, StatusCode}};
use json_codec::{Value, json};
use service_stack::ServiceExt;

fn state() -> State {
    let (stopping, _receiver) = watch::channel(false);
    State::new(SocketAddr::from(([0,0,0,0],9040)), "repair-csrf".into(), stopping)
}
fn post(path: &str, body: Value) -> Request<Body> {
    Request::builder().method("POST").uri(path)
        .header("host", "127.0.0.1:9040").header("origin", "http://127.0.0.1:9040")
        .header("x-zxs-helper-csrf", "repair-csrf").header("content-type", "application/json")
        .body(Body::from(json_codec::to_vec(&body).unwrap())).unwrap()
}

#[tokio::test]
async fn every_repair_route_retains_csrf_and_one_worker_admission() {
    for path in ["/api/install/repair-preview", "/api/install/repair-recover-preview", "/api/install/repair-commit"] {
        let mut request = post(path, json!({}));
        request.headers_mut().remove("x-zxs-helper-csrf");
        assert_eq!(http::router(state()).oneshot(request).await.unwrap().status(), StatusCode::FORBIDDEN);
        let state = state();
        let _owner = Arc::clone(&state.jobs).try_acquire_owned().unwrap();
        assert_eq!(http::router(state).oneshot(post(path,json!({}))).await.unwrap().status(), StatusCode::CONFLICT);
    }
}

#[tokio::test]
async fn repair_without_external_ack_never_reads_absolute_system_paths() {
    let response = http::router(state()).oneshot(post("/api/install/repair-commit", json!({
        "request_digest":"invalid", "confirmation":"repair-same-payload-program-only", "external_administration_stopped":false,
    }))).await.unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body: Value = json_codec::from_slice(&to_bytes(response.into_body(), MAX_REQUEST_BYTES).await.unwrap()).unwrap();
    assert_eq!(body["error"]["code"], "same_payload_program_repair");
    assert!(body["error"]["message"].as_str().unwrap().contains("before reading or writing"));
}

#[tokio::test]
async fn repair_preview_accepts_neither_destination_nor_uploaded_payload() {
    for body in [json!({"destination":"/tmp/elsewhere"}), json!({"payload":"uploaded bytes"})] {
        assert_eq!(http::router(state()).oneshot(post("/api/install/repair-preview",body)).await.unwrap().status(), StatusCode::BAD_REQUEST);
    }
}
