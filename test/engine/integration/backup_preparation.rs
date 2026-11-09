//! Real background history preparation. HistoryPrepared is NOT topology activation.
//! Owner restart coverage below is not a physical-power-loss certification.
#[path = "backup_preparation_batch.rs"]
mod batch;
#[path = "backup_preparation_progress_io.rs"]
mod progress_io;
#[path = "backup_preparation_snapshot.rs"]
mod snapshot;
#[path = "backup_preparation_target_admission.rs"]
mod target_admission;
#[path = "backup_transaction_serving.rs"]
mod transaction_serving;
use std::{
    fs,
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
};
use storage_engine::{
    BackupPreparationPhase, BackupPreparationRequest, BranchName, CommitRequest, Config,
    DeleteFileRequest, DiskExitPhase, Engine, Entry, ErrorCode, FileDeletionTarget,
    FileNodeTagsUpdateRequest, FileTagTarget, MembershipExitAction, OperationId, Repository,
    RepositoryName,
    history_window::{HistoryFileId, HistoryRepositoryId},
    offline_check_history_package,
};

fn op() -> OperationId {
    OperationId::generate().unwrap()
}

fn setup() -> (tempfile::TempDir, Engine, Repository) {
    let temp = tempfile::Builder::new()
        .permissions(std::os::unix::fs::PermissionsExt::from_mode(0o700))
        .tempdir()
        .unwrap();
    let config =
        Config::development((0..3).map(|n| temp.path().join(format!("disk-{n}")))).unwrap();
    let mut engine = Engine::initialize(&temp.path().join("system"), config).unwrap();
    let repository = engine.create_disk_root("P01", "admin", op()).unwrap();
    (temp, engine, repository)
}

fn save(engine: &mut Engine, repository: &Repository, path: &str, bytes: &[u8]) -> Entry {
    let workspace = engine
        .create_workspace(&repository.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, path, bytes)
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: op(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "preparation fixture".into(),
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

fn target(root: &Path, name: &str) -> PathBuf {
    let path = root.join(name);
    fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
    path
}

fn request(
    engine: &Engine,
    paths: Vec<PathBuf>,
    operation_id: OperationId,
) -> BackupPreparationRequest {
    BackupPreparationRequest {
        operation_id,
        principal: "admin".into(),
        source_slot_id: "B01".into(),
        target_paths: paths,
        expected_sequence: engine.state().sequence,
    }
}

#[test]
fn preparation_automatically_copies_and_verifies_whole_histories_without_activation() {
    let (temp, mut engine, repository) = setup();
    let old = save(
        &mut engine,
        &repository,
        "docs/report",
        b"old complete body",
    );
    save(
        &mut engine,
        &repository,
        "docs/report",
        b"new complete body",
    );
    assert_eq!(
        engine.state().placements[&old.version_id]
            .route_slot_ids
            .len(),
        1
    );
    let branch = engine
        .create_file_branch(
            &old.file_id,
            &old.version_id,
            &BranchName::parse("retained").unwrap(),
            "admin",
            op(),
        )
        .unwrap()
        .branch_id
        .unwrap();
    let head = engine.file_branch_head(&branch).unwrap();
    let tags = engine
        .update_file_node_tags(FileNodeTagsUpdateRequest {
            file_id: &old.file_id,
            targets: &[FileTagTarget {
                branch_id: branch,
                source_node_id: head.clone(),
                expected_node_id: head,
            }],
            add: &["must-survive-copy".into()],
            remove: &[],
            location: None,
            principal: "admin",
            operation_id: op(),
        })
        .unwrap();
    let hidden = save(&mut engine, &repository, "hidden", b"hidden retained body");
    let repository_id = HistoryRepositoryId::parse(&repository.repository_id).unwrap();
    let file_id = HistoryFileId::parse(&hidden.file_id).unwrap();
    let path = name_policy::LookupPath::parse("hidden").unwrap();
    let deletion = engine
        .soft_delete_file(DeleteFileRequest {
            target: FileDeletionTarget {
                repository_id: &repository_id,
                file_id: &file_id,
                path: &path,
            },
            principal: "admin",
            operation_id: op(),
        })
        .unwrap();
    engine
        .file_namespace(
            &repository.repository_id,
            storage_engine::NamespaceOperation::parse(
                storage_engine::NamespaceOperationKind::Mkdir,
                "docs/empty",
                None,
            )
            .unwrap(),
            "admin",
            op(),
        )
        .unwrap();
    // Child initialized first, then its parent: both independent identities and
    // all their retained snapshots must remain inside this one history package.
    for path in ["docs/empty", "docs"] {
        let head = engine.state().branches[&repository.main_branch_id]
            .head_commit_id
            .clone();
        engine
            .initialize_directory_git(
                &repository.repository_id,
                &storage_contract::directory_git::DirectoryGitInitializeBody {
                    path: path.into(),
                    expected_namespace_head: head,
                    operation_id: op(),
                },
                "admin",
            )
            .unwrap();
    }
    let second = engine
        .create_repository(
            &RepositoryName::parse("AlsoPrepared").unwrap(),
            "P01",
            3,
            "admin",
            op(),
        )
        .unwrap();
    save(&mut engine, &second, "other", b"another whole history");
    // Independent official package verifier establishes the exact expected history
    // metadata (including tags/deletion graph), not merely a current-file count.
    let expected = [&repository, &second].map(|repo| {
        engine
            .export_history_package(
                &repo.history_tree_id,
                &temp
                    .path()
                    .join(format!("reference-{}", repo.history_tree_id)),
            )
            .unwrap()
    });
    let before = storage_engine::canonical_json(engine.state()).unwrap();
    let config = storage_engine::canonical_json(engine.config()).unwrap();
    let destination = target(temp.path(), "new-terminal");
    let id = op();
    engine
        .start_backup_preparation(
            request(&engine, vec![destination.clone()], id),
            fs::File::open(&destination).unwrap(),
        )
        .unwrap();
    // No manual copy/step API: start launches the worker; wait only joins it.
    let completed = engine.wait_backup_preparation().unwrap();
    assert_eq!(completed.operation_id, id);
    assert_eq!(completed.phase, BackupPreparationPhase::HistoryPrepared);
    assert!(completed.error_code.is_none());
    assert!(completed.total_files > 0);
    assert!(completed.total_bytes > 0);
    assert_eq!(completed.completed_files, completed.total_files);
    assert_eq!(completed.completed_bytes, completed.total_bytes);
    for report in expected {
        let actual = offline_check_history_package(
            &destination.join("histories").join(&report.history_tree_id),
        )
        .unwrap();
        assert_eq!(actual.manifest_hash, report.manifest_hash);
        assert_eq!(actual.versions, report.versions);
        assert_eq!(actual.full_files, report.full_files);
        assert_eq!(actual.full_bytes, report.full_bytes);
    }
    assert_eq!(
        storage_engine::canonical_json(engine.state()).unwrap(),
        before
    );
    assert_eq!(
        storage_engine::canonical_json(engine.config()).unwrap(),
        config
    );
    assert_eq!(engine.operation(tags.operation_id).unwrap(), tags);
    assert_eq!(engine.operation(deletion.operation_id).unwrap(), deletion);
    assert!(engine.file_deletion(&hidden.file_id).is_some());
    assert_eq!(engine.backup_preparation_status().unwrap().operation_id, id);
}

#[test]
fn preparation_owner_restart_automatically_resumes_the_same_durable_task() {
    let (temp, mut engine, repository) = setup();
    save(&mut engine, &repository, "file", b"restart complete body");
    let destination = target(temp.path(), "restart-target");
    let id = op();
    let before = storage_engine::canonical_json(engine.state()).unwrap();
    engine
        .start_backup_preparation(
            request(&engine, vec![destination.clone()], id),
            fs::File::open(&destination).unwrap(),
        )
        .unwrap();
    // Completion may win this race. Both completed and unfinished durable tasks
    // must keep the original identity; no new start call is made after open.
    drop(engine);
    let mut engine = Engine::open(&temp.path().join("system")).unwrap();
    assert_eq!(engine.backup_preparation_status().unwrap().operation_id, id);
    let completed = engine.wait_backup_preparation().unwrap();
    assert_eq!(completed.operation_id, id);
    assert_eq!(completed.phase, BackupPreparationPhase::HistoryPrepared);
    offline_check_history_package(
        &destination
            .join("histories")
            .join(&repository.history_tree_id),
    )
    .unwrap();
    assert_eq!(
        storage_engine::canonical_json(engine.state()).unwrap(),
        before
    );
}

#[test]
fn preparation_rejects_nonempty_target_without_overwriting_conflicting_bytes() {
    let (temp, mut engine, repository) = setup();
    save(&mut engine, &repository, "file", b"source");
    let destination = target(temp.path(), "conflicting-target");
    fs::write(destination.join("foreign-material"), b"must remain exact").unwrap();
    let before = storage_engine::canonical_json(engine.state()).unwrap();
    let error = engine
        .start_backup_preparation(
            request(&engine, vec![destination.clone()], op()),
            fs::File::open(&destination).unwrap(),
        )
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::ObjectCollision);
    assert_eq!(
        fs::read(destination.join("foreign-material")).unwrap(),
        b"must remain exact"
    );
    assert_eq!(fs::read_dir(&destination).unwrap().count(), 1);
    assert!(engine.backup_preparation_status().is_none());
    assert_eq!(
        storage_engine::canonical_json(engine.state()).unwrap(),
        before
    );
}

#[test]
fn preparation_does_not_treat_multiple_candidates_as_copy_count_or_split_a_repository() {
    let (temp, mut engine, repository) = setup();
    save(&mut engine, &repository, "file", b"whole repository");
    let first = target(temp.path(), "candidate-one");
    let second = target(temp.path(), "candidate-two");
    let before = storage_engine::canonical_json(engine.state()).unwrap();
    let error = engine
        .start_backup_preparation(
            request(&engine, vec![first.clone(), second.clone()], op()),
            fs::File::open(&first).unwrap(),
        )
        .unwrap_err();
    assert!(matches!(
        error.code(),
        ErrorCode::ConfigConflict | ErrorCode::InvalidRequest
    ));
    for path in [first, second] {
        assert_eq!(fs::read_dir(path).unwrap().count(), 0);
    }
    assert!(engine.backup_preparation_status().is_none());
    assert_eq!(
        storage_engine::canonical_json(engine.state()).unwrap(),
        before
    );
}

#[test]
fn preparation_reopen_rejects_corrupted_target_body_without_overwriting_evidence() {
    let (temp, mut engine, repository) = setup();
    let entry = save(&mut engine, &repository, "file", b"original exact body");
    let destination = target(temp.path(), "corrupted-target");
    let id = op();
    engine
        .start_backup_preparation(
            request(&engine, vec![destination.clone()], id),
            fs::File::open(&destination).unwrap(),
        )
        .unwrap();
    assert_eq!(
        engine.wait_backup_preparation().unwrap().phase,
        BackupPreparationPhase::HistoryPrepared
    );
    let version = engine.state().versions[&entry.version_id].clone();
    let body = destination
        .join("histories")
        .join(&repository.history_tree_id)
        .join("versions")
        .join(&entry.version_id)
        .join("content")
        .join(&version.creation_name);
    let before = storage_engine::canonical_json(engine.state()).unwrap();
    drop(engine);
    fs::write(&body, b"corrupt exact bytes").unwrap();
    let mut engine = Engine::open(&temp.path().join("system")).unwrap();
    let result = engine.wait_backup_preparation().unwrap();
    assert_eq!(result.phase, BackupPreparationPhase::Blocked);
    assert!(result.error_code.is_some());
    assert_eq!(fs::read(&body).unwrap(), b"corrupt exact bytes");
    assert_eq!(
        storage_engine::canonical_json(engine.state()).unwrap(),
        before
    );
}

#[test]
fn preparation_never_resamples_a_later_source_publication_on_restart() {
    let (temp, mut engine, repository) = setup();
    save(&mut engine, &repository, "file", b"frozen source");
    let destination = target(temp.path(), "stale-target");
    engine
        .start_backup_preparation(
            request(&engine, vec![destination.clone()], op()),
            fs::File::open(&destination).unwrap(),
        )
        .unwrap();
    assert_eq!(
        engine.wait_backup_preparation().unwrap().phase,
        BackupPreparationPhase::HistoryPrepared
    );
    let marker = fs::read(destination.join("history-prepared.json")).unwrap();
    save(&mut engine, &repository, "later", b"not in the old task");
    assert_eq!(
        engine.backup_preparation_status().unwrap().phase,
        BackupPreparationPhase::SourceChanged
    );
    drop(engine);
    let mut engine = Engine::open(&temp.path().join("system")).unwrap();
    assert_eq!(
        engine.wait_backup_preparation().unwrap().phase,
        BackupPreparationPhase::SourceChanged
    );
    assert_eq!(
        fs::read(destination.join("history-prepared.json")).unwrap(),
        marker
    );
}

#[test]
fn preparation_reopen_refuses_target_rebinding_and_keeps_original_copy() {
    let (temp, mut engine, repository) = setup();
    save(&mut engine, &repository, "file", b"identity-bound body");
    let destination = target(temp.path(), "bound-target");
    engine
        .start_backup_preparation(
            request(&engine, vec![destination.clone()], op()),
            fs::File::open(&destination).unwrap(),
        )
        .unwrap();
    assert_eq!(
        engine.wait_backup_preparation().unwrap().phase,
        BackupPreparationPhase::HistoryPrepared
    );
    drop(engine);
    let moved = temp.path().join("retained-original");
    fs::rename(&destination, &moved).unwrap();
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&destination)
        .unwrap();
    let mut engine = Engine::open(&temp.path().join("system")).unwrap();
    let result = engine.wait_backup_preparation().unwrap();
    assert_eq!(result.phase, BackupPreparationPhase::Blocked);
    assert_eq!(result.error_code, Some(ErrorCode::DiskIdentityMismatch));
    assert_eq!(fs::read_dir(&destination).unwrap().count(), 0);
    offline_check_history_package(&moved.join("histories").join(&repository.history_tree_id))
        .unwrap();
}

#[test]
fn preparation_uses_current_registry_without_reviving_a_retired_primary() {
    let mut fixture = super::Fixture::new(true);
    let entry = fixture.save("docs/file", b"current-authority retained body");
    let destination = target(fixture.temp.path(), "current-target");
    let before = storage_engine::canonical_json(fixture.engine.state()).unwrap();
    let expected_registry = fixture
        .engine
        .current_disk_registry_commitment()
        .unwrap()
        .clone();
    fixture
        .engine
        .start_backup_preparation(
            request(&fixture.engine, vec![destination.clone()], op()),
            fs::File::open(&destination).unwrap(),
        )
        .unwrap();
    let completed = fixture.engine.wait_backup_preparation().unwrap();
    assert_eq!(
        completed.phase,
        BackupPreparationPhase::HistoryPrepared,
        "{completed:?}"
    );
    let report = offline_check_history_package(
        &destination
            .join("histories")
            .join(&fixture.repository.history_tree_id),
    )
    .unwrap();
    assert!(report.full_files >= 1);
    let complete = storage_engine::offline_check_backup_preparation(&destination).unwrap();
    assert_eq!(
        complete.state_hash,
        storage_engine::object_hash("backup-prepared-state", fixture.engine.state()).unwrap()
    );
    assert_eq!(
        storage_engine::canonical_json(fixture.engine.state()).unwrap(),
        before
    );
    assert_eq!(
        fixture.engine.current_disk_registry_commitment().unwrap(),
        &expected_registry
    );
    assert_eq!(
        fixture.engine.state().versions[&entry.version_id].file_id,
        entry.file_id
    );
}

#[test]
fn preparation_hot_status_rejects_configuration_only_exit_before_business_publication() {
    let (temp, mut engine, repository) = setup();
    let entry = save(&mut engine, &repository, "file", b"frozen complete body");
    let original = engine.config().clone();
    let destination = target(temp.path(), "config-bound-target");
    let prepared = op();
    engine
        .start_backup_preparation(
            request(&engine, vec![destination.clone()], prepared),
            fs::File::open(&destination).unwrap(),
        )
        .unwrap();
    assert_eq!(
        engine.wait_backup_preparation().unwrap().phase,
        BackupPreparationPhase::HistoryPrepared
    );
    let before = storage_engine::canonical_json(engine.state()).unwrap();
    let original_receipts = engine.state().operations.clone();
    let job_path = temp.path().join("system/backup-preparation/job.json");
    let job = fs::read(&job_path).unwrap();
    let marker = fs::read(destination.join("history-prepared.json")).unwrap();

    // The one-copy file lives only on P01. Remove C01 via the existing public
    // lifecycle protocol; no source body or target preparation is fabricated.
    let exit = op();
    engine
        .begin_disk_exit(
            &original.disks[2].disk_id,
            MembershipExitAction::Deregister,
            "admin",
            exit,
        )
        .unwrap();
    let archive = engine.build_managed_membership_control_archive().unwrap();
    engine
        .enroll_pending_members(&archive.archive_path, &archive.root_hash, Vec::new())
        .unwrap();
    assert!(
        engine
            .resume_membership_joint_decision()
            .unwrap()
            .all_certificates_synchronized()
    );
    // This existing fault point follows the new Config installation but precedes
    // publication of the exit State. The exact frozen publication is still live.
    engine.inject_fault_once(&format!(
        "after_disk_exit_marker_{}",
        original.disks[0].disk_id
    ));
    assert_eq!(
        engine.activate_pending_disk_exit().unwrap_err().code(),
        ErrorCode::InjectedFailure
    );
    assert_eq!(
        storage_engine::canonical_json(engine.state()).unwrap(),
        before
    );
    assert_ne!(
        storage_engine::canonical_json(engine.config()).unwrap(),
        storage_engine::canonical_json(&original).unwrap()
    );
    assert_eq!(
        engine.disk_exit_status().unwrap().unwrap().phase,
        DiskExitPhase::Activating
    );
    assert!(!engine.state().operations.contains_key(&exit));
    let changed = engine.backup_preparation_status().unwrap();
    assert_eq!(changed.operation_id, prepared);
    assert_eq!(changed.phase, BackupPreparationPhase::SourceChanged);
    assert_eq!(changed.error_code, Some(ErrorCode::ConfigConflict));
    assert_eq!(fs::read(&job_path).unwrap(), job);
    assert_eq!(
        fs::read(destination.join("history-prepared.json")).unwrap(),
        marker
    );

    // Complete the already-authorized lifecycle operation, not a backup switch.
    // Cold backup admission must classify the old source before touching a now
    // absent target path. It must neither replace the task nor recreate that path.
    let result = engine.activate_pending_disk_exit().unwrap();
    assert_eq!(result.operation_id, exit);
    let retained = temp.path().join("retained-config-bound-target");
    fs::rename(&destination, &retained).unwrap();
    drop(engine);
    let mut engine = Engine::open(&temp.path().join("system")).unwrap();
    let cold = engine.wait_backup_preparation().unwrap();
    assert_eq!(cold.operation_id, prepared);
    assert_eq!(cold.phase, BackupPreparationPhase::SourceChanged);
    assert_eq!(cold.error_code, Some(ErrorCode::ConfigConflict));
    assert!(!destination.exists());
    assert_eq!(fs::read(&job_path).unwrap(), job);
    assert_eq!(
        fs::read(retained.join("history-prepared.json")).unwrap(),
        marker
    );
    for (id, receipt) in original_receipts.iter() {
        assert_eq!(engine.operation(*id).unwrap(), *receipt);
    }
    assert_eq!(engine.operation(exit).unwrap(), result);
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(
        &mut engine.open_content(&entry.version_id).unwrap().file,
        &mut bytes,
    )
    .unwrap();
    assert_eq!(bytes, b"frozen complete body");
    offline_check_history_package(&retained.join("histories").join(&repository.history_tree_id))
        .unwrap();
}

// This is a composition check, not migration of these derived databases. The
// package target remains history-only, while normal source recovery owns SQLite.
#[test]
fn preparation_coexists_with_local_database_installation_and_cold_repair() {
    fn databases(root: &Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
        let mut found = std::collections::BTreeMap::new();
        let mut pending = vec![root.to_owned()];
        while let Some(path) = pending.pop() {
            for entry in fs::read_dir(path).unwrap() {
                let entry = entry.unwrap();
                if entry.file_type().unwrap().is_dir() {
                    pending.push(entry.path());
                } else if matches!(
                    entry.file_name().to_str(),
                    Some(".zxs-file.sqlite" | ".zxs-directory.sqlite")
                ) {
                    let bytes = fs::read(entry.path()).unwrap();
                    assert!(bytes.starts_with(b"SQLite format 3\0"));
                    found.insert(entry.path(), bytes);
                }
            }
        }
        found
    }
    let (temp, mut engine, repository) = setup();
    let original = save(&mut engine, &repository, "docs/file", b"old retained body");
    let latest = save(
        &mut engine,
        &repository,
        "docs/file",
        b"latest retained body",
    );
    for path in ["", "docs"] {
        let head = engine.state().branches[&repository.main_branch_id]
            .head_commit_id
            .clone();
        engine
            .initialize_directory_git(
                &repository.repository_id,
                &storage_contract::directory_git::DirectoryGitInitializeBody {
                    path: path.into(),
                    expected_namespace_head: head,
                    operation_id: op(),
                },
                "admin",
            )
            .unwrap();
    }
    engine
        .reconcile_directory_layout(&repository.repository_id)
        .unwrap();
    let primary = engine
        .config()
        .disks
        .iter()
        .find(|disk| disk.slot_id == "P01")
        .unwrap()
        .root
        .clone();
    let before = databases(&primary);
    assert!(
        before
            .keys()
            .any(|path| path.ends_with(".zxs-directory.sqlite"))
    );
    let file_database = before
        .keys()
        .find(|path| path.ends_with(".zxs-file.sqlite"))
        .unwrap()
        .clone();
    let state = storage_engine::canonical_json(engine.state()).unwrap();
    let config = storage_engine::canonical_json(engine.config()).unwrap();
    let destination = target(temp.path(), "database-composition-target");
    let request = request(&engine, vec![destination.clone()], op());
    engine
        .start_backup_preparation(request.clone(), fs::File::open(&destination).unwrap())
        .unwrap();
    assert_eq!(
        engine.wait_backup_preparation().unwrap().phase,
        BackupPreparationPhase::HistoryPrepared
    );
    assert_eq!(databases(&primary), before);
    assert!(databases(&destination).is_empty());
    let report = offline_check_history_package(
        &destination
            .join("histories")
            .join(&repository.history_tree_id),
    )
    .unwrap();
    assert_eq!(report.versions, 2);
    drop(engine);

    // A missing derived SQLite is rebuilt by ordinary admitted-State recovery,
    // not copied from the history-only target or accepted as new authority.
    fs::remove_file(&file_database).unwrap();
    let mut engine = Engine::open(&temp.path().join("system")).unwrap();
    assert_eq!(
        storage_engine::canonical_json(engine.state()).unwrap(),
        state
    );
    assert_eq!(
        storage_engine::canonical_json(engine.config()).unwrap(),
        config
    );
    assert!(databases(&primary).contains_key(&file_database));
    let completed = engine.wait_backup_preparation().unwrap();
    assert_eq!(completed.operation_id, request.operation_id);
    assert_eq!(completed.phase, BackupPreparationPhase::HistoryPrepared);
    assert!(databases(&destination).is_empty());
    for (version, expected) in [
        (original.version_id, b"old retained body".as_slice()),
        (latest.version_id, b"latest retained body".as_slice()),
    ] {
        use std::io::Read;
        let mut bytes = Vec::new();
        engine
            .open_content(&version)
            .unwrap()
            .file
            .read_to_end(&mut bytes)
            .unwrap();
        assert_eq!(bytes, expected);
    }
}
