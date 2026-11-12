//! Original scopes reuse SQL bytes, never target admission or completed decisions.
use super::*;
use crate::engine::backup_preparation::native_control::archive;
use crate::engine::backup_preparation::native_control::databases::scoped::{
    self, encoding_memo,
};
use crate::engine::index::preparation::SystemEncodingInput;
use crate::{
    BackupPreparationPhase, BackupPreparationRequest, BackupTopologyTarget,
    BackupTransactionRequest, CommitRequest, Config, Entry, NamespaceOperation,
    NamespaceOperationKind, OperationId, Repository, RepositoryName,
};
use std::{cell::Cell, fs, os::unix::fs::DirBuilderExt, path::Path};

fn operation() -> OperationId {
    OperationId::generate().unwrap()
}

fn save(engine: &mut Engine, repository: &Repository, body: &[u8]) -> Entry {
    let workspace = engine
        .create_workspace(&repository.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "nested/file", body)
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: operation(),
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
        .get("nested/file")
        .unwrap()
        .clone()
}

fn target(root: &Path, name: &str) -> PathBuf {
    let path = root.join(name);
    fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
    path
}

fn fixture() -> (tempfile::TempDir, Engine, BackupTransactionRequest) {
    let temp = tempfile::Builder::new()
        .permissions(std::os::unix::fs::PermissionsExt::from_mode(0o700))
        .tempdir()
        .unwrap();
    let config =
        Config::development((0..3).map(|index| temp.path().join(format!("disk-{index}"))))
            .unwrap();
    let mut engine = Engine::initialize(&temp.path().join("system"), config).unwrap();
    engine
        .create_disk_root("P01", "admin", operation())
        .unwrap();
    let repository = engine
        .create_repository(
            &RepositoryName::parse("WholeRoute").unwrap(),
            "P01",
            3,
            "admin",
            operation(),
        )
        .unwrap();
    let old = save(&mut engine, &repository, b"retained first body");
    let latest = save(&mut engine, &repository, b"retained current body");
    assert_eq!(old.file_id, latest.file_id);
    assert_ne!(old.version_id, latest.version_id);
    engine
        .file_namespace(
            &repository.repository_id,
            NamespaceOperation::parse(NamespaceOperationKind::Mkdir, "nested/empty", None)
                .unwrap(),
            "admin",
            operation(),
        )
        .unwrap();
    assert_eq!(engine.state().versions.len(), 2);
    assert!(engine.state().directory_git_repositories.is_empty());
    let paths = vec![
        target(temp.path(), "selected-one"),
        target(temp.path(), "selected-two"),
    ];
    let request = BackupTransactionRequest {
        preparation: BackupPreparationRequest {
            operation_id: operation(),
            principal: "admin".into(),
            source_slot_id: "B01".into(),
            target_paths: paths.clone(),
            expected_sequence: engine.state().sequence,
        },
        targets: paths
            .iter()
            .enumerate()
            .map(|(index, _)| BackupTopologyTarget {
                disk_id: crate::new_id("disk").unwrap(),
                slot_id: format!("C{:02}", index + 2),
                upstream_slot_id: "B01".into(),
            })
            .collect(),
    };
    (temp, engine, request)
}

fn prepare_entities(engine: &mut Engine, request: &BackupTransactionRequest) {
    let operation = request.preparation.operation_id;
    engine
        .start_backup_transaction(
            request.clone(),
            request
                .preparation
                .target_paths
                .iter()
                .map(|path| fs::File::open(path).unwrap())
                .collect(),
        )
        .unwrap();
    for phase in [
        BackupPreparationPhase::HistoryPrepared,
        BackupPreparationPhase::NativeControlPrepared,
        BackupPreparationPhase::ObjectDatabasesPrepared,
        BackupPreparationPhase::ScopedDatabasesPrepared,
        BackupPreparationPhase::CutoverInputsPrepared,
    ] {
        assert_eq!(engine.wait_backup_preparation().unwrap().phase, phase);
        assert!(engine.membership_fence_status().is_none());
        assert!(!engine.state().operations.contains_key(&operation));
        if phase != BackupPreparationPhase::CutoverInputsPrepared {
            engine
                .continue_backup_transaction(operation, "admin")
                .unwrap();
        }
    }
    assert_eq!(
        engine.continue_backup_transaction(operation, "admin").unwrap().phase,
        BackupPreparationPhase::CutoverInputsPrepared,
    );
    engine
        .begin_backup_cutover_fence(operation, "admin")
        .unwrap();
    engine
        .continue_backup_transaction(operation, "admin")
        .unwrap();
    assert_eq!(
        engine.wait_backup_preparation().unwrap().phase,
        BackupPreparationPhase::TargetEntitiesPrepared,
    );
    assert_eq!(
        engine.continue_backup_transaction(operation, "admin").unwrap().phase,
        BackupPreparationPhase::TargetEntitiesPrepared,
    );
    for disk in &engine.config().disks {
        assert!(!disk.root.join("metadata/membership-generations").exists());
    }
}


struct RegistryFixture {
    temp: tempfile::TempDir,
    engine: Engine,
    prepared: PreparedBackupRegistry,
}

impl RegistryFixture {
    fn new() -> Self {
        let (temp, mut engine, request) = fixture();
        prepare_entities(&mut engine, &request);
        if let Some(worker) = engine.backup_preparation.as_mut() {
            worker.join().unwrap();
        }
        let task = Task {
            format: FORMAT.into(),
            installation: InstallationTask::current(
                &engine, request.preparation.operation_id, "admin",
            ).unwrap(),
        };
        let (targets, _choices) = runtime::acquire(&engine, &task).unwrap();
        let prepared = PreparedBackupRegistry::capture(
            task, targets, &AtomicBool::new(false),
        ).unwrap();
        Self { temp, engine, prepared }
    }

    fn enable(&mut self) {
        self.prepared.scope_encoding = Some(encoding_memo::ScopeEncodingMemo::new());
    }

    fn verify(&self) -> Result<()> {
        self.prepared.verify(&AtomicBool::new(false))
    }

    fn paths(&self) -> Vec<PathBuf> {
        self.prepared.targets.iter().map(|target| target.root.path().to_owned()).collect()
    }

    fn report(&self) -> Vec<u8> {
        canonical_json(&scoped::offline_check_backup_scoped_databases(&self.paths()).unwrap())
            .unwrap()
    }

    fn probe_system(&self, index: usize, expect_hit: bool) -> Vec<u8> {
        let stop = AtomicBool::new(false);
        let checkpoint = || preparation_checkpoint(&stop);
        let task = &self.prepared.task.installation.evidence.task.scoped;
        let job = &self.prepared.task.selection().jobs()[index];
        let root = &self.prepared.targets[index].root;
        let (_, state) = archive::read_observed(
            root, job, &task.objects.native.archive_hash, &stop,
            &mut |_, _, _, _| Ok(()),
        ).unwrap();
        let binding = |stop: &AtomicBool| {
            encoding_memo::digest(&(&self.prepared.task, &self.prepared.projection), stop)
        };
        let transaction = encoding_memo::Transaction::new(
            self.prepared.scope_encoding.as_ref(), &binding,
        );
        let mut session = transaction.session(&(task, job), &stop).unwrap();
        let input = SystemEncodingInput::new(&state, &checkpoint).unwrap();
        let encodes = Cell::new(0);
        let bytes = session.system(input.input_hash(), || {
            encodes.set(encodes.get() + 1);
            input.encode(&checkpoint)
        }).unwrap();
        input.verify(&bytes, &checkpoint).unwrap();
        assert_eq!(encodes.get(), usize::from(!expect_hit));
        // A probe never commits its own miss; only the real registry verifier
        // may establish the successful transaction boundary under test.
        bytes
    }

    fn tree(&self) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
        fn visit(root: &Path, path: &Path, entries: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
            let metadata = fs::symlink_metadata(path).unwrap();
            let relative = path.strip_prefix(root).unwrap().to_path_buf();
            if metadata.is_dir() {
                assert!(entries.insert(relative, None).is_none());
                for entry in fs::read_dir(path).unwrap() {
                    visit(root, &entry.unwrap().path(), entries);
                }
            } else {
                assert!(metadata.is_file());
                assert!(entries.insert(relative, Some(fs::read(path).unwrap())).is_none());
            }
        }
        let mut entries = BTreeMap::new();
        visit(self.temp.path(), self.temp.path(), &mut entries);
        entries
    }
}

#[test]
fn original_scopes_none_cold_and_hit_keep_real_bytes_and_reports_identical() {
    let mut fixture = RegistryFixture::new();
    let expected = fixture.report();
    let state = canonical_json(fixture.engine.state()).unwrap();
    let projection = encode(&fixture.prepared.projection).unwrap();
    let before = fixture.tree();
    fixture.verify().unwrap();
    fixture.enable();
    for _ in 0..2 {
        fixture.verify().unwrap();
        for index in 0..fixture.prepared.targets.len() {
            let bytes = fixture.probe_system(index, true);
            assert_eq!(
                bytes,
                fs::read(
                    fixture.paths()[index].join("scoped-databases/system/index.sqlite"),
                ).unwrap(),
            );
        }
        assert_eq!(fixture.report(), expected);
        assert_eq!(encode(&fixture.prepared.projection).unwrap(), projection);
        assert_eq!(canonical_json(fixture.engine.state()).unwrap(), state);
        assert_eq!(fixture.tree(), before);
    }
}

#[test]
fn warmed_original_scopes_still_reject_changed_archive_sqlite_and_completion() {
    let mut fixture = RegistryFixture::new();
    fixture.enable();
    fixture.verify().unwrap();
    let expected = fixture.report();
    for relative in [
        "native-control/manifest.json",
        "scoped-databases/system/index.sqlite",
        "scoped-databases-prepared.json",
    ] {
        let path = fixture.paths()[1].join(relative);
        let original = fs::read(&path).unwrap();
        let mut changed = original.clone();
        assert!(!changed.is_empty());
        changed[0] ^= 1;
        fs::write(&path, changed).unwrap();
        let before = fixture.tree();
        assert!(fixture.verify().is_err());
        assert_eq!(fixture.tree(), before);
        // Only the fixture owner repairs its deliberately damaged bytes.
        fs::write(&path, original).unwrap();
        fixture.verify().unwrap();
        fixture.probe_system(1, true);
        assert_eq!(fixture.report(), expected);
    }
}

#[test]
fn later_target_failure_drops_all_pending_original_encodings() {
    let mut fixture = RegistryFixture::new();
    fixture.enable();
    let path = fixture.paths()[1].join("scoped-databases-prepared.json");
    let original = fs::read(&path).unwrap();
    fs::write(&path, b"deliberately incomplete").unwrap();
    let before = fixture.tree();
    assert!(fixture.verify().is_err());
    assert_eq!(fixture.tree(), before);
    fs::write(&path, original).unwrap();
    // Target zero finished real scoped verification before target one's bad
    // completion. Neither target's pending encoding may survive that failure.
    fixture.probe_system(0, false);
    fixture.probe_system(1, false);
    fixture.verify().unwrap();
    fixture.probe_system(0, true);
    fixture.probe_system(1, true);
}

#[test]
fn original_scope_cancel_and_replaced_target_never_use_cached_admission() {
    let mut fixture = RegistryFixture::new();
    fixture.enable();
    fixture.verify().unwrap();
    let before = fixture.tree();
    assert_eq!(
        fixture.prepared.verify(&AtomicBool::new(true)).unwrap_err().code(),
        ErrorCode::OperationAborted,
    );
    assert_eq!(fixture.tree(), before);
    let target = fixture.paths()[0].clone();
    let displaced = fixture.temp.path().join("displaced-target");
    fs::rename(&target, &displaced).unwrap();
    fs::DirBuilder::new().mode(0o700).create(&target).unwrap();
    let changed = fixture.tree();
    assert!(fixture.verify().is_err());
    assert_eq!(fixture.tree(), changed);
    fs::remove_dir(&target).unwrap();
    fs::rename(&displaced, &target).unwrap();
    fixture.verify().unwrap();
    fixture.probe_system(0, true);
}

#[test]
fn cancellation_between_target_encodings_drops_pending_whole_verification() {
    use std::sync::atomic::Ordering;

    let mut fixture = RegistryFixture::new();
    fixture.enable();
    let stop = AtomicBool::new(false);
    let calls = Cell::new(0);
    let binding = |stop: &AtomicBool| {
        calls.set(calls.get() + 1);
        if calls.get() == 2 {
            stop.store(true, Ordering::Release);
        }
        encoding_memo::digest(&(&fixture.prepared.task, &fixture.prepared.projection), stop)
    };
    let mut transaction = encoding_memo::Transaction::new(
        fixture.prepared.scope_encoding.as_ref(), &binding,
    );
    let roots = fixture.prepared.targets.iter().map(|target| &target.root).collect::<Vec<_>>();
    let before = fixture.tree();
    let error = scoped::verify_at_with_encodings(
        &fixture.prepared.task.installation.evidence.task.scoped,
        &roots, &stop, &mut transaction,
    ).unwrap_err();
    assert_eq!(error.code(), ErrorCode::OperationAborted);
    assert_eq!(calls.get(), 2);
    drop(transaction);
    assert_eq!(fixture.tree(), before);
    fixture.probe_system(0, false);
    fixture.probe_system(1, false);
    fixture.verify().unwrap();
    fixture.probe_system(0, true);
}
