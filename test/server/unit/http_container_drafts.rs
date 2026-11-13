use super::Body;
use crate::http_boundary::JsonBody;
use json_codec::json;

fn body() -> json_codec::Value {
    json!({
        "repository": storage_contract::RepositoryId::from_random([2; 16]),
        "path": "file.txt",
        "expected_generation": "7",
        "mode": "manual",
        "operation_id": storage_contract::OperationId::from_random([3; 16]),
    })
}

#[test]
fn edit_mode_preserves_typed_context_and_generation() {
    let parsed = Body::from_json(body()).unwrap().0;
    assert_eq!(parsed.path.as_str(), "file.txt");
    assert_eq!(parsed.expected_generation.get(), 7);
    assert_eq!(parsed.mode, storage_contract::container_edit::CommitMode::Manual);
    let mut automatic = body();
    automatic["mode"] = json!("automatic");
    assert_eq!(
        Body::from_json(automatic).unwrap().0.mode,
        storage_contract::container_edit::CommitMode::Automatic,
    );
}

#[test]
fn edit_mode_rejects_client_principal_and_historical_target() {
    for field in ["principal", "version", "draft", "branch"] {
        let mut candidate = body();
        candidate[field] = json!("not-a-policy-field");
        assert!(Body::from_json(candidate).is_err(), "{field}");
    }
    for (field, value) in [
        ("mode", json!("deferred")),
        ("expected_generation", json!(-1)),
        ("operation_id", json!("not-an-operation")),
        ("path", json!("../outside")),
    ] {
        let mut candidate = body();
        candidate[field] = value;
        assert!(Body::from_json(candidate).is_err(), "{field}");
    }
}

#[test]
fn explicit_prepare_requires_exact_draft_generation_and_operation_without_body_upload() {
    let request = json!({
        "repository":storage_contract::RepositoryId::from_random([2; 16]),
        "path":"file.txt", "expected_generation":"2",
        "draft_operation":storage_contract::OperationId::from_random([4; 16]),
        "operation_id":storage_contract::OperationId::from_random([5; 16]),
    });
    let parsed = super::DraftCommitBody::from_json(request.clone()).unwrap();
    assert_eq!(parsed.0.expected_generation.get(), 2);
    let mut injected = request;
    injected["bytes"] = json!("client-selected replacement");
    assert!(super::DraftCommitBody::from_json(injected).is_err());
}
