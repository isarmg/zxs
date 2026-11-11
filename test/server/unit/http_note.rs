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
            let path: FileNotePath = json_codec::from_value(json!("file")).unwrap();
            NoteTarget::File(path.bind(&body)?)
        }
        NoteRoute::Version => {
            let path: VersionNotePath = json_codec::from_value(json!("version")).unwrap();
            NoteTarget::Version(path.bind(&body)?)
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

#[test]
fn version_path_binds_original_spelling_and_retains_lookup_failure() {
    for original in [
        "ver_0123456789abcdef0123456789abcdef",
        "malformed",
        "ver_0123456789ABCDEF0123456789abcdef",
        "file_0123456789abcdef0123456789abcdef",
        "ver_0123456789abcdef0123456789abcde",
        " ver_0123456789abcdef0123456789abcdef",
    ] {
        let path: VersionNotePath = json_codec::from_value(json!(original)).unwrap();
        let body = NoteBody::from_json(json!({"version_id":original})).unwrap();
        let lookup = path.bind(&body).unwrap().into_id();
        if original == "ver_0123456789abcdef0123456789abcdef" {
            assert_eq!(lookup.unwrap().to_string(), original);
        } else {
            assert!(matches!(
                lookup.unwrap_err(),
                storage_contract::Error::InvalidIdentifier { prefix: "ver", .. }
            ));
        }
        for supplied in [json!(null), json!(7), json!("different")] {
            let path: VersionNotePath = json_codec::from_value(json!(original)).unwrap();
            let body = NoteBody::from_json(json!({"version_id":supplied})).unwrap();
            let error = path.bind(&body).unwrap_err();
            assert_eq!(error.status, http::StatusCode::BAD_REQUEST);
            assert_eq!(error.code, storage_contract::ErrorCode::InvalidRequest);
        }
    }
}

#[test]
fn file_path_binds_original_spelling_and_retains_lookup_failure() {
    for original in [
        "file_0123456789abcdef0123456789abcdef",
        "malformed",
        "file_0123456789ABCDEF0123456789abcdef",
        "ver_0123456789abcdef0123456789abcdef",
        "file_0123456789abcdef0123456789abcde",
        " file_0123456789abcdef0123456789abcdef",
    ] {
        let path: FileNotePath = json_codec::from_value(json!(original)).unwrap();
        let body = NoteBody::from_json(json!({"file_id":original})).unwrap();
        let lookup = path.bind(&body).unwrap().into_id();
        if original == "file_0123456789abcdef0123456789abcdef" {
            assert_eq!(lookup.unwrap().to_string(), original);
        } else {
            assert!(matches!(
                lookup.unwrap_err(),
                storage_contract::Error::InvalidIdentifier { prefix: "file", .. }
            ));
        }
        for supplied in [json!(null), json!(7), json!("different")] {
            let path: FileNotePath = json_codec::from_value(json!(original)).unwrap();
            let body = NoteBody::from_json(json!({"file_id":supplied})).unwrap();
            let error = path.bind(&body).unwrap_err();
            assert_eq!(error.status, http::StatusCode::BAD_REQUEST);
            assert_eq!(error.code, storage_contract::ErrorCode::InvalidRequest);
        }
    }
}

#[test]
fn version_path_preserves_fixed_digest_input_bytes() {
    let path: VersionNotePath =
        json_codec::from_value(json!("ver_000102030405060708090a0b0c0d0e0f")).unwrap();
    let body = NoteBody::from_json(json!({
        "version_id":"ver_000102030405060708090a0b0c0d0e0f",
        "note":"typed note",
        "operation_id":"op_000102030405060708090a0b0c0d0e0f",
    }))
    .unwrap();
    let target = path.bind(&body).unwrap();
    let command = parse(body, NoteTarget::Version(target)).unwrap();
    let NoteTarget::Version(target) = command.target else {
        panic!("version path must preserve the version target");
    };
    let request = json!({
        "kind":"version.metadata",
        "principal":"admin",
        "body":{
            "file_id":"file_101112131415161718191a1b1c1d1e1f",
            "version_id":target.into_id().unwrap().to_string(),
            "note":command.note.as_str(),
        },
    });
    let bytes = concat!(
        "{\"body\":{\"file_id\":\"file_101112131415161718191a1b1c1d1e1f\",",
        "\"note\":\"typed note\",\"version_id\":\"ver_000102030405060708090a0b0c0d0e0f\"},",
        "\"kind\":\"version.metadata\",\"principal\":\"admin\"}",
    )
    .as_bytes();
    assert_eq!(storage_contract::canonical_json(&request).unwrap(), bytes);
}

fn custom_note_router() -> (tempfile::TempDir, http_routing::Router, String) {
    let root = crate::tests::private_tempdir();
    let mut auth = crate::auth::AuthStore::initialize(
        root.path(),
        "admin",
        "integration-test-password",
    )
    .unwrap();
    let token = auth
        .login("admin", "integration-test-password")
        .unwrap()
        .token;
    let app = crate::router(
        crate::tests::history_window_test_backend(root.path().join("content")),
        auth,
        crate::ServerConfig::new(root.path()),
    )
    .unwrap();
    (root, app, token)
}

async fn custom_note_response(
    app: &http_routing::Router,
    token: &str,
    route: &str,
    body: Value,
) -> (http::StatusCode, Value) {
    use http_routing::{body::Body, http::Request};
    use service_stack::ServiceExt;

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(route)
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(json_codec::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = http_routing::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    (status, json_codec::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn malformed_version_paths_reach_custom_backend_after_body_admission() {
    let route = "/api/file-manager/versions/malformed/metadata";
    let (_root, app, token) = custom_note_router();
    let operation = storage_contract::OperationId::from_random([0; 16]);
    for body in [
        json!({"note":"","operation_id":operation}),
        json!({"note":"","operation_id":operation,"version_id":"malformed"}),
    ] {
        let (status, response) = custom_note_response(&app, &token, route, body).await;
        assert_eq!(status, http::StatusCode::NOT_IMPLEMENTED);
        assert_eq!(response["error"]["code"], "UNSUPPORTED_OPERATION");
    }
    for body in [
        json!({"note":null,"operation_id":operation}),
        json!({"note":"","operation_id":"bad"}),
        json!({"note":"","operation_id":operation,"unknown":0}),
        json!({"note":"","operation_id":operation,"version_id":null}),
        json!({"note":"","operation_id":operation,"version_id":"other"}),
    ] {
        let (status, response) = custom_note_response(&app, &token, route, body).await;
        assert_eq!(status, http::StatusCode::BAD_REQUEST);
        assert_eq!(response["error"]["code"], "INVALID_REQUEST");
    }
}

#[tokio::test]
async fn malformed_file_paths_reach_custom_backend_after_body_admission() {
    let route = "/api/file-manager/files/malformed/metadata";
    let (_root, app, token) = custom_note_router();
    let operation = storage_contract::OperationId::from_random([0; 16]);
    for body in [
        json!({"note":"","operation_id":operation}),
        json!({"note":"","operation_id":operation,"file_id":"malformed"}),
    ] {
        let (status, response) = custom_note_response(&app, &token, route, body).await;
        assert_eq!(status, http::StatusCode::NOT_IMPLEMENTED);
        assert_eq!(response["error"]["code"], "UNSUPPORTED_OPERATION");
    }
    for body in [
        json!({"note":null,"operation_id":operation}),
        json!({"note":"","operation_id":"bad"}),
        json!({"note":"","operation_id":operation,"unknown":0}),
        json!({"note":"","operation_id":operation,"file_id":null}),
        json!({"note":"","operation_id":operation,"file_id":"other"}),
    ] {
        let (status, response) = custom_note_response(&app, &token, route, body).await;
        assert_eq!(status, http::StatusCode::BAD_REQUEST);
        assert_eq!(response["error"]["code"], "INVALID_REQUEST");
    }
}

#[test]
fn file_path_preserves_fixed_digest_input_bytes() {
    let path: FileNotePath =
        json_codec::from_value(json!("file_000102030405060708090a0b0c0d0e0f")).unwrap();
    let body = NoteBody::from_json(json!({
        "file_id":"file_000102030405060708090a0b0c0d0e0f",
        "note":"typed note",
        "operation_id":"op_000102030405060708090a0b0c0d0e0f",
    }))
    .unwrap();
    let target = path.bind(&body).unwrap();
    let command = parse(body, NoteTarget::File(target)).unwrap();
    let NoteTarget::File(target) = command.target else {
        panic!("file path must preserve the file target");
    };
    let request = json!({
        "kind":"file.metadata",
        "principal":"admin",
        "body":{
            "file_id":target.into_id().unwrap().to_string(),
            "version_id":null,
            "note":command.note.as_str(),
        },
    });
    let bytes = concat!(
        "{\"body\":{\"file_id\":\"file_000102030405060708090a0b0c0d0e0f\",",
        "\"note\":\"typed note\",\"version_id\":null},",
        "\"kind\":\"file.metadata\",\"principal\":\"admin\"}",
    )
    .as_bytes();
    assert_eq!(storage_contract::canonical_json(&request).unwrap(), bytes);
}
