use super::*;
use crate::{
    headless_auth::{FIXTURE_BASIC, SESSION_TTL_SECONDS, fixture_account},
    headless_http::{head, profile, resource},
};
use std::{
    io::{self, Read},
    time::Duration,
};

const C: &str = "11111111111111111111111111111111";
const ROOT: &str = "/zxs/v1/containers";

fn token(account: &Account) -> String {
    let valid = account
        .issue_session(&[("authorization", FIXTURE_BASIC)], &|| false)
        .unwrap();
    format!("Bearer {valid}")
}

#[test]
fn typed_browse_rejects_cross_lane_headers_duplicates_or_nonempty_body_before_store() {
    let account = fixture_account();
    let bearer = token(&account);
    for (path, fields) in [
        (
            ROOT.to_owned(),
            "Zxs-View: compact\r\nContent-Type: application/json\r\n",
        ),
        (
            ROOT.to_owned(),
            "Zxs-View: compact\r\nZXS-VIEW: compact\r\n",
        ),
        (
            ROOT.to_owned(),
            "Zxs-View: compact\r\nZxs-Operation: ignored\r\n",
        ),
        (
            ROOT.to_owned(),
            "Zxs-View: compact\r\nZxs-Unknown: ignored\r\n",
        ),
        (
            ROOT.to_owned(),
            "Zxs-View: compact\r\nContent-Length: 1\r\n",
        ),
        (
            ROOT.to_owned(),
            "Zxs-View: compact\r\nContent-Length: 00\r\n",
        ),
        (
            ROOT.to_owned(),
            "Zxs-View: compact\r\nContent-Encoding: identity\r\n",
        ),
        (
            format!("{ROOT}/{C}"),
            "Zxs-Detail: paged\r\nZxs-Detail-After: main\r\n",
        ),
        (format!("{ROOT}/{C}/tags"), "Zxs-Chunk-Sha256: no\r\n"),
        (format!("{ROOT}/{C}/tags"), "Range: bytes=0-1\r\n"),
    ] {
        let raw = format!(
            "GET {path} HTTP/1.1\r\nHost: fixture\r\nAuthorization: {bearer}\r\n{fields}\r\n"
        );
        let head = head(raw.as_bytes()).ok().unwrap();
        let next = b"untouched later bytes";
        let mut inbound = next.as_slice();
        let reply = resource(
            &account,
            &head,
            &mut inbound,
            profile(&head),
            &|| false,
            &|_, _, _| panic!("an ambiguous browse request must not invoke Store"),
        )
        .unwrap();
        assert!(matches!(reply.outcome, Outcome::Failed(Problem::Malformed)));
        assert_eq!(inbound, next);
    }
}

#[test]
fn projection_and_response_wire_require_live_scope_and_typed_stop_or_expiry() {
    let account = fixture_account();
    let bearer = token(&account);
    let raw =
        format!("GET {ROOT}/{C}/tags HTTP/1.1\r\nHost: fixture\r\nAuthorization: {bearer}\r\n\r\n");
    let head = head(raw.as_bytes()).ok().unwrap();
    let project = |request, incoming: &mut Incoming<'_>, live: &dyn Fn() -> bool| {
        assert!(matches!(request, Request::Tags(_)));
        assert!(matches!(incoming, Incoming::Empty));
        assert!(live());
        Outcome::Json(br#"["selected"]"#.to_vec())
    };
    let reply = resource(
        &account,
        &head,
        &mut io::empty(),
        profile(&head),
        &|| false,
        &project,
    )
    .unwrap();
    assert!(matches!(reply.outcome, Outcome::Json(ref bytes) if bytes == br#"["selected"]"#));
    assert!(
        reply
            .before_wire(&account, &|| false, Instant::now())
            .is_ok()
    );
    let expired = Instant::now() + Duration::from_secs(SESSION_TTL_SECONDS + 1);
    assert!(matches!(
        reply.before_wire(&account, &|| false, expired),
        Err(Problem::BearerUnauthorized)
    ));
    assert!(matches!(
        reply.before_wire(&account, &|| true, expired),
        Err(Problem::Unavailable)
    ));
    assert!(matches!(
        reply.before_wire(&fixture_account(), &|| false, Instant::now()),
        Err(Problem::Unavailable)
    ));

    let reply = resource(
        &account,
        &head,
        &mut io::empty(),
        profile(&head),
        &|| false,
        &|_, _, live| {
            assert!(live());
            account.stop_admission();
            assert!(!live());
            Outcome::Failed(Problem::Missing)
        },
    )
    .unwrap();
    assert!(matches!(
        reply.outcome,
        Outcome::Failed(Problem::Unavailable)
    ));
}

#[test]
fn valid_bearer_metadata_never_accepts_a_streaming_or_legacy_reply() {
    let account = fixture_account();
    let bearer = token(&account);
    let raw =
        format!("GET {ROOT}/{C}/tags HTTP/1.1\r\nHost: fixture\r\nAuthorization: {bearer}\r\n\r\n");
    let head = head(raw.as_bytes()).ok().unwrap();
    let mut inbound: &[u8] = b"next request";
    let reply = resource(
        &account,
        &head,
        &mut inbound,
        profile(&head),
        &|| false,
        &|_, _, _| Outcome::ReceiptAbsent,
    )
    .unwrap();
    assert!(matches!(
        reply.outcome,
        Outcome::Failed(Problem::Unavailable)
    ));
    let mut readback = Vec::new();
    inbound.read_to_end(&mut readback).unwrap();
    assert_eq!(readback, b"next request");
}
