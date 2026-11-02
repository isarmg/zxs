use http_routing::{Router, routing::{get, post}, body::Body, http::{Request, StatusCode}, extract::{Path, Query}};
use http_body::BodyExt;
use service_stack::ServiceExt;
use std::{collections::HashMap, future::Future, sync::Arc, task::{Context, Poll, Wake, Waker}};
struct Noop;
impl Wake for Noop { fn wake(self: Arc<Self>) {} }
fn run<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(Arc::new(Noop));
    match Box::pin(future).as_mut().poll(&mut Context::from_waker(&waker)) {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("in-memory routing unexpectedly pending"),
    }
}
#[test]
fn routes_path_and_query_without_runtime() {
    let app = Router::new().route("/item/{id}", get(|Path(id): Path<String>, Query(query): Query<HashMap<String,String>>| async move { format!("{id}:{}", query["mode"]) }));
    let response = run(app.oneshot(Request::builder().uri("/item/42?mode=read").body(Body::empty()).unwrap())).unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(run(response.into_body().collect()).unwrap().to_bytes(), "42:read");
}
#[test]
fn distinguishes_missing_route_and_wrong_method() {
    let app = Router::new().route("/", get(|| async { "ok" }));
    let wrong = run(app.clone().oneshot(Request::builder().method("POST").body(Body::empty()).unwrap())).unwrap();
    let missing = run(app.oneshot(Request::builder().uri("/missing").body(Body::empty()).unwrap())).unwrap();
    assert_eq!(wrong.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
}
#[test]
fn request_body_and_response_status_are_preserved() {
    let app = Router::new().route("/", post(|body: String| async move { (StatusCode::CREATED, body) }));
    let response = run(app.oneshot(Request::builder().method("POST").body(Body::from("payload")).unwrap())).unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    assert_eq!(run(response.into_body().collect()).unwrap().to_bytes(), "payload");
}
#[test]
fn extraction_error_produces_client_error() {
    let app = Router::new().route("/item/{id}", get(|Path(id): Path<u64>| async move { id.to_string() }));
    let response = run(app.oneshot(Request::builder().uri("/item/not-a-number").body(Body::empty()).unwrap())).unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}
#[http_routing::debug_handler]
async fn generated_handler() -> &'static str { "macro" }
#[derive(Clone, http_routing::extract::FromRef)]
struct State { value: String }
#[test]
fn procedural_macros_target_the_new_single_owner() {
    use http_routing::extract::FromRef;
    assert_eq!(String::from_ref(&State { value: "state".into() }), "state");
    let response = run(Router::new().route("/", get(generated_handler)).oneshot(Request::new(Body::empty()))).unwrap();
    assert_eq!(run(response.into_body().collect()).unwrap().to_bytes(), "macro");
}

// Unsupported nested path shapes retain the named rejection.
mod nested_path_regression {
    use http_routing::{
        body::{to_bytes, Body},
        extract::{path::ErrorKind, rejection::PathRejection, Path},
        http::{Request, StatusCode},
        response::{IntoResponse, Response},
        routing::get,
        Router,
    };
    use service_stack::ServiceExt;
    use std::{
        future::Future,
        sync::Arc,
        task::{Context, Poll, Wake, Waker},
    };

    struct PathFixtureWake;

    impl Wake for PathFixtureWake {
        fn wake(self: Arc<Self>) {}
    }

    fn ready_path_fixture<F: Future>(future: F) -> F::Output {
        let waker = Waker::from(Arc::new(PathFixtureWake));
        match Box::pin(future)
            .as_mut()
            .poll(&mut Context::from_waker(&waker))
        {
            Poll::Ready(output) => output,
            Poll::Pending => panic!("invariant: an in-memory path fixture has no pending I/O"),
        }
    }

    fn path_request(uri: &str) -> Request<Body> {
        Request::builder()
            .uri(uri)
            .body(Body::empty())
            .expect("invariant: the test passes fixed valid request URIs")
    }

    fn path_response_text(response: Response) -> String {
        let bytes = ready_path_fixture(to_bytes(response.into_body(), 4096))
            .expect("invariant: the fixed fixture response fits the 4096-byte limit");
        String::from_utf8(bytes.to_vec())
            .expect("invariant: the fixture handlers and rejection format produce UTF-8")
    }

    async fn nested_tuple_handler(
        Path(_values): Path<Vec<(String, (String, String))>>,
    ) -> StatusCode {
        StatusCode::NO_CONTENT
    }

    #[test]
    fn nested_path_tuple_returns_existing_unsupported_type_response() {
        let app = Router::new().route("/items/{id}", get(nested_tuple_handler));
        let response = ready_path_fixture(app.oneshot(path_request("/items/value")))
            .expect("invariant: Router's service error is Infallible");

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = path_response_text(response);
        assert!(
            body.starts_with("Unsupported type `"),
            "expected the existing unsupported-path-type rejection, got {body:?}",
        );
    }

    async fn inspect_nested_tuple_rejection(
        result: Result<Path<Vec<(String, (String, String))>>, PathRejection>,
    ) -> Response {
        let rejection = result.expect_err("a nested pair value is an unsupported path shape");
        match &rejection {
            PathRejection::FailedToDeserializePathParams(error) => {
                assert!(matches!(error.kind(), ErrorKind::UnsupportedType { .. }));
            }
            other => panic!("unexpected path rejection: {other}"),
        }
        // Preserve the original named rejection and its existing source/status
        // policy. Do not substitute a string response or a default value.
        rejection.into_response()
    }

    #[test]
    fn nested_path_tuple_preserves_named_rejection() {
        let app = Router::new().route("/items/{id}", get(inspect_nested_tuple_rejection));
        let response = ready_path_fixture(app.oneshot(path_request("/items/value")))
            .expect("invariant: Router's service error is Infallible");

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert!(path_response_text(response).starts_with("Unsupported type `"));
    }

    #[test]
    fn existing_path_tuple_and_named_pair_meanings_are_preserved() {
        let flat = Router::new().route(
            "/flat/{id}/{name}",
            get(|Path((id, name)): Path<(u32, String)>| async move { format!("{id}:{name}") }),
        );
        let flat_response = ready_path_fixture(flat.oneshot(path_request("/flat/7/alpha")))
            .expect("invariant: Router's service error is Infallible");
        assert_eq!(flat_response.status(), StatusCode::OK);
        assert_eq!(path_response_text(flat_response), "7:alpha");

        let pairs = Router::new().route(
            "/pairs/{first}/{second}",
            get(|Path(values): Path<Vec<(String, String)>>| async move {
                values
                    .into_iter()
                    .map(|(key, value)| format!("{key}={value}"))
                    .collect::<Vec<_>>()
                    .join("&")
            }),
        );
        let pair_response = ready_path_fixture(pairs.oneshot(path_request("/pairs/one/two")))
            .expect("invariant: Router's service error is Infallible");
        assert_eq!(pair_response.status(), StatusCode::OK);
        assert_eq!(path_response_text(pair_response), "first=one&second=two");
    }
}
