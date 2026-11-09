use super::*;
use http_routing::{body::Body, http::{Request, StatusCode}};
use json_codec::{Value, json};
use service_stack::ServiceExt;

fn state() -> State {
    let (stopping, _receiver) = watch::channel(false);
    State::new(SocketAddr::from(([0, 0, 0, 0], 9040)), "materials-csrf".into(), stopping)
}
fn post(path: &str, body: Value) -> Request<Body> {
    Request::builder().method("POST").uri(path)
        .header("host", "127.0.0.1:9040").header("origin", "http://127.0.0.1:9040")
        .header("x-zxs-helper-csrf", "materials-csrf").header("content-type", "application/json")
        .body(Body::from(json_codec::to_vec(&body).unwrap())).unwrap()
}

#[tokio::test]
async fn material_downloads_keep_same_origin_and_bounded_work_admission() {
    for path in ["/api/install/materials-download", "/api/install/materials-manifest"] {
        let mut request = post(path, json!({}));
        request.headers_mut().remove("x-zxs-helper-csrf");
        assert_eq!(http::router(state()).oneshot(request).await.unwrap().status(), StatusCode::FORBIDDEN);
        let state = state();
        let _owner = Arc::clone(&state.jobs).try_acquire_owned().unwrap();
        assert_eq!(http::router(state).oneshot(post(path, json!({}))).await.unwrap().status(), StatusCode::CONFLICT);
    }
}

#[tokio::test]
async fn download_accepts_no_server_path_and_development_absence_stays_explicit() {
    for path in ["/api/install/materials-download", "/api/install/materials-manifest"] {
        assert_eq!(http::router(state()).oneshot(post(path, json!({"output":"/tmp/other"}))).await.unwrap().status(), StatusCode::BAD_REQUEST);
        if crate::installation::materials::metadata().is_none() {
            assert_eq!(http::router(state()).oneshot(post(path, json!({}))).await.unwrap().status(), StatusCode::CONFLICT);
        }
    }
}
