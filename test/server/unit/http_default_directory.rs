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
    let operation = OperationId::generate();
    let valid = json!({"directory_branch_id":"directory", "operation_id":operation});
    let parsed = decode(valid.clone()).unwrap();
    assert_eq!(parsed.repository_id, "repository");
    assert_eq!(parsed.directory_branch_id, "directory");
    assert_eq!(parsed.operation_id, operation);
    for value in [json!(null), json!([]), json!({"repository_id":"other"})] {
        assert_eq!(decode(value).err().unwrap().code, ErrorCode::InvalidRequest);
    }
    for key in ["directory_branch_id", "operation_id"] {
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

fn decode_rename(body: Value) -> Result<crate::RenameDirectoryBranch, ApiError> {
    let body = crate::with_id(body, "repository_id", "repository".into())?;
    let Value::Object(body) = crate::with_id(body, "directory_branch_id", "directory".into())?
    else {
        unreachable!()
    };
    parse_rename(body)
}
#[test]
fn rename_ingress_binds_both_paths_and_keeps_operation_before_name_policy() {
    let operation = OperationId::generate();
    let command = decode_rename(json!({"name":"Café","operation_id":operation})).unwrap();
    assert_eq!(command.repository_id, "repository");
    assert_eq!(command.directory_branch_id, "directory");
    assert_eq!(command.name.as_str(), "Café");
    assert_eq!(command.name.key(), "café");
    assert_eq!(command.operation_id, operation);
    for key in ["repository_id", "directory_branch_id"] {
        let mut body = json!({"name":"Café","operation_id":operation});
        body[key] = json!("other");
        assert_eq!(
            decode_rename(body).err().unwrap().message,
            "path and body identity disagree"
        );
    }
    let error = decode_rename(json!({"name":"bad/name","operation_id":"bad-id"}))
        .err()
        .unwrap();
    assert_eq!(error.code, ErrorCode::InvalidId);
    assert!(std::error::Error::source(&error).is_some());
    assert_eq!(
        decode_rename(json!({"name":"","operation_id":"bad-id"}))
            .err()
            .unwrap()
            .code,
        ErrorCode::InvalidRequest
    );
    for (name, code) in [
        ("bad/name".into(), ErrorCode::NameInvalidCharacter),
        ("Cafe\u{301}".into(), ErrorCode::NameNotNormalized),
        ("x".repeat(129), ErrorCode::MetadataTooLong),
    ] {
        let error = decode_rename(json!({"name":name,"operation_id":operation}))
            .err()
            .unwrap();
        assert_eq!(error.code, code);
        assert_eq!(error.status, http::StatusCode::BAD_REQUEST);
        assert!(std::error::Error::source(&error).is_some());
    }
    for value in [json!(null), json!(false), json!([]), json!(17)] {
        assert_eq!(
            decode_rename(json!({"name":value,"operation_id":operation}))
                .err()
                .unwrap()
                .code,
            ErrorCode::InvalidRequest
        );
    }
    assert_eq!(
        decode_rename(json!({"name":"Café","operation_id":operation,"actor":"forged"}))
            .err()
            .unwrap()
            .message,
        "unknown field actor"
    );
}
