use super::*;
use json_codec::json;
#[derive(Clone, Copy)]
enum NoteRoute {
    File,
    Version,
    Node,
}

fn decode(body: Value, route: NoteRoute) -> Result<UpdateNote, ApiError> {
    let body = NoteBody::from_json(body)?;
    let target = match route {
        NoteRoute::File => {
            body.matches_path("file_id", "file")?;
            NoteTarget::File("file".into())
        }
        NoteRoute::Version => {
            body.matches_path("version_id", "version")?;
            NoteTarget::Version("version".into())
        }
        NoteRoute::Node => {
            body.matches_path("file_id", "file")?;
            body.matches_path("node_id", "node")?;
            NoteTarget::Node {
                file: "file".into(),
                node: "node".into(),
            }
        }
    };
    parse(body, target)
}
#[test]
fn note_routes_distinguish_clear_from_missing_null_and_preserve_error_order() {
    let operation = storage_contract::OperationId::generate().unwrap();
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
        assert_eq!(
            decode(json!({"operation_id":"bad"}), route)
                .err()
                .unwrap()
                .code,
            storage_contract::ErrorCode::InvalidRequest
        );
    }
}

#[test]
fn note_routes_preserve_contract_error_details_and_sources() {
    for route in [NoteRoute::File, NoteRoute::Version, NoteRoute::Node] {
        let error = decode(
            json!({"note":"x".repeat(129),"operation_id":storage_contract::OperationId::generate().unwrap()}),
            route,
        ).err().unwrap();
        crate::tests::assert_contract_input(
            &error,
            &storage_contract::Note::parse("x".repeat(129)).unwrap_err(),
        );
        let error = decode(json!({"note":"x".repeat(129),"operation_id":"bad"}), route)
            .err()
            .unwrap();
        assert_eq!(error.code, storage_contract::ErrorCode::InvalidRequest);

    }
}
