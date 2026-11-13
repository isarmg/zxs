//! PREPARE-only source tests on disposable roots. No ACCEPT, live SMB or
//! power-loss claim; a successful observation still has all five obligations.
use super::*;

const JOURNAL: &str = "metadata/application-log/journal.bin";

fn journals(engine: &Engine) -> BTreeMap<PathBuf, Vec<u8>> {
    engine
        .config()
        .disks
        .iter()
        .filter(|disk| disk.metadata_member)
        .map(|disk| {
            let path = disk.root.join(JOURNAL);
            let bytes = fs::read(&path).unwrap();
            (path, bytes)
        })
        .collect()
}
fn assert_saved(saved: &BTreeMap<PathBuf, Vec<u8>>) {
    for (path, original) in saved {
        assert_eq!(
            &fs::read(path).unwrap(),
            original,
            "{} changed",
            path.display()
        );
    }
}
fn prepare_journal(
    engine: &mut Engine,
    projection: &FileHistoryMoveProjection,
    recovery: &FileHistoryMoveRecoveryInfo,
) -> FileHistoryMoveJournalPreparation {
    engine
        .prepare_file_history_move_journal(
            projection,
            recovery.digest(),
            &HistoryWindowCancellation::new(),
        )
        .unwrap()
}
fn read_journal(
    engine: &Engine,
    projection: &FileHistoryMoveProjection,
    recovery: &FileHistoryMoveRecoveryInfo,
) -> FileHistoryMoveJournalPreparation {
    engine
        .read_file_history_move_journal(
            projection,
            recovery.digest(),
            &HistoryWindowCancellation::new(),
        )
        .unwrap()
}
fn ordinary_body(fixture: &Fixture, projection: &FileHistoryMoveProjection) -> PathBuf {
    let root = target(&fixture.engine);
    let id = projection.version_mapping().values().next().unwrap();
    let version = stored_version(&root, &fixture.target.history_tree_id, id);
    installed_body(
        &named(
            &root,
            &fixture.target.history_tree_id,
            projection.target_file_id().as_str(),
        ),
        &version,
    )
}
fn assert_source(engine: &Engine, before: &State, source: &Repository, target: &Repository) {
    assert_eq!(engine.state(), before);
    let original = entry(engine, source, "source.txt");
    assert!(engine.file_deletion(&original.file_id).is_none());
    assert!(
        engine
            .namespace_tree(&target.main_branch_id)
            .unwrap()
            .entries()
            .is_empty()
    );
    assert!(engine.inspect_file_history_move_joint_choice().is_some());
}

#[test]
fn all_original_prepare_frames_are_idempotent_without_moving_either_namespace() {
    let mut fixture = Fixture::new(false);
    let (projection, recovery) = input(&mut fixture);
    prepare(&mut fixture.engine, &projection, &recovery);
    let before = fixture.engine.state().clone();
    let mut databases = unchanged(&fixture.engine);
    databases.retain(|path, _| path.file_name().unwrap() != "journal.bin");
    assert_eq!(
        read_journal(&fixture.engine, &projection, &recovery).prepared_members(),
        0
    );
    let report = prepare_journal(&mut fixture.engine, &projection, &recovery);
    assert_eq!(report.metadata_members(), 3);
    assert_eq!(report.prepared_members(), 3);
    assert_eq!(report.sequence(), before.sequence + 1);
    assert_eq!(report.operation_id(), recovery.operation_id());
    assert_eq!(report.outstanding().len(), 5);
    let written = journals(&fixture.engine);
    let repeated = prepare_journal(&mut fixture.engine, &projection, &recovery);
    assert_eq!(repeated.candidate_hash(), report.candidate_hash());
    assert_eq!(repeated.snapshot_hash(), report.snapshot_hash());
    assert_eq!(journals(&fixture.engine), written);
    assert_saved(&databases);
    assert_source(&fixture.engine, &before, &fixture.source, &fixture.target);
    assert_eq!(
        fixture
            .engine
            .create_workspace(&fixture.source.main_branch_id, "admin")
            .unwrap_err()
            .code(),
        ErrorCode::RecoveryRequired
    );
}

#[test]
fn complete_prepare_cold_replay_keeps_the_original_slot_and_ordinary_body_inode() {
    let mut fixture = Fixture::new(false);
    let (projection, recovery) = input(&mut fixture);
    prepare(&mut fixture.engine, &projection, &recovery);
    let report = prepare_journal(&mut fixture.engine, &projection, &recovery);
    let body = ordinary_body(&fixture, &projection);
    let original = fs::read(&body).unwrap();
    let inode = fs::metadata(&body).unwrap().ino();
    let before = fixture.engine.state().clone();
    let saved = unchanged(&fixture.engine);
    let Fixture {
        temp,
        engine,
        source,
        target,
        ..
    } = fixture;
    drop(engine);
    let mut engine = Engine::open(&temp.path().join("system")).unwrap();
    assert_saved(&saved);
    assert_source(&engine, &before, &source, &target);
    let resumed = engine
        .resume_file_history_move_recovery(
            recovery.operation_id(),
            recovery.digest(),
            "admin",
            &HistoryWindowCancellation::new(),
        )
        .unwrap();
    let observed = read_journal(&engine, &resumed, &recovery);
    assert_eq!(observed.prepared_members(), 3);
    assert_eq!(observed.sequence(), report.sequence());
    assert_eq!(observed.candidate_hash(), report.candidate_hash());
    assert_eq!(observed.snapshot_hash(), report.snapshot_hash());
    assert_eq!(
        prepare_journal(&mut engine, &resumed, &recovery).sequence(),
        report.sequence()
    );
    assert_eq!(fs::read(body.clone()).unwrap(), original);
    assert_eq!(fs::metadata(body).unwrap().ino(), inode);
    assert_saved(&saved);
    assert_source(&engine, &before, &source, &target);
}

#[test]
fn one_retained_prepare_is_observed_not_filled_until_explicit_original_resume() {
    let mut fixture = Fixture::new(false);
    let (projection, recovery) = input(&mut fixture);
    prepare(&mut fixture.engine, &projection, &recovery);
    let prefixes = journals(&fixture.engine);
    let report = prepare_journal(&mut fixture.engine, &projection, &recovery);
    let complete = journals(&fixture.engine);
    let before = fixture.engine.state().clone();
    let Fixture {
        temp,
        engine,
        source,
        target,
        ..
    } = fixture;
    drop(engine);
    // Only disposable test roots: model two members that never appended PREPARE.
    for (path, bytes) in prefixes.iter().skip(1) {
        fs::write(path, bytes).unwrap();
    }
    let partial: BTreeMap<_, _> = prefixes
        .keys()
        .map(|path| (path.clone(), fs::read(path).unwrap()))
        .collect();
    let mut engine = Engine::open(&temp.path().join("system")).unwrap();
    assert_saved(&partial);
    assert_source(&engine, &before, &source, &target);
    let resumed = engine
        .resume_file_history_move_recovery(
            recovery.operation_id(),
            recovery.digest(),
            "admin",
            &HistoryWindowCancellation::new(),
        )
        .unwrap();
    let observed = read_journal(&engine, &resumed, &recovery);
    assert_eq!(observed.prepared_members(), 1);
    assert_eq!(observed.candidate_hash(), report.candidate_hash());
    assert_saved(&partial);
    let completed = prepare_journal(&mut engine, &resumed, &recovery);
    assert_eq!(completed.prepared_members(), 3);
    assert_eq!(completed.sequence(), report.sequence());
    assert_saved(&complete);
    assert_source(&engine, &before, &source, &target);
}

#[test]
fn missing_prepared_body_or_original_envelope_blocks_cold_open_without_recreation() {
    for remove_body in [false, true] {
        let mut fixture = Fixture::new(false);
        let (projection, recovery) = input(&mut fixture);
        prepare(&mut fixture.engine, &projection, &recovery);
        prepare_journal(&mut fixture.engine, &projection, &recovery);
        let missing = if remove_body {
            ordinary_body(&fixture, &projection)
        } else {
            fixture
                .engine
                .config()
                .disks
                .iter()
                .find(|disk| disk.metadata_member)
                .unwrap()
                .root
                .join("file-history-move-recovery/preparation.json")
        };
        let saved = unchanged(&fixture.engine);
        let root = target(&fixture.engine);
        let id = projection.version_mapping().values().next().unwrap();
        let version = stored_version(&root, &fixture.target.history_tree_id, id);
        let staged = root
            .join("file-history-move-target/versions")
            .join(id)
            .join("content")
            .join(version.creation_name);
        let staged_bytes = fs::read(&staged).unwrap();
        let Fixture { temp, engine, .. } = fixture;
        drop(engine);
        fs::remove_file(&missing).unwrap();
        assert!(Engine::open(&temp.path().join("system")).is_err());
        assert!(!missing.exists());
        assert_saved(&saved);
        assert_eq!(fs::read(staged).unwrap(), staged_bytes);
    }
}

#[test]
fn torn_prepare_tail_is_repaired_only_after_the_complete_original_preparation() {
    let mut fixture = Fixture::new(false);
    let (projection, recovery) = input(&mut fixture);
    prepare(&mut fixture.engine, &projection, &recovery);
    prepare_journal(&mut fixture.engine, &projection, &recovery);
    let saved = journals(&fixture.engine);
    let path = saved.keys().next().unwrap().clone();
    let mut torn = saved[&path].clone();
    torn.push(0x5a);
    let Fixture { temp, engine, .. } = fixture;
    drop(engine);
    fs::write(&path, &torn).unwrap();
    let engine = Engine::open_for_recovery(&temp.path().join("system")).unwrap();
    assert_eq!(fs::read(&path).unwrap(), torn);
    drop(engine);
    let engine = Engine::open(&temp.path().join("system")).unwrap();
    assert!(
        !engine
            .state()
            .operations
            .contains_key(&recovery.operation_id())
    );
    assert_saved(&saved);
}

#[test]
fn replaced_equal_byte_journal_is_not_the_held_original_writer() {
    let mut fixture = Fixture::new(false);
    let (projection, recovery) = input(&mut fixture);
    prepare(&mut fixture.engine, &projection, &recovery);
    prepare_journal(&mut fixture.engine, &projection, &recovery);
    let (path, bytes) = journals(&fixture.engine).into_iter().next().unwrap();
    let old = path.with_file_name("retained-original-journal.bin");
    fs::rename(&path, &old).unwrap();
    fs::write(&path, &bytes).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(
        fixture
            .engine
            .read_file_history_move_journal(
                &projection,
                recovery.digest(),
                &HistoryWindowCancellation::new()
            )
            .is_err()
    );
    assert!(
        fixture
            .engine
            .prepare_file_history_move_journal(
                &projection,
                recovery.digest(),
                &HistoryWindowCancellation::new()
            )
            .is_err()
    );
    assert_eq!(fs::read(path).unwrap(), bytes);
    assert_eq!(fs::read(old).unwrap(), bytes);
}

#[test]
fn cancellation_before_prepare_keeps_the_original_slot_and_empty_tail() {
    let mut fixture = Fixture::new(false);
    let (projection, recovery) = input(&mut fixture);
    prepare(&mut fixture.engine, &projection, &recovery);
    let before = fixture.engine.state().clone();
    let saved = unchanged(&fixture.engine);
    let cancel = HistoryWindowCancellation::new();
    cancel.cancel();
    assert_eq!(
        fixture
            .engine
            .prepare_file_history_move_journal(&projection, recovery.digest(), &cancel)
            .unwrap_err()
            .code(),
        ErrorCode::OperationAborted
    );
    assert_saved(&saved);
    let report = read_journal(&fixture.engine, &projection, &recovery);
    assert_eq!(report.prepared_members(), 0);
    assert_eq!(report.sequence(), before.sequence + 1);
    assert_source(&fixture.engine, &before, &fixture.source, &fixture.target);
}

#[test]
fn accepted_duplicate_foreign_or_later_frames_cannot_become_move_recovery_authority() {
    for (kind, later, other) in [
        ("ACCEPT", false, false),
        ("FINALIZE", false, false),
        ("PUBLICATION", false, false),
        ("PREPARE", false, false),
        ("PREPARE", true, false),
        ("PREPARE", false, true),
    ] {
        let mut fixture = Fixture::new(false);
        let (projection, recovery) = input(&mut fixture);
        prepare(&mut fixture.engine, &projection, &recovery);
        let prefixes = journals(&fixture.engine);
        let report = prepare_journal(&mut fixture.engine, &projection, &recovery);
        let mut saved = unchanged(&fixture.engine);
        let path = prefixes.keys().next().unwrap();
        let retained = &saved[path];
        let tail = &retained[prefixes[path].len()..];
        // Construct a checksummed real wire frame, not a runtime fault hook.
        let mut record: json_codec::Value =
            storage_contract::decode_strict(&tail[12..tail.len() - 32]).unwrap();
        record["record_type"] = json_codec::json!(kind);
        record["sequence"] = json_codec::json!((report.sequence() + u64::from(later)).to_string());
        record["previous_frame_hash"] = json_codec::json!(
            blake3::Hash::from_bytes(tail[tail.len() - 32..].try_into().unwrap())
                .to_hex()
                .to_string()
        );
        if other {
            record["candidate_hash"] = json_codec::json!("0".repeat(64));
        }
        let payload = storage_contract::canonical_json(&record).unwrap();
        let mut frame = b"ZXSLOG\0\0".to_vec();
        frame.extend(u32::try_from(payload.len()).unwrap().to_le_bytes());
        frame.extend(payload);
        let digest = blake3::hash(&frame);
        frame.extend(digest.as_bytes());
        let Fixture { temp, engine, .. } = fixture;
        drop(engine);
        saved.get_mut(path).unwrap().extend(frame);
        fs::write(path, &saved[path]).unwrap();
        assert!(
            Engine::open(&temp.path().join("system")).is_err(),
            "accepted {kind}/{later}/{other}"
        );
        assert_saved(&saved);
    }
}
