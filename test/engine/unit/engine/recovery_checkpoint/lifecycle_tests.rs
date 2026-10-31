use super::*;

#[derive(Clone, Copy, Debug)]
enum RecoveryPath {
    EligibleCheckpoint,
    OneCorruptCheckpoint,
    MemberCheckpointMismatch,
    TornTail,
}

struct LifecycleFixture {
    _temp: tempfile::TempDir,
    root: std::path::PathBuf,
    config: Config,
    repository: Repository,
    historical_plan: Plan,
    historical_operation: OperationId,
    historical_result: OperationResult,
    pending: Workspace,
    pending_item: String,
    upload_source: std::path::PathBuf,
    full_generations: Vec<StagedFile>,
    old_checkpoint: Vec<u8>,
    capacity: WorkspaceCapacity,
    state: Vec<u8>,
}

fn two_generations(
    engine: &mut Engine,
    workspace: &Workspace,
    source: &Path,
) -> (String, [StagedFile; 2]) {
    let item = new_id("entry");
    std::fs::write(source, b"old!").unwrap();
    let old = engine
        .stage_upload(&workspace.workspace_id, &item, "file", source, 1)
        .unwrap();
    std::fs::write(source, b"new!").unwrap();
    let current = engine
        .stage_upload(&workspace.workspace_id, &item, "file", source, 2)
        .unwrap();
    assert_ne!(old.source_path, current.source_path);
    (item, [old, current])
}

fn checked_checkpoint(bytes: &[u8], config: &Config) -> Checkpoint {
    let envelope: Envelope = decode_strict(bytes).unwrap();
    assert_eq!(
        envelope.hash,
        object_hash("replay-checkpoint", &envelope.checkpoint).unwrap()
    );
    for disk in &config.disks {
        assert!(
            scan(disk)
                .unwrap()
                .records
                .iter()
                .any(|(_, hash)| { envelope.checkpoint.tips.get(&disk.disk_id) == Some(hash) })
        );
    }
    envelope.checkpoint
}

impl LifecycleFixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("system");
        let mut config = Config::development([
            temp.path().join("a"),
            temp.path().join("b"),
            temp.path().join("c"),
        ]);
        config.max_retained_workspaces = 1;
        config.max_retained_workspaces_per_principal = 1;
        config.max_file_bytes = 4;
        config.max_workspace_bytes = 8;
        config.max_entries = 2;
        let mut engine = Engine::initialize(&root, config.clone()).unwrap();
        let repository = engine
            .create_repository(
                &RepositoryName::parse("lifecycle").unwrap(),
                "P01",
                3,
                "admin",
                OperationId::generate(),
            )
            .unwrap();
        drop(engine);
        let (mut engine, _) = reopen(&root);
        let old_checkpoint = std::fs::read(path(&config.disks[0])).unwrap();
        let head = engine.state().branches[&repository.main_branch_id]
            .head_commit_id
            .clone();
        let historical = engine
            .create_historical_workspace(&repository.main_branch_id, &head, "admin")
            .unwrap();
        let upload_source = temp.path().join("source");
        let (_, historical_full) = two_generations(&mut engine, &historical, &upload_source);
        let historical_plan = engine
            .create_historical_save_plan(
                &repository.history_tree_id,
                crate::HistoricalSaveOptions {
                    source_commit_id: head,
                    name: "retained history".into(),
                    workspace_id: Some(historical.workspace_id),
                    primary_slot_id: None,
                    target_copies: None,
                },
                "admin",
            )
            .unwrap();
        let historical_operation = OperationId::generate();
        let historical_result = engine
            .commit_historical_save_plan(&historical_plan.plan_id, "admin", historical_operation)
            .unwrap();
        assert_eq!(historical_result.commit_state, CommitState::Published);
        let pending = engine
            .create_workspace(&repository.main_branch_id, "admin")
            .unwrap();
        let (pending_item, pending_full) = two_generations(&mut engine, &pending, &upload_source);
        drop(engine);
        let (engine, _) = reopen(&root);
        let current = std::fs::read(path(&config.disks[0])).unwrap();
        assert_ne!(old_checkpoint, current);
        assert!(
            checked_checkpoint(&old_checkpoint, &config).sequence
                < checked_checkpoint(&current, &config).sequence
        );
        for disk in &config.disks {
            assert_eq!(std::fs::read(path(disk)).unwrap(), current);
        }
        let capacity = engine.workspace_capacity("admin");
        assert_eq!(capacity.global.records.used, 1);
        assert_eq!(capacity.principal.records.used, 1);
        assert_eq!(capacity.global.cold_records, 1);
        assert_eq!(capacity.principal.cold_records, 1);
        let state = canonical_json(engine.state()).unwrap();
        Self {
            _temp: temp,
            root,
            config,
            repository,
            historical_plan,
            historical_operation,
            historical_result,
            pending,
            pending_item,
            upload_source,
            full_generations: historical_full.into_iter().chain(pending_full).collect(),
            old_checkpoint,
            capacity,
            state,
        }
    }

    fn assert_full_generations(&self) {
        for (generation, expected) in self
            .full_generations
            .iter()
            .zip([b"old!", b"new!", b"old!", b"new!"])
        {
            assert_eq!(std::fs::read(&generation.source_path).unwrap(), expected);
        }
    }

    fn assert_original_historical_result(&self, engine: &mut Engine) {
        let sequence = engine.state().sequence;
        assert_eq!(
            engine
                .commit_historical_save_plan(
                    &self.historical_plan.plan_id,
                    "admin",
                    self.historical_operation,
                )
                .unwrap(),
            self.historical_result
        );
        assert_eq!(engine.state().sequence, sequence);
    }

    fn admission_evidence(&self) -> BTreeMap<std::path::PathBuf, Option<Vec<u8>>> {
        let mut files = BTreeMap::new();
        collect_files(&self.root.join("workspaces"), &mut files);
        collect_files(
            &self.config.disks[0].root.join("staging/workspaces"),
            &mut files,
        );
        for disk in &self.config.disks {
            files.insert(log_path(disk), Some(std::fs::read(log_path(disk)).unwrap()));
        }
        files
    }
}

fn collect_files(root: &Path, files: &mut BTreeMap<std::path::PathBuf, Option<Vec<u8>>>) {
    for entry in std::fs::read_dir(root).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if entry.file_type().unwrap().is_dir() {
            files.insert(path.clone(), None);
            collect_files(&path, files);
        } else {
            files.insert(path.clone(), Some(std::fs::read(path).unwrap()));
        }
    }
}

fn exercise_lifecycle(recovery: RecoveryPath) {
    let fixture = LifecycleFixture::new();
    let journals: Vec<_> = fixture
        .config
        .disks
        .iter()
        .map(|disk| std::fs::read(log_path(disk)).unwrap())
        .collect();
    match recovery {
        RecoveryPath::EligibleCheckpoint => (),
        RecoveryPath::OneCorruptCheckpoint => {
            std::fs::write(path(&fixture.config.disks[0]), b"not a checkpoint").unwrap();
        }
        RecoveryPath::MemberCheckpointMismatch => {
            std::fs::write(path(&fixture.config.disks[0]), &fixture.old_checkpoint).unwrap();
        }
        RecoveryPath::TornTail => {
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(log_path(&fixture.config.disks[0]))
                .unwrap();
            file.write_all(b"ZXSLO").unwrap();
            file.sync_all().unwrap();
        }
    }
    let (mut engine, decoded) = reopen(&fixture.root);
    match recovery {
        RecoveryPath::EligibleCheckpoint => assert_eq!(decoded, 1),
        _ => assert!(
            decoded >= 2,
            "{recovery:?} must replay both published slots"
        ),
    }
    let warnings = engine.take_recovery_warnings();
    if matches!(recovery, RecoveryPath::OneCorruptCheckpoint) {
        assert!(std::error::Error::source(&warnings.checkpoint_load.unwrap()).is_some());
    } else {
        assert!(warnings.checkpoint_load.is_none());
    }
    assert!(warnings.checkpoint_save.is_none());
    for (disk, original) in fixture.config.disks.iter().zip(&journals) {
        assert_eq!(std::fs::read(log_path(disk)).unwrap(), *original);
    }
    assert_eq!(canonical_json(engine.state()).unwrap(), fixture.state);
    assert_eq!(engine.workspace_capacity("admin"), fixture.capacity);
    fixture.assert_full_generations();
    fixture.assert_original_historical_result(&mut engine);
    assert_budget_rejections(&fixture, &mut engine);
    let request = CommitRequest {
        workspace_id: fixture.pending.workspace_id.clone(),
        operation_id: OperationId::generate(),
        principal: "admin".into(),
        expected_head: fixture.pending.base_head.clone(),
        fencing_token: fixture.pending.fencing_token,
        message: "after recovery".into(),
        request_digest: None,
    };
    let sequence = engine.state().sequence;
    let result = engine.commit_workspace(request.clone()).unwrap();
    assert_eq!(result.commit_state, CommitState::Published);
    assert_eq!(engine.state().sequence, sequence + 1);
    assert_eq!(engine.workspace_capacity("admin").principal.records.used, 0);
    assert_eq!(engine.workspace_capacity("admin").principal.cold_records, 2);
    drop(engine);
    let (mut engine, decoded) = reopen(&fixture.root);
    assert_eq!(
        decoded, 2,
        "checkpoint plus the new publication are replayed"
    );
    assert_eq!(engine.commit_workspace(request).unwrap(), result);
    assert_eq!(engine.state().sequence, sequence + 1);
    fixture.assert_original_historical_result(&mut engine);
    fixture.assert_full_generations();
}

fn assert_budget_rejections(fixture: &LifecycleFixture, engine: &mut Engine) {
    let evidence = fixture.admission_evidence();
    assert_eq!(
        engine
            .create_workspace(&fixture.repository.main_branch_id, "admin")
            .unwrap_err()
            .code(),
        ErrorCode::ResourceLimit
    );
    assert_eq!(fixture.admission_evidence(), evidence);
    assert_eq!(
        engine
            .stage_upload(
                &fixture.pending.workspace_id,
                &fixture.pending_item,
                "file",
                &fixture.upload_source,
                3,
            )
            .unwrap_err()
            .code(),
        ErrorCode::ResourceLimit
    );
    assert_eq!(fixture.admission_evidence(), evidence);
    let retried = engine
        .stage_upload(
            &fixture.pending.workspace_id,
            &fixture.pending_item,
            "file",
            &fixture.upload_source,
            2,
        )
        .unwrap();
    assert_eq!(retried.source_path, fixture.full_generations[3].source_path);
    assert_eq!(retried.dirty_generation, 2);
    assert_eq!(fixture.admission_evidence(), evidence);
    assert_eq!(engine.workspace_capacity("admin"), fixture.capacity);
}

#[test]
fn checkpoint_hit_preserves_cold_history_full_generations_and_exact_staging_budget() {
    exercise_lifecycle(RecoveryPath::EligibleCheckpoint);
}

#[test]
fn corrupt_checkpoint_fallback_preserves_cold_history_full_generations_and_exact_staging_budget() {
    exercise_lifecycle(RecoveryPath::OneCorruptCheckpoint);
}

#[test]
fn member_checkpoint_mismatch_preserves_cold_history_full_generations_and_exact_staging_budget() {
    exercise_lifecycle(RecoveryPath::MemberCheckpointMismatch);
}

#[test]
fn torn_tail_recovery_preserves_cold_history_full_generations_and_exact_staging_budget() {
    exercise_lifecycle(RecoveryPath::TornTail);
}
