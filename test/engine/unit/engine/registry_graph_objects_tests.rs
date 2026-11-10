use super::*;
use crate::{disk::MAX_METADATA, journal::AuthenticatedMetadataObject};

fn graph() -> GraphObjects {
    collect_objects([("registry-test", "one", &b"{\"number\":1}"[..])].into_iter()).unwrap()
}

#[test]
fn graph_envelope_representation_is_unchanged_and_strict() {
    let objects = graph();
    let expected = br#"{"one":{"kind":"registry-test","value":{"number":1}}}"#;
    assert_eq!(json_codec::to_vec(&objects).unwrap(), expected);
    let decoded: GraphObjects = decode_strict(expected).unwrap();
    assert_eq!(objects, decoded);
    for bytes in [
        &br#"{"one":{"kind":"registry-test","value":{},"extra":true}}"#[..],
        &br#"{"one":{"kind":"registry-test","kind":"other","value":{}}}"#[..],
    ] {
        assert!(decode_strict::<GraphObjects>(bytes).is_err());
    }
}

#[test]
fn graph_merge_accepts_identical_pages_and_preserves_collision_classification() {
    let original = graph();
    let mut objects = original.clone();
    add_objects(
        &mut objects,
        [("registry-test", "one", &b"{\"number\":1}"[..])].into_iter(),
        ErrorCode::ObjectCorrupt,
    )
    .unwrap();
    assert_eq!(objects, original);
    for code in [ErrorCode::ObjectCorrupt, ErrorCode::LogFork] {
        for (kind, bytes) in [
            ("registry-test", &b"{\"number\":2}"[..]),
            ("other", &b"{\"number\":1}"[..]),
        ] {
            let error =
                add_objects(&mut objects, [(kind, "one", bytes)].into_iter(), code).unwrap_err();
            assert_eq!(error.code(), code);
            assert_eq!(objects, original);
        }
    }
}

#[test]
fn graph_reader_checks_address_and_kind_before_returning_page_bytes() {
    let objects = graph();
    assert_eq!(
        read_objects(&objects, "registry-test", "one").unwrap(),
        b"{\"number\":1}"
    );
    for (kind, hash) in [("other", "one"), ("registry-test", "missing")] {
        assert_eq!(
            read_objects(&objects, kind, hash).unwrap_err().code(),
            ErrorCode::ObjectCorrupt
        );
    }
}

#[test]
fn graph_content_and_budget_remain_authenticated_at_the_loader_boundary() {
    let prepared =
        crate::journal::PreparedMetadataObject::new("registry-test", &1, MAX_METADATA).unwrap();
    let objects =
        collect_objects([("registry-test", prepared.hash.as_str(), &b"2"[..])].into_iter())
            .unwrap();
    let bytes = read_objects(&objects, "registry-test", &prepared.hash).unwrap();
    let value = decode_strict(&bytes).unwrap();
    assert!(matches!(
        AuthenticatedMetadataObject::authenticate("registry-test", &prepared.hash, value),
        Err(error) if error.code() == ErrorCode::ObjectCorrupt
    ));
    assert!(matches!(
        AuthenticatedMetadataObject::authenticate(
            "registry-test",
            &prepared.hash,
            json_codec::Value::String("x".repeat(MAX_METADATA)),
        ),
        Err(error) if error.code() == ErrorCode::ResourceLimit
    ));
}

#[test]
fn graph_visitor_keeps_canonical_order_and_propagates_consumer_errors() {
    let objects = collect_objects(
        [
            ("second", "z", &br#"{ "b": 2, "a": 1 }"#[..]),
            ("first", "a", &b"null"[..]),
        ]
        .into_iter(),
    )
    .unwrap();
    let mut seen = Vec::new();
    visit_objects(&objects, &mut |kind, hash, bytes| {
        seen.push((kind.to_owned(), hash.to_owned(), bytes.to_vec()));
        Ok(())
    })
    .unwrap();
    assert_eq!(
        seen,
        vec![
            ("first".into(), "a".into(), b"null".to_vec()),
            ("second".into(), "z".into(), br#"{"a":1,"b":2}"#.to_vec()),
        ]
    );
    let mut calls = 0;
    let error = visit_objects(&objects, &mut |_, _, _| {
        calls += 1;
        Err(Error::new(ErrorCode::IoError, "consumer stopped"))
    })
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::IoError);
    assert_eq!(calls, 1);
}
