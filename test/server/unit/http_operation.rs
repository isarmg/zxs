use super::*;
use std::error::Error as _;
#[test]
fn operation_lookup_keeps_not_found_mapping_and_parse_source() {
    for value in [
        "",
        "wrong",
        "op_0000000000000000000000000000000A",
        "op_000000000000000000000000000000000",
    ] {
        let error = operation_id(value).unwrap_err();
        assert_eq!(error.status, ::http::StatusCode::NOT_FOUND);
        assert_eq!(error.code, storage_contract::ErrorCode::OperationNotFound);
        assert_eq!(error.message, "operation not found");
        assert!(error.source().is_some());
    }
    let operation = storage_contract::OperationId::generate();
    assert_eq!(operation_id(&operation.to_string()).unwrap(), operation);
}
