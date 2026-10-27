use super::*;
use crate::journal::RecordType;
use std::io::Write;

fn setup() -> (tempfile::TempDir, std::path::PathBuf, Config) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("system");
    let config = Config::development([
        temp.path().join("a"),
        temp.path().join("b"),
        temp.path().join("c"),
    ]);
    let mut engine = Engine::initialize(&root, config.clone()).unwrap();
    for n in 0..6 {
        engine
            .create_repository(
                &storage_contract::RepositoryName::parse(&format!("repo{n}")).unwrap(),
                "P01",
                3,
                "admin",
                storage_contract::OperationId::generate(),
            )
            .unwrap();
    }
    drop(engine);
    (temp, root, config)
}
fn reopen(root: &Path) -> (Engine, usize) {
    SNAPSHOT_LOAD_COUNT.with(|count| count.set(0));
    let engine = Engine::open(root).unwrap();
    let count = SNAPSHOT_LOAD_COUNT.with(|count| count.get());
    (engine, count)
}
#[test]
fn missing_or_corrupt_selected_snapshot_source_repairs_from_verified_replica() {
    for corrupt in [false, true] {
        let (_temp, root, config) = setup();
        let (engine, _) = reopen(&root);
        let expected = canonical_json(engine.state()).unwrap();
        drop(engine);
        let source = config.disks.iter().min_by_key(|d| &d.disk_id).unwrap();
        let first = scan(source)
            .unwrap()
            .records
            .into_iter()
            .find(|(r, _)| r.record_type == RecordType::Prepare)
            .unwrap()
            .0;
        let candidate: Candidate = load_object(source, "candidate", &first.candidate_hash).unwrap();
        let snapshot: Snapshot = load_object(source, "snapshot", &candidate.snapshot_hash).unwrap();
        let page =
            crate::journal::map_pages(source, snapshot.maps.values().next().unwrap())[0].clone();
        let path = object_path(source, "state-page", &page);
        let original = std::fs::read(&path).unwrap();
        if corrupt {
            std::fs::write(&path, b"corrupt").unwrap();
        } else {
            std::fs::remove_file(&path).unwrap();
        }
        let (engine, _) = reopen(&root);
        assert_eq!(canonical_json(engine.state()).unwrap(), expected);
        assert_eq!(std::fs::read(&path).unwrap(), original);
        drop(engine);
        assert_eq!(reopen(&root).1, 1);
    }
}
#[test]
fn all_snapshot_replicas_invalid_fail_closed() {
    let (_temp, root, config) = setup();
    drop(reopen(&root).0);
    let source = &config.disks[0];
    let first = scan(source)
        .unwrap()
        .records
        .into_iter()
        .find(|(r, _)| r.record_type == RecordType::Prepare)
        .unwrap()
        .0;
    let candidate: Candidate = load_object(source, "candidate", &first.candidate_hash).unwrap();
    let snapshot: Snapshot = load_object(source, "snapshot", &candidate.snapshot_hash).unwrap();
    let page = crate::journal::map_pages(source, snapshot.maps.values().next().unwrap())[0].clone();
    for disk in &config.disks {
        std::fs::remove_file(object_path(disk, "state-page", &page)).unwrap();
    }
    assert!(Engine::open(&root).is_err());
}

#[test]
fn same_length_legacy_payload_changes_invalidate_coverage() {
    let (_temp, root, config) = setup();
    let legacy = config.disks[0]
        .root
        .join("histories/history_test/file-containers/file_test/name/legacy/ver_test/body");
    std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    std::fs::write(&legacy, b"good").unwrap();
    drop(reopen(&root).0);
    std::fs::write(&legacy, b"evil").unwrap();
    assert!(
        reopen(&root).1 >= 6,
        "legacy bytes must invalidate the skip, even at equal length"
    );
    assert!(
        !payload(
            &config.disks[0].root,
            Path::new("histories/history_test/versions/ver_test/content/body")
        )
        .unwrap()
    );
    assert!(
        !payload(
            &config.disks[0].root,
            Path::new("histories/history_test/file-containers/file_test/name/legacy/ver_test/body")
        )
        .unwrap()
    );
}

#[test]
fn clean_checkpoint_reopens_decode_one_snapshot_and_preserve_state() {
    let (_temp, root, config) = setup();
    let (engine, full) = reopen(&root);
    let expected = canonical_json(engine.state()).unwrap();
    assert!(full >= 6);
    assert!(config.disks.iter().all(|d| path(d).exists()));
    drop(engine);
    let (engine, fast) = reopen(&root);
    assert_eq!(fast, 1);
    assert_eq!(canonical_json(engine.state()).unwrap(), expected);
}
#[test]
fn cold_replay_reuses_member_specific_receipts_across_decision_slots() {
    let (_temp, root, config) = setup();
    let disk = &config.disks[0];
    let (last, _) = scan(disk).unwrap().records.pop().unwrap();
    // Give one member a different journal chain without adding a decision.
    append(
        disk,
        RecordType::Reprotect,
        last.sequence,
        &last.candidate_hash,
        &last.object_hash,
    )
    .unwrap();
    let (mut engine, _) = reopen(&root);
    for name in ["suffix-a", "suffix-b", "suffix-c"] {
        engine
            .create_repository(
                &storage_contract::RepositoryName::parse(name).unwrap(),
                "P01",
                3,
                "admin",
                OperationId::generate(),
            )
            .unwrap();
    }
    let expected = canonical_json(engine.state()).unwrap();
    drop(engine);
    let first_tip = scan(&config.disks[0]).unwrap().records.pop().unwrap().1;
    let second_tip = scan(&config.disks[1]).unwrap().records.pop().unwrap().1;
    assert_ne!(first_tip, second_tip);
    let journals: Vec<_> = config
        .disks
        .iter()
        .map(|disk| {
            std::fs::remove_file(path(disk)).unwrap();
            std::fs::read(log_path(disk)).unwrap()
        })
        .collect();
    let (mut engine, full) = reopen(&root);
    assert!(full >= 9);
    assert_eq!(canonical_json(engine.state()).unwrap(), expected);
    let warnings = engine.take_recovery_warnings();
    assert!(warnings.checkpoint_load.is_none());
    assert!(warnings.checkpoint_save.is_none());
    for (disk, before) in config.disks.iter().zip(&journals) {
        assert_eq!(&std::fs::read(log_path(disk)).unwrap(), before);
    }
    drop(engine);
    let (engine, fast) = reopen(&root);
    assert_eq!(fast, 1);
    assert_eq!(canonical_json(engine.state()).unwrap(), expected);
}

#[test]
fn added_conflicting_layout_cannot_hide_behind_checkpoint_coverage() {
    let (_temp, root, config) = setup();
    drop(reopen(&root).0);
    let source = &config.disks[0];
    let first = scan(source)
        .unwrap()
        .records
        .into_iter()
        .find(|(r, _)| r.record_type == RecordType::Publication)
        .unwrap()
        .0;
    for disk in &config.disks {
        let flat = disk
            .root
            .join("metadata/objects/candidate")
            .join(format!("{}.json", first.candidate_hash));
        std::fs::write(flat, b"{}").unwrap();
    }
    assert!(Engine::open(&root).is_err());
}

#[test]
fn missing_historical_page_falls_back_and_repairs_replica() {
    let (_temp, root, config) = setup();
    drop(reopen(&root).0);
    let source = config.disks.iter().max_by_key(|d| &d.disk_id).unwrap();
    let first = scan(source)
        .unwrap()
        .records
        .into_iter()
        .find(|(r, _)| r.record_type == RecordType::Prepare)
        .unwrap()
        .0;
    let candidate: Candidate = load_object(source, "candidate", &first.candidate_hash).unwrap();
    let snapshot: Snapshot = load_object(source, "snapshot", &candidate.snapshot_hash).unwrap();
    let page = crate::journal::map_pages(source, snapshot.maps.values().next().unwrap())[0].clone();
    let missing = object_path(source, "state-page", &page);
    assert!(missing.exists());
    std::fs::remove_file(&missing).unwrap();
    let (engine, count) = reopen(&root);
    assert!(count >= 6);
    assert!(missing.exists());
    drop(engine);
    assert_eq!(reopen(&root).1, 1);
}
fn save_file(engine: &mut Engine, branch: &str, name: &str, bytes: &[u8]) {
    let workspace = engine.create_workspace(branch, "admin").unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, name, bytes)
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: storage_contract::OperationId::generate(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "checkpoint suffix fixture".into(),
            request_digest: None,
        })
        .unwrap();
}
#[test]
fn restored_absent_historical_version_directory_requires_full_proof_repair() {
    let (_temp, root, config) = setup();
    let mut engine = Engine::open(&root).unwrap();
    let branch = engine
        .state
        .repositories
        .values()
        .next()
        .unwrap()
        .main_branch_id
        .clone();
    save_file(&mut engine, &branch, "lost-proof.txt", b"body");
    let version = engine.state.versions.values().next().unwrap().clone();
    drop(engine);
    let directory = version_dir(
        &config.disks[0],
        &version.history_tree_id,
        &version.version_id,
    );
    std::fs::remove_dir_all(&directory).unwrap();
    drop(reopen(&root).0);
    // Recovery legitimately skipped the absent local directory; making it
    // exist again must not bypass reconstruction of its historical proof.
    private_dir(&directory).unwrap();
    let (_, count) = reopen(&root);
    assert!(count >= 7);
    assert!(directory.join("publication-proof.json").exists());
}

#[test]
fn forked_suffix_cannot_authorize_derived_cache_exception() {
    let (_temp, root, config) = setup();
    let engine = reopen(&root).0;
    let envelope: Envelope = read_json(&path(&config.disks[0])).unwrap();
    let sequence = envelope.checkpoint.sequence + 1;
    let mut candidates = BTreeMap::new();
    for hash in ["a".repeat(64), "b".repeat(64)] {
        candidates.insert(
            hash.clone(),
            vec![Receipt {
                disk_id: config.disks[0].disk_id.clone(),
                slot_id: config.disks[0].slot_id.clone(),
                frame_hash: "0".repeat(64),
                record: Record {
                    record_type: RecordType::Accept,
                    sequence,
                    candidate_hash: hash.clone(),
                    object_hash: hash,
                    previous_frame_hash: "0".repeat(64),
                },
            }],
        );
    }
    assert!(
        suffix_rebuilt_containers(
            &engine,
            &envelope.checkpoint,
            &BTreeMap::from([(sequence, candidates)])
        )
        .is_err()
    );
}

#[test]
fn repeated_same_file_saves_replay_only_checkpoint_and_suffix() {
    let (_temp, root, _config) = setup();
    let mut engine = Engine::open(&root).unwrap();
    let branch = engine
        .state
        .repositories
        .values()
        .next()
        .unwrap()
        .main_branch_id
        .clone();
    save_file(&mut engine, &branch, "same.txt", b"first");
    drop(engine);
    let (mut engine, _) = reopen(&root);
    for bytes in [
        b"second".as_slice(),
        b"third".as_slice(),
        b"fourth".as_slice(),
    ] {
        save_file(&mut engine, &branch, "same.txt", bytes);
    }
    let expected = canonical_json(engine.state()).unwrap();
    drop(engine);
    let (engine, count) = reopen(&root);
    assert_eq!(
        count, 4,
        "one checkpoint snapshot plus three suffix decisions"
    );
    assert_eq!(canonical_json(engine.state()).unwrap(), expected);
    drop(engine);
    assert_eq!(reopen(&root).1, 1);
}
#[test]
fn corrupted_container_outside_suffix_forces_full_repair() {
    let (_temp, root, config) = setup();
    let mut engine = Engine::open(&root).unwrap();
    let branch = engine
        .state
        .repositories
        .values()
        .next()
        .unwrap()
        .main_branch_id
        .clone();
    save_file(&mut engine, &branch, "first.txt", b"first");
    save_file(&mut engine, &branch, "other.txt", b"other");
    let other = engine
        .state
        .versions
        .values()
        .find(|v| v.creation_name == "other.txt")
        .unwrap()
        .clone();
    drop(engine);
    let (mut engine, _) = reopen(&root);
    save_file(&mut engine, &branch, "first.txt", b"new first");
    drop(engine);
    let body = crate::container_storage::content_path(
        &version_dir(&config.disks[0], &other.history_tree_id, &other.version_id),
        &other,
    )
    .unwrap();
    let cache = body
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("container.json");
    std::fs::write(&cache, b"{}").unwrap();
    let (_, count) = reopen(&root);
    assert!(
        count >= 9,
        "unrelated suffix evidence must not waive this cache"
    );
    assert!(std::fs::read(&cache).unwrap().len() > 2);
}

#[test]
fn missing_local_publication_proof_falls_back_and_repairs() {
    let (_temp, root, config) = setup();
    let mut engine = Engine::open(&root).unwrap();
    let branch = engine
        .state
        .repositories
        .values()
        .next()
        .unwrap()
        .main_branch_id
        .clone();
    let workspace = engine.create_workspace(&branch, "admin").unwrap();
    engine
        .stage_bytes(
            &workspace.workspace_id,
            "example.txt",
            b"checkpoint payload",
        )
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: storage_contract::OperationId::generate(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "checkpoint proof fixture".into(),
            request_digest: None,
        })
        .unwrap();
    drop(engine);
    drop(reopen(&root).0);
    fn find_proof(root: &Path) -> Option<std::path::PathBuf> {
        for entry in std::fs::read_dir(root).ok()? {
            let path = entry.ok()?.path();
            if path
                .file_name()
                .is_some_and(|n| n == "publication-proof.json")
            {
                return Some(path);
            }
            if path.is_dir()
                && let Some(proof) = find_proof(&path)
            {
                return Some(proof);
            }
        }
        None
    }
    let proof = find_proof(&config.disks[1].root.join("histories")).unwrap();
    std::fs::remove_file(&proof).unwrap();
    let (engine, count) = reopen(&root);
    assert!(count >= 7);
    assert!(proof.exists());
    drop(engine);
    assert_eq!(reopen(&root).1, 1);
}

#[test]
fn malformed_or_partially_replicated_checkpoint_falls_back() {
    let (_temp, root, config) = setup();
    drop(reopen(&root).0);
    std::fs::write(path(&config.disks[0]), b"not a checkpoint").unwrap();
    let (mut engine, count) = reopen(&root);
    assert!(count >= 6);
    let warnings = engine.take_recovery_warnings();
    let error = warnings
        .checkpoint_load
        .expect("malformed cache is reported once");
    assert!(std::error::Error::source(&error).is_some());
    assert!(warnings.checkpoint_save.is_none());
    assert!(engine.take_recovery_warnings().checkpoint_load.is_none());
    drop(engine);
    assert_eq!(reopen(&root).1, 1);
}
#[test]
fn checkpoint_never_hides_corrupt_journal_prefix() {
    use std::io::{Seek, SeekFrom};
    let (_temp, root, config) = setup();
    drop(reopen(&root).0);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .open(log_path(&config.disks[0]))
        .unwrap();
    file.seek(SeekFrom::Start(20)).unwrap();
    file.write_all(b"!").unwrap();
    let error = Engine::open(&root)
        .err()
        .expect("corrupt authoritative journal must fail");
    assert_eq!(error.code(), storage_contract::ErrorCode::LogCorrupt);
}
#[test]
fn checkpoint_survives_system_disk_loss_without_becoming_authority() {
    let (_temp, root, config) = setup();
    let (engine, _) = reopen(&root);
    let expected = canonical_json(engine.state()).unwrap();
    drop(engine);
    std::fs::remove_dir_all(&root).unwrap();
    let disks = config
        .disks
        .iter()
        .map(|d| d.root.clone())
        .collect::<Vec<_>>();
    let engine = Engine::restore_from_disks(&root, &disks).unwrap();
    assert_eq!(canonical_json(engine.state()).unwrap(), expected);
}

#[test]
fn degraded_open_keeps_full_replay_and_visibility_rules() {
    let (_temp, root, config) = setup();
    let (engine, _) = reopen(&root);
    let expected = canonical_json(engine.state()).unwrap();
    drop(engine);
    std::fs::rename(
        &config.disks[2].root,
        config.disks[2].root.with_extension("offline"),
    )
    .unwrap();
    let (engine, count) = reopen(&root);
    assert!(count >= 6);
    assert!(engine.blocked);
    assert_eq!(canonical_json(engine.state()).unwrap(), expected);
}

#[test]
fn sealed_suffix_replays_only_new_decision_and_refreshes_checkpoint() {
    let (_temp, root, _config) = setup();
    let (mut engine, _) = reopen(&root);
    engine
        .create_repository(
            &storage_contract::RepositoryName::parse("newer").unwrap(),
            "P01",
            3,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let expected = canonical_json(engine.state()).unwrap();
    drop(engine);
    let (engine, count) = reopen(&root);
    assert_eq!(count, 2);
    assert_eq!(canonical_json(engine.state()).unwrap(), expected);
    drop(engine);
    assert_eq!(reopen(&root).1, 1);
}

#[test]
fn torn_journal_tail_disables_checkpoint_replay_skip() {
    let (_temp, root, config) = setup();
    drop(reopen(&root).0);
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(log_path(&config.disks[0]))
        .unwrap();
    file.write_all(b"ZXSLO").unwrap();
    let (_, count) = reopen(&root);
    assert!(count >= 6);
}

#[test]
fn checkpoint_save_failure_is_nonfatal_and_reported_once() {
    let (_temp, root, config) = setup();
    let checkpoint = path(&config.disks[0]);
    std::fs::create_dir(&checkpoint).unwrap();
    let (mut engine, count) = reopen(&root);
    assert!(count >= 6);
    let warnings = engine.take_recovery_warnings();
    assert!(warnings.checkpoint_save.is_some());
    let consumed = engine.take_recovery_warnings();
    assert!(consumed.checkpoint_load.is_none());
    assert!(consumed.checkpoint_save.is_none());
}

#[test]
fn coverage_byte_budget_and_overflow_keep_typed_cause() {
    for initial in [MAX_COVERAGE_BYTES, usize::MAX] {
        let mut entries = Vec::new();
        let mut budget = initial;
        let error = add_entry(
            &mut entries,
            CoverageEntry {
                path: "metadata/example".into(),
                value: Covered::Absent,
            },
            &mut budget,
        )
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::CheckpointInvalid);
        assert!(matches!(
            error,
            Error::Checkpoint(CheckpointError::CoverageBytes {
                limit: MAX_COVERAGE_BYTES,
                current,
                added,
                ..
            }) if current == initial && added > 0
        ));
        assert!(entries.is_empty());
    }
}

#[test]
fn directory_budgets_have_distinct_causes_at_existing_limits() {
    let path = Path::new("metadata");
    directory_budget(path, MAX_COVERAGE_ENTRIES - 1, MAX_COVERAGE_BYTES).unwrap();
    assert!(matches!(
        directory_budget(path, MAX_COVERAGE_ENTRIES, 0),
        Err(Error::Checkpoint(CheckpointError::DirectoryEntries { .. }))
    ));
    assert!(matches!(
        directory_budget(path, 0, MAX_COVERAGE_BYTES + 1),
        Err(Error::Checkpoint(
            CheckpointError::DirectoryNameBytes { .. }
        ))
    ));
}

#[test]
fn invalid_paths_and_file_types_have_distinct_causes() {
    assert!(matches!(
        relative_parts(Path::new("../metadata")),
        Err(Error::Checkpoint(CheckpointError::InvalidPath(_)))
    ));
    let temp = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink("missing", temp.path().join("link")).unwrap();
    let error = inventory(temp.path(), Path::new("link"), &mut Vec::new(), &mut 0).unwrap_err();
    assert!(matches!(
        error,
        Error::Checkpoint(CheckpointError::FileType(_))
    ));
}

#[test]
fn non_utf8_directory_name_retains_source_and_classification() {
    use std::os::unix::ffi::OsStringExt;
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(
        temp.path().join(std::ffi::OsString::from_vec(vec![0xff])),
        b"",
    )
    .unwrap();
    let error = children(temp.path(), Path::new("")).unwrap_err();
    assert_eq!(error.code(), ErrorCode::CheckpointInvalid);
    match error {
        Error::Context { source, .. } => assert!(matches!(*source, Error::PathEncoding(_))),
        other => panic!("expected preserved path encoding source, got {other:?}"),
    }
}

#[test]
fn coverage_mismatch_reasons_distinguish_members_content_and_io() {
    let (_temp, root, config) = setup();
    let engine = reopen(&root).0;
    let envelope: Envelope = read_json(&path(&config.disks[0])).unwrap();
    let mut checkpoint = envelope.checkpoint;
    let disk = &config.disks[0];
    let entries = checkpoint.coverage.remove(&disk.disk_id).unwrap();
    assert!(matches!(
        verify_coverage(&engine, &checkpoint, &BTreeSet::new()),
        Err(Error::Checkpoint(CheckpointError::Members))
    ));
    checkpoint.coverage.insert(disk.disk_id.clone(), entries);
    let file = checkpoint.coverage[&disk.disk_id]
        .iter()
        .find(|entry| {
            entry.path.starts_with("metadata/objects/")
                && matches!(&entry.value, Covered::File { hash: Some(_), .. })
        })
        .unwrap();
    let covered = disk.root.join(&file.path);
    let bytes = std::fs::read(&covered).unwrap();
    let mut changed = bytes.clone();
    changed[0] ^= 1;
    std::fs::write(&covered, &changed).unwrap();
    assert!(matches!(
        verify_coverage(&engine, &checkpoint, &BTreeSet::new()),
        Err(Error::Checkpoint(CheckpointError::FileContent(_)))
    ));
    std::fs::write(&covered, &bytes).unwrap();
    std::fs::remove_file(&covered).unwrap();
    let error = verify_coverage(&engine, &checkpoint, &BTreeSet::new()).unwrap_err();
    assert_eq!(error.code(), ErrorCode::CheckpointInvalid);
    match error {
        Error::Context { source, .. } => {
            assert!(std::error::Error::source(source.as_ref()).is_some())
        }
        other => panic!("expected covered path I/O source, got {other:?}"),
    }
}

#[test]
fn save_rejects_unsafe_inventory_without_reading_old_checkpoint() {
    let (_temp, root, config) = setup();
    let engine = reopen(&root).0;
    let old = std::fs::read(path(&config.disks[0])).unwrap();
    // The excluded old checkpoint can be corrupt; save only inventories current dependencies.
    std::fs::write(path(&config.disks[0]), b"not json").unwrap();
    let journals = config
        .disks
        .iter()
        .map(|d| (d.disk_id.clone(), scan(d).unwrap().records))
        .collect();
    save(&engine, &journals).unwrap();
    assert_eq!(std::fs::read(path(&config.disks[0])).unwrap(), old);
    std::os::unix::fs::symlink("missing", config.disks[0].root.join("metadata/unsafe-link"))
        .unwrap();
    let error = save(&engine, &journals).unwrap_err();
    assert!(matches!(
        error,
        Error::Checkpoint(CheckpointError::FileType(_))
    ));
    assert_eq!(std::fs::read(path(&config.disks[0])).unwrap(), old);
}

#[test]
fn coverage_entry_budget_has_its_own_cause() {
    let entry = CoverageEntry {
        path: String::new(),
        value: Covered::Absent,
    };
    // Small in-memory entries reach the existing count limit without disk fixtures.
    let mut entries = vec![entry.clone(); MAX_COVERAGE_ENTRIES];
    let error = add_entry(&mut entries, entry, &mut 0).unwrap_err();
    assert!(matches!(
        error,
        Error::Checkpoint(CheckpointError::CoverageEntries {
            limit: MAX_COVERAGE_ENTRIES,
            ..
        })
    ));
    assert_eq!(entries.len(), MAX_COVERAGE_ENTRIES);
}
