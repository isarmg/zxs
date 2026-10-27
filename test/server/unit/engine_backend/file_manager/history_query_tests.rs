//! History query validation through the authenticated backend dispatch.
use super::*;
use crate::{Backend, auth::AuthStore, engine_backend::EngineBackend};
use std::sync::Arc;
use storage_engine::{CommitRequest, Config, new_id};

#[test]
fn history_context_preserves_projection_and_current_admission() {
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
    let read = |input: Value| {
        let query = input.get("query");
        let request = crate::ReadFileHistory::parse(
            input["file_id"].as_str().unwrap().into(),
            query
                .and_then(|q| q.get("repository_id"))
                .map(|v| v.as_str().unwrap().into()),
            query
                .and_then(|q| q.get("path"))
                .map(|v| v.as_str().unwrap().into()),
        )?;
        backend
            .file_history_snapshot(&identity, request)
            .map(crate::backend_file_history::FileHistoryResponse::from)
            .map(|response| json_codec::to_value(response).unwrap())
    };
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
    let error = read(json!({"file_id":entry.file_id})).unwrap_err();
    assert_eq!(error.status, http::StatusCode::UNAUTHORIZED);
    assert_eq!(error.code, storage_contract::ErrorCode::Unauthorized);
}
