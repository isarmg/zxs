use super::*;
use crate::{Backend, auth::AuthStore, tests::history_read_control as control};
use json_codec::{Value, json};
use storage_contract::{BranchState, Config, OperationId};

fn fixture() -> (tempfile::TempDir, Engine, Repository, Identity) {
    let root = crate::tests::private_tempdir();
    let mut engine = Engine::initialize(
        &root.path().join("system"),
        Config::development([root.path().join("disk")]),
    )
    .unwrap();
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("branches").unwrap(),
            "P01",
            1,
            "owner",
            OperationId::generate(),
        )
        .unwrap();
    let identity = crate::engine_backend::install_test_administrator(&mut engine, "owner");
    (root, engine, repo, identity)
}
fn value(snapshot: &crate::BranchMetadataSnapshot) -> Value {
    json_codec::from_slice(snapshot.body()).unwrap()
}
fn save(engine: &mut Engine, branch: &str) {
    let workspace = engine.create_workspace(branch, "owner").unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "file.txt", b"content")
        .unwrap();
    engine
        .commit_workspace(storage_engine::CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: OperationId::generate(),
            principal: "owner".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "save".into(),
            request_digest: None,
        })
        .unwrap();
}
#[test]
fn branch_reads_preserve_create_wire_list_key_order_and_migrated_admission() {
    let (_root, engine, repo, identity) = fixture();
    let source = engine.state().branches[&repo.main_branch_id]
        .head_commit_id
        .clone();
    let backend = EngineBackend::new(engine);
    let created = backend
        .create_branch(
            &identity,
            crate::CreateBranch {
                repository_id: repo.repository_id.clone(),
                source_commit_id: source,
                name: storage_contract::BranchName::parse("fork").unwrap(),
                operation_id: OperationId::generate(),
            },
        )
        .unwrap();
    let id = created.branch_id.clone();
    let expected =
        json_codec::to_value(crate::backend_branch::BranchResponse::from(created)).unwrap();
    assert_eq!(
        value(&backend.branch_snapshot(&identity, &id, &control()).unwrap()),
        expected
    );
    let mut engine = backend.engine.lock().unwrap();
    let mut state = engine.state().clone();
    let mut migrated = state.branches[&id].clone();
    migrated.state = BranchState::Migrated;
    state.branches.insert(id.clone(), migrated);
    let mut unrelated = state.branches[&id].clone();
    unrelated.branch_id = "unrelated".into();
    unrelated.history_tree_id = "another-history".into();
    state.branches.insert("unrelated".into(), unrelated);
    storage_engine::test_support::replace_state(&mut engine, state);
    let expected = engine
        .state()
        .branches
        .values()
        .filter(|branch| branch.history_tree_id == repo.history_tree_id)
        .map(|branch| {
            json_codec::to_value(crate::backend_branch::BranchResponse::from(
                crate::engine_backend::branch::capture(&engine, branch).unwrap(),
            ))
            .unwrap()
        })
        .collect::<Vec<_>>();
    drop(engine);
    assert_eq!(
        value(
            &backend
                .branches_snapshot(&identity, &repo.repository_id, &control())
                .unwrap()
        ),
        json!({"branches":expected})
    );
    assert_eq!(
        backend
            .branch_snapshot(&identity, &id, &control())
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::TreeMoved
    );
    assert_eq!(
        backend
            .branch_snapshot(&identity, "missing", &control())
            .unwrap_err()
            .message,
        "branch not found"
    );
    assert_eq!(
        backend
            .branches_snapshot(&identity, "missing", &control())
            .unwrap_err()
            .message,
        "repository not found"
    );
}
#[test]
fn detached_branch_rows_borrow_maps_release_writer_and_outlive_publication() {
    let (_root, engine, repo, identity) = fixture();
    let expected = value(
        &single(
            engine.published_read_view(),
            &repo.main_branch_id,
            &control(),
        )
        .unwrap(),
    );
    let head = &engine.state().branches[&repo.main_branch_id].head_commit_id;
    let commit = engine.state().commits.get_shared(head).unwrap();
    let weak = Arc::downgrade(&commit);
    drop(commit);
    let backend = EngineBackend::new(engine);
    let view = backend.engine.lock().unwrap().published_read_view();
    let branch = &view.state().branches[&repo.main_branch_id];
    let borrowed = row(&view, branch, &control()).unwrap();
    assert!(std::ptr::eq(borrowed.branch, branch));
    assert_eq!(borrowed.branch.name.as_ptr(), branch.name.as_ptr());
    assert_eq!(
        value(&crate::BranchMetadataSnapshot::encode_borrowed(borrowed).unwrap()),
        expected
    );
    let mut writer = backend.engine.lock().unwrap();
    save(&mut writer, &repo.main_branch_id);
    let branch = repo.main_branch_id.clone();
    let (send, receive) = std::sync::mpsc::sync_channel(1);
    let worker = std::thread::spawn(move || send.send(single(view, &branch, &control())).unwrap());
    let result = receive.recv_timeout(std::time::Duration::from_secs(30));
    drop(writer);
    worker.join().unwrap();
    let encoded = backend
        .finish_branch_metadata(
            &identity,
            result.expect("branch reader waited for writer mutex"),
            &control(),
        )
        .unwrap();
    assert_eq!(value(&encoded), expected);
    drop(backend);
    assert!(
        weak.upgrade().is_none(),
        "branch metadata result retained old publication"
    );
    let pointer = encoded.body().as_ptr();
    assert_eq!(encoded.into_encoded().body().as_ptr(), pointer);
}
#[test]
fn cold_namespace_errors_precede_list_cap_even_with_pending_and_explicit_node() {
    let (_root, mut engine, repo, _) = fixture();
    let mut state = engine.state().clone();
    let mut first = state.branches[&repo.main_branch_id].clone();
    first.branch_id = "a".into();
    first.name = "x".repeat(storage_contract::client_protocol::MAX_RESPONSE_BYTES);
    let mut bad = first.clone();
    bad.branch_id = "z".into();
    bad.name = "bad".into();
    bad.file_id = Some("file".into());
    bad.file_node_id = Some("explicit".into());
    bad.pending_file_versions
        .insert("file".into(), "source".into());
    let head = bad.head_commit_id.clone();
    // A fresh, syntactically valid absent object cannot be satisfied by warm caches.
    state.commits.get_mut(&head).unwrap().namespace.root.hash = "0".repeat(64);
    state.commits.get_mut(&head).unwrap().namespace.entries = 1;
    state.branches.clear();
    state.branches.insert("a".into(), first);
    state.branches.insert("z".into(), bad.clone());
    storage_engine::test_support::replace_state(&mut engine, state);
    let expected = error(crate::engine_backend::branch::capture(&engine, &bad).unwrap_err());
    assert_ne!(
        expected.code,
        storage_contract::ErrorCode::ResourceLimitExceeded
    );
    let single_error = single(engine.published_read_view(), "z", &control()).unwrap_err();
    let list_error = list(
        engine.published_read_view(),
        &repo.repository_id,
        &control(),
    )
    .unwrap_err();
    for actual in [single_error, list_error] {
        assert_eq!(actual.code, expected.code);
        assert_eq!(actual.message, expected.message);
        assert!(actual.source.is_some());
    }
}
#[test]
fn branch_completion_current_auth_precedes_success_error_and_cancel() {
    for managed in [false, true] {
        let (root, engine, repo, _) = fixture();
        let backend = Arc::new(EngineBackend::new(engine));
        let auth =
            AuthStore::initialize(root.path(), "owner", "integration-test-password").unwrap();
        let mut auth = if managed {
            backend.set_management_auth(auth.reader()).unwrap();
            auth
        } else {
            backend.protect_auth(auth).unwrap()
        };
        let token = auth
            .login("owner", "integration-test-password")
            .unwrap()
            .token;
        let identity = auth.authenticate(&token).unwrap();
        let success = single(
            backend.engine.lock().unwrap().published_read_view(),
            &repo.main_branch_id,
            &control(),
        );
        let failure = || {
            ApiError::new(
                ::http::StatusCode::INTERNAL_SERVER_ERROR,
                storage_contract::ErrorCode::ObjectCorrupt,
                "captured namespace failed",
            )
        };
        assert_eq!(
            backend
                .finish_branch_metadata(&identity, Err(failure()), &control())
                .unwrap_err()
                .code,
            storage_contract::ErrorCode::ObjectCorrupt
        );
        let cancelled = control();
        drop(cancelled.wait_owner());
        assert_eq!(
            backend
                .finish_branch_metadata(&identity, Err(failure()), &cancelled)
                .unwrap_err()
                .code,
            storage_contract::ErrorCode::RequestTimeout
        );
        assert_eq!(
            backend
                .branch_snapshot(&identity, &repo.main_branch_id, &cancelled)
                .unwrap_err()
                .code,
            storage_contract::ErrorCode::RequestTimeout
        );
        assert_eq!(
            backend
                .branches_snapshot(&identity, &repo.repository_id, &cancelled)
                .unwrap_err()
                .code,
            storage_contract::ErrorCode::RequestTimeout
        );
        auth.logout(&token).unwrap();
        for result in [success, Err(failure())] {
            assert_eq!(
                backend
                    .finish_branch_metadata(&identity, result, &control())
                    .unwrap_err()
                    .code,
                storage_contract::ErrorCode::Unauthorized
            );
        }
        assert_eq!(
            backend
                .finish_branch_metadata(&identity, Err(failure()), &cancelled)
                .unwrap_err()
                .code,
            storage_contract::ErrorCode::Unauthorized
        );
        assert_eq!(
            backend
                .branch_snapshot(&identity, "missing", &control())
                .unwrap_err()
                .code,
            storage_contract::ErrorCode::Unauthorized
        );
        assert_eq!(
            backend
                .branches_snapshot(&identity, "missing", &control())
                .unwrap_err()
                .code,
            storage_contract::ErrorCode::Unauthorized
        );
    }
}
