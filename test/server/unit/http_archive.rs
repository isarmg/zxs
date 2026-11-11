use super::*;
use json_codec::json;

fn decode_at(body: Value, branch: &str) -> Result<ArchiveBranch, ApiError> {
    let body = ArchiveBody::from_json(body)?;
    let branch: ArchiveBranchPath = json_codec::from_value(json!(branch)).unwrap();
    let branch = branch.bind(&body)?;
    parse(body, branch)
}

#[test]
fn archive_ingress_preserves_field_unknown_and_operation_error_order() {
    let operation = storage_contract::OperationId::generate().unwrap();
    let parsed = decode_at(json!({"operation_id":operation}), "branch").unwrap();
    assert!(parsed.branch_id.into_id().is_err());
    assert_eq!(parsed.operation_id, operation);
    for value in [json!(null), json!(false), json!(17), json!(""), json!([])] {
        let invalid = decode_at(
            json!({"operation_id":value,"unexpected":true}),
            "branch",
        )
        .err()
        .unwrap();
        assert_eq!(invalid.code, storage_contract::ErrorCode::InvalidRequest);
        assert!(invalid.source.is_none());
    }
    for body in [
        json!({"unexpected":true}),
        json!({"operation_id":"bad","unexpected":true}),
        json!({"branch_id":"other","operation_id":"bad"}),
        json!([]),
    ] {
        let invalid = decode_at(body, "branch").err().unwrap();
        assert_eq!(invalid.code, storage_contract::ErrorCode::InvalidRequest);
        assert!(invalid.source.is_none());
    }
    let invalid = decode_at(json!({"operation_id":"bad"}), "branch").err().unwrap();
    crate::tests::assert_contract_input(
        &invalid,
        &"bad".parse::<storage_contract::OperationId>().unwrap_err(),
    );
    assert!(storage_contract::strict_json(br#"{"operation_id":"a","operation_id":"b"}"#).is_err());
}

#[test]
fn archive_path_binds_original_spelling_before_retaining_lookup_failure() {
    let operation = storage_contract::OperationId::from_random([0; 16]);
    for original in [
        "br_0123456789abcdef0123456789abcdef",
        "br_0123456789ABCDEF0123456789abcdef",
        "br_0123456789abcdef0123456789abcde",
        "file_0123456789abcdef0123456789abcdef",
        " br_0123456789abcdef0123456789abcdef",
        "malformed",
    ] {
        let command = decode_at(
            json!({"branch_id":original,"operation_id":operation}),
            original,
        )
        .unwrap();
        let parsed = command.branch_id.into_id();
        if original == "br_0123456789abcdef0123456789abcdef" {
            assert_eq!(parsed.unwrap().to_string(), original);
        } else {
            assert!(matches!(
                parsed.unwrap_err(),
                storage_contract::Error::InvalidIdentifier { prefix: "br", .. },
            ));
        }
        for supplied in [json!(null), json!(9), json!("different")] {
            let error = decode_at(
                json!({"branch_id":supplied,"operation_id":"bad"}),
                original,
            )
            .err()
            .unwrap();
            assert_eq!(error.status, http::StatusCode::BAD_REQUEST);
            assert_eq!(error.code, storage_contract::ErrorCode::InvalidRequest);
            assert!(error.source.is_none());
        }
    }
}

#[test]
fn archive_path_keeps_fixed_request_digest_bytes() {
    let command = decode_at(
        json!({
            "branch_id":"br_000102030405060708090a0b0c0d0e0f",
            "operation_id":"op_101112131415161718191a1b1c1d1e1f",
        }),
        "br_000102030405060708090a0b0c0d0e0f",
    )
    .unwrap();
    let request = json!({
        "kind":"branch.archive",
        "principal":"admin",
        "body":{"branch_id":command.branch_id.into_id().unwrap().to_string()},
    });
    let bytes = concat!(
        "{\"body\":{\"branch_id\":\"br_000102030405060708090a0b0c0d0e0f\"},",
        "\"kind\":\"branch.archive\",\"principal\":\"admin\"}",
    )
    .as_bytes();
    assert_eq!(storage_contract::canonical_json(&request).unwrap(), bytes);
}

#[tokio::test]
async fn malformed_archive_paths_reach_custom_backend_after_body_admission() {
    use http_routing::{body::Body, http::Request};
    use service_stack::ServiceExt;

    let root = crate::tests::private_tempdir();
    let mut auth = crate::auth::AuthStore::initialize(
        root.path(), "admin", "integration-test-password",
    )
    .unwrap();
    let token = auth.login("admin", "integration-test-password").unwrap().token;
    let app = crate::router(
        crate::tests::history_window_test_backend(root.path().join("content")),
        auth,
        crate::ServerConfig::new(root.path()),
    )
    .unwrap();
    let operation = storage_contract::OperationId::from_random([0; 16]);
    for body in [
        json!({"operation_id":operation}),
        json!({"branch_id":"malformed","operation_id":operation}),
    ] {
        let response = app.clone().oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/branches/malformed/archive")
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(json_codec::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), http::StatusCode::NOT_IMPLEMENTED);
        let bytes = http_routing::body::to_bytes(response.into_body(), 4096).await.unwrap();
        let response: Value = json_codec::from_slice(&bytes).unwrap();
        assert_eq!(response["error"]["code"], "UNSUPPORTED_OPERATION");
    }
}
