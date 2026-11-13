//! Structural removal uses Current emptiness while retaining immutable history.
use super::*;
use std::io::Read;

fn namespace(kind: smb_protocol::protocol::NamespaceOperation, path: &str) -> Request {
    Request::Namespace {
        kind, path: path.into(), destination: None, replace: false,
        operation_id: new_id("op").unwrap(),
    }
}

fn deleted_child() -> (Fixture, Arc<Shared>, Arc<LostCommitReply>, String) {
    let (root, shared, backend) = empty_fixture(false);
    let (mut stream, worker, _) = wire(shared.clone(), "alice");
    use smb_protocol::protocol::NamespaceOperation as Kind;
    okay(&mut stream, namespace(Kind::Mkdir, "p/folder"));
    let (opened, file) = open(&mut stream, "p/folder/file.txt", true, true);
    save(&mut stream, &opened, &file, b"retained bytes");
    // save already flushes and closes the handle; retain only its returned FD here.
    drop(file);
    okay(&mut stream, namespace(Kind::Unlink, "p/folder/file.txt"));
    assert!(directory(&mut stream, "p/folder").is_empty());
    close_wire(stream, worker);
    let branch = shared.backend.repositories_snapshot(&identity(&shared)).unwrap()[0]
        .main_branch_id.clone();
    (root, shared, backend, branch)
}

fn stage_remove(engine: &mut Engine, branch: &str) -> CommitRequest {
    let workspace = engine.create_workspace(branch, "alice").unwrap();
    engine.namespace_operation(&workspace.workspace_id,
        storage_contract::NamespaceOperation::Delete {
            path: name_policy::LookupPath::parse("folder").unwrap(),
        },
    ).unwrap();
    request(workspace)
}
fn request(workspace: storage_contract::Workspace) -> CommitRequest {
    CommitRequest {
        workspace_id: workspace.workspace_id,
        operation_id: OperationId::generate().unwrap(), principal: "alice".into(),
        expected_head: workspace.base_head, fencing_token: workspace.fencing_token,
        message: "ordinary current structure".into(), request_digest: None,
    }
}

#[test]
fn current_empty_rmdir_keeps_all_old_history_and_deletion_facts_after_cold_open() {
    let (root, shared, backend, branch) = deleted_child();
    let before = backend.inner.test_with_engine_locked(|engine| engine.state().clone());
    let (mut stream, worker, _) = wire(shared.clone(), "alice");
    let operation = OperationId::generate().unwrap();
    let removal = || Request::Namespace {
        kind: smb_protocol::protocol::NamespaceOperation::Rmdir,
        path: "p/folder".into(),
        destination: None,
        replace: false,
        operation_id: operation.to_string(),
    };
    // Lose the reply only after the real backend has durably published.
    backend.fail.store(true, Ordering::SeqCst);
    let (reply, descriptor) = binding_tests::exchange(&mut stream, removal());
    assert!(descriptor.is_none());
    assert!(matches!(reply, Response::Error {
        code: storage_contract::ErrorCode::InjectedFailure, ..
    }), "{reply:?}");
    let actor = identity(&shared);
    let outcome = shared.backend.operation_outcome(&actor, operation).unwrap();
    assert_eq!(outcome.operation_id, operation);
    assert_eq!(outcome.commit_state, storage_contract::CommitState::Published);
    let published = backend.inner.test_with_engine_locked(|engine| engine.state().clone());
    // Namespace is not a replay API: reconcile the original operation instead.
    let (reply, descriptor) = binding_tests::exchange(&mut stream, removal());
    assert!(descriptor.is_none());
    assert!(matches!(reply, Response::Error {
        code: storage_contract::ErrorCode::OperationIdConflict, ..
    }), "{reply:?}");
    backend.inner.test_with_engine_locked(|engine| assert_eq!(engine.state(), &published));
    assert_eq!(shared.backend.operation_outcome(&actor, operation).unwrap(), outcome);
    assert!(directory(&mut stream, "p").is_empty());
    close_wire(stream, worker);
    let receipt = backend.inner.test_with_engine_locked(|engine| {
        for (id, commit) in before.commits.iter() {
            assert_eq!(engine.state().commits.get(id), Some(commit));
        }
        assert_eq!(engine.state().versions, before.versions);
        assert_eq!(engine.state().file_deletions, before.file_deletions);
        assert_eq!(engine.state().directory_git_repositories, before.directory_git_repositories);
        engine.state().operations.get(&operation).unwrap().clone()
    });
    drop((backend, shared));
    let engine = Engine::open(&root.path().join("state")).unwrap();
    assert!(engine.namespace_tree(&branch).unwrap().visible_entries().next().is_none());
    assert_eq!(engine.state().operations.get(&operation), Some(&receipt));
    assert_eq!(engine.state().file_deletions, before.file_deletions);
    for (version, _) in before.versions.iter() {
        let mut original = engine.open_content(version).unwrap();
        let mut bytes = Vec::new();
        original.file.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"retained bytes");
    }
    for (id, commit) in before.commits.iter() {
        assert_eq!(engine.state().commits.get(id), Some(commit));
    }
    let cold = EngineBackend::new(engine);
    cold.set_management_auth(crate::lock(&root.auth).reader()).unwrap();
    for _ in 0..2 {
        assert_eq!(cold.operation_outcome(&actor, operation).unwrap(), outcome);
    }
    cold.test_with_engine_locked(|engine| assert_eq!(engine.state(), &published));
}

#[test]
fn restored_visible_child_rejects_prepared_structural_removal() {
    let (_root, shared, backend, branch) = deleted_child();
    backend.inner.test_with_engine_locked(|engine| {
        let request = stage_remove(engine, &branch);
        let deletion = engine.state().file_deletions.values().next().unwrap().clone();
        let file = storage_contract::history_window::HistoryFileId::parse(&deletion.file_id)
            .unwrap();
        engine.undo_file_deletion(storage_engine::UndoFileDeletionRequest {
            file_id: &file, deletion_node_id: &deletion.node_id,
            principal: "alice", operation_id: OperationId::generate().unwrap(),
        }).unwrap();
        let before = engine.state().clone();
        assert_eq!(engine.commit_workspace(request).unwrap_err().code(),
            storage_contract::ErrorCode::DirectoryNotEmpty);
        assert_eq!(engine.state(), &before);
        assert!(engine.namespace_tree(&branch).unwrap().get("folder/file.txt").is_some());
    });
    let (mut stream, worker, _) = wire(shared.clone(), "alice");
    let (reply, descriptor) = binding_tests::exchange(&mut stream,
        namespace(smb_protocol::protocol::NamespaceOperation::Rmdir, "p/folder"));
    assert!(descriptor.is_none());
    assert!(matches!(reply, Response::Error {
        code: storage_contract::ErrorCode::DirectoryNotEmpty, ..
    }), "{reply:?}");
    close_wire(stream, worker);
}

#[test]
fn concurrent_visible_child_rejects_original_directory_baseline() {
    let (_root, _shared, backend, branch) = deleted_child();
    backend.inner.test_with_engine_locked(|engine| {
        let pending = stage_remove(engine, &branch);
        let next = engine.create_workspace(&branch, "alice").unwrap();
        engine.stage_bytes(&next.workspace_id, "folder/new.txt", b"new").unwrap();
        engine.commit_workspace(request(next)).unwrap();
        let before = engine.state().clone();
        assert_eq!(engine.commit_workspace(pending).unwrap_err().code(),
            storage_contract::ErrorCode::NamespaceConflict);
        assert_eq!(engine.state(), &before);
        assert!(engine.namespace_tree(&branch).unwrap().get("folder/new.txt").is_some());
    });
}

#[test]
fn replaced_directory_entry_cannot_inherit_an_older_removal_request() {
    let (_root, _shared, backend, branch) = deleted_child();
    backend.inner.test_with_engine_locked(|engine| {
        let pending = stage_remove(engine, &branch);
        let removal = stage_remove(engine, &branch);
        engine.commit_workspace(removal).unwrap();
        let next = engine.create_workspace(&branch, "alice").unwrap();
        engine.namespace_operation(&next.workspace_id,
            storage_contract::NamespaceOperation::Mkdir {
                path: name_policy::PortablePath::parse("folder").unwrap(),
            },
        ).unwrap();
        engine.commit_workspace(request(next)).unwrap();
        let before = engine.state().clone();
        assert_eq!(engine.commit_workspace(pending).unwrap_err().code(),
            storage_contract::ErrorCode::NamespaceConflict);
        assert_eq!(engine.state(), &before);
        assert!(engine.namespace_tree(&branch).unwrap().get("folder").is_some());
    });
}
