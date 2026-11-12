use super::*;
use json_codec::json;
use storage_contract::{ErrorCode, OperationId};

fn decode(body: Value) -> Result<SelectDefaultDirectoryBranch, ApiError> {
    let Value::Object(body) = crate::with_id(body, "repository_id", "repository".into())? else {
        unreachable!()
    };
    parse(body)
}
#[test]
fn selection_ingress_binds_path_and_parses_operation_once() {
    let operation = OperationId::generate().unwrap();
    let valid = json!({"directory_branch_id":"directory", "expected_tree_id":"tree", "expected_head":"head", "operation_id":operation});
    let parsed = decode(valid.clone()).unwrap();
    assert_eq!(parsed.repository_id, "repository");
    assert_eq!(parsed.directory_branch_id, "directory");
    assert_eq!(parsed.operation_id, operation);
    for value in [json!(null), json!([]), json!({"repository_id":"other"})] {
        assert_eq!(decode(value).err().unwrap().code, ErrorCode::InvalidRequest);
    }
    for key in [
        "directory_branch_id",
        "expected_tree_id",
        "expected_head",
        "operation_id",
    ] {
        for value in [json!(null), json!(false), json!(17), json!("")] {
            let mut body = valid.clone();
            body[key] = value;
            assert_eq!(decode(body).err().unwrap().code, ErrorCode::InvalidRequest);
        }
        let mut body = valid.clone();
        body.as_object_mut().unwrap().remove(key);
        assert_eq!(decode(body).err().unwrap().code, ErrorCode::InvalidRequest);
    }
    let mut invalid = valid.clone();
    invalid["operation_id"] = json!("bad-id");
    let error = decode(invalid.clone()).err().unwrap();
    assert_eq!(error.code, ErrorCode::InvalidId);
    assert!(std::error::Error::source(&error).is_some());
    invalid["directory_branch_id"] = json!(null);
    assert_eq!(
        decode(invalid.clone()).err().unwrap().message,
        "missing string field directory_branch_id"
    );
    invalid["actor"] = json!("forged");
    assert_eq!(
        decode(invalid).err().unwrap().message,
        "unknown field actor"
    );
    assert!(storage_contract::strict_json(br#"{"operation_id":"a","operation_id":"b"}"#).is_err());
}
