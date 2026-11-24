use super::{
    ByteWindow, ExactBody, Head, MAX_WINDOW, PayloadError, PayloadSelection, ReadPurpose,
    slice_request, strong_etag, validate_head,
};
use crate::r3::{Body, Digest, Id};
use http::{HeaderMap, HeaderValue, Method, Request, StatusCode, header};

fn body(length: u64) -> Body {
    Body {
        length,
        sha256: Digest::parse(&"ab".repeat(32)).unwrap(),
    }
}

fn header_value(value: &str) -> HeaderValue {
    HeaderValue::from_str(value).unwrap()
}

fn framed(selected: &Body) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(header::ACCEPT_RANGES, header_value("bytes"));
    headers.insert(
        header::CONTENT_TYPE,
        header_value("application/octet-stream"),
    );
    headers.insert(header::ETAG, header_value(&strong_etag(selected)));
    headers
}

#[test]
fn equal_wire_fields_still_require_the_original_choice() {
    let container = Id::parse(&"aa".repeat(16)).unwrap();
    let node = Id::parse(&"bb".repeat(16)).unwrap();
    let window = ByteWindow::new(8, 2).unwrap();
    // Within one workspace, separately approving A cannot authorize reselecting B.
    // The wire fields can be identical while the held local output is different.
    let choice_a = PayloadSelection::new(container.clone(), node.clone(), body(12), window);
    let approved = choice_a.clone();
    let choice_b = PayloadSelection::new(container, node, body(12), window);
    approved.require_same(&choice_a).unwrap();
    assert!(approved.require_same(&choice_b).is_err());
}

#[test]
fn exact_payload_request_is_not_receipt_or_legacy_access() {
    let container = Id::parse(&"aa".repeat(16)).unwrap();
    let node = Id::parse(&"bb".repeat(16)).unwrap();
    let selected = body(1 << 42);
    let window = ByteWindow::new(1 << 40, MAX_WINDOW).unwrap();
    assert_eq!(
        window.intersection(selected.length),
        Some(((1 << 40) + MAX_WINDOW - 1, MAX_WINDOW))
    );
    assert!(ByteWindow::new(0, 0).is_err());
    assert!(ByteWindow::new(0, MAX_WINDOW + 1).is_err());
    assert!(ByteWindow::new(u64::MAX, 2).is_err());
    let req = slice_request(
        &container,
        &node,
        &selected,
        ByteWindow::new(20, 33).unwrap(),
    )
    .unwrap();
    let path = format!("/zxs/v1/containers/{container}/nodes/{node}/payload");
    assert_eq!(req.method(), Method::GET);
    assert_eq!(req.uri().path(), path);
    assert_eq!(req.headers()[header::IF_RANGE], strong_etag(&selected));
    assert_eq!(req.headers()[header::RANGE], "bytes=20-52");
    assert_eq!(req.headers()[header::ACCEPT_ENCODING], "identity");
    assert!(!req.headers().contains_key(header::AUTHORIZATION));
    ReadPurpose::Payload.require_route(&req).unwrap();
    assert!(ReadPurpose::Receipt.require_route(&req).is_err());
    let receipt = format!("/zxs/v1/containers/{container}/operations/{node}");
    let req = Request::builder().uri(receipt).body(()).unwrap();
    ReadPurpose::Receipt.require_route(&req).unwrap();
    assert!(ReadPurpose::Payload.require_route(&req).is_err());
    for (method, uri) in [
        (Method::HEAD, path.clone()),
        (Method::POST, path.clone()),
        (Method::GET, path.replace("/payload", "/data")),
        (Method::GET, format!("{path}?other=1")),
        (Method::GET, format!("https://other.invalid{path}")),
    ] {
        let req = Request::builder().method(method).uri(uri).body(()).unwrap();
        assert!(ReadPurpose::Payload.require_route(&req).is_err());
    }
}

#[test]
fn partial_requires_exact_validator_content_range_and_span_length() {
    let selected = body(100);
    let window = ByteWindow::new(98, 10).unwrap();
    let mut good = framed(&selected);
    good.insert(header::CONTENT_LENGTH, header_value("2"));
    good.insert(header::CONTENT_RANGE, header_value("bytes 98-99/100"));
    assert_eq!(
        validate_head(StatusCode::PARTIAL_CONTENT, &good, &selected, window).unwrap(),
        Head::Part(2)
    );
    for (name, value) in [
        ("etag", "W/\"untrusted\""),
        ("etag", "\"this-is-not-the-same-version\""),
        ("content-range", "bytes 98-99/101"),
        ("content-range", "bytes 97-98/100"),
        ("content-length", "3"),
        ("content-length", "02"),
        ("content-type", "application/zip"),
        ("accept-ranges", "none"),
        ("content-encoding", "identity"),
        ("transfer-encoding", "chunked"),
        ("trailer", "zxs-sha256"),
        ("zxs-sha256", selected.sha256.as_str()),
    ] {
        let mut bad = good.clone();
        bad.insert(name, header_value(value));
        assert!(
            validate_head(StatusCode::PARTIAL_CONTENT, &bad, &selected, window).is_err(),
            "accepted invalid {name}"
        );
    }
    let mut duplicate = good.clone();
    duplicate.append(header::ETAG, header_value(&strong_etag(&selected)));
    assert!(validate_head(StatusCode::PARTIAL_CONTENT, &duplicate, &selected, window).is_err());
    assert!(
        validate_head(
            StatusCode::PARTIAL_CONTENT,
            &good,
            &selected,
            ByteWindow::new(100, 1).unwrap()
        )
        .is_err()
    );
}

#[test]
fn full_fallback_and_precise_empty_416_are_not_parts() {
    let selected = body(100);
    let window = ByteWindow::new(99, 2).unwrap();
    let outside = ByteWindow::new(100, 1).unwrap();
    let mut full = framed(&selected);
    full.insert(header::CONTENT_LENGTH, header_value("100"));
    full.insert("zxs-sha256", header_value(selected.sha256.as_str()));
    assert_eq!(
        validate_head(StatusCode::OK, &full, &selected, window).unwrap(),
        Head::Whole
    );
    full.insert(header::CONTENT_RANGE, header_value("bytes 0-99/100"));
    assert!(validate_head(StatusCode::OK, &full, &selected, window).is_err());

    let mut empty = HeaderMap::new();
    empty.insert(header::ACCEPT_RANGES, header_value("bytes"));
    empty.insert(header::CONTENT_LENGTH, header_value("0"));
    empty.insert(header::CONTENT_RANGE, header_value("bytes */100"));
    assert_eq!(
        validate_head(
            StatusCode::RANGE_NOT_SATISFIABLE,
            &empty,
            &selected,
            outside
        )
        .unwrap(),
        Head::Empty
    );
    assert!(validate_head(StatusCode::RANGE_NOT_SATISFIABLE, &empty, &selected, window).is_err());
    for (name, value) in [
        ("content-range", "bytes */99"),
        ("content-length", "1"),
        ("etag", "\"disallowed-for-416\""),
        ("content-type", "application/octet-stream"),
        ("zxs-sha256", selected.sha256.as_str()),
    ] {
        let mut bad = empty.clone();
        bad.insert(name, header_value(value));
        assert!(
            validate_head(StatusCode::RANGE_NOT_SATISFIABLE, &bad, &selected, outside).is_err()
        );
    }
}

#[test]
fn short_and_overlong_bodies_cannot_be_complete_slices() {
    let mut received = Vec::new();
    let mut short = ExactBody::new(3);
    short.push(b"ab", &mut received).unwrap();
    assert_eq!(
        short.interrupted(),
        PayloadError::ShortBody {
            expected: 3,
            seen: 2
        }
    );
    let error = short.finish().unwrap_err();
    assert_eq!(
        error.downcast_ref::<PayloadError>(),
        Some(&PayloadError::ShortBody {
            expected: 3,
            seen: 2
        })
    );
    assert_eq!(received, b"ab");

    received.clear();
    let mut long = ExactBody::new(2);
    let error = long.push(b"abc", &mut received).unwrap_err();
    assert_eq!(
        error.downcast_ref::<PayloadError>(),
        Some(&PayloadError::ExtraBody)
    );
    assert!(received.is_empty());
    long.push(b"a", &mut received).unwrap();
    long.push(b"b", &mut received).unwrap();
    assert_eq!(
        long.finish().unwrap().sha256.as_str(),
        "fb8e20fc2e4c3f248c60c39bd652f3c1347298bb977b8b4d5903b85055620603"
    );
    let mut no_bytes = ExactBody::new(0);
    assert!(no_bytes.push(b"x", std::io::sink()).is_err());
}
