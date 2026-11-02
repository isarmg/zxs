// Public redirect header lifecycle regressions.
use bytes::Bytes;
use http::{
    HeaderMap, HeaderValue, Request, Response, StatusCode, Uri,
    header::{AUTHORIZATION, COOKIE, LOCATION, PROXY_AUTHORIZATION},
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
    policy::{self, Action, Attempt, Policy, PolicyExt},
};

type Body = Full<Bytes>;

#[derive(Clone, Copy)]
struct Step {
    expected_uri: &'static str,
    location: Option<&'static str>,
}

#[derive(Clone)]
struct SeenRequest {
    uri: Uri,
    headers: HeaderMap,
}

#[derive(Debug)]
struct FixtureFailure(&'static str);

impl fmt::Display for FixtureFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

impl Error for FixtureFailure {}

#[derive(Clone)]
struct ScriptedService {
    steps: [Step; 3],
    seen: Arc<Mutex<Vec<SeenRequest>>>,
}

impl Service<Request<Body>> for ScriptedService {
    type Response = Response<Body>;
    type Error = FixtureFailure;
    type Future = Ready<Result<Response<Body>, FixtureFailure>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: Request<Body>) -> Self::Future {
        let mut seen = self.seen.lock().expect(
            "invariant: the finite fixture does not panic while holding its observation mutex",
        );
        let index = seen.len();
        seen.push(SeenRequest {
            uri: request.uri().clone(),
            headers: request.headers().clone(),
        });
        drop(seen);
        let Some(step) = self.steps.get(index) else {
            return ready(Err(FixtureFailure("an unexpected fourth request was made")));
        };
        if request.uri() != &Uri::from_static(step.expected_uri) {
            return ready(Err(FixtureFailure(
                "the request used an unexpected effective URI",
            )));
        }
        let mut response = Response::new(Body::default());
        if let Some(location) = step.location {
            *response.status_mut() = StatusCode::FOUND;
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
    // Three fixed immediate responses need two explicit redirect wakeups.
    // This is a bounded test executor, with no timer, socket, sleep or retry policy.
    for _ in 0..8 {
        let before = wake.0.load(Ordering::Relaxed);
        match future.as_mut().poll(&mut context) {
            Poll::Ready(output) => return output,
            Poll::Pending => assert!(
                wake.0.load(Ordering::Relaxed) > before,
                "the immediate fixture must arrange its own next poll",
            ),
        }
    }
    panic!("the three-request fixture exceeded its finite poll bound")
}

fn standard() -> policy::Standard {
    policy::Limited::new(2).and::<_, Body, FixtureFailure>(policy::FilterCredentials::new())
}

fn steps(cross_origin: bool) -> [Step; 3] {
    if cross_origin {
        [
            Step {
                expected_uri: "https://a.example/start",
                location: Some("https://b.example/step"),
            },
            Step {
                expected_uri: "https://b.example/step",
                location: Some("/finish"),
            },
            Step {
                expected_uri: "https://b.example/finish",
                location: None,
            },
        ]
    } else {
        [
            Step {
                expected_uri: "https://a.example/start",
                location: Some("/step"),
            },
            Step {
                expected_uri: "https://a.example/step",
                location: Some("/finish"),
            },
            Step {
                expected_uri: "https://a.example/finish",
                location: None,
            },
        ]
    }
}

fn original_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    for (name, text) in [
        (AUTHORIZATION, "Fixture origin-a"),
        (COOKIE, "fixture=origin-a"),
        (PROXY_AUTHORIZATION, "Fixture proxy-a"),
    ] {
        let mut value = HeaderValue::from_static(text);
        value.set_sensitive(true);
        headers.insert(name, value);
    }
    headers.insert("x-once", HeaderValue::from_static("fixture-initial"));
    headers
}

fn run<P>(cross_origin: bool, policy: P) -> (Response<Body>, Vec<SeenRequest>, HeaderMap)
where
    P: Policy<Body, FixtureFailure> + Clone,
{
    let seen = Arc::new(Mutex::new(Vec::new()));
    let service = ScriptedService {
        steps: steps(cross_origin),
        seen: Arc::clone(&seen),
    };
    let mut redirect = FollowRedirect::with_policy(service, policy);
    let original = original_headers();
    let mut request = Request::new(Body::default());
    *request.uri_mut() = Uri::from_static("https://a.example/start");
    *request.headers_mut() = original.clone();
    let waker = Waker::from(Arc::new(CountWake::default()));
    assert!(matches!(
        redirect.poll_ready(&mut Context::from_waker(&waker)),
        Poll::Ready(Ok(()))
    ));
    let response = drive(redirect.call(request))
        .expect("invariant: the fixed three-step service must succeed");
    let snapshots = seen
        .lock()
        .expect("invariant: the observation mutex is not poisoned")
        .clone();
    (response, snapshots, original)
}

fn assert_effective_uri(
    response: &Response<Body>,
    snapshots: &[SeenRequest],
    expected: &'static str,
) {
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(snapshots.len(), 3);
    assert_eq!(snapshots[2].uri, Uri::from_static(expected));
    assert_eq!(
        response
            .extensions()
            .get::<RequestUri>()
            .expect("invariant: FollowRedirect records the effective request URI")
            .0,
        Uri::from_static(expected)
    );
}

#[test]
fn cross_origin_then_same_origin_never_restores_filtered_headers() {
    let (response, snapshots, original) = run(true, standard());
    assert_effective_uri(&response, &snapshots, "https://b.example/finish");
    assert_eq!(snapshots[0].headers, original);
    for request in &snapshots[1..] {
        assert!(!request.headers.contains_key(AUTHORIZATION));
        assert!(!request.headers.contains_key(COOKIE));
        assert!(!request.headers.contains_key(PROXY_AUTHORIZATION));
    }
    assert_eq!(original.get(AUTHORIZATION).unwrap(), "Fixture origin-a");
}

#[test]
fn same_origin_three_request_control_keeps_headers_and_relative_uri_base() {
    let (response, snapshots, original) = run(false, standard());
    assert_effective_uri(&response, &snapshots, "https://a.example/finish");
    for request in &snapshots {
        assert_eq!(request.headers, original);
    }
}

#[derive(Clone, Copy)]
enum Preparation {
    RemoveOnce,
    ExplicitCredentials,
}

#[derive(Clone)]
struct PreparedPolicy {
    standard: policy::Standard,
    calls: u8,
    preparation: Preparation,
}

impl Policy<Body, FixtureFailure> for PreparedPolicy {
    fn redirect(&mut self, attempt: &Attempt<'_>) -> Result<Action, FixtureFailure> {
        <policy::Standard as Policy<Body, FixtureFailure>>::redirect(&mut self.standard, attempt)
    }

    fn on_request(&mut self, request: &mut Request<Body>) {
        <policy::Standard as Policy<Body, FixtureFailure>>::on_request(&mut self.standard, request);
        self.calls += 1;
        let visit = match self.calls {
            1 => "1",
            2 => "2",
            3 => "3",
            _ => panic!("invariant: this fixture prepares exactly three requests"),
        };
        request
            .headers_mut()
            .append("x-visit", HeaderValue::from_static(visit));
        match self.preparation {
            Preparation::RemoveOnce => {
                if self.calls == 2 {
                    request.headers_mut().remove("x-once");
                }
            }
            Preparation::ExplicitCredentials => {
                if self.calls >= 2 {
                    request
                        .headers_mut()
                        .insert(AUTHORIZATION, HeaderValue::from_static("Fixture target-b"));
                }
            }
        }
    }
}

#[test]
fn policy_header_changes_form_the_next_requests_authoritative_header_state() {
    let policy = PreparedPolicy {
        standard: standard(),
        calls: 0,
        preparation: Preparation::RemoveOnce,
    };
    let (response, snapshots, _) = run(false, policy);
    assert_effective_uri(&response, &snapshots, "https://a.example/finish");
    assert!(snapshots[0].headers.contains_key("x-once"));
    assert!(!snapshots[1].headers.contains_key("x-once"));
    assert!(!snapshots[2].headers.contains_key("x-once"));
    let values: Vec<_> = snapshots[2]
        .headers
        .get_all("x-visit")
        .iter()
        .map(|value| value.as_bytes().to_vec())
        .collect();
    assert_eq!(values, vec![b"1".to_vec(), b"2".to_vec(), b"3".to_vec()]);
}

#[test]
fn policy_may_explicitly_supply_the_next_origins_fixed_fixture_credentials() {
    let policy = PreparedPolicy {
        standard: standard(),
        calls: 0,
        preparation: Preparation::ExplicitCredentials,
    };
    let (response, snapshots, original) = run(true, policy);
    assert_effective_uri(&response, &snapshots, "https://b.example/finish");
    assert_eq!(snapshots[0].headers, {
        let mut expected = original;
        expected.append("x-visit", HeaderValue::from_static("1"));
        expected
    });
    for request in &snapshots[1..] {
        assert_eq!(
            request.headers.get(AUTHORIZATION).unwrap(),
            "Fixture target-b"
        );
        assert!(!request.headers.contains_key(COOKIE));
        assert!(!request.headers.contains_key(PROXY_AUTHORIZATION));
    }
}
