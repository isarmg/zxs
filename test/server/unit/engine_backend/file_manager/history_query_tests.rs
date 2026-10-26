//! History query validation through the authenticated backend dispatch.
use super::*;
use crate::{Backend, BackendAction, auth::AuthStore, engine_backend::EngineBackend};
use std::sync::Arc;
use storage_engine::{CommitRequest, Config, new_id};

#[test]
fn history_query_rejects_unknown_fields_without_changing_context_or_admission() {
    let root = crate::tests::private_tempdir();
    let mut engine = Engine::initialize(
        &root.path().join("system"),
        Config::development([root.path().join("disk")]),
    )
    .unwrap();
    let repo = engine
        .create_disk_root("P01", "owner", storage_contract::OperationId::generate())
        .unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "owner")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "report.txt", b"history body")
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: storage_contract::OperationId::generate(),
            principal: "owner".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "history query fixture".into(),
            request_digest: None,
        })
        .unwrap();
    let entry = engine.list_tree(&repo.main_branch_id).unwrap()[0].clone();
    let backend = Arc::new(EngineBackend::new(engine));
    let mut auth = backend
        .protect_auth(
            AuthStore::initialize(root.path(), "owner", "integration-test-password").unwrap(),
        )
        .unwrap();
    let token = auth
        .login("owner", "integration-test-password")
        .unwrap()
        .token;
    let identity = auth.authenticate(&token).unwrap();
    let read = |input| backend.call(BackendAction::FileManagerFileHistory, &identity, input);
    let plain = json!({"file_id":entry.file_id});
    let expected = read(plain.clone()).unwrap();
    let context = json!({"repository_id":repo.repository_id,"path":"report.txt"});
    for query in [json!({}), context.clone()] {
        assert_eq!(
            read(json!({"file_id":entry.file_id,"query":query})).unwrap(),
            expected
        );
    }
    assert_eq!(expected["version_id"], entry.version_id);
    assert_eq!(expected["current_node_id"], entry.version_id);
    assert_eq!(expected["path"], "report.txt");

    for field in ["limit", "cursor", "repositoryId", "Path"] {
        for mut query in [json!({}), context.clone()] {
            query[field] = "unused".into();
            let input = json!({"file_id":entry.file_id,"query":query});
            let error = read(input.clone()).unwrap_err();
            assert_eq!(error.status, http::StatusCode::BAD_REQUEST, "{input}");
            assert_eq!(
                error.code,
                storage_contract::ErrorCode::InvalidRequest,
                "{input}"
            );
            assert_eq!(error.message, format!("unknown field {field}"));
        }
    }
    for query in [
        json!({"repository_id":repo.repository_id}),
        json!({"path":"report.txt"}),
    ] {
        let error = read(json!({"file_id":entry.file_id,"query":query})).unwrap_err();
        assert_eq!(error.status, http::StatusCode::BAD_REQUEST);
        assert_eq!(error.code, storage_contract::ErrorCode::InvalidRequest);
        assert_eq!(
            error.message,
            "repository_id and path must be supplied together"
        );
    }
    for query in [
        json!({"repository_id":new_id("repo"),"path":"report.txt"}),
        json!({"repository_id":repo.repository_id,"path":"missing.txt"}),
    ] {
        let error = read(json!({"file_id":entry.file_id,"query":query})).unwrap_err();
        assert_eq!(error.status, http::StatusCode::NOT_FOUND);
        assert_eq!(error.code, storage_contract::ErrorCode::FileNotFound);
    }
    assert_eq!(read(plain).unwrap(), expected);

    auth.logout(&token).unwrap();
    let error = read(json!({"file_id":entry.file_id,"query":{"limit":"1"}})).unwrap_err();
    assert_eq!(error.status, http::StatusCode::UNAUTHORIZED);
    assert_eq!(error.code, storage_contract::ErrorCode::Unauthorized);
}
