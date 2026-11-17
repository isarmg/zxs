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
        Self::prepared_with_sibling(false)
    }

    fn prepared_with_sibling(include_sibling: bool) -> Self {
        let temp = tempfile::Builder::new()
            .permissions(std::os::unix::fs::PermissionsExt::from_mode(0o700))
            .tempdir()
            .unwrap();
        let config =
            Config::development(crate::test_disks(
                (0..3).map(|index| temp.path().join(format!("disk-{index}"))),
            ))
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
        let mut targets = vec![target.clone()];
        if include_sibling {
            let sibling = temp.path().join("sibling-target");
            fs::DirBuilder::new().mode(0o700).create(&sibling).unwrap();
            targets.push(sibling);
        }
        let operation = operation();
        let request = BackupPreparationRequest {
            operation_id: operation,
            principal: "admin".into(),
            source_slot_id: "B01".into(),
            target_paths: targets.clone(),
            expected_sequence: engine.state().sequence,
        };
        if include_sibling {
            engine.start_backup_batch_preparation(
                request,
                targets.iter().map(|path| fs::File::open(path).unwrap()).collect(),
            ).unwrap();
        } else {
            engine.start_backup_preparation(request, fs::File::open(&target).unwrap()).unwrap();
        }
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
    assert_eq!(FORMAT, "zxs-container-current-name-backup-native-control");
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
        assert_eq!(value["format"], "container-current-name-managed-directory");
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
        assert_eq!(manifest.format, "zxs-container-current-name-backup-native-control");
        let member = &mut manifest.members[0];
        let original =
            object_path(&fixture.target, "backup-original-seal", &member.seal_hash).unwrap();
        let mut seal = storage_contract::strict_json(&fs::read(original).unwrap()).unwrap();
        assert_eq!(seal["format"], "container-current-name-managed-directory");
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

#[test]
fn partial_native_marker_fanout_never_refills_a_peer_or_ignores_a_late_conflict() {
    use std::os::unix::fs::MetadataExt;
    let mut fixture = Fixture::prepared_with_sibling(true);
    let system = fixture.temp.path().join("system");
    let sibling = fixture.temp.path().join("sibling-target");
    let first_marker = fixture.target.join(super::COMPLETE);
    let last_marker = sibling.join(super::COMPLETE);
    let expected = fs::read(&last_marker).unwrap();
    assert_eq!(fs::read(&first_marker).unwrap(), expected);
    let last_inode = fs::metadata(&last_marker).unwrap().ino();
    let first_journals = journal_path(&fixture.target, 0).parent().unwrap().to_owned();
    let saved = fixture.temp.path().join("saved-original-journals");
    // Crash during marker fan-out: the only marker is on the *last* peer.
    fs::remove_file(&first_marker).unwrap();
    fs::rename(&first_journals, &saved).unwrap();
    drop(fixture.engine);
    fixture.engine = Engine::open(&system).unwrap();
    assert_eq!(fixture.engine.wait_backup_preparation().unwrap().phase, BackupPreparationPhase::Blocked);
    assert!(!first_journals.exists()); // no mkdir, not just no rewritten journal
    assert!(!first_marker.exists());
    assert_eq!(fs::read(&last_marker).unwrap(), expected);
    // Even with the original physical evidence back, a later conflicting
    // completion may not be discovered only after the first marker was created.
    fs::rename(&saved, &first_journals).unwrap();
    fs::write(&last_marker, b"unrelated retained evidence").unwrap();
    fixture.engine.start_backup_native_preparation(fixture.operation, "admin").unwrap();
    assert_eq!(fixture.engine.wait_backup_preparation().unwrap().phase, BackupPreparationPhase::Blocked);
    assert!(!first_marker.exists());
    assert_eq!(fs::read(&last_marker).unwrap(), b"unrelated retained evidence");
    // With all material and existing marker exact, only the interrupted marker
    // tail completes. The observed peer inode, target bindings and original task survive.
    fs::write(&last_marker, &expected).unwrap();
    fixture.engine.start_backup_native_preparation(fixture.operation, "admin").unwrap();
    assert_eq!(fixture.engine.wait_backup_preparation().unwrap().phase, BackupPreparationPhase::NativeControlPrepared);
    assert_eq!(fs::metadata(&last_marker).unwrap().ino(), last_inode);
    assert_eq!(fs::read(first_marker).unwrap(), expected);
    let report = super::offline_check_backup_native_preparation(&[fixture.target, sibling]).unwrap();
    assert_eq!(canonical_json(&report).unwrap(), expected);
}

#[test]
fn incomplete_native_archive_resumes_original_task_but_unowned_evidence_is_not_adopted() {
    let mut fixture = Fixture::prepared();
    let journal = journal_path(&fixture.target, 0);
    let expected = fs::read(&journal).unwrap();
    fs::remove_file(fixture.target.join(super::COMPLETE)).unwrap();
    fs::remove_file(&journal).unwrap();
    fixture.engine.start_backup_native_preparation(fixture.operation, "admin").unwrap();
    assert_eq!(fixture.engine.wait_backup_preparation().unwrap().phase, BackupPreparationPhase::NativeControlPrepared);
    assert_eq!(fs::read(journal).unwrap(), expected);
    let system_intent = fixture.temp.path().join("system").join(super::TASK);
    fs::remove_file(&system_intent).unwrap();
    let before = fixture.bytes();
    assert_eq!(fixture.engine.start_backup_native_preparation(fixture.operation, "admin").unwrap_err().code(), ErrorCode::ConfigConflict);
    assert!(!system_intent.exists());
    assert_eq!(fixture.bytes(), before);
}

#[test]
fn native_allocation_visits_the_exact_written_archive_and_forecasts_the_real_fanout() {
    let fixture = Fixture::prepared_with_sibling(true);
    let task = fixture.task();
    let archive = PreparedArchive::freeze(&fixture.engine, task.selection.first()).unwrap();
    let stop = AtomicBool::new(false);
    let roots = task.selection.jobs().iter().map(|job| DiskRoot::open(&job.request.target_paths[0]).unwrap()).collect::<Vec<_>>();
    let held = roots.iter().collect::<Vec<_>>();
    let mut installed = BTreeMap::new();
    archive.visit_files(Path::new(""), VisitOrder::Install, |relative, bytes| {
        for root in &roots {
            assert_eq!(root.read_bytes(&root.path().join(&relative), bytes.len())?, bytes);
        }
        assert!(installed.insert(relative, bytes.len()).is_none());
        Ok(())
    }).unwrap();
    let mut ordered = Vec::new();
    archive.visit_files(Path::new(""), VisitOrder::Address, |relative, bytes| {
        ordered.push((relative, bytes.len()));
        Ok(())
    }).unwrap();
    assert_eq!(installed.into_iter().collect::<Vec<_>>(), ordered);
    assert!(ordered.iter().any(|(path, _)| path.starts_with("native-control/journals")));
    assert!(ordered.iter().any(|(path, _)| path.starts_with("native-control/objects/backup-original-seal")));
    assert!(ordered.iter().any(|(path, _)| path == Path::new("native-control/manifest.json")));
    let materials = task.selection.verify_material(&held, &stop).unwrap();
    let length = super::allocation::completion_length(&task, &archive.report, materials.clone()).unwrap();
    for root in &roots {
        assert_eq!(fs::metadata(root.path().join(super::COMPLETE)).unwrap().len(), length as u64);
    }
    let mut too_large = materials;
    too_large[0].format = "\"".repeat(super::MAX_TASK / 2);
    assert_eq!(super::allocation::completion_length(&task, &archive.report, too_large).unwrap_err().code(), ErrorCode::ResourceLimit);
}

#[test]
fn partial_native_archive_on_a_late_peer_is_checked_before_any_earlier_refill() {
    let mut fixture = Fixture::prepared_with_sibling(true);
    let sibling = fixture.temp.path().join("sibling-target");
    for root in [&fixture.target, &sibling] {
        fs::remove_file(root.join(super::COMPLETE)).unwrap();
    }
    // With neither marker left this is a copier, but all original targets must
    // pass their *local* preflight before it restores any missing input.
    let first = journal_path(&fixture.target, 0);
    let expected = fs::read(&first).unwrap();
    fs::remove_file(&first).unwrap();
    let last = journal_path(&sibling, 0);
    let mut conflict = fs::read(&last).unwrap();
    conflict[0] ^= 1;
    fs::write(&last, &conflict).unwrap();
    fixture.engine.start_backup_native_preparation(fixture.operation, "admin").unwrap();
    let status = fixture.engine.wait_backup_preparation().unwrap();
    assert_eq!(status.phase, BackupPreparationPhase::Blocked);
    assert_eq!(status.error_code, Some(ErrorCode::ObjectCollision));
    assert!(!first.exists());
    assert_eq!(fs::read(&last).unwrap(), conflict);
    assert!(!fixture.target.join(super::COMPLETE).exists());
    assert!(!sibling.join(super::COMPLETE).exists());
    fs::write(&last, expected.as_slice()).unwrap();
    fixture.engine.start_backup_native_preparation(fixture.operation, "admin").unwrap();
    assert_eq!(fixture.engine.wait_backup_preparation().unwrap().phase, BackupPreparationPhase::NativeControlPrepared);
    assert_eq!(fs::read(first).unwrap(), expected);
}

#[test]
fn object_scoped_and_cutover_marker_tails_require_the_unmarked_peers_own_evidence() {
    use crate::{BackupCutoverPreparationRequest, BackupTopologyTarget};
    use std::os::unix::fs::MetadataExt;
    let mut f = Fixture::prepared_with_sibling(true);
    let op = f.operation;
    fn exercise(
        f: &mut Fixture, marker: &str, material: &str, phase: BackupPreparationPhase,
        start: impl Fn(&mut Engine) -> Result<BackupPreparationStatus>,
    ) {
        start(&mut f.engine).unwrap();
        assert_eq!(f.engine.wait_backup_preparation().unwrap().phase, phase);
        let first = f.target.join(marker);
        let last = f.temp.path().join("sibling-target").join(marker);
        let material = f.target.join(material);
        let saved = f.temp.path().join("exact-stage-material");
        let expected = fs::read(&last).unwrap();
        let last_inode = fs::metadata(&last).unwrap().ino();
        fs::remove_file(&first).unwrap();
        fs::rename(&material, &saved).unwrap();
        start(&mut f.engine).unwrap();
        assert_eq!(f.engine.wait_backup_preparation().unwrap().phase, BackupPreparationPhase::Blocked);
        assert!(!first.exists());
        assert!(!material.exists());
        assert_eq!(fs::read(&last).unwrap(), expected);
        assert!(f.engine.membership_fence_status().is_none());
        // A partial marker fan-out with intact physical evidence is resumable.
        fs::rename(&saved, &material).unwrap();
        start(&mut f.engine).unwrap();
        assert_eq!(f.engine.wait_backup_preparation().unwrap().phase, phase);
        assert_eq!(fs::metadata(&last).unwrap().ino(), last_inode);
        assert_eq!(fs::read(&first).unwrap(), expected);
    }
    exercise(&mut f, "object-databases-prepared.json", "object-databases", BackupPreparationPhase::ObjectDatabasesPrepared,
        |engine| engine.start_backup_object_database_preparation(op, "admin"));
    exercise(&mut f, "scoped-databases-prepared.json", "scoped-databases", BackupPreparationPhase::ScopedDatabasesPrepared,
        |engine| engine.start_backup_scoped_database_preparation(op, "admin"));
    let cutover = BackupCutoverPreparationRequest {
        operation_id: op, principal: "admin".into(),
        targets: (0..2).map(|i| BackupTopologyTarget {
            name: name_policy::Name::parse(&format!("New archive {}", i + 1)).unwrap(),
            disk_id: crate::new_id("disk").unwrap(), slot_id: format!("C{:02}", i + 2), upstream_slot_id: "B01".into(),
        }).collect(),
    };
    exercise(&mut f, "cutover-inputs-prepared.json", "cutover-target-prepared.json", BackupPreparationPhase::CutoverInputsPrepared,
        |engine| engine.start_backup_cutover_preparation(cutover.clone()));
    assert!(f.engine.membership_fence_status().is_none());
    assert!(!f.engine.state().operations.contains_key(&op));
}
