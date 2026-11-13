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
    deleted_child_with_renames(false)
}

fn deleted_child_with_renames(
    rename: bool,
) -> (Fixture, Arc<Shared>, Arc<LostCommitReply>, String) {
    let (root, shared, backend) = empty_fixture(false);
    let (mut stream, worker, _) = wire(shared.clone(), "alice");
    use smb_protocol::protocol::NamespaceOperation as Kind;
    okay(&mut stream, namespace(Kind::Mkdir, "p/folder"));
    let (opened, file) = open(&mut stream, "p/folder/file.txt", true, true);
    save(&mut stream, &opened, &file, b"retained bytes");
    // save already flushes and closes the handle; retain only its returned FD here.
    drop(file);
    if rename {
        let (opened, file) = open(&mut stream, "p/outside.txt", true, true);
        save(&mut stream, &opened, &file, b"retained bytes");
        drop(file);
        for (source, destination) in [
            ("p/folder/file.txt", "p/folder/renamed.txt"),
            ("p/outside.txt", "p/outside-renamed.txt"),
        ] {
            okay(&mut stream, Request::Namespace {
                kind: Kind::Rename, path: source.into(), destination: Some(destination.into()),
                replace: false, operation_id: new_id("op").unwrap(),
            });
        }
    }
    let path = if rename { "p/folder/renamed.txt" } else { "p/folder/file.txt" };
    okay(&mut stream, namespace(Kind::Unlink, path));
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
    for rename in [false, true] {
        cold_removal_round_trip(rename);
    }
}

fn cold_removal_round_trip(rename: bool) {
    let (root, shared, backend, branch) = deleted_child_with_renames(rename);
    let before = backend.inner.test_with_engine_locked(|engine| engine.state().clone());
    assert!(before.branches[&branch].file_id.is_none());
    if rename {
        let source = &before.commits[&before.branches[&branch].head_commit_id];
        assert!(source.entries.iter().any(|entry| entry.path() == "folder/file.txt"));
        assert!(source.entries.iter().any(|entry| entry.path() == "outside.txt"));
        backend.inner.test_with_engine_locked(|engine| {
            assert!(engine.namespace_tree(&branch).unwrap().get("outside-renamed.txt").is_some());
        });
    }
    for deletion in before.file_deletions.values() {
        assert!(!deletion.branch_heads.is_empty());
        for file_branch in deletion.branch_heads.keys() {
            assert_ne!(file_branch, &branch);
            assert_eq!(before.branches[file_branch].file_id.as_deref(),
                Some(deletion.file_id.as_str()));
        }
        if let Some(selected) = before.file_selections.get(&deletion.file_id) {
            assert_ne!(selected, &branch);
        }
    }
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
    let expected = if rename { vec!["outside-renamed.txt"] } else { vec![] };
    assert_eq!(directory(&mut stream, "p"), expected);
    close_wire(stream, worker);
    let receipt = backend.inner.test_with_engine_locked(|engine| {
        for (id, commit) in before.commits.iter() {
            assert_eq!(engine.state().commits.get(id), Some(commit));
        }
        assert_eq!(engine.state().versions, before.versions);
        assert_eq!(engine.state().file_deletions, before.file_deletions);
        assert_eq!(engine.state().file_selections, before.file_selections);
        assert_eq!(engine.state().file_write_origins, before.file_write_origins);
        assert_eq!(engine.state().directory_git_repositories, before.directory_git_repositories);
        for (id, branch) in before.branches.iter().filter(|(_, branch)| branch.file_id.is_some()) {
            assert_eq!(engine.state().branches.get(id), Some(branch));
        }
        engine.state().operations.get(&operation).unwrap().clone()
    });
    drop((backend, shared));
    let mut engine = Engine::open(&root.path().join("state")).unwrap();
    let visible = engine.namespace_tree(&branch).unwrap().visible_entries()
        .map(|entry| entry.path().to_owned()).collect::<Vec<_>>();
    assert_eq!(visible, expected);
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
    let retained = engine.state().clone();
    let deletion = retained.file_deletions.values().next().unwrap();
    let file = storage_contract::history_window::HistoryFileId::parse(&deletion.file_id)
        .unwrap();
    assert_eq!(engine.undo_file_deletion(storage_engine::UndoFileDeletionRequest {
        file_id: &file, deletion_node_id: &deletion.node_id,
        principal: "alice", operation_id: OperationId::generate().unwrap(),
    }).unwrap_err().code(), storage_contract::ErrorCode::UnsupportedOperation);
    assert_eq!(engine.state(), &retained);
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
