use super::*;
use serde_json::{Value, json};

fn first() -> Intent {
    Intent {
        container_id: Id::parse("11111111111111111111111111111111").unwrap(),
        operation_id: Id::parse("22222222222222222222222222222222").unwrap(),
        node_id: Id::parse("33333333333333333333333333333333").unwrap(),
        seen_sequence: 0,
        target: Target::FirstMain {
            name: DisplayName::parse("CON-e\u{301}").unwrap(),
        },
        body: Body {
            length: 0,
            sha256: Digest::parse(
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            )
            .unwrap(),
        },
    }
}

fn reply(original: &Intent, sequence: u64) -> Value {
    json!({"kind":"save_node","container_id":original.container_id, "operation_id":original.operation_id,
           "sequence":sequence,"node_id":original.node_id,"length":original.body.length,
           "sha256":original.body.sha256,"target":original.target})
}

#[test]
fn first_exact_full_wire_and_receipt() {
    let original = first();
    let request = original.save().unwrap();
    assert_eq!(request.method(), Method::POST);
    assert_eq!(
        request.uri().path(),
        "/zxs/v1/containers/11111111111111111111111111111111/branches/main/nodes"
    );
    assert_eq!(request.headers()["zxs-initial-name"], "434f4e2d65cc81");
    assert_eq!(request.headers()["zxs-base-node"], "0");
    assert_eq!(request.headers()["content-length"], "0");
    assert!(!request.headers().contains_key("zxs-base-generation"));
    let body = |seq| serde_json::to_vec(&reply(&original, seq)).unwrap();
    assert_eq!(
        outcome(&original, StatusCode::OK, &body(1), true),
        Lookup::Confirmed { sequence: 1 }
    );
    assert_eq!(
        outcome(&original, StatusCode::OK, &body(2), true),
        Lookup::Unknown
    );
}

#[test]
fn exact_404_and_exact_effect_are_mandatory() {
    let original = first();
    for raw in [
        &br#"{"state":"not_found","extra":0}"#[..],
        br#"{"state":"not_found","state":"not_found"}"#,
        b"",
    ] {
        assert_eq!(
            outcome(&original, StatusCode::NOT_FOUND, raw, true),
            Lookup::Unknown
        );
    }
    let absent = br#"{"state":"not_found"}"#;
    assert_eq!(
        outcome(&original, StatusCode::NOT_FOUND, absent, true),
        Lookup::Absent
    );
    assert_eq!(
        outcome(&original, StatusCode::NOT_FOUND, absent, false),
        Lookup::Unknown
    );
    assert_eq!(
        outcome(&original, StatusCode::SERVICE_UNAVAILABLE, b"", true),
        Lookup::Unknown
    );
    let mut wrong = reply(&original, 1);
    wrong["node_id"] = json!("44444444444444444444444444444444");
    assert_eq!(
        outcome(
            &original,
            StatusCode::OK,
            &serde_json::to_vec(&wrong).unwrap(),
            true
        ),
        Lookup::Unknown
    );
}

#[test]
fn matching_head_append_accepts_unrelated_sequence_gaps() {
    let mut original = first();
    original.target = Target::Branch {
        branch: Branch::parse("main").unwrap(),
        expected_head: Id::parse("44444444444444444444444444444444").unwrap(),
    };
    original.seen_sequence = 4;
    let request = original.save().unwrap();
    assert_eq!(
        request.headers()["zxs-base-node"],
        "44444444444444444444444444444444"
    );
    let body = serde_json::to_vec(&reply(&original, 11)).unwrap();
    assert_eq!(
        outcome(&original, StatusCode::OK, &body, true),
        Lookup::Confirmed { sequence: 11 }
    );
}
