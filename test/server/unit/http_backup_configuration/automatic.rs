use super::*;
use json_codec::{Value, json};

fn body() -> Value {
    json!({
        "path": "docs",
        "directory_id": "entry_11111111111111111111111111111111",
        "target_copies": 3,
        "expected_generation": "0",
        "operation_id": "op_22222222222222222222222222222222",
    })
}

#[test]
fn automatic_directory_body_is_typed_and_excludes_caller_authority_or_prefixes() {
    let admitted = Body::from_json(body()).unwrap();
    assert_eq!(admitted.path.as_str(), "docs");
    assert_eq!(admitted.target_copies, 3);
    assert_eq!(admitted.expected_generation, Generation::new(0));
    for field in ["principal", "repository_id", "backup_prefixes", "capture"] {
        let mut extra = body();
        extra[field] = json!("caller-supplied");
        assert!(Body::from_json(extra).is_err(), "{field}");
    }
    for (field, invalid) in [
        ("path", json!("../outside")),
        (
            "directory_id",
            json!("file_11111111111111111111111111111111"),
        ),
        ("expected_generation", json!(0)),
        ("operation_id", json!("not-an-operation")),
    ] {
        let mut wrong = body();
        wrong[field] = invalid;
        assert!(Body::from_json(wrong).is_err(), "{field}");
    }
}
