use super::*;
use json_codec::json;
fn decode(body: Value, route: NoteRoute) -> Result<UpdateNote, ApiError> {
    let body = match route {
        NoteRoute::File => crate::with_id(body, "file_id", "file".into())?,
        NoteRoute::Version => crate::with_id(body, "version_id", "version".into())?,
        NoteRoute::Node => crate::with_id(
            crate::with_id(body, "file_id", "file".into())?,
            "node_id",
            "node".into(),
        )?,
    };
    let Value::Object(body) = body else {
        unreachable!("fixture binds object")
    };
    parse(body, route)
}
#[test]
fn note_routes_distinguish_clear_from_missing_null_and_preserve_error_order() {
    let operation = storage_contract::OperationId::generate();
    for route in [NoteRoute::File, NoteRoute::Version, NoteRoute::Node] {
        for value in ["", "  ", "Cafe\u{301}"] {
            let command = decode(json!({"note":value,"operation_id":operation}), route).unwrap();
            assert_eq!(command.note.as_str(), value);
            assert_eq!(command.operation_id, operation);
        }
        for body in [
            json!({"operation_id":operation}),
            json!({"note":null,"operation_id":operation}),
            json!({"note":false,"operation_id":operation}),
            json!({"note":"x".repeat(129),"operation_id":operation}),
            json!({"note":"bad\ntext","operation_id":operation}),
            json!({"note":"","operation_id":operation,"unknown":0}),
        ] {
            assert_eq!(
                decode(body, route).err().unwrap().status,
                http::StatusCode::BAD_REQUEST
            );
        }
        let error = decode(json!({"note":"x".repeat(129),"operation_id":"bad"}), route)
            .err()
            .unwrap();
        assert_eq!(error.code, storage_contract::ErrorCode::InvalidRequest);
        assert!(std::error::Error::source(&error).is_some());
        assert!(!error.message.contains("notes must"));
        assert_eq!(
            decode(json!({"operation_id":"bad"}), route)
                .err()
                .unwrap()
                .message,
            "note must be a string"
        );
    }
}
