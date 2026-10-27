use super::*;
use json_codec::{Value, json};
fn decode(value: Value) -> Result<ReadDirectorySelection, ApiError> {
    parse(crate::with_id(value, "repository_id", "repo".into())?)
}
#[test]
fn selection_http_shape_is_strict_and_does_not_default_missing_arrays() {
    let valid = json!({"path":"","sources":[{"directory_branch_id":"branch","expected_tree_id":"tree"}],"entries":[{"entry_id":"entry","path":"file.txt"}]});
    let parsed = decode(valid.clone()).unwrap();
    assert_eq!(parsed.repository_id(), "repo");
    assert_eq!(parsed.entries()[0].path().key(), "file.txt");
    for key in ["path", "sources", "entries"] {
        let mut missing = valid.clone();
        missing.as_object_mut().unwrap().remove(key);
        assert_eq!(
            decode(missing).err().unwrap().status,
            http::StatusCode::BAD_REQUEST
        );
        for value in [json!(null), json!(false), json!(12)] {
            let mut wrong = valid.clone();
            wrong[key] = value;
            let error = decode(wrong).err().unwrap();
            assert_eq!(error.status, http::StatusCode::BAD_REQUEST);
            assert!(std::error::Error::source(&error).is_some());
        }
    }
    for member in ["sources", "entries"] {
        for value in [json!(null), json!([]), json!("entry")] {
            let mut wrong = valid.clone();
            wrong[member][0] = value;
            assert!(decode(wrong).is_err());
        }
        let mut unknown = valid.clone();
        unknown[member][0]["unknown"] = json!(true);
        assert!(decode(unknown).is_err());
    }
    let mut mismatch = valid.clone();
    mismatch["repository_id"] = json!("other");
    assert_eq!(
        decode(mismatch).err().unwrap().message,
        "path and body identity disagree"
    );
    let mut unknown = valid;
    unknown["principal"] = json!("forged");
    assert!(decode(unknown).is_err());
    assert!(
        storage_contract::strict_json(br#"{"path":"","path":"other","sources":[],"entries":[]}"#)
            .is_err()
    );
}
