//! New native rename semantics retain original capture names and old receipts.
use super::*;

#[test]
fn recursive_follow_preserves_legacy_retry_and_registry_continuation() {
    let mut fixture = Fixture::initial(true, 5);
    let file = fixture.save("docs/original", b"continued across registry");
    let legacy = rename(&mut fixture, "docs/original", "docs/report", operation()).unwrap();
    assert_eq!(legacy.kind, "file.namespace");
    fixture.exit(3, MembershipExitAction::Deregister);
    let capture = fixture.engine.recursive_backup_capture().unwrap();
    fixture.configure("docs", &capture, 0, operation()).unwrap();
    rename(&mut fixture, "docs", "renamed", operation()).unwrap();
    fixture.exit(4, MembershipExitAction::Deregister);
    let system = fixture.temp.path().join("system");
    drop(fixture.engine);
    fixture.engine = Engine::open(&system).unwrap();
    assert_eq!(
        rename(
            &mut fixture,
            "docs/original",
            "docs/report",
            legacy.operation_id
        )
        .unwrap(),
        legacy
    );
    rename(&mut fixture, "renamed/report", "renamed/final", operation()).unwrap();
    fixture.assert_body_paths(&file, "renamed/final", b"continued across registry");
}

#[test]
fn recursive_follow_rejects_stopped_backup_target_without_publishing() {
    let mut fixture = Fixture::new(true);
    let file = fixture.save("docs/report", b"stopped target stays frozen");
    let capture = fixture.engine.recursive_backup_capture().unwrap();
    fixture.configure("docs", &capture, 0, operation()).unwrap();
    fixture.exit(1, MembershipExitAction::PlannedOffline);
    let before = fixture.engine.state().clone();
    let frozen = files(&fixture.config.disks[1].root);
    let error = rename(&mut fixture, "docs/report", "docs/final", operation()).unwrap_err();
    assert!(matches!(
        error.code(),
        ErrorCode::PlacementInvalid | ErrorCode::SlotNotFound
    ));
    assert_eq!(fixture.engine.state().sequence, before.sequence);
    assert_eq!(
        fixture.engine.state().file_backup_paths,
        before.file_backup_paths
    );
    assert_eq!(files(&fixture.config.disks[1].root), frozen);
    assert!(
        fixture
            .engine
            .namespace_tree(&fixture.repository.main_branch_id)
            .unwrap()
            .get("docs/report")
            .is_some()
    );
    assert_eq!(
        fixture.engine.state().file_locations[&file.file_id],
        "docs/report"
    );
}

fn rename(fixture: &mut Fixture, from: &str, to: &str, id: OperationId) -> Result<OperationResult> {
    fixture.engine.file_namespace(
        &fixture.repository.repository_id,
        NamespaceOperation::parse(NamespaceOperationKind::Rename, from, Some(to)).unwrap(),
        "admin",
        id,
    )
}

#[test]
fn recursive_follow_moves_scope_and_future_children_through_cold_replay() {
    let mut fixture = Fixture::new(false);
    let first = fixture.save("ancestor/docs/report", b"retained first");
    let last = fixture.save("ancestor/docs/report", b"retained last");
    let capture = fixture.engine.recursive_backup_capture().unwrap();
    let configured = fixture
        .configure("ancestor/docs", &capture, 0, operation())
        .unwrap();
    let renamed = rename(
        &mut fixture,
        "ancestor/docs",
        "ancestor/renamed",
        operation(),
    )
    .unwrap();
    assert_eq!(renamed.kind, "file.namespace.recursive-routes");
    fixture.assert_body_paths(&first, "ancestor/renamed/report", b"retained first");
    fixture.assert_body_paths(&last, "ancestor/renamed/report", b"retained last");
    let policy = fixture
        .engine
        .backup_configuration(&fixture.repository.repository_id, "ancestor/renamed")
        .unwrap();
    assert_eq!(policy.generation, 2);
    assert_eq!(
        policy.backup_prefixes["B01"],
        "disk-0/Repository/ancestor/renamed"
    );
    let child = fixture.save("ancestor/renamed/child", b"new child follows");
    fixture.assert_body_paths(&child, "ancestor/renamed/child", b"new child follows");
    assert_eq!(
        fixture.engine.operation(configured.operation_id).unwrap(),
        configured
    );
    let system = fixture.temp.path().join("system");
    drop(fixture.engine);
    fixture.engine = Engine::open(&system).unwrap();
    assert_eq!(
        rename(
            &mut fixture,
            "ancestor/docs",
            "ancestor/renamed",
            renamed.operation_id
        )
        .unwrap(),
        renamed
    );
    fixture.assert_body_paths(&last, "ancestor/renamed/report", b"retained last");
    rename(
        &mut fixture,
        "ancestor/renamed/report",
        "ancestor/renamed/final",
        operation(),
    )
    .unwrap();
    fixture.assert_body_paths(&first, "ancestor/renamed/final", b"retained first");
    fixture.assert_body_paths(&last, "ancestor/renamed/final", b"retained last");
    assert_eq!(
        fixture.engine.operation(configured.operation_id).unwrap(),
        configured
    );
}

#[test]
fn recursive_follow_does_not_reinterpret_a_custom_policy_reusing_its_id() {
    let mut fixture = Fixture::new(true);
    let file = fixture.save("docs/report", b"custom root stays frozen");
    let capture = fixture.engine.recursive_backup_capture().unwrap();
    fixture.configure("docs", &capture, 0, operation()).unwrap();
    let policy = fixture
        .engine
        .backup_configuration(&fixture.repository.repository_id, "docs")
        .unwrap();
    let prefixes = BTreeMap::from([
        (fixture.config.disks[1].slot_id.clone(), "custom-one".into()),
        (fixture.config.disks[2].slot_id.clone(), "custom-two".into()),
    ]);
    fixture
        .engine
        .configure_backups(storage_engine::BackupConfigurationRequest {
            repository_id: &fixture.repository.repository_id,
            path: "docs",
            target_copies: 3,
            backup_prefixes: &prefixes,
            expected_generation: policy.generation,
            principal: "admin",
            operation_id: operation(),
        })
        .unwrap();
    let paths = fixture.engine.state().file_backup_paths[&file.file_id].clone();
    let custom = fixture
        .engine
        .backup_configuration(&fixture.repository.repository_id, "docs")
        .unwrap();
    assert_eq!(custom.policy_id, policy.policy_id);
    rename(&mut fixture, "docs", "moved", operation()).unwrap();
    assert_eq!(
        fixture.engine.state().file_backup_paths[&file.file_id],
        paths
    );
    assert_eq!(
        fixture
            .engine
            .backup_configuration(&fixture.repository.repository_id, "moved")
            .unwrap()
            .generation,
        custom.generation
    );
    let system = fixture.temp.path().join("system");
    drop(fixture.engine);
    fixture.engine = Engine::open(&system).unwrap();
    rename(&mut fixture, "moved/report", "moved/final", operation()).unwrap();
    assert_eq!(
        fixture.engine.state().file_backup_paths[&file.file_id],
        paths
    );
}

#[test]
fn recursive_follow_cannot_rename_a_soft_deleted_file() {
    use storage_engine::history_window::{HistoryFileId, HistoryRepositoryId};
    use storage_engine::{DeleteFileRequest, FileDeletionTarget};
    let mut fixture = Fixture::new(true);
    let file = fixture.save("docs/report", b"frozen");
    let capture = fixture.engine.recursive_backup_capture().unwrap();
    fixture.configure("docs", &capture, 0, operation()).unwrap();
    let repository = HistoryRepositoryId::parse(&fixture.repository.repository_id).unwrap();
    let id = HistoryFileId::parse(&file.file_id).unwrap();
    let path = name_policy::LookupPath::parse("docs/report").unwrap();
    fixture
        .engine
        .soft_delete_file(DeleteFileRequest {
            target: FileDeletionTarget {
                repository_id: &repository,
                file_id: &id,
                path: &path,
            },
            principal: "admin",
            operation_id: operation(),
        })
        .unwrap();
    let before = fixture.engine.state().clone();
    assert!(rename(&mut fixture, "docs/report", "docs/final", operation()).is_err());
    assert_eq!(fixture.engine.state().sequence, before.sequence);
    assert_eq!(
        fixture.engine.state().file_backup_paths,
        before.file_backup_paths
    );
    assert_eq!(fixture.engine.state().file_deletions, before.file_deletions);
    fixture.assert_body_paths(&file, "docs/report", b"frozen");
}

#[test]
fn recursive_follow_uses_the_real_directory_batch_move_entrypoint() {
    batch_move_with_layout(false);
}

#[test]
fn recursive_follow_moves_backups_with_an_activated_shared_directory_layout() {
    batch_move_with_layout(true);
}

fn batch_move_with_layout(activate: bool) {
    use storage_engine::{
        DirectoryBatchKind, DirectoryNamespaceBatch, DirectoryNamespaceItem,
        DirectoryNamespaceSource, DirectoryNamespaceTarget,
        directory_git::DirectoryGitInitializeBody,
    };
    let mut fixture = Fixture::new(true);
    let file = fixture.save("docs/report", b"batch owner body");
    fixture
        .engine
        .file_namespace(
            &fixture.repository.repository_id,
            NamespaceOperation::parse(NamespaceOperationKind::Mkdir, "moved", None).unwrap(),
            "admin",
            operation(),
        )
        .unwrap();
    let capture = fixture.engine.recursive_backup_capture().unwrap();
    fixture.configure("docs", &capture, 0, operation()).unwrap();
    let head = fixture.engine.state().branches[&fixture.repository.main_branch_id]
        .head_commit_id
        .clone();
    fixture
        .engine
        .initialize_directory_git(
            &fixture.repository.repository_id,
            &DirectoryGitInitializeBody {
                path: "".into(),
                expected_namespace_head: head,
                operation_id: operation(),
            },
            "admin",
        )
        .unwrap();
    if activate {
        let layout = fixture
            .engine
            .reconcile_directory_layout(&fixture.repository.repository_id)
            .unwrap();
        assert!(layout.directories > 0);
        assert!(layout.containers > 0);
        assert!(layout.unavailable_histories.is_empty());
        assert!(layout.unavailable_containers.is_empty());
    }
    let status = fixture
        .engine
        .directory_git_status(&fixture.repository.repository_id, "")
        .unwrap();
    let branch = fixture.engine.state().directory_git_repositories[&status.directory_id]
        .main_branch_id
        .clone();
    let head = fixture.engine.state().directory_branches[&branch]
        .head_tree_id
        .clone();
    let entry = fixture.engine.state().directory_branches[&branch]
        .entries
        .iter()
        .find(|entry| entry.file_id == file.file_id)
        .unwrap();
    let request = DirectoryNamespaceBatch {
        kind: DirectoryBatchKind::Move,
        items: vec![DirectoryNamespaceItem {
            source: DirectoryNamespaceSource {
                directory_branch_id: branch.clone(),
                expected_tree_id: head.clone(),
                path: "docs/report".into(),
                entry_id: entry.entry_id.clone(),
            },
            target: Some(DirectoryNamespaceTarget {
                directory_branch_id: branch,
                expected_tree_id: head,
                path: "moved/report".into(),
            }),
        }],
    };
    let id = operation();
    let result = fixture
        .engine
        .directory_namespace_batch(&fixture.repository.repository_id, &request, "admin", id)
        .unwrap();
    assert_eq!(result.kind, "directory.namespace.batch.recursive-routes");
    fixture.assert_body_paths(&file, "moved/report", b"batch owner body");
    let system = fixture.temp.path().join("system");
    drop(fixture.engine);
    fixture.engine = Engine::open(&system).unwrap();
    assert_eq!(
        fixture
            .engine
            .directory_namespace_batch(&fixture.repository.repository_id, &request, "admin", id)
            .unwrap(),
        result
    );
    fixture.assert_body_paths(&file, "moved/report", b"batch owner body");
}
