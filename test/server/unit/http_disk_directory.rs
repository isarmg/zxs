use super::*;
use json_codec::json;
fn decode(body: Value, action: DiskDirectoryAction) -> Result<MutateDiskDirectory, ApiError> {
    let Value::Object(body) = crate::with_id(body, "slot_id", "P01".into())? else {
        unreachable!()
    };
    parse(body, action)
}
#[test]
fn directory_commands_parse_optional_root_and_keep_static_error_priority() {
    let operation = storage_contract::OperationId::generate();
    for action in [DiskDirectoryAction::Create, DiskDirectoryAction::Select] {
        let root = decode(json!({"name":"Side","operation_id":operation}), action).unwrap();
        assert!(root.path.is_none());
        assert_eq!(root.operation_id, operation);
        assert_eq!(root.name.key(), "side");
        assert!(
            decode(
                json!({"path":"","name":"Side","operation_id":operation}),
                action
            )
            .unwrap()
            .path
            .is_none()
        );
        let child = decode(
            json!({"path":"Café/Child","name":"Side","operation_id":operation}),
            action,
        )
        .unwrap();
        assert_eq!(child.path.unwrap().key(), "café/child");
        for body in [
            json!({"path":"","operation_id":operation}),
            json!({"path":"","name":"Side","operation_id":"bad"}),
            json!({"path":"","name":"x".repeat(129),"operation_id":operation}),
            json!({"path":"","name":null,"operation_id":operation}),
            json!({"path":false,"name":"Side","operation_id":operation}),
            json!({"path":"Cafe\u{301}","name":"Side","operation_id":operation}),
            json!({"path":"../x","name":"Side","operation_id":operation}),
            json!({"name":"Side","operation_id":operation,"unknown":0}),
        ] {
            assert_eq!(
                decode(body, action).err().unwrap().status,
                http::StatusCode::BAD_REQUEST
            );
        }
        let error = decode(json!({"name":"x".repeat(129),"operation_id":"bad"}), action)
            .err()
            .unwrap();
        assert_eq!(error.code, storage_contract::ErrorCode::MetadataTooLong);
        assert!(std::error::Error::source(&error).is_some());
        let error = decode(json!({"name":"Side","operation_id":"bad"}), action)
            .err()
            .unwrap();
        assert_eq!(error.code, storage_contract::ErrorCode::InvalidRequest);
        assert!(std::error::Error::source(&error).is_some());
    }
}
