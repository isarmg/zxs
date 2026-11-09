//! Source-only discovery behavior using production Engine operations on temp roots.
use super::*;
use crate::{CommitRequest, Config, NamespaceOperation, OperationId};

fn fixture() -> (tempfile::TempDir, Engine, crate::Repository) {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &temp.path().join("system"),
        Config::development([temp.path().join("disk")]).unwrap(),
    )
    .unwrap();
    let repository = engine
        .create_disk_root("P01", "admin", OperationId::generate().unwrap())
        .unwrap();
    (temp, engine, repository)
}
fn save(
    engine: &mut Engine,
    repository: &crate::Repository,
    path: &str,
    body: &[u8],
) -> crate::Entry {
    let workspace = engine
        .create_workspace(&repository.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, path, body)
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: OperationId::generate().unwrap(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "discovery fixture".into(),
            request_digest: None,
        })
        .unwrap();
    engine
        .namespace_tree(&repository.main_branch_id)
        .unwrap()
        .get(path)
        .unwrap()
        .clone()
}
fn page(engine: &Engine) -> Page {
    let read = engine
        .capture_detached_history(Request::default(), HistoryWindowCancellation::new())
        .unwrap();
    let result = read.page().unwrap();
    engine.recheck_detached_history(&read).unwrap();
    result
}
#[test]
fn discovery_excludes_current_and_soft_deleted_files_and_keeps_detached_stable_identity() {
    let (_temp, mut engine, repository) = fixture();
    let old = save(&mut engine, &repository, "gone", b"one");
    save(&mut engine, &repository, "gone", b"two");
    let current = save(&mut engine, &repository, "current", b"live");
    assert!(page(&engine).files.is_empty());
    engine
        .file_namespace(
            &repository.repository_id,
            NamespaceOperation::parse(
                storage_contract::NamespaceOperationKind::Delete,
                "gone",
                None,
            )
            .unwrap(),
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    let listed = page(&engine);
    assert_eq!(listed.files.len(), 1);
    assert_eq!(listed.files[0].file_id.as_str(), old.file_id);
    assert_eq!(listed.files[0].retained_versions, 2);
    assert_ne!(listed.files[0].file_id.as_str(), current.file_id);
    engine
        .soft_delete_file(crate::DeleteFileRequest {
            target: crate::FileDeletionTarget {
                repository_id: &HistoryRepositoryId::parse(&repository.repository_id).unwrap(),
                file_id: &HistoryFileId::parse(&current.file_id).unwrap(),
                path: &name_policy::LookupPath::parse("current").unwrap(),
            },
            principal: "admin",
            operation_id: OperationId::generate().unwrap(),
        })
        .unwrap();
    assert_eq!(page(&engine).files.len(), 1);
    assert!(engine.state().file_deletions.contains_key(&current.file_id));
}
#[test]
fn discovery_rechecks_publication_and_refuses_stale_cursor_or_cancellation() {
    let (_temp, mut engine, repository) = fixture();
    let captured = engine
        .capture_detached_history(Request::default(), HistoryWindowCancellation::new())
        .unwrap();
    let old = captured.page().unwrap();
    let saved = save(&mut engine, &repository, "new", b"body");
    assert_eq!(
        engine
            .recheck_detached_history(&captured)
            .unwrap_err()
            .code(),
        ErrorCode::HistoryChanged
    );
    let request = Request {
        snapshot: Some(old.snapshot),
        after_file: Some(HistoryFileId::parse(&saved.file_id).unwrap()),
    };
    assert_eq!(
        engine
            .capture_detached_history(request, HistoryWindowCancellation::new())
            .unwrap()
            .page()
            .unwrap_err()
            .code(),
        ErrorCode::HistoryChanged
    );
    let cancellation = HistoryWindowCancellation::new();
    let read = engine
        .capture_detached_history(Request::default(), cancellation.clone())
        .unwrap();
    cancellation.cancel();
    assert_eq!(read.page().unwrap_err().code(), ErrorCode::OperationAborted);
}
#[test]
fn terminal_files_leave_discovery_without_rewriting_their_old_records() {
    let (_temp, mut engine, repository) = fixture();
    let old = save(&mut engine, &repository, "gone", b"last");
    engine
        .file_namespace(
            &repository.repository_id,
            NamespaceOperation::parse(
                storage_contract::NamespaceOperationKind::Delete,
                "gone",
                None,
            )
            .unwrap(),
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    let listed = page(&engine);
    let found = &listed.files[0];
    let read = engine
        .capture_shared_history(
            storage_contract::shared_history::SharedHistoryRequest {
                file_id: found.file_id.clone(),
                history_root: found.history_root,
            },
            HistoryWindowCancellation::new(),
        )
        .unwrap();
    let selected =
        crate::PermanentNodeRemovalSelection::new(vec![found.representative_version_id.clone()])
            .unwrap();
    engine
        .retire_file_nodes(&read, &selected, "admin", OperationId::generate().unwrap())
        .unwrap();
    assert!(page(&engine).files.is_empty());
    assert!(engine.state().versions.contains_key(&old.version_id));
    assert_eq!(
        engine.state().content_states[&old.version_id],
        ContentState::Retired
    );
}
