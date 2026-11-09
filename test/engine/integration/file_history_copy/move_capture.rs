//! Production capture API only. These tests never claim a move was published.
use super::*;
use storage_contract::history_window::{HistoryFileId, HistoryRepositoryId};

fn capture(fixture: &Fixture) -> FileHistoryMoveCapture {
    let original = entry(&fixture.engine, &fixture.source, "source.txt");
    fixture
        .engine
        .capture_file_history_move(
            FileHistoryMoveRequest {
                namespace: &request(&fixture.engine, &fixture.source, &fixture.target),
                source_file_id: &HistoryFileId::parse(&original.file_id).unwrap(),
            },
            &HistoryWindowCancellation::new(),
        )
        .unwrap()
}

fn recheck_error(engine: &Engine, capture: &FileHistoryMoveCapture) -> ErrorCode {
    engine
        .recheck_file_history_move(capture, &HistoryWindowCancellation::new())
        .map(|_| ())
        .unwrap_err()
        .code()
}

#[test]
fn move_capture_retains_all_branch_frontiers_without_copy_or_source_deletion() {
    for current in [false, true] {
        let mut fixture = Fixture::new(current);
        let original = entry(&fixture.engine, &fixture.source, "source.txt");
        let branch = fixture
            .engine
            .create_file_branch(
                &original.file_id,
                &original.version_id,
                &BranchName::parse("retained-review").unwrap(),
                "admin",
                op(),
            )
            .unwrap()
            .branch_id
            .unwrap();
        fixture
            .engine
            .archive_branch(&branch, "admin", op())
            .unwrap();
        let sequence = fixture.engine.state().sequence;
        let operations = fixture.engine.state().operations.len();
        let capture = capture(&fixture);
        let expected: BTreeMap<_, _> = fixture
            .engine
            .state()
            .branches
            .values()
            .filter(|branch| branch.file_id.as_deref() == Some(original.file_id.as_str()))
            .map(|branch| {
                (
                    branch.branch_id.clone(),
                    fixture.engine.file_branch_head(&branch.branch_id).unwrap(),
                )
            })
            .collect();
        assert_eq!(capture.branch_heads(), &expected);
        assert!(capture.branch_heads().contains_key(&branch));
        assert_eq!(
            capture.parent_node_ids(),
            expected
                .values()
                .cloned()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>()
        );
        assert_eq!(capture.version_ids().len(), 2);
        assert_eq!(capture.source_sequence(), sequence);
        assert_eq!(capture.source_records_digest().to_string().len(), 64);
        assert_eq!(capture.target_slots(), ["D04"]);
        let current = fixture
            .engine
            .recheck_file_history_move(&capture, &HistoryWindowCancellation::new())
            .unwrap();
        assert_eq!(
            current.capture().source_file_id().as_str(),
            original.file_id
        );
        assert_eq!(fixture.engine.state().sequence, sequence);
        assert_eq!(fixture.engine.state().operations.len(), operations);
        assert_eq!(
            entry(&fixture.engine, &fixture.source, "source.txt"),
            original
        );
        assert!(fixture.engine.file_deletion(&original.file_id).is_none());
        assert!(
            fixture
                .engine
                .namespace_tree(&fixture.target.main_branch_id)
                .unwrap()
                .entries()
                .is_empty()
        );
    }
}

#[test]
fn move_capture_rejects_notes_and_new_branches_even_when_namespace_heads_do_not_change() {
    let mut fixture = Fixture::new(false);
    let original = entry(&fixture.engine, &fixture.source, "source.txt");
    let before_note = capture(&fixture);
    fixture
        .engine
        .set_file_note(
            &original.file_id,
            None,
            &Note::parse("must be included in a later move".into()).unwrap(),
            "admin",
            op(),
        )
        .unwrap();
    assert_eq!(
        fixture.engine.state().branches[&fixture.source.main_branch_id].head_commit_id,
        before_note.namespace().expected_source_head
    );
    assert_eq!(
        recheck_error(&fixture.engine, &before_note),
        ErrorCode::WorkspaceChanged
    );
    let before_branch = capture(&fixture);
    assert_ne!(
        before_note.source_records_digest(),
        before_branch.source_records_digest()
    );
    fixture
        .engine
        .create_file_branch(
            &original.file_id,
            &original.version_id,
            &BranchName::parse("new-frontier").unwrap(),
            "admin",
            op(),
        )
        .unwrap();
    assert_eq!(
        fixture.engine.state().branches[&fixture.source.main_branch_id].head_commit_id,
        before_branch.namespace().expected_source_head
    );
    assert_eq!(
        recheck_error(&fixture.engine, &before_branch),
        ErrorCode::WorkspaceChanged
    );
    let after_branch = capture(&fixture);
    assert_eq!(
        after_branch.branch_heads().len(),
        before_branch.branch_heads().len() + 1
    );
}

#[test]
fn move_capture_rejects_target_change_and_preserves_source() {
    let mut fixture = Fixture::new(false);
    let before = capture(&fixture);
    let original = entry(&fixture.engine, &fixture.source, "source.txt");
    save(
        &mut fixture.engine,
        &fixture.target,
        "occupied.txt",
        b"target was filled",
    );
    assert_eq!(
        recheck_error(&fixture.engine, &before),
        ErrorCode::WorkspaceChanged
    );
    assert_eq!(
        entry(&fixture.engine, &fixture.source, "source.txt"),
        original
    );
    assert!(fixture.engine.file_deletion(&original.file_id).is_none());
}

#[test]
fn move_capture_includes_tag_only_branch_changes() {
    let mut fixture = Fixture::new(false);
    let original = entry(&fixture.engine, &fixture.source, "source.txt");
    let branch = fixture
        .engine
        .create_file_branch(
            &original.file_id,
            &original.version_id,
            &BranchName::parse("tagged-review").unwrap(),
            "admin",
            op(),
        )
        .unwrap()
        .branch_id
        .unwrap();
    let before = capture(&fixture);
    let head = fixture.engine.file_branch_head(&branch).unwrap();
    fixture
        .engine
        .update_file_node_tags(FileNodeTagsUpdateRequest {
            file_id: &original.file_id,
            targets: &[FileTagTarget {
                branch_id: branch.clone(),
                source_node_id: head.clone(),
                expected_node_id: head,
            }],
            add: &["retained-label".into()],
            remove: &[],
            location: None,
            principal: "admin",
            operation_id: op(),
        })
        .unwrap();
    assert_eq!(
        recheck_error(&fixture.engine, &before),
        ErrorCode::WorkspaceChanged
    );
    let after = capture(&fixture);
    assert_ne!(
        before.source_records_digest(),
        after.source_records_digest()
    );
    assert_ne!(
        before.branch_heads()[&branch],
        after.branch_heads()[&branch]
    );
}

#[test]
fn move_capture_includes_new_undone_deletion_audit_with_unchanged_heads() {
    let mut fixture = Fixture::new(false);
    let original = entry(&fixture.engine, &fixture.source, "source.txt");
    let before = capture(&fixture);
    fixture
        .engine
        .soft_delete_file(DeleteFileRequest {
            target: FileDeletionTarget {
                repository_id: &HistoryRepositoryId::parse(&fixture.source.repository_id).unwrap(),
                file_id: &HistoryFileId::parse(&original.file_id).unwrap(),
                path: &name_policy::LookupPath::parse(original.path()).unwrap(),
            },
            principal: "admin",
            operation_id: op(),
        })
        .unwrap();
    let deletion = fixture
        .engine
        .file_deletion(&original.file_id)
        .unwrap()
        .node_id
        .clone();
    fixture
        .engine
        .undo_file_deletion(UndoFileDeletionRequest {
            file_id: &HistoryFileId::parse(&original.file_id).unwrap(),
            deletion_node_id: &deletion,
            principal: "admin",
            operation_id: op(),
        })
        .unwrap();
    assert_eq!(
        fixture.engine.state().branches[&fixture.source.main_branch_id].head_commit_id,
        before.namespace().expected_source_head
    );
    assert_eq!(
        recheck_error(&fixture.engine, &before),
        ErrorCode::WorkspaceChanged
    );
    let after = capture(&fixture);
    assert_eq!(before.branch_heads(), after.branch_heads());
    assert_ne!(
        before.source_records_digest(),
        after.source_records_digest()
    );
}

#[test]
fn move_capture_rechecks_unpublished_workspaces_on_both_ends() {
    for source_end in [false, true] {
        let mut fixture = Fixture::new(false);
        let before = capture(&fixture);
        let branch = if source_end {
            &fixture.source.main_branch_id
        } else {
            &fixture.target.main_branch_id
        };
        fixture.engine.create_workspace(branch, "admin").unwrap();
        assert_eq!(fixture.engine.state().sequence, before.source_sequence());
        assert_eq!(
            recheck_error(&fixture.engine, &before),
            ErrorCode::WorkspaceBusy
        );
    }
}

#[test]
fn move_capture_requires_exact_stable_file_and_honors_cancellation() {
    let fixture = Fixture::new(false);
    let namespace = request(&fixture.engine, &fixture.source, &fixture.target);
    let wrong = HistoryFileId::parse(&format!("file_{}", "f".repeat(32))).unwrap();
    let failure = fixture
        .engine
        .capture_file_history_move(
            FileHistoryMoveRequest {
                namespace: &namespace,
                source_file_id: &wrong,
            },
            &HistoryWindowCancellation::new(),
        )
        .map(|_| ())
        .unwrap_err();
    assert_eq!(failure.code(), ErrorCode::FileNotFound);
    let capture = capture(&fixture);
    let cancellation = HistoryWindowCancellation::new();
    cancellation.cancel();
    assert_eq!(
        fixture
            .engine
            .capture_file_history_move(
                FileHistoryMoveRequest {
                    namespace: &namespace,
                    source_file_id: capture.source_file_id(),
                },
                &cancellation,
            )
            .map(|_| ())
            .unwrap_err()
            .code(),
        ErrorCode::OperationAborted
    );
    assert_eq!(
        fixture
            .engine
            .recheck_file_history_move(&capture, &cancellation)
            .map(|_| ())
            .unwrap_err()
            .code(),
        ErrorCode::OperationAborted
    );
    assert_eq!(fixture.engine.state().sequence, capture.source_sequence());
}

#[test]
fn move_capture_cannot_be_reused_after_cold_reopen() {
    let fixture = Fixture::new(false);
    let captured = capture(&fixture);
    let Fixture {
        temp,
        engine,
        source,
        target,
    } = fixture;
    let original = entry(&engine, &source, "source.txt");
    drop(engine);
    let engine = Engine::open(&temp.path().join("system")).unwrap();
    assert_eq!(
        recheck_error(&engine, &captured),
        ErrorCode::WorkspaceChanged
    );
    assert_eq!(entry(&engine, &source, "source.txt"), original);
    assert!(engine.file_deletion(&original.file_id).is_none());
    assert!(
        engine
            .namespace_tree(&target.main_branch_id)
            .unwrap()
            .entries()
            .is_empty()
    );
}
