// Public redirect method, error and request lifetime controls.
use bytes::Bytes;
use http::{
    HeaderMap, HeaderValue, Method, Request, Response, StatusCode, Uri,
    header::{CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE, LOCATION, TRANSFER_ENCODING},
};
use http_body::Full;
use service_stack::Service;
use std::{
    error::Error,
    fmt,
    future::{Future, Ready, ready},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll, Wake, Waker},
};
use tower_http::follow_redirect::{
    FollowRedirect, RequestUri,
    policy::{Action, Attempt, Policy},
};

type Body = Full<Bytes>;

#[derive(Debug)]
struct FailureMarker(&'static str);

impl fmt::Display for FailureMarker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl Error for FailureMarker {}

#[derive(Clone, Debug)]
struct FixtureError(Arc<FailureMarker>);

impl fmt::Display for FixtureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self.0.as_ref(), f)
    }
}

impl Error for FixtureError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.0.as_ref())
    }
}

fn error(text: &'static str) -> FixtureError {
    FixtureError(Arc::new(FailureMarker(text)))
}

fn assert_original_error(actual: &FixtureError, expected: &FixtureError) {
    assert!(Arc::ptr_eq(&actual.0, &expected.0));
    let cause = actual
        .source()
        .expect("invariant: the fixture error deliberately owns its marker")
        .downcast_ref::<FailureMarker>()
        .expect("invariant: the concrete fixture cause must survive propagation");
    assert!(std::ptr::eq(cause, expected.0.as_ref()));
}

#[derive(Clone)]
struct DropProbe(Arc<AtomicUsize>);

impl Drop for DropProbe {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[derive(Clone)]
enum Decision {
    Follow,
    Stop,
    Error(FixtureError),
}

#[derive(Clone)]
struct ControlPolicy {
    decision: Decision,
    preparations: Arc<AtomicUsize>,
    redirected_request_drops: Option<Arc<AtomicUsize>>,
}

impl Policy<Body, FixtureError> for ControlPolicy {
    fn redirect(&mut self, _: &Attempt<'_>) -> Result<Action, FixtureError> {
        match &self.decision {
            Decision::Follow => Ok(Action::Follow),
            Decision::Stop => Ok(Action::Stop),
            Decision::Error(error) => Err(error.clone()),
        }
    }

    fn on_request(&mut self, request: &mut Request<Body>) {
        let previous = self.preparations.fetch_add(1, Ordering::Relaxed);
        if previous > 0 {
            if let Some(drops) = &self.redirected_request_drops {
                request
                    .extensions_mut()
                    .insert(DropProbe(Arc::clone(drops)));
            }
        }
    }

    fn clone_body(&self, body: &Body) -> Option<Body> {
        Some(body.clone())
    }
}

#[derive(Clone, Copy)]
struct Step {
    uri: &'static str,
    status: StatusCode,
    location: Option<&'static str>,
}

#[derive(Clone)]
struct Seen {
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
}

#[derive(Clone)]
struct ScriptedService {
    steps: Arc<Vec<Step>>,
    seen: Arc<Mutex<Vec<Seen>>>,
    redirected_readiness_error: Option<FixtureError>,
}

impl Service<Request<Body>> for ScriptedService {
    type Response = Response<Body>;
    type Error = FixtureError;
    type Future = Ready<Result<Response<Body>, FixtureError>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), FixtureError>> {
        let already_called = !self
            .seen
            .lock()
            .expect("invariant: the finite fixture does not panic under this lock")
            .is_empty();
        if already_called {
            if let Some(error) = &self.redirected_readiness_error {
                return Poll::Ready(Err(error.clone()));
            }
        }
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: Request<Body>) -> Self::Future {
        let (parts, body) = request.into_parts();
        let bytes = match body.into_inner() {
            Some(bytes) => bytes,
            None => Bytes::new(),
        };
        let index = {
            let mut seen = self
                .seen
                .lock()
                .expect("invariant: the finite fixture does not panic under this lock");
            let index = seen.len();
            seen.push(Seen {
                method: parts.method,
                uri: parts.uri.clone(),
                headers: parts.headers,
                body: bytes,
            });
            index
        };
        let Some(step) = self.steps.get(index) else {
            return ready(Err(error("unexpected extra request")));
        };
        if parts.uri != Uri::from_static(step.uri) {
            return ready(Err(error("unexpected effective request URI")));
        }
        let mut response = Response::new(Body::default());
        *response.status_mut() = step.status;
        if let Some(location) = step.location {
            response
                .headers_mut()
                .insert(LOCATION, HeaderValue::from_static(location));
        }
        ready(Ok(response))
    }
}

#[derive(Default)]
struct CountWake(AtomicUsize);

impl Wake for CountWake {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

fn drive<F: Future>(future: F) -> F::Output {
    let wake = Arc::new(CountWake::default());
    let waker = Waker::from(Arc::clone(&wake));
    let mut context = Context::from_waker(&waker);
    let mut future = Box::pin(future);
    for _ in 0..8 {
        let before = wake.0.load(Ordering::Relaxed);
        match future.as_mut().poll(&mut context) {
            Poll::Ready(output) => return output,
            Poll::Pending => assert!(
                wake.0.load(Ordering::Relaxed) > before,
                "the finite immediate service must arrange the next poll",
            ),
        }
    }
    panic!("the finite fixture exceeded its poll bound")
}

fn script(first: StatusCode) -> Vec<Step> {
    vec![
        Step {
            uri: "https://origin.example/start",
            status: first,
            location: Some("/middle"),
        },
        Step {
            uri: "https://origin.example/middle",
            status: StatusCode::TEMPORARY_REDIRECT,
            location: Some("/finish"),
        },
        Step {
            uri: "https://origin.example/finish",
            status: StatusCode::OK,
            location: None,
        },
    ]
}

fn request(method: Method, body: Body) -> Request<Body> {
    let mut request = Request::new(body);
    *request.method_mut() = method;
    *request.uri_mut() = Uri::from_static("https://origin.example/start");
    request
        .headers_mut()
        .insert("x-retained", HeaderValue::from_static("fixed"));
    request
}

fn ready_redirect(
    service: ScriptedService,
    policy: ControlPolicy,
) -> FollowRedirect<ScriptedService, ControlPolicy> {
    let mut redirect = FollowRedirect::with_policy(service, policy);
    let waker = Waker::from(Arc::new(CountWake::default()));
    assert!(matches!(
        redirect.poll_ready(&mut Context::from_waker(&waker)),
        Poll::Ready(Ok(())),
    ));
    redirect
}

#[test]
fn post_302_and_307_preserve_their_distinct_method_body_and_payload_header_rules() {
    for first in [StatusCode::FOUND, StatusCode::TEMPORARY_REDIRECT] {
        for framing in [CONTENT_LENGTH, TRANSFER_ENCODING] {
            let seen = Arc::new(Mutex::new(Vec::new()));
            let preparations = Arc::new(AtomicUsize::new(0));
            let service = ScriptedService {
                steps: Arc::new(script(first)),
                seen: Arc::clone(&seen),
                redirected_readiness_error: None,
            };
            let policy = ControlPolicy {
                decision: Decision::Follow,
                preparations: Arc::clone(&preparations),
                redirected_request_drops: None,
            };
            let mut redirect = ready_redirect(service, policy);
            let mut request = request(Method::POST, Full::new(Bytes::from_static(b"payload")));
            request.headers_mut().insert(
                CONTENT_TYPE,
                HeaderValue::from_static("application/octet-stream"),
            );
            request
                .headers_mut()
                .insert(CONTENT_ENCODING, HeaderValue::from_static("identity"));
            let value = if framing == CONTENT_LENGTH {
                "7"
            } else {
                "chunked"
            };
            request
                .headers_mut()
                .insert(framing.clone(), HeaderValue::from_static(value));
            let response =
                drive(redirect.call(request)).expect("invariant: the three fixed steps succeed");
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(
                response.extensions().get::<RequestUri>().unwrap().0,
                Uri::from_static("https://origin.example/finish")
            );
            let snapshots = seen.lock().unwrap().clone();
            assert_eq!(snapshots.len(), 3);
            assert_eq!(preparations.load(Ordering::Relaxed), 3);
            assert_eq!(snapshots[0].method, Method::POST);
            assert_eq!(snapshots[0].body.as_ref(), b"payload");
            for snapshot in &snapshots[1..] {
                assert_eq!(snapshot.headers.get("x-retained").unwrap(), "fixed");
                if first == StatusCode::FOUND {
                    assert_eq!(snapshot.method, Method::GET);
                    assert!(snapshot.body.is_empty());
                    for field in [
                        CONTENT_TYPE,
                        CONTENT_LENGTH,
                        CONTENT_ENCODING,
                        TRANSFER_ENCODING,
                    ] {
                        assert!(!snapshot.headers.contains_key(field));
                    }
                } else {
                    assert_eq!(snapshot.method, Method::POST);
                    assert_eq!(snapshot.body.as_ref(), b"payload");
                    assert_eq!(snapshot.headers, snapshots[0].headers);
                }
            }
            assert_eq!(
                snapshots[2].uri,
                Uri::from_static("https://origin.example/finish")
            );
        }
    }
}

#[test]
fn stop_and_policy_error_do_not_prepare_a_second_request() {
    let original = error("fixed policy rejection");
    for decision in [Decision::Stop, Decision::Error(original.clone())] {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let preparations = Arc::new(AtomicUsize::new(0));
        let policy = ControlPolicy {
            decision: decision.clone(),
            preparations: Arc::clone(&preparations),
            redirected_request_drops: None,
        };
        let service = ScriptedService {
            steps: Arc::new(script(StatusCode::FOUND)),
            seen: Arc::clone(&seen),
            redirected_readiness_error: None,
        };
        let mut redirect = ready_redirect(service, policy);
        let output = drive(redirect.call(request(Method::GET, Body::default())));
        match decision {
            Decision::Stop => {
                let response = output.unwrap();
                assert_eq!(response.status(), StatusCode::FOUND);
                assert_eq!(
                    response.extensions().get::<RequestUri>().unwrap().0,
                    Uri::from_static("https://origin.example/start")
                );
            }
            Decision::Error(expected) => assert_original_error(&output.unwrap_err(), &expected),
            Decision::Follow => unreachable!("invariant: this control only supplies Stop/Error"),
        }
        assert_eq!(preparations.load(Ordering::Relaxed), 1);
        assert_eq!(seen.lock().unwrap().len(), 1);
    }
}

#[test]
fn cancelling_after_follow_preparation_drops_the_unsent_request() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let preparations = Arc::new(AtomicUsize::new(0));
    let drops = Arc::new(AtomicUsize::new(0));
    let service = ScriptedService {
        steps: Arc::new(script(StatusCode::FOUND)),
        seen: Arc::clone(&seen),
        redirected_readiness_error: None,
    };
    let policy = ControlPolicy {
        decision: Decision::Follow,
        preparations: Arc::clone(&preparations),
        redirected_request_drops: Some(Arc::clone(&drops)),
    };
    let mut redirect = ready_redirect(service, policy);
    let mut future = Box::pin(redirect.call(request(Method::GET, Body::default())));
    let wake = Arc::new(CountWake::default());
    let waker = Waker::from(Arc::clone(&wake));
    assert!(matches!(
        future.as_mut().poll(&mut Context::from_waker(&waker)),
        Poll::Pending
    ));
    assert!(wake.0.load(Ordering::Relaxed) > 0);
    assert_eq!(preparations.load(Ordering::Relaxed), 2);
    assert_eq!(seen.lock().unwrap().len(), 1);
    assert_eq!(drops.load(Ordering::Relaxed), 0);
    drop(future);
    assert_eq!(drops.load(Ordering::Relaxed), 1);
    assert_eq!(seen.lock().unwrap().len(), 1);
}

#[test]
fn redirected_readiness_error_keeps_its_source_and_drops_the_unsent_request() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let preparations = Arc::new(AtomicUsize::new(0));
    let drops = Arc::new(AtomicUsize::new(0));
    let original = error("fixed redirected readiness failure");
    let service = ScriptedService {
        steps: Arc::new(script(StatusCode::FOUND)),
        seen: Arc::clone(&seen),
        redirected_readiness_error: Some(original.clone()),
    };
    let policy = ControlPolicy {
        decision: Decision::Follow,
        preparations: Arc::clone(&preparations),
        redirected_request_drops: Some(Arc::clone(&drops)),
    };
    let mut redirect = ready_redirect(service, policy);
    let actual = drive(redirect.call(request(Method::GET, Body::default()))).unwrap_err();
    assert_original_error(&actual, &original);
    assert_eq!(preparations.load(Ordering::Relaxed), 2);
    assert_eq!(seen.lock().unwrap().len(), 1);
    assert_eq!(drops.load(Ordering::Relaxed), 1);
}
