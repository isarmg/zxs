//! Native move success and interruption scenarios on disposable roots. These
//! sources are not an executed result or physical power-loss / SMB certificate.
use storage_engine as engine_api;
#[path = "../../support/container_tags.rs"]
mod container_tags_fixture;
use super::*;
use std::fs;
use storage_contract::file_relocation::OBJECT_MOVE_KIND;

fn path_request(fixture: &Fixture) -> FileHistoryMovePathRequest {
    FileHistoryMovePathRequest {
        source_repository_id: fixture.source.repository_id.clone(),
        source_path: "source.txt".into(),
        target_repository_id: fixture.target.repository_id.clone(),
        target_name: "moved.txt".into(),
    }
}
fn assert_visible(
    engine: &Engine,
    source: &Repository,
    target: &Repository,
    old: &Entry,
    operation: OperationId,
) {
    assert!(
        engine
            .namespace_tree(&source.main_branch_id)
            .unwrap()
            .get("source.txt")
            .is_none()
    );
    let moved = entry(engine, target, "moved.txt");
    assert_ne!(moved.file_id, old.file_id);
    let deletion = engine.file_deletion(&old.file_id).unwrap();
    assert_eq!(deletion.operation_id, operation);
    let receipt = &engine.state().operations[&operation];
    assert_eq!(receipt.kind, OBJECT_MOVE_KIND);
    assert_eq!(receipt.commit_state, CommitState::Published);
    assert_eq!(
        receipt.protection_at_publish,
        ProtectionAtPublish::TargetMet
    );
    assert_eq!(receipt.current_health, CurrentHealth::Healthy);
    assert_eq!(engine.state().operation_principals[&operation], "admin");
    assert_eq!(
        engine
            .state()
            .events
            .iter()
            .filter(|event| event.operation_id == operation)
            .count(),
        1
    );
    assert!(
        !engine
            .state()
            .operations
            .values()
            .any(|result| result.sequence == receipt.sequence && result.kind == "file.delete")
    );
    assert!(engine.inspect_file_history_move_joint_choice().is_none());
    assert!(
        engine
            .inspect_file_history_move_recovery()
            .unwrap()
            .is_none()
    );
    assert!(
        matches!(engine.index_health(), IndexHealth::Ready { sequence } if sequence == engine.state().sequence)
    );
    for disk in engine.registered_disks() {
        assert_eq!(
            engine
                .disk_catalog_observation(disk.slot_id)
                .unwrap()
                .snapshot
                .source_sequence,
            receipt.sequence
        );
        assert_eq!(
            engine
                .disk_history_observation(disk.slot_id)
                .unwrap()
                .snapshot
                .header
                .through_sequence,
            receipt.sequence
        );
    }
}

#[test]
fn native_move_publishes_all_retained_bodies_labels_branches_and_three_scopes_together() {
    let mut fixture = Fixture::new(false);
    let old = entry(&fixture.engine, &fixture.source, "source.txt");
    let versions = fixture.engine.file_history(&old.file_id).unwrap();
    fixture
        .engine
        .create_file_branch(
            &old.file_id,
            &versions[0].version_id,
            &BranchName::parse("retained").unwrap(),
            "admin",
            op(),
        )
        .unwrap();
    let context = fixture
        .engine
        .file_context(&fixture.source.repository_id, "source.txt")
        .unwrap();
    let node = fixture.engine.file_branch_head(&context.branch_id).unwrap();
    container_tags_fixture::edit(&mut fixture.engine, &old.file_id, &["move-label".into()], &[], "admin", op())
        .unwrap();
    fixture
        .engine
        .set_file_note(
            &old.file_id,
            None,
            &Note::parse("retained note".into()).unwrap(),
            "admin",
            op(),
        )
        .unwrap();
    let request = path_request(&fixture);
    let operation = op();
    let receipt = fixture
        .engine
        .move_current_file(&request, "admin", operation)
        .unwrap();
    assert_visible(
        &fixture.engine,
        &fixture.source,
        &fixture.target,
        &old,
        operation,
    );
    let moved = entry(&fixture.engine, &fixture.target, "moved.txt");
    assert_independent_graph(&fixture.engine, &fixture.target, &moved.file_id);
    assert_eq!(
        fixture.engine.state().file_notes[&moved.file_id],
        fixture.engine.state().file_notes[&old.file_id]
    );
    let moved_context = fixture
        .engine
        .file_context(&fixture.target.repository_id, "moved.txt")
        .unwrap();
    assert!(
        container_tags_fixture::tags(&fixture.engine, &moved.file_id)
            .contains("move-label")
    );
    let copies = fixture.engine.file_history(&moved.file_id).unwrap();
    assert_eq!(copies.len(), versions.len());
    for old_version in &versions {
        let mapped = receipt.result["version_mapping"][&old_version.version_id]
            .as_str()
            .unwrap();
        assert_eq!(
            bytes(&fixture.engine, mapped),
            bytes(&fixture.engine, &old_version.version_id)
        );
        assert_ne!(
            fixture
                .engine
                .open_content(mapped)
                .unwrap()
                .file
                .metadata()
                .unwrap()
                .ino(),
            fixture
                .engine
                .open_content(&old_version.version_id)
                .unwrap()
                .file
                .metadata()
                .unwrap()
                .ino()
        );
    }
    let Fixture {
        temp,
        engine,
        source,
        target,
    } = fixture;
    drop(engine);
    let mut engine = Engine::open(&temp.path().join("system")).unwrap();
    assert_visible(&engine, &source, &target, &old, operation);
    assert_eq!(
        engine
            .move_current_file(&request, "admin", operation)
            .unwrap(),
        receipt
    );
    engine
        .file_namespace(
            &target.repository_id,
            NamespaceOperation::parse(
                NamespaceOperationKind::Rename,
                "moved.txt",
                Some("later.txt"),
            )
            .unwrap(),
            "admin",
            op(),
        )
        .unwrap();
    let sequence = engine.state().sequence;
    assert_eq!(
        engine
            .move_current_file(&request, "admin", operation)
            .unwrap(),
        receipt
    );
    assert_eq!(engine.state().sequence, sequence);
    assert_eq!(
        engine
            .move_current_file(&request, "other-admin", operation)
            .unwrap_err()
            .code(),
        ErrorCode::IdempotencyKeyReused
    );
    let changed = FileHistoryMovePathRequest {
        target_name: "different.txt".into(),
        ..request
    };
    assert_eq!(
        engine
            .move_current_file(&changed, "admin", operation)
            .unwrap_err()
            .code(),
        ErrorCode::IdempotencyKeyReused
    );
}

#[test]
fn completed_move_retires_its_binding_and_a_later_move_keeps_both_receipts() {
    let mut fixture = Fixture::new(false);
    let request = path_request(&fixture);
    let first = op();
    let original = fixture
        .engine
        .move_current_file(&request, "admin", first)
        .unwrap();
    let third = fixture
        .engine
        .create_disk_root("D05", "admin", op())
        .unwrap();
    let next = FileHistoryMovePathRequest {
        source_repository_id: fixture.target.repository_id.clone(),
        source_path: "moved.txt".into(),
        target_repository_id: third.repository_id.clone(),
        target_name: "twice.txt".into(),
    };
    let second = op();
    fixture
        .engine
        .move_current_file(&next, "admin", second)
        .unwrap();
    assert_eq!(
        bytes(
            &fixture.engine,
            &entry(&fixture.engine, &third, "twice.txt").version_id
        ),
        b"new full body"
    );
    assert_eq!(
        fixture
            .engine
            .move_current_file(&request, "admin", first)
            .unwrap(),
        original
    );
    for operation in [first, second] {
        assert!(
            fixture
                .temp
                .path()
                .join("system/move-completion")
                .join(format!("{operation}.json"))
                .is_file()
        );
        assert_eq!(
            fixture.engine.state().operations[&operation].kind,
            OBJECT_MOVE_KIND
        );
    }
}

#[test]
fn native_move_rejects_a_name_collision_without_a_new_intent_or_source_deletion() {
    let mut fixture = Fixture::new(false);
    save(
        &mut fixture.engine,
        &fixture.target,
        "moved.txt",
        b"keep target",
    );
    let request = path_request(&fixture);
    let before = fixture.engine.state().clone();
    assert_eq!(
        fixture
            .engine
            .move_current_file(&request, "admin", op())
            .unwrap_err()
            .code(),
        ErrorCode::NameCollision
    );
    assert_eq!(fixture.engine.state(), &before);
    assert_eq!(
        bytes(
            &fixture.engine,
            &entry(&fixture.engine, &fixture.target, "moved.txt").version_id,
        ),
        b"keep target"
    );
    assert!(
        !fixture
            .temp
            .path()
            .join("system/move-completion.json")
            .exists()
    );
    assert!(
        fixture
            .engine
            .inspect_file_history_move_recovery()
            .unwrap()
            .is_none()
    );
}

#[test]
fn native_move_keeps_noncolliding_target_files_in_an_activated_current_generation() {
    let mut fixture = Fixture::new(true);
    let retired = fixture.engine.config().disks[4].slot_id.clone();
    let assert_retired = |engine: &Engine| {
        assert_eq!(engine.registered_disks().count(), 4);
        assert!(
            engine
                .registered_disks()
                .all(|disk| disk.slot_id != retired)
        );
        assert_eq!(
            engine
                .disk_catalog_observation(&retired)
                .err()
                .unwrap()
                .code(),
            ErrorCode::DiskUnavailable
        );
        assert_eq!(
            engine
                .disk_history_observation(&retired)
                .err()
                .unwrap()
                .code(),
            ErrorCode::DiskUnavailable
        );
    };
    assert_retired(&fixture.engine);
    save(
        &mut fixture.engine,
        &fixture.target,
        "existing.txt",
        b"keep target",
    );
    let existing = entry(&fixture.engine, &fixture.target, "existing.txt");
    let old = entry(&fixture.engine, &fixture.source, "source.txt");
    let request = path_request(&fixture);
    let operation = op();
    let receipt = fixture
        .engine
        .move_current_file(&request, "admin", operation)
        .unwrap();
    assert_visible(
        &fixture.engine,
        &fixture.source,
        &fixture.target,
        &old,
        operation,
    );
    assert_retired(&fixture.engine);
    assert_eq!(
        entry(&fixture.engine, &fixture.target, "existing.txt"),
        existing
    );
    assert_eq!(bytes(&fixture.engine, &existing.version_id), b"keep target");
    let Fixture {
        temp,
        engine,
        source,
        target,
    } = fixture;
    drop(engine);
    let mut engine = Engine::open(&temp.path().join("system")).unwrap();
    assert_visible(&engine, &source, &target, &old, operation);
    assert_retired(&engine);
    assert_eq!(entry(&engine, &target, "existing.txt"), existing);
    assert_eq!(bytes(&engine, &existing.version_id), b"keep target");
    assert_eq!(
        engine
            .move_current_file(&request, "admin", operation)
            .unwrap(),
        receipt
    );
}

#[test]
fn native_move_refuses_an_unfinished_membership_transition() {
    let mut fixture = Fixture::new(false);
    let retiring = fixture.engine.config().disks[4].disk_id.clone();
    fixture
        .engine
        .begin_disk_exit(&retiring, MembershipExitAction::Deregister, "admin", op())
        .unwrap();
    let request = path_request(&fixture);
    let before = fixture.engine.state().clone();
    assert_eq!(
        fixture
            .engine
            .move_current_file(&request, "admin", op())
            .unwrap_err()
            .code(),
        ErrorCode::RecoveryRequired
    );
    assert_eq!(fixture.engine.state(), &before);
    assert!(
        !fixture
            .temp
            .path()
            .join("system/move-completion.json")
            .exists()
    );
}

#[cfg(feature = "fault-injection")]
#[test]
fn native_move_early_selection_keeps_source_frozen_and_recovers_the_original_candidate() {
    let mut fixture = Fixture::new(false);
    let request = path_request(&fixture);
    let old = entry(&fixture.engine, &fixture.source, "source.txt");
    let before = fixture.engine.state().clone();
    let operation = op();
    fixture
        .engine
        .inject_fault_once("move_after_completion_binding");
    assert_eq!(
        fixture
            .engine
            .move_current_file(&request, "admin", operation)
            .unwrap_err()
            .code(),
        ErrorCode::RecoveryRequired
    );
    assert_eq!(fixture.engine.state(), &before);
    assert_eq!(bytes(&fixture.engine, &old.version_id), b"new full body");
    let retained = fs::read(fixture.temp.path().join("system/move-completion.json")).unwrap();
    let binding: json_codec::Value = storage_contract::decode_strict(&retained).unwrap();
    assert_eq!(binding["operation"], operation.to_string());
    let candidate = selected_candidate(&fixture);
    assert_eq!(candidate["operation_id"], operation.to_string());
    assert_eq!(candidate["sequence"], (before.sequence + 1).to_string());
    assert_eq!(candidate["request_digest"], binding["request_digest"]);
    assert_eq!(
        fixture
            .engine
            .create_workspace(&fixture.source.main_branch_id, "admin")
            .unwrap_err()
            .code(),
        ErrorCode::RecoveryRequired
    );
    let Fixture {
        temp,
        engine,
        source,
        target,
    } = fixture;
    drop(engine);
    let mut engine = Engine::open(&temp.path().join("system")).unwrap();
    assert_visible(&engine, &source, &target, &old, operation);
    let receipt = engine
        .move_current_file(&request, "admin", operation)
        .unwrap();
    assert_eq!(receipt.sequence, before.sequence + 1);
    assert_eq!(candidate["request_digest"], receipt.request_digest);
    let archived = temp
        .path()
        .join("system/move-completion")
        .join(format!("{operation}.json"));
    assert_eq!(fs::read(archived).unwrap(), retained);
    assert!(!temp.path().join("system/move-completion.json").exists());
}

#[cfg(feature = "fault-injection")]
fn selected_candidate(fixture: &Fixture) -> json_codec::Value {
    let binding: json_codec::Value = storage_contract::decode_strict(
        &fs::read(fixture.temp.path().join("system/move-completion.json")).unwrap(),
    )
    .unwrap();
    let hash = binding["candidate_hash"].as_str().unwrap();
    let member = fixture
        .engine
        .config()
        .disks
        .iter()
        .find(|disk| disk.metadata_member)
        .unwrap();
    storage_contract::decode_strict(
        &fs::read(
            member
                .root
                .join("metadata/objects/candidate")
                .join(&hash[..2])
                .join(format!("{hash}.json")),
        )
        .unwrap(),
    )
    .unwrap()
}

#[cfg(feature = "fault-injection")]
#[test]
fn every_decided_boundary_recovers_the_same_receipt_and_keeps_old_source_until_activation() {
    for fault in [
        "after_accept_1",
        "after_accept_2",
        "after_finalize",
        "after_database_publish",
        "after_container_names",
        "after_commit_proofs",
        "after_publication_1",
        "after_local_publications",
        "move_after_disk_database_pair",
        "move_after_system_database",
    ] {
        let mut fixture = Fixture::new(false);
        let request = path_request(&fixture);
        let old = entry(&fixture.engine, &fixture.source, "source.txt");
        let before = fixture.engine.state().clone();
        let operation = op();
        fixture.engine.inject_fault_once(fault);
        assert_eq!(
            fixture
                .engine
                .move_current_file(&request, "admin", operation)
                .unwrap_err()
                .code(),
            ErrorCode::InDoubt,
            "{fault}"
        );
        assert_eq!(fixture.engine.state(), &before, "{fault}");
        assert_eq!(bytes(&fixture.engine, &old.version_id), b"new full body");
        assert!(
            fixture
                .engine
                .namespace_tree(&fixture.target.main_branch_id)
                .unwrap()
                .entries()
                .is_empty()
        );
        let Fixture {
            temp,
            engine,
            source,
            target,
        } = fixture;
        drop(engine);
        let mut engine = Engine::open(&temp.path().join("system"))
            .unwrap_or_else(|error| panic!("reopen after {fault}: {error:?}"));
        assert_visible(&engine, &source, &target, &old, operation);
        let receipt = engine
            .move_current_file(&request, "admin", operation)
            .unwrap();
        assert_eq!(receipt.sequence, before.sequence + 1);
        assert_eq!(bytes(&engine, &old.version_id), b"new full body");
        assert_eq!(
            engine
                .state()
                .operations
                .keys()
                .filter(|id| **id == operation)
                .count(),
            1
        );
    }
}

#[cfg(feature = "fault-injection")]
#[test]
fn decided_move_with_corrupted_ordinary_full_does_not_hide_source_or_recreate_from_staging() {
    use std::os::unix::fs::PermissionsExt;

    let mut fixture = Fixture::new(false);
    let request = path_request(&fixture);
    let operation = op();
    let old = entry(&fixture.engine, &fixture.source, "source.txt");
    fixture.engine.inject_fault_once("after_container_names");
    assert!(
        fixture
            .engine
            .move_current_file(&request, "admin", operation)
            .is_err()
    );
    let disk = fixture
        .engine
        .config()
        .disks
        .iter()
        .find(|disk| disk.slot_id == "D04")
        .unwrap();
    let candidate = selected_candidate(&fixture);
    assert_eq!(candidate["operation_id"], operation.to_string());
    let version: FileVersion =
        json_codec::from_value(candidate["versions"][0]["version"].clone()).unwrap();
    assert_eq!(version.history_tree_id, fixture.target.history_tree_id);
    let body = disk
        .root
        .join("histories")
        .join(&version.history_tree_id)
        .join("file-containers")
        .join(&version.file_id)
        .join("namespace/moved.txt/versions")
        .join(format!("{}-moved.txt", version.storage_id));
    let original = fs::read(&body).unwrap();
    let metadata = fs::metadata(&body).unwrap();
    let permissions = metadata.permissions();
    let modified = metadata.modified().unwrap();
    assert_eq!(permissions.mode() & 0o777, 0o400);
    let mut damaged = original.clone();
    damaged[0] ^= 1;
    fs::set_permissions(&body, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(&body, &damaged).unwrap();
    fs::File::open(&body)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    fs::set_permissions(&body, permissions.clone()).unwrap();
    assert_eq!(bytes(&fixture.engine, &old.version_id), b"new full body");
    let Fixture { temp, engine, .. } = fixture;
    drop(engine);
    assert!(Engine::open(&temp.path().join("system")).is_err());
    assert_eq!(fs::read(&body).unwrap(), damaged);
    assert_eq!(fs::metadata(&body).unwrap().permissions(), permissions);
    assert_eq!(fs::metadata(&body).unwrap().modified().unwrap(), modified);
    fs::set_permissions(&body, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(&body, original).unwrap();
    fs::File::open(&body)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    fs::set_permissions(&body, permissions).unwrap();
    let mut engine = Engine::open(&temp.path().join("system")).unwrap();
    assert_eq!(
        engine
            .move_current_file(&request, "admin", operation)
            .unwrap()
            .operation_id,
        operation
    );
}
