//! Public listener/Client tests for directory-repository rename publication.
use super::*;
use storage_contract::{
    directory_git::{DirectoryGitInitializeBody, DirectoryGitStageOwnedBody},
    history_window::HistoryWindowCancellation,
};

fn initialize(fixture: &Fixture, repo: &Repository, committed: bool) {
    let identity = fixture.identity();
    let snapshot = fixture
        .backend
        .tree_snapshot(
            &identity,
            ReadTree {
                branch: &repo.main_branch_id,
                commit: None,
            },
        )
        .unwrap();
    fixture
        .backend
        .initialize_directory_git(
            &identity,
            &repo.repository_id,
            DirectoryGitInitializeBody {
                path: "box".into(),
                expected_namespace_head: snapshot.commit_id,
                operation_id: OperationId::generate().unwrap(),
            },
        )
        .unwrap();
    let status = || {
        fixture
            .backend
            .directory_git_status(
                &identity,
                &repo.repository_id,
                "box",
                HistoryWindowCancellation::new(),
            )
            .unwrap()
    };
    let review = status();
    fixture
        .backend
        .stage_directory_git_owned(
            &identity,
            &repo.repository_id,
            DirectoryGitStageOwnedBody {
                directory_id: review.directory_id,
                directory_branch_id: review.directory_branch_id.unwrap(),
                expected_tree_id: review.working_tree_id.unwrap(),
                expected_index_generation: review.index_generation,
                operation_id: OperationId::generate().unwrap(),
            },
        )
        .unwrap();
    if committed {
        let review = status();
        fixture
            .backend
            .commit_directory_changes(
                &identity,
                &repo.repository_id,
                review.directory_branch_id.as_deref().unwrap(),
                storage_contract::directory_staging::DirectoryCommitBody {
                    expected_tree_id: review.working_tree_id.unwrap(),
                    expected_index_generation: review.index_generation,
                    message: "explicit directory history".into(),
                    operation_id: OperationId::generate().unwrap(),
                },
            )
            .unwrap();
    }
}

fn entries(fixture: &Fixture, repo: &Repository) -> Vec<Entry> {
    fixture
        .backend
        .tree_snapshot(
            &fixture.identity(),
            ReadTree {
                branch: &repo.main_branch_id,
                commit: None,
            },
        )
        .unwrap()
        .entries
}

#[test]
fn public_smb_directory_git_rename_staged_file_moves_directly_and_survives_restart() {
    let mut fixture = Fixture::new();
    let client = fixture.connect();
    save(&client, "p/box/file", b"source", true);
    let repo = fixture.repository();
    initialize(&fixture, &repo, false);
    let source = fixture.entry(&repo.main_branch_id, "box/file");
    let (reader, _fd) = open(&client, "p/box/file", false, false, false);
    for (source, target) in [("p/box/file", "p/outside"), ("p/outside", "p/box/returned")] {
        client
            .call(Request::Namespace {
                kind: NamespaceOperation::Rename,
                path: source.into(),
                destination: Some(target.into()),
                replace: false,
                operation_id: OperationId::generate().unwrap(),
            })
            .unwrap();
    }
    let moved = fixture.entry(&repo.main_branch_id, "box/returned");
    assert_eq!(moved.file_id, source.file_id);
    assert_eq!(moved.entry_id, source.entry_id);
    assert_eq!(moved.version_id, source.version_id);
    let mut old_bytes = [0; 6];
    assert_eq!(client.read(reader.handle, 0, &mut old_bytes).unwrap(), 6);
    assert_eq!(&old_bytes, b"source");
    close(&client, reader.handle);
    assert_eq!(read(&client, "p/box/returned").1, b"source");
    drop(client);
    let (root, engine) = fixture.cold_open();
    assert_eq!(
        engine
            .namespace_tree(&repo.main_branch_id)
            .unwrap()
            .get("box/returned"),
        Some(&moved)
    );
    assert!(
        engine
            .namespace_tree(&repo.main_branch_id)
            .unwrap()
            .get("box/file")
            .is_none()
    );
    assert_eq!(
        exported(&engine, root.path(), &source.version_id),
        b"source"
    );
    assert_eq!(engine.file_history(&source.file_id).unwrap().len(), 1);
}

#[test]
fn public_smb_directory_git_rename_committed_export_preserves_source_target_and_receipt_identity() {
    let mut fixture = Fixture::new();
    let client = fixture.connect();
    save(&client, "p/box/file", b"source", true);
    save(&client, "p/target", b"target", true);
    let repo = fixture.repository();
    initialize(&fixture, &repo, true);
    let before = entries(&fixture, &repo);
    let spool = fixture.root.path().join("management/smb-spool");
    for (target, replace) in [("p/missing", false), ("p/target", true)] {
        let operation_id = OperationId::generate().unwrap();
        let error = client
            .call(Request::Namespace {
                kind: NamespaceOperation::Rename,
                path: "p/box/file".into(),
                destination: Some(target.into()),
                replace,
                operation_id,
            })
            .unwrap_err();
        let remote = error
            .get_ref()
            .unwrap()
            .downcast_ref::<RemoteError>()
            .unwrap();
        assert_eq!(
            remote.code,
            RemoteErrorCode::Server(ErrorCode::UnsupportedOperation)
        );
        assert_eq!(remote.operation_id, Some(operation_id.to_string()));
        assert!(!remote.retryable);
        assert_eq!(entries(&fixture, &repo), before);
        assert!(
            spool
                .join(format!("{operation_id}.namespace.json"))
                .is_file()
        );
    }
    assert_eq!(read(&client, "p/box/file").1, b"source");
    assert_eq!(read(&client, "p/target").1, b"target");
    drop(client);
    let (root, engine) = fixture.cold_open();
    assert_eq!(engine.list_tree(&repo.main_branch_id).unwrap(), before);
    for (path, body) in [("box/file", b"source"), ("target", b"target")] {
        let entry = engine
            .namespace_tree(&repo.main_branch_id)
            .unwrap()
            .get(path)
            .unwrap();
        assert_eq!(exported(&engine, root.path(), &entry.version_id), body);
    }
}
