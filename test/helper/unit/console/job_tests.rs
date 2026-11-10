use super::*;
use http_routing::{
    Router,
    body::{Body, to_bytes},
    http::Request,
};
use json_codec::{Value, json};
use service_stack::ServiceExt;
use std::sync::{Arc, Mutex, mpsc};
use tokio::sync::{oneshot, watch};

fn state() -> State {
    let (stopping, _) = watch::channel(false);
    State::new(([0, 0, 0, 0], 9040).into(), "job-csrf".into(), stopping)
}
fn request(body: impl Into<Body>) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/job")
        .header("host", "127.0.0.1:9040")
        .header("origin", "http://127.0.0.1:9040")
        .header("x-zxs-helper-csrf", "job-csrf")
        .header("content-type", "application/json")
        .body(body.into())
        .unwrap()
}
fn app<I, O>(state: State, operation: fn(I) -> crate::Result<O>) -> Router
where
    I: DeserializeOwned + Send + 'static,
    O: Serialize + Send + 'static,
{
    Router::new()
        .route(
            "/job",
            Job::new("operation-failed", "worker-failed").route(operation),
        )
        .with_state(state)
}
async fn body(response: http_routing::response::Response) -> Value {
    json_codec::from_slice(&to_bytes(response.into_body(), 1 << 20).await.unwrap()).unwrap()
}
#[derive(Deserialize, Serialize)]
#[serialization(deny_unknown_fields)]
struct Echo {
    number: u32,
}
fn echo(input: Echo) -> crate::Result<Echo> {
    Ok(input)
}
fn empty(_: Empty) -> crate::Result<Value> {
    Ok(json!({"empty":true}))
}

#[tokio::test]
async fn typed_jobs_preserve_response_and_release_owner() {
    let state = state();
    let response = app(state.clone(), echo)
        .oneshot(request(r#"{"number":7}"#))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body(response).await, json!({"number":7}));
    assert_eq!(state.jobs.available_permits(), 1);
    let response = app(state.clone(), empty)
        .oneshot(request("{}"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body(response).await, json!({"empty":true}));
    assert_eq!(state.jobs.available_permits(), 1);
}

#[tokio::test]
async fn typed_and_empty_input_are_strict_and_budgeted_before_work() {
    fn unreachable_work(_: Echo) -> crate::Result<Echo> {
        panic!("invalid input reached worker")
    }
    let state = state();
    for input in [
        "not JSON",
        "{}",
        r#"{"number":-1}"#,
        r#"{"number":4294967296}"#,
        r#"{"number":1,"other":true}"#,
    ] {
        let response = app(state.clone(), unreachable_work)
            .oneshot(request(input))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{input}");
        assert_eq!(body(response).await["error"]["code"], "request_json");
        assert_eq!(state.jobs.available_permits(), 1);
    }
    let response = app(state.clone(), empty)
        .oneshot(request(r#"{"ignored":true}"#))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let response = app(state.clone(), empty)
        .oneshot(request(
            " ".repeat(super::super::super::MAX_REQUEST_BYTES + 1),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(body(response).await["error"]["code"], "request_body");
    assert_eq!(state.jobs.available_permits(), 1);
}

#[tokio::test]
async fn csrf_stopping_and_busy_precede_body_decode() {
    let state = state();
    let permit = Arc::clone(&state.jobs).try_acquire_owned().unwrap();
    let mut missing_csrf = request("not JSON");
    missing_csrf.headers_mut().remove("x-zxs-helper-csrf");
    let response = app(state.clone(), empty)
        .oneshot(missing_csrf)
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let response = app(state.clone(), empty)
        .oneshot(request("not JSON"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(body(response).await["error"]["code"], "helper_busy");
    state.stopping.send_replace(true);
    let response = app(state.clone(), empty)
        .oneshot(request("not JSON"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body(response).await["error"]["code"], "helper_stopping");
    assert_eq!(state.jobs.available_permits(), 0);
    drop(permit);
    assert_eq!(state.jobs.available_permits(), 1);
}

#[derive(Debug)]
struct OperationError(std::io::Error);
impl std::fmt::Display for OperationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("operation context")
    }
}
impl std::error::Error for OperationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}
#[tokio::test]
async fn operation_and_join_errors_keep_distinct_codes_and_original_causes() {
    fn failure(_: Empty) -> crate::Result<Value> {
        Err(Box::new(OperationError(std::io::Error::other(
            "original cause",
        ))))
    }
    fn panic_worker(_: Empty) -> crate::Result<Value> {
        panic!("worker cause")
    }
    let state = state();
    let response = app(state.clone(), failure)
        .oneshot(request("{}"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let value = body(response).await;
    assert_eq!(value["error"]["code"], "operation-failed");
    let message = value["error"]["message"].as_str().unwrap();
    assert!(message.contains("operation context"));
    assert!(message.contains("original cause"));
    let response = app(state.clone(), panic_worker)
        .oneshot(request("{}"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let value = body(response).await;
    assert_eq!(value["error"]["code"], "worker-failed");
    assert!(
        value["error"]["message"]
            .as_str()
            .unwrap()
            .contains("worker cause")
    );
    assert_eq!(state.jobs.available_permits(), 1);
}

#[tokio::test]
async fn disconnect_does_not_release_the_blocking_job_owner() {
    struct Gate {
        started: oneshot::Sender<()>,
        release: mpsc::Receiver<()>,
    }
    static GATE: Mutex<Option<Gate>> = Mutex::new(None);
    fn blocked(_: Empty) -> crate::Result<Value> {
        let gate = GATE.lock().unwrap().take().unwrap();
        gate.started.send(()).unwrap();
        gate.release.recv()?;
        Ok(json!({"completed":true}))
    }
    let (started, ready) = oneshot::channel();
    let (release, waiting) = mpsc::channel();
    *GATE.lock().unwrap() = Some(Gate {
        started,
        release: waiting,
    });
    let state = state();
    let request_task = tokio::spawn(app(state.clone(), blocked).oneshot(request("{}")));
    tokio::time::timeout(std::time::Duration::from_secs(5), ready)
        .await
        .unwrap()
        .unwrap();
    request_task.abort();
    assert!(request_task.await.unwrap_err().is_cancelled());
    assert_eq!(state.jobs.available_permits(), 0);
    let response = app(state.clone(), empty)
        .oneshot(request("{}"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(body(response).await["error"]["code"], "helper_busy");
    release.send(()).unwrap();
    let finished = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        Arc::clone(&state.jobs).acquire_owned(),
    )
    .await
    .unwrap()
    .unwrap();
    drop(finished);
    assert_eq!(state.jobs.available_permits(), 1);
}

#[tokio::test]
async fn all_native_job_routes_reject_malformed_json_before_work() {
    let state = state();
    let router = super::super::router(state.clone());
    for path in [
        "/api/install/preview",
        "/api/install/commit",
        "/api/install/repair-preview",
        "/api/install/repair-recover-preview",
        "/api/install/repair-commit",
        "/api/deployment/preview",
        "/api/deployment/prepare",
        "/api/host/preview",
        "/api/host/prepare",
        "/api/initialization/layout-preview",
        "/api/initialization/layout-prepare",
        "/api/initialization/runtime-preview",
        "/api/initialization/runtime-prepare",
        "/api/deployment/install-preview",
        "/api/deployment/install-files",
        "/api/deployment/install-recover-preview",
        "/api/deployment/install-recover-files",
        "/api/initialization/account-preview",
        "/api/initialization/account-prepare",
        "/api/initialization/packages-preview",
        "/api/initialization/packages-refresh",
        "/api/initialization/packages-install",
        "/api/initialization/packages-finish",
        "/api/install/vfs-preview",
        "/api/install/vfs-status",
        "/api/install/vfs-install",
        "/api/deployment/activation-preview",
        "/api/deployment/activation-status",
    ] {
        let mut request = request("not JSON");
        *request.uri_mut() = path.parse().unwrap();
        let response = router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{path}");
        assert_eq!(
            body(response).await["error"]["code"],
            "request_json",
            "{path}"
        );
        assert_eq!(state.jobs.available_permits(), 1, "{path}");
    }
}
