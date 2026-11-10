//! Native move success and interruption scenarios on disposable roots. These
//! sources are not an executed result or physical power-loss / SMB certificate.
use super::*;
use std::fs;
use storage_contract::file_relocation::{OBJECT_MOVE_KIND, ObjectMoveFact};

fn path_request(fixture: &Fixture) -> FileHistoryMovePathRequest {
    FileHistoryMovePathRequest {
        source_repository_id: fixture.source.repository_id.clone(),
        source_path: "source.txt".into(),
        target_repository_id: fixture.target.repository_id.clone(),
        target_name: "moved.txt".into(),
    }
}

fn retain_original_move(fixture: &mut Fixture, operation: OperationId) {
    let mut namespace = request(&fixture.engine, &fixture.source, &fixture.target);
    namespace.target_name = path_request(fixture).target_name;
    let proposed = super::move_recovery::projection_for(fixture, operation, &namespace);
    fixture
        .engine
        .save_file_history_move_recovery(&proposed, &HistoryWindowCancellation::new())
        .unwrap();
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
    for disk in &engine.config().disks {
        assert_eq!(
            engine
                .disk_catalog_observation(&disk.slot_id)
                .unwrap()
                .snapshot
                .source_sequence,
            receipt.sequence
        );
        assert_eq!(
            engine
                .disk_history_observation(&disk.slot_id)
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
    fixture
        .engine
        .update_file_node_tags(FileNodeTagsUpdateRequest {
            file_id: &old.file_id,
            targets: &[FileTagTarget {
                branch_id: context.branch_id,
                source_node_id: node.clone(),
                expected_node_id: node,
            }],
            add: &["move-label".into()],
            remove: &[],
            location: None,
            principal: "admin",
            operation_id: op(),
        })
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
    assert_eq!(receipt.kind, OBJECT_MOVE_KIND);
    let fact: ObjectMoveFact =
        json_codec::from_value(receipt.result["movement"].clone()).unwrap();
    assert_eq!(fact.request.source_entry_id.as_str(), old.entry_id);
    assert_eq!(fact.source_file_id.as_ref().unwrap().as_str(), old.file_id);
    assert_eq!(fact.target_repository_id.as_str(), fixture.target.repository_id);
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
        fixture
            .engine
            .file_node_tags(&moved.file_id, moved_context.node_id.as_ref().unwrap())
            .unwrap()
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
fn completed_object_move_retires_its_binding_and_a_later_move_keeps_both_receipts() {
    let mut fixture = Fixture::new(false);
    let request = path_request(&fixture);
    let first = op();
    let original = fixture
        .engine
        .move_current_file(&request, "admin", first)
        .unwrap();
    let third = fixture
        .engine
        .create_repository(
            &RepositoryName::parse("third").unwrap(),
            "D05",
            1,
            "admin",
            op(),
        )
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
fn native_move_rejects_an_occupied_target_without_a_new_intent_or_source_deletion() {
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
    assert!(
        fixture
            .engine
            .inspect_file_history_move_recovery()
            .unwrap()
            .is_none()
    );
}

#[test]
fn native_move_waits_for_membership_activation_then_uses_the_admitted_current() {
    let mut fixture = Fixture::new(false);
    let request = path_request(&fixture);
    let old = entry(&fixture.engine, &fixture.source, "source.txt");
    let unused_disk = fixture.engine.config().disks[4].disk_id.clone();
    fixture
        .engine
        .begin_disk_exit(&unused_disk, MembershipExitAction::Deregister, "admin", op())
        .unwrap();
    let operation = op();
    let before = fixture.engine.state().clone();
    assert_eq!(
        fixture
            .engine
            .move_current_file(&request, "admin", operation)
            .unwrap_err()
            .code(),
        ErrorCode::RecoveryRequired
    );
    assert_eq!(fixture.engine.state(), &before);
    assert_eq!(entry(&fixture.engine, &fixture.source, "source.txt"), old);
    assert!(
        !fixture
            .temp
            .path()
            .join("system/move-completion.json")
            .exists()
    );

    let archive = fixture
        .engine
        .build_managed_membership_control_archive()
        .unwrap();
    fixture
        .engine
        .enroll_pending_members(&archive.archive_path, &archive.root_hash, Vec::new())
        .unwrap();
    assert!(
        fixture
            .engine
            .resume_membership_joint_decision()
            .unwrap()
            .all_certificates_synchronized()
    );
    fixture.engine.activate_pending_disk_exit().unwrap();
    let receipt = fixture
        .engine
        .move_current_file(&request, "admin", operation)
        .unwrap();
    assert_eq!(receipt.kind, OBJECT_MOVE_KIND);
    let moved = entry(&fixture.engine, &fixture.target, "moved.txt");
    assert_eq!(bytes(&fixture.engine, &moved.version_id), b"new full body");
    assert!(
        fixture
            .engine
            .namespace_tree(&fixture.source.main_branch_id)
            .unwrap()
            .get("source.txt")
            .is_none()
    );
    assert_eq!(
        fixture.engine.file_deletion(&old.file_id).unwrap().operation_id,
        operation
    );
    let Fixture {
        temp,
        engine,
        source,
        target,
    } = fixture;
    drop(engine);
    let mut engine = Engine::open(&temp.path().join("system")).unwrap();
    let published = engine.state().clone();
    assert_eq!(
        engine
            .move_current_file(&request, "admin", operation)
            .unwrap(),
        receipt
    );
    assert_eq!(engine.state(), &published);
    assert!(
        engine
            .namespace_tree(&source.main_branch_id)
            .unwrap()
            .get("source.txt")
            .is_none()
    );
    assert_eq!(entry(&engine, &target, "moved.txt"), moved);
}

#[test]
fn retained_file_intent_keeps_source_frozen_and_explicit_retry_uses_original_clock() {
    let mut fixture = Fixture::new(false);
    let request = path_request(&fixture);
    let operation = op();
    retain_original_move(&mut fixture, operation);
    let retained = fs::read(
        fixture
            .temp
            .path()
            .join("system/file-history-move-recovery/preparation.json"),
    )
    .unwrap();
    assert_eq!(
        fixture
            .engine
            .create_workspace(&fixture.source.main_branch_id, "admin")
            .unwrap_err()
            .code(),
        ErrorCode::RecoveryRequired
    );
    let Fixture { temp, engine, .. } = fixture;
    drop(engine);
    let mut engine = Engine::open(&temp.path().join("system")).unwrap();
    assert!(!engine.state().operations.contains_key(&operation));
    let receipt = engine
        .move_current_file(&request, "admin", operation)
        .unwrap();
    assert_eq!(receipt.kind, "file.history.move");
    let archived = temp
        .path()
        .join("system/file-history-move-completed")
        .join(operation.to_string())
        .join("file-history-move-recovery/preparation.json");
    assert_eq!(fs::read(archived).unwrap(), retained);
}

#[cfg(feature = "fault-injection")]
#[test]
fn retained_file_decided_boundaries_recover_the_same_receipt_until_activation() {
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
        "move_before_choice_retirement",
        "move_after_choice_retirement_1",
    ] {
        let mut fixture = Fixture::new(false);
        let request = path_request(&fixture);
        let old = entry(&fixture.engine, &fixture.source, "source.txt");
        let before = fixture.engine.state().clone();
        let operation = op();
        retain_original_move(&mut fixture, operation);
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
        let mut engine = Engine::open(&temp.path().join("system")).unwrap();
        assert_visible(&engine, &source, &target, &old, operation);
        let receipt = engine
            .move_current_file(&request, "admin", operation)
            .unwrap();
        assert_eq!(receipt.kind, "file.history.move");
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
fn retained_file_move_with_corrupted_ordinary_full_does_not_recreate_from_staging() {
    let mut fixture = Fixture::new(false);
    let request = path_request(&fixture);
    let operation = op();
    let old = entry(&fixture.engine, &fixture.source, "source.txt");
    retain_original_move(&mut fixture, operation);
    fixture.engine.inject_fault_once("after_accept_1");
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
    let intent: json_codec::Value = storage_contract::decode_strict(
        &fs::read(
            disk.root
                .join("file-history-move-ordinary/ordinary-inputs.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let version_id = intent["versions"][0]["version_id"].as_str().unwrap();
    let version: FileVersion = storage_contract::decode_strict(
        &fs::read(
            disk.root
                .join("histories")
                .join(&fixture.target.history_tree_id)
                .join("versions")
                .join(version_id)
                .join("version.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let body = disk
        .root
        .join("histories")
        .join(&fixture.target.history_tree_id)
        .join("file-containers")
        .join(intent["file"].as_str().unwrap())
        .join("moved.txt/versions")
        .join(&version.storage_name);
    let original = fs::read(&body).unwrap();
    fs::write(&body, b"incomplete").unwrap();
    assert_eq!(bytes(&fixture.engine, &old.version_id), b"new full body");
    let Fixture { temp, engine, .. } = fixture;
    drop(engine);
    assert!(Engine::open(&temp.path().join("system")).is_err());
    assert_eq!(fs::read(&body).unwrap(), b"incomplete");
    fs::write(&body, original).unwrap();
    let mut engine = Engine::open(&temp.path().join("system")).unwrap();
    assert_eq!(
        engine
            .move_current_file(&request, "admin", operation)
            .unwrap()
            .operation_id,
        operation
    );
}
