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

#[test]
fn automatic_file_body_requires_file_and_entry_identity_without_directory_fields() {
    let original = json!({
        "path": "docs/report.txt",
        "entry_id": "entry_11111111111111111111111111111111",
        "file_id": "file_33333333333333333333333333333333",
        "target_copies": 3,
        "expected_generation": "0",
        "operation_id": "op_22222222222222222222222222222222",
    });
    let admitted = FileBody::from_json(original.clone()).unwrap();
    assert_eq!(admitted.path.as_str(), "docs/report.txt");
    assert_eq!(
        admitted.file_id.to_string(),
        "file_33333333333333333333333333333333"
    );
    for field in ["entry_id", "file_id", "path"] {
        let mut missing = original.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert!(FileBody::from_json(missing).is_err(), "{field}");
    }
    for field in ["directory_id", "backup_prefixes", "principal", "capture"] {
        let mut extra = original.clone();
        extra[field] = json!("caller-supplied");
        assert!(FileBody::from_json(extra).is_err(), "{field}");
    }
}
