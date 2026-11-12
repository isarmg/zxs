//! Native archive format admission preserves rejected source and target bytes.
use super::*;
use crate::{BackupPreparationPhase, CommitRequest, OperationId, RepositoryName};
use std::{fs, os::unix::fs::DirBuilderExt};

fn operation() -> OperationId {
    OperationId::generate().unwrap()
}

struct Fixture {
    temp: tempfile::TempDir,
    engine: Engine,
    target: PathBuf,
    operation: OperationId,
}

impl Fixture {
    fn prepared() -> Self {
        let temp = tempfile::Builder::new()
            .permissions(std::os::unix::fs::PermissionsExt::from_mode(0o700))
            .tempdir()
            .unwrap();
        let config =
            Config::development((0..3).map(|index| temp.path().join(format!("disk-{index}"))))
                .unwrap();
        let mut engine = Engine::initialize(&temp.path().join("system"), config).unwrap();
        engine.create_disk_root("P01", "admin", operation()).unwrap();
        let repository = engine
            .create_repository(
                &RepositoryName::parse("ArchiveFormat").unwrap(),
                "P01",
                3,
                "admin",
                operation(),
            )
            .unwrap();
        let workspace = engine
            .create_workspace(&repository.main_branch_id, "admin")
            .unwrap();
        engine
            .stage_bytes(&workspace.workspace_id, "file", b"native format body")
            .unwrap();
        engine
            .commit_workspace(CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: operation(),
                principal: "admin".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "native format fixture".into(),
                request_digest: None,
            })
            .unwrap();
        assert_eq!(engine.state().versions.len(), 1);
        let target = temp.path().join("selected-target");
        fs::DirBuilder::new().mode(0o700).create(&target).unwrap();
        let operation = operation();
        let request = BackupPreparationRequest {
            operation_id: operation,
            principal: "admin".into(),
            source_slot_id: "B01".into(),
            target_paths: vec![target.clone()],
            expected_sequence: engine.state().sequence,
        };
        engine
            .start_backup_preparation(request, fs::File::open(&target).unwrap())
            .unwrap();
        assert_eq!(
            engine.wait_backup_preparation().unwrap().phase,
            BackupPreparationPhase::HistoryPrepared
        );
        engine
            .start_backup_native_preparation(operation, "admin")
            .unwrap();
        assert_eq!(
            engine.wait_backup_preparation().unwrap().phase,
            BackupPreparationPhase::NativeControlPrepared
        );
        Self {
            temp,
            engine,
            target,
            operation,
        }
    }

    fn task(&self) -> NativeTask {
        NativeTask::read(&DiskRoot::open(&self.temp.path().join("system")).unwrap()).unwrap()
    }

    fn manifest(&self) -> Manifest {
        decode_proof(&fs::read(manifest_path(&self.target)).unwrap()).unwrap()
    }

    fn read(&self, expected: &str) -> Result<BackupNativeControlReport> {
        let task = self.task();
        read(
            &DiskRoot::open(&self.target).unwrap(),
            task.selection.first(),
            expected,
            &AtomicBool::new(false),
        )
    }

    fn write_manifest(&self, manifest: &Manifest) -> String {
        fs::write(manifest_path(&self.target), canonical_json(manifest).unwrap()).unwrap();
        object_hash(FORMAT, manifest).unwrap()
    }

    fn bytes(&self) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
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
fn current_native_archive_survives_cold_reopen_and_exact_continuation() {
    let mut fixture = Fixture::prepared();
    assert_eq!(FORMAT, "zxs-container-path-facts-backup-native-control");
    let task = fixture.task();
    let report = fixture.read(&task.archive_hash).unwrap();
    assert_eq!(report.original_members, 3);
    assert_eq!(report.sequence, fixture.engine.state().sequence);
    let manifest = fixture.manifest();
    assert_eq!(manifest.format, FORMAT);
    for member in &manifest.members {
        let path = object_path(&fixture.target, "backup-original-seal", &member.seal_hash).unwrap();
        let bytes = fs::read(path).unwrap();
        let value = storage_contract::strict_json(&bytes).unwrap();
        assert_eq!(value["format"], "container-path-facts-managed-directory");
        let seal: DiskSeal = decode_proof(&bytes).unwrap();
        assert_eq!(seal.format, DiskSealFormat::ManagedDirectory);
    }
    let state = canonical_json(fixture.engine.state()).unwrap();
    let config = canonical_json(fixture.engine.config()).unwrap();
    let system = fixture.temp.path().join("system");
    drop(fixture.engine);
    fixture.engine = Engine::open(&system).unwrap();
    assert_eq!(
        fixture.engine.wait_backup_preparation().unwrap().phase,
        BackupPreparationPhase::NativeControlPrepared
    );
    fixture
        .engine
        .start_backup_native_preparation(fixture.operation, "admin")
        .unwrap();
    assert_eq!(
        fixture.engine.wait_backup_preparation().unwrap().phase,
        BackupPreparationPhase::NativeControlPrepared
    );
    assert_eq!(fixture.task().archive_hash, task.archive_hash);
    assert_eq!(fixture.read(&task.archive_hash).unwrap(), report);
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), state);
    assert_eq!(canonical_json(fixture.engine.config()).unwrap(), config);
    assert!(fixture.engine.membership_fence_status().is_none());
    assert!(!fixture.engine.state().operations.contains_key(&fixture.operation));
}

#[test]
fn old_native_manifest_is_rejected_even_with_current_hash_domain_without_writes() {
    for format in ["zxs-backup-native-control", "zxs-container-tags-backup-native-control"] {
        let fixture = Fixture::prepared();
        let mut manifest = fixture.manifest();
        manifest.format = format.into();
        let expected = fixture.write_manifest(&manifest);
        assert_eq!(object_hash(FORMAT, &fixture.manifest()).unwrap(), expected);
        let before = fixture.bytes();
        assert_eq!(
            fixture.read(&expected).unwrap_err().code(),
            ErrorCode::PublicationInvalid
        );
        assert_eq!(fixture.bytes(), before);
    }
}

#[test]
fn old_original_seal_is_rejected_inside_current_rehashed_archive_without_writes() {
    for format in ["managed-directory", "container-tags-managed-directory"] {
        let fixture = Fixture::prepared();
        let mut manifest = fixture.manifest();
        assert_eq!(manifest.format, "zxs-container-path-facts-backup-native-control");
        let member = &mut manifest.members[0];
        let original = object_path(&fixture.target, "backup-original-seal", &member.seal_hash).unwrap();
        let mut seal = storage_contract::strict_json(&fs::read(original).unwrap()).unwrap();
        assert_eq!(seal["format"], "container-path-facts-managed-directory");
        seal["format"] = json_codec::Value::String(format.into());
        let hash = storage_contract::object_hash_value("backup-original-seal", &seal).unwrap();
        let bytes = canonical_json(&seal).unwrap();
        fs::write(
            object_path(&fixture.target, "backup-original-seal", &hash).unwrap(),
            &bytes,
        )
        .unwrap();
        member.seal_hash = hash.clone();
        let expected = fixture.write_manifest(&manifest);
        // Authenticate the changed object and outer envelope independently. Failure
        // must be format admission, not stale digests from the deliberate fixture edit.
        AuthenticatedMetadataObject::authenticate("backup-original-seal", &hash, seal).unwrap();
        assert_eq!(decode_proof::<DiskSeal>(&bytes).unwrap_err().code(), ErrorCode::InvalidJson);
        assert_eq!(object_hash(FORMAT, &fixture.manifest()).unwrap(), expected);
        let before = fixture.bytes();
        assert_eq!(fixture.read(&expected).unwrap_err().code(), ErrorCode::InvalidJson);
        assert_eq!(fixture.bytes(), before);
    }
}
