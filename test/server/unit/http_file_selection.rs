use super::*;
use json_codec::json;
fn decode(body: Value) -> Result<SelectFileBranch, ApiError> {
    let Value::Object(body) = crate::with_id(body, "file_id", "file".into())? else {
        unreachable!("fixture binds object")
    };
    parse(body)
}
#[test]
fn selection_context_is_absent_or_a_complete_parsed_pair() {
    let operation = storage_contract::OperationId::generate().unwrap();
    let bare = decode(json!({"branch_id":"branch","operation_id":operation})).unwrap();
    assert!(bare.location.is_none());
    assert_eq!(bare.file_id, "file");
    let contextual=decode(json!({"branch_id":"branch","repository_id":"repository","path":"Café/File.txt","operation_id":operation})).unwrap();
    let location = contextual.location.unwrap();
    assert_eq!(location.repository_id, "repository");
    assert_eq!(location.path.as_str(), "Café/File.txt");
    assert_eq!(location.path.key(), "café/file.txt");
    for body in [
        json!({"branch_id":"branch","repository_id":"repository","operation_id":operation}),
        json!({"branch_id":"branch","path":"file.txt","operation_id":operation}),
        json!({"branch_id":"branch","repository_id":null,"path":null,"operation_id":operation}),
        json!({"branch_id":"branch","repository_id":"repository","path":"","operation_id":operation}),
        json!({"branch_id":"branch","repository_id":"repository","path":"../file","operation_id":operation}),
        json!({"branch_id":"branch","repository_id":"repository","path":"Cafe\u{301}/file","operation_id":operation}),
        json!({"branch_id":null,"operation_id":operation}),
        json!({"branch_id":"branch","operation_id":operation,"file_id":"other"}),
        json!({"branch_id":"branch","operation_id":operation,"unknown":0}),
    ] {
        assert_eq!(
            decode(body).err().unwrap().status,
            http::StatusCode::BAD_REQUEST
        );
    }
    let error=decode(json!({"branch_id":"branch","repository_id":"repository","path":"Cafe\u{301}/file","operation_id":operation})).err().unwrap();
    assert_eq!(error.code, storage_contract::ErrorCode::NameNotNormalized);
    assert!(std::error::Error::source(&error).is_some());
    let error=decode(json!({"branch_id":"branch","repository_id":"repository","path":"../file","operation_id":"bad"})).err().unwrap();
    assert_eq!(error.code, storage_contract::ErrorCode::InvalidRequest);
    assert!(std::error::Error::source(&error).is_some());
}

#[test]
fn explicit_default_requires_a_complete_view_baseline_and_logical_path() {
    let operation = storage_contract::OperationId::generate().unwrap();
    let valid = json!({"file_id":"file","branch_id":"branch","repository_id":"repo","path":"Café/file.txt","expected_node_id":"node","expected_head":"head","operation_id":operation});
    let command = parse_default(valid.as_object().unwrap().clone()).unwrap();
    let baseline = command.default_baseline.unwrap();
    assert_eq!(baseline.expected_node_id, "node");
    assert_eq!(baseline.expected_head, "head");
    assert_eq!(command.location.unwrap().path.key(), "café/file.txt");
    for key in ["expected_node_id", "expected_head", "repository_id", "path"] {
        let mut missing = valid.clone();
        missing.as_object_mut().unwrap().remove(key);
        assert!(parse_default(missing.as_object().unwrap().clone()).is_err());
        for value in [json!(null), json!(false), json!("")] {
            let mut invalid = valid.clone();
            invalid[key] = value;
            assert!(parse_default(invalid.as_object().unwrap().clone()).is_err());
        }
    }
    for key in ["expected_node_id", "expected_head"] {
        let mut oversized = valid.clone();
        oversized[key] = "x".repeat(257).into();
        assert!(parse_default(oversized.as_object().unwrap().clone()).is_err());
    }
}
