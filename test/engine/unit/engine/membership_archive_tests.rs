use super::*;
use std::fs;

fn fixture() -> (tempfile::TempDir, Config, Engine) {
    let temp = tempfile::tempdir().unwrap();
    let config =
        Config::development((0..3).map(|n| temp.path().join(format!("disk-{n}")))).unwrap();
    let mut engine = Engine::initialize(&temp.path().join("system"), config.clone()).unwrap();
    for name in ["first", "second"] {
        engine
            .create_repository(
                &RepositoryName::parse(name).unwrap(),
                "P01",
                1,
                "admin",
                OperationId::generate().unwrap(),
            )
            .unwrap();
    }
    let mut target = config.clone();
    target.disks[2].metadata_member = false;
    engine
        .begin_membership_handoff(target, "admin", OperationId::generate().unwrap())
        .unwrap();
    (temp, config, engine)
}
fn bytes(config: &Config) -> BTreeMap<std::path::PathBuf, Vec<u8>> {
    fn visit(path: &std::path::Path, out: &mut BTreeMap<std::path::PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(&path, out);
            } else {
                out.insert(path.clone(), fs::read(path).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    for disk in &config.disks {
        visit(&disk.root, &mut out);
    }
    out
}

#[test]
fn membership_archive_inspection_preserves_all_source_bytes_and_original_identities() {
    let (_temp, config, engine) = fixture();
    let before = bytes(&config);
    let inspection = engine.inspect_membership_metadata_archive().unwrap();
    assert_eq!(inspection.original_members, 3);
    assert_eq!(inspection.published_candidates, 2);
    assert!(inspection.immutable_metadata_objects > 10);
    assert!(inspection.immutable_metadata_bytes > 0);
    assert_eq!(inspection.journals.len(), 3);
    for journal in &inspection.journals {
        let disk = config
            .disks
            .iter()
            .find(|disk| disk.disk_id == journal.disk_id)
            .unwrap();
        let original = fs::read(log_path(disk)).unwrap();
        assert_eq!(journal.slot_id, disk.slot_id);
        assert_eq!(journal.binding_generation, disk.binding_generation);
        assert_eq!(journal.bytes, original.len() as u64);
        assert_eq!(
            journal.content_hash,
            blake3::hash(&original).to_hex().as_str()
        );
        assert_eq!(
            journal.tail_frame_hash,
            scan(disk).unwrap().records.last().unwrap().1
        );
    }
    assert_eq!(bytes(&config), before);
    assert_eq!(
        engine.inspect_membership_metadata_archive().unwrap(),
        inspection
    );
}

#[test]
fn membership_archive_inspection_checks_old_snapshot_and_independent_finalize() {
    for damage in ["snapshot", "finalize", "control-auth", "history"] {
        let (_temp, config, engine) = fixture();
        let records = scan(&config.disks[0]).unwrap().records;
        let first = records
            .iter()
            .find(|(record, _)| record.record_type == RecordType::Publication)
            .unwrap();
        let candidate: Candidate = verified_object_at(
            &engine.disk_resources,
            &engine.members(),
            "candidate",
            &first.0.candidate_hash,
        )
        .unwrap();
        let snapshot: Snapshot = verified_object_at(
            &engine.disk_resources,
            &engine.members(),
            "snapshot",
            &candidate.snapshot_hash,
        )
        .unwrap();
        let publication: Publication = verified_object_at(
            &engine.disk_resources,
            &engine.members(),
            "publication",
            &first.0.object_hash,
        )
        .unwrap();
        let (kind, hash) = match damage {
            "snapshot" => ("snapshot", candidate.snapshot_hash.clone()),
            "finalize" => (
                "finalize",
                object_hash("finalize", &publication.finalize).unwrap(),
            ),
            "control-auth" => (
                "state-control-auth",
                snapshot.control_auth.hash().to_owned(),
            ),
            _ => (
                candidate.history_roots.kind().domain(),
                candidate.history_roots.hash().to_owned(),
            ),
        };
        for disk in &config.disks {
            fs::write(object_path(disk, kind, &hash), b"{").unwrap();
        }
        let before = bytes(&config);
        assert!(
            engine.inspect_membership_metadata_archive().is_err(),
            "{damage}"
        );
        assert_eq!(bytes(&config), before, "{damage}");
    }
}

#[test]
fn membership_archive_inspection_accepts_healthy_object_replica_without_repair() {
    let (_temp, config, engine) = fixture();
    let expected = engine.inspect_membership_metadata_archive().unwrap();
    let records = scan(&config.disks[0]).unwrap().records;
    let first = records
        .iter()
        .find(|(record, _)| record.record_type == RecordType::Publication)
        .unwrap();
    fs::write(
        object_path(&config.disks[0], "candidate", &first.0.candidate_hash),
        b"{",
    )
    .unwrap();
    let before = bytes(&config);
    assert_eq!(
        engine.inspect_membership_metadata_archive().unwrap(),
        expected
    );
    assert_eq!(bytes(&config), before);
}

#[test]
fn membership_archive_inspection_rejects_changed_journal_and_partial_fence() {
    let (temp, config, engine) = fixture();
    let disk = &config.disks[0];
    let mut journal = fs::read(log_path(disk)).unwrap();
    journal.pop();
    fs::write(log_path(disk), journal).unwrap();
    let before = bytes(&config);
    assert!(engine.inspect_membership_metadata_archive().is_err());
    assert_eq!(bytes(&config), before);
    drop(engine);
    drop(temp);
    let temp = tempfile::tempdir().unwrap();
    let config =
        Config::development((0..3).map(|n| temp.path().join(format!("disk-{n}")))).unwrap();
    let mut engine = Engine::initialize(&temp.path().join("system"), config.clone()).unwrap();
    let mut target = config;
    target.disks[2].metadata_member = false;
    engine.inject_fault_once("after_membership_fence_2");
    assert!(
        engine
            .begin_membership_handoff(target, "admin", OperationId::generate().unwrap())
            .is_err()
    );
    assert_eq!(
        engine
            .inspect_membership_metadata_archive()
            .unwrap_err()
            .code(),
        ErrorCode::RecoveryRequired
    );
}

#[test]
fn membership_archive_prefix_rejects_same_sequence_with_different_candidate_bytes() {
    let (_temp, config, engine) = fixture();
    let records = scan(&config.disks[0]).unwrap().records;
    let first = records
        .iter()
        .find(|(record, _)| record.record_type == RecordType::Publication)
        .unwrap();
    let candidate: Candidate = verified_object_at(
        &engine.disk_resources,
        &engine.members(),
        "candidate",
        &first.0.candidate_hash,
    )
    .unwrap();
    let publications = BTreeMap::from([(candidate.sequence, first.0.candidate_hash.clone())]);
    let prefix = VerifiedFrozenPrefix {
        config: &engine.config,
        publications: &publications,
    };
    prefix.verify_candidate(&candidate).unwrap();
    let mut changed = candidate.clone();
    changed.snapshot_hash = "0".repeat(64);
    assert_eq!(
        prefix.verify_candidate(&changed).unwrap_err().code(),
        ErrorCode::LogFork
    );
    changed = candidate;
    changed.previous_snapshot = Some("0".repeat(64));
    assert_eq!(
        prefix.verify_candidate(&changed).unwrap_err().code(),
        ErrorCode::LogFork
    );
}

#[test]
fn membership_archive_stage_references_reject_unrelated_objects() {
    let (_temp, config, engine) = fixture();
    let records = scan(&config.disks[0]).unwrap().records;
    let mut source = LiveSource {
        engine: &engine,
        members: engine.members(),
        destination: None,
        seals: BTreeMap::new(),
        copied: BTreeSet::new(),
    };
    let mut inspection = Inspection {
        config: &engine.config,
        source: &mut source,
        members: engine.members(),
        objects: BTreeSet::new(),
        candidates: BTreeSet::new(),
        metadata_bytes: 0,
    };
    let mut prepare = records
        .iter()
        .find(|(record, _)| record.record_type == RecordType::Prepare)
        .unwrap()
        .0
        .clone();
    inspection.decision_reference(&prepare).unwrap();
    prepare.object_hash = "0".repeat(64);
    assert_eq!(
        inspection.decision_reference(&prepare).unwrap_err().code(),
        ErrorCode::PublicationInvalid
    );
    let mut accept = records
        .iter()
        .find(|(record, _)| record.record_type == RecordType::Accept)
        .unwrap()
        .0
        .clone();
    inspection.decision_reference(&accept).unwrap();
    accept.object_hash = "0".repeat(64);
    assert_eq!(
        inspection.decision_reference(&accept).unwrap_err().code(),
        ErrorCode::PublicationInvalid
    );
    let mut finalize_record = records
        .iter()
        .find(|(record, _)| record.record_type == RecordType::Finalize)
        .unwrap()
        .0
        .clone();
    inspection.decision_reference(&finalize_record).unwrap();
    let mut finalize: Finalize = verified_object_at(
        &engine.disk_resources,
        &engine.members(),
        "finalize",
        &finalize_record.object_hash,
    )
    .unwrap();
    finalize.candidate_hash = "0".repeat(64);
    for disk in &config.disks {
        finalize_record.object_hash = store_object(disk, "finalize", &finalize).unwrap();
    }
    let before = bytes(&config);
    assert_eq!(
        inspection
            .decision_reference(&finalize_record)
            .unwrap_err()
            .code(),
        ErrorCode::PublicationInvalid
    );
    assert_eq!(bytes(&config), before);
}
