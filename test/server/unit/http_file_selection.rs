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
    let operation = storage_contract::OperationId::generate();
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
