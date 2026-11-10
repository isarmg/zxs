//! Unit-only private choice fixtures exercise denial, never FULL proof or
//! permission to select/publish a production move. These tests are unrun.
use super::*;

fn selected(engine: &Engine) -> Choice {
    let mut participants: Vec<_> = engine
        .registered_disks()
        .map(|disk| disk.identity())
        .collect();
    participants.sort_by(|left, right| left.disk_id.cmp(&right.disk_id));
    let record = Record {
        format: FORMAT.into(),
        operation_id: OperationId::generate().unwrap(),
        principal: "admin".into(),
        pool_id: engine.config.pool_id.clone(),
        configuration_hash: object_hash("file-history-move-config", &engine.config).unwrap(),
        source_snapshot: engine.snapshot.clone().unwrap(),
        source_sequence: engine.state.sequence,
        decision_slot: engine.next_slot,
        recovery_digest: "0".repeat(64),
        decision_inputs_digest: "1".repeat(64),
        native_digest: "2".repeat(64),
        scoped_digest: "3".repeat(64),
        metadata_members: participants.clone(),
        participants,
    };
    let choice = Choice::decode(
        &engine.config,
        &encode_metadata(&record, MAX_BYTES).unwrap(),
    )
    .unwrap();
    choice.require_baseline(engine).unwrap();
    choice
}

fn engine() -> (tempfile::TempDir, Engine) {
    let temp = tempfile::tempdir().unwrap();
    let config =
        Config::development((0..3).map(|n| temp.path().join(format!("disk-{n}")))).unwrap();
    let mut engine = Engine::initialize(&temp.path().join("system"), config).unwrap();
    engine
        .create_disk_root("P01", "admin", OperationId::generate().unwrap())
        .unwrap();
    (temp, engine)
}

#[test]
fn detached_ordinary_work_captured_before_selection_cannot_cross_the_choice_fence() {
    let (_temp, mut engine) = engine();
    let value = json_codec::json!({"auth": "must remain unaccepted"});
    let digest = object_hash("auth-state", &value).unwrap();
    let plan = engine
        .planning_context()
        .plan_new_mutation(
            "auth.control-state",
            "admin",
            OperationId::generate().unwrap(),
            digest.clone(),
            |_, next, _| {
                next.control_auth = Some(value);
                Ok(json_codec::json!({"auth_state_digest": digest}))
            },
        )
        .unwrap();
    let crate::engine::MutationPlan::Prepared {
        next,
        prepared,
        result,
    } = plan
    else {
        panic!("invariant: a new operation is prepared");
    };
    let detached = engine
        .prepare_publication(*next, prepared, result, MAX_METADATA)
        .unwrap();
    let before = engine.state().clone();
    let slot = engine.next_slot;
    engine.history_move_choice = Some(selected(&engine));
    assert_eq!(
        engine.decide_publication(detached).unwrap_err().code(),
        ErrorCode::PreparationStale
    );
    assert_eq!(
        engine
            .planning_context()
            .require_write()
            .unwrap_err()
            .code(),
        ErrorCode::RecoveryRequired
    );
    assert_eq!(
        engine.require_write().unwrap_err().code(),
        ErrorCode::RecoveryRequired
    );
    engine.require_move_source_read().unwrap();
    assert_eq!(engine.state(), &before);
    assert_eq!(engine.next_slot, slot);
}

#[test]
fn persisted_choice_parser_rejects_wrong_slot_membership_and_noncanonical_bytes() {
    let (_temp, engine) = engine();
    let choice = selected(&engine);
    let mut wrong = choice.record.clone();
    wrong.decision_slot += 1;
    assert!(Choice::decode(&engine.config, &encode_metadata(&wrong, MAX_BYTES).unwrap()).is_err());
    let mut wrong = choice.record.clone();
    wrong.metadata_members.pop();
    assert!(Choice::decode(&engine.config, &encode_metadata(&wrong, MAX_BYTES).unwrap()).is_err());
    let mut whitespace = choice.bytes.clone();
    whitespace.push(b'\n');
    assert!(Choice::decode(&engine.config, &whitespace).is_err());
}

#[test]
fn absent_move_materials_do_not_require_an_active_generation_seal() {
    for pending in [true, false] {
        let (_temp, mut engine) = engine();
        let disk = engine.config.disks[0].clone();
        let path = disk.root.join("disk.json");
        let bytes = if pending {
            br#"{"format":"membership-pending","control_archive_root":"unpromoted"}"#.to_vec()
        } else {
            let mut previous = engine.disk_resources.get(&disk).unwrap().seal.clone();
            previous.format = DiskSealFormat::MembershipGeneration {
                authority_hash: "previous-generation".into(),
            };
            encode_metadata(&previous, MAX_METADATA).unwrap()
        };
        std::fs::write(&path, &bytes).unwrap();
        engine.discover_file_history_move_choice().unwrap();
        assert!(engine.history_move_choice.is_none());
        assert!(engine.history_move_intent.is_none());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_eq!(
            validate_seal(&engine.config, &disk).unwrap_err().code(),
            ErrorCode::DiskIdentityMismatch
        );
    }
}

#[test]
fn an_observed_move_choice_still_requires_the_exact_active_seal() {
    let (_temp, mut engine) = engine();
    let choice = selected(&engine);
    let disk = engine.config.disks[0].clone();
    let root = &engine.disk_resources.get(&disk).unwrap().root;
    let stage = disk.root.join(DIRECTORY);
    root.private_dir(&stage).unwrap();
    root.write_atomic(&stage.join(NAME), &choice.bytes, false)
        .unwrap();
    engine.discover_file_history_move_choice().unwrap();
    assert!(engine.history_move_choice.as_ref() == Some(&choice));
    let mut changed = engine.disk_resources.get(&disk).unwrap().seal.clone();
    changed.inode += 1;
    let bytes = encode_metadata(&changed, MAX_METADATA).unwrap();
    std::fs::write(disk.root.join("disk.json"), &bytes).unwrap();
    assert_eq!(
        engine
            .discover_file_history_move_choice()
            .unwrap_err()
            .code(),
        ErrorCode::DiskIdentityMismatch
    );
    assert_eq!(std::fs::read(stage.join(NAME)).unwrap(), choice.bytes);
    assert_eq!(std::fs::read(disk.root.join("disk.json")).unwrap(), bytes);
}

#[test]
fn intent_discovery_rejects_every_present_incomplete_slot() {
    for kind in ["empty", "file", "symlink", "corrupt"] {
        let (_temp, mut engine) = engine();
        let disk = engine.config.disks[0].clone();
        let root = &engine.disk_resources.get(&disk).unwrap().root;
        let slot = disk.root.join("file-history-move-recovery");
        match kind {
            "empty" => root.private_dir(&slot).unwrap(),
            "file" => std::fs::write(&slot, b"unchanged").unwrap(),
            "symlink" => std::os::unix::fs::symlink("missing", &slot).unwrap(),
            "corrupt" => {
                root.private_dir(&slot).unwrap();
                std::fs::write(slot.join("preparation.json"), b"{}").unwrap();
            }
            _ => unreachable!(),
        }
        assert!(engine.discover_move_intent().is_err(), "{kind}");
        assert!(engine.history_move_intent.is_none());
        match kind {
            "empty" => assert_eq!(std::fs::read_dir(&slot).unwrap().count(), 0),
            "file" => assert_eq!(std::fs::read(&slot).unwrap(), b"unchanged"),
            "symlink" => assert_eq!(
                std::fs::read_link(&slot).unwrap(),
                std::path::Path::new("missing")
            ),
            "corrupt" => {
                assert_eq!(std::fs::read(slot.join("preparation.json")).unwrap(), b"{}");
            }
            _ => unreachable!(),
        }
    }
}

#[test]
fn absent_move_materials_still_reject_replaced_or_quiesced_roots() {
    for replaced in [true, false] {
        let (temp, mut engine) = engine();
        let disk = engine.config.disks[0].clone();
        if replaced {
            std::fs::rename(&disk.root, temp.path().join("parked")).unwrap();
            private_dir(&disk.root).unwrap();
        } else {
            let root = &engine.disk_resources.get(&disk).unwrap().root;
            drop(root.io_binding().unwrap().begin_quiesce().unwrap());
        }
        assert!(engine.discover_file_history_move_choice().is_err());
        assert!(engine.discover_move_intent().is_err());
        assert!(engine.history_move_choice.is_none());
        assert!(engine.history_move_intent.is_none());
    }
}
