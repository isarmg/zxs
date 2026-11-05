//! Successive terminal selectors preserve exact resumed authority and receipts.
use super::*;
use std::sync::Arc;

fn resumed() -> (Fixture, Engine) {
    let mut fixture = Fixture::new(MembershipExitAction::Deregister, 0);
    let target = fixture.target(1);
    let engine = fixture.prepare(&target).activate().unwrap();
    (fixture, engine)
}

fn begin_terminal(engine: &mut Engine) -> OperationId {
    let disk_id = engine
        .config
        .metadata_members()
        .next()
        .unwrap()
        .disk_id
        .clone();
    let operation = OperationId::generate().unwrap();
    engine
        .begin_disk_exit(
            &disk_id,
            MembershipExitAction::Deregister,
            "admin",
            operation,
        )
        .unwrap();
    operation
}

fn next_target(engine: &Config, root: &Path, label: &str) -> Config {
    let mut target = engine.clone();
    for disk in &mut target.disks {
        disk.metadata_member = false;
    }
    let mut added = Config::development(
        (0..=target.disks.len()).map(|index| root.join(format!("{label}-{index}"))),
    )
    .unwrap()
    .disks
    .pop()
    .unwrap();
    added.level = 1;
    added.upstream_slot_id = None;
    fs::create_dir(&added.root).unwrap();
    fs::set_permissions(&added.root, fs::Permissions::from_mode(0o700)).unwrap();
    target.disks.push(added);
    target
}

#[test]
fn terminal_head_rotates_twice_and_cold_start_rejects_a_valid_older_selector() {
    let (fixture, mut engine) = resumed();
    let directory = fixture.system().join("terminal-management");
    let head = directory.join("head.json");
    let first_head = fs::read(&head).unwrap();
    let old_evidence = files(&fixture.source_path());
    let first_resume = engine.state.operations[&fixture.resume].clone();
    let operation = begin_terminal(&mut engine);
    let mut exit = engine.prepare_terminal_disk_exit().unwrap();
    let management = exit.complete(&mut engine).unwrap();
    assert_eq!(management.result().operation_id, operation);
    assert_eq!(management.operation(fixture.resume), Some(&first_resume));
    assert_eq!(files(&fixture.source_path()), old_evidence);
    assert_ne!(fs::read(&head).unwrap(), first_head);
    let target = next_target(engine.config(), fixture.temporary.path(), "second-resume");
    drop(exit);
    drop(engine);
    let second_resume = OperationId::generate().unwrap();
    let engine = management
        .prepare_metadata_resume(target.clone(), "admin", second_resume, directories(&target))
        .unwrap()
        .activate()
        .unwrap();
    let second_result = engine.state.operations[&second_resume].clone();
    let second_head = fs::read(&head).unwrap();
    drop(engine);
    assert!(!TerminalManagement::requires_management(&fixture.system()).unwrap());
    let engine = Engine::open(&fixture.system()).unwrap();
    assert_eq!(engine.state.operations[&fixture.resume], first_resume);
    assert_eq!(engine.state.operations[&second_resume], second_result);
    drop(engine);

    // Both old selector and all of its source records are genuinely valid.
    // They still cannot select an ancestor of the latest TerminalResume.
    fs::write(&head, &first_head).unwrap();
    let error = match Engine::open(&fixture.system()) {
        Ok(_) => panic!("an older valid terminal selector must not authorize startup"),
        Err(error) => error,
    };
    assert_eq!(error.code(), ErrorCode::LogFork);
    assert_eq!(fs::read(&head).unwrap(), first_head);
    assert_eq!(files(&fixture.source_path()), old_evidence);
    fs::write(&head, second_head).unwrap();
    let mut engine = Engine::open(&fixture.system()).unwrap();
    let third_terminal = begin_terminal(&mut engine);
    let mut exit = engine.prepare_terminal_disk_exit().unwrap();
    let management = exit.complete(&mut engine).unwrap();
    assert_eq!(management.result().operation_id, third_terminal);
    assert_eq!(management.operation(second_resume), Some(&second_result));
}

#[test]
fn terminal_head_preparation_resumes_before_and_after_selector_replacement() {
    for selected in [false, true] {
        let (fixture, mut engine) = resumed();
        let head = fixture.system().join("terminal-management/head.json");
        let first_head = fs::read(&head).unwrap();
        let operation = begin_terminal(&mut engine);
        if selected {
            // This returns after the real authority/head barriers and before
            // stop or closure; cold management must complete only this handoff.
            drop(engine.prepare_terminal_disk_exit().unwrap());
            assert_ne!(fs::read(&head).unwrap(), first_head);
            let target = engine
                .config
                .metadata_members()
                .next()
                .unwrap()
                .root
                .clone();
            let parked = fixture
                .temporary
                .path()
                .join("stopped-before-cold-completion");
            let before = files(&target);
            drop(engine);
            fs::rename(&target, &parked).unwrap();
            fs::write(&target, b"stopped target path must not be opened").unwrap();
            let management = TerminalManagement::open(&fixture.system()).unwrap();
            assert_eq!(management.result().operation_id, operation);
            assert_eq!(files(&parked), before);
            assert_eq!(
                fs::read(&target).unwrap(),
                b"stopped target path must not be opened"
            );
        } else {
            // Materialize the exact immutable pre-head stage. The old selector
            // remains authoritative until the successor head barrier completes.
            let authority = engine.prepare_terminal_authority().unwrap();
            let system = DiskRoot::open(&fixture.system()).unwrap();
            let path = system
                .path()
                .join("terminal-management")
                .join(operation.to_string());
            system.private_dir(&path).unwrap();
            system
                .write_json(&path.join("authority.json"), &authority, false)
                .unwrap();
            assert_eq!(fs::read(&head).unwrap(), first_head);
            drop(system);
            drop(engine);
            let mut engine = Engine::open(&fixture.system()).unwrap();
            let mut exit = engine.prepare_terminal_disk_exit().unwrap();
            let management = exit.complete(&mut engine).unwrap();
            assert_eq!(management.result().operation_id, operation);
        }
    }
}

#[test]
fn terminal_head_rotation_requires_both_exact_permanent_receipts_and_actors() {
    let (fixture, mut engine) = resumed();
    begin_terminal(&mut engine);
    let authority = engine.prepare_terminal_authority().unwrap();
    let completed = activation::terminal_resume::completed_selector(&fixture.system())
        .unwrap()
        .unwrap();
    completed.verify_handoff(&engine, &authority).unwrap();
    for operation in [fixture.terminal, fixture.resume] {
        let expected = engine.state.operations[&operation].clone();
        Arc::make_mut(&mut engine.state)
            .operations
            .remove(&operation);
        assert_eq!(
            completed
                .verify_handoff(&engine, &authority)
                .unwrap_err()
                .code(),
            ErrorCode::PublicationInvalid
        );
        Arc::make_mut(&mut engine.state)
            .operations
            .insert(operation, expected.clone());
        Arc::make_mut(&mut engine.state)
            .operations
            .get_mut(&operation)
            .unwrap()
            .request_digest = "0".repeat(64);
        assert_eq!(
            completed
                .verify_handoff(&engine, &authority)
                .unwrap_err()
                .code(),
            ErrorCode::PublicationInvalid
        );
        Arc::make_mut(&mut engine.state)
            .operations
            .insert(operation, expected);
        Arc::make_mut(&mut engine.state)
            .operation_principals
            .insert(operation, "other".into());
        assert_eq!(
            completed
                .verify_handoff(&engine, &authority)
                .unwrap_err()
                .code(),
            ErrorCode::PublicationInvalid
        );
        Arc::make_mut(&mut engine.state)
            .operation_principals
            .insert(operation, "admin".into());
    }
    completed.verify_handoff(&engine, &authority).unwrap();
    let (_other_fixture, mut unrelated) = resumed();
    begin_terminal(&mut unrelated);
    let other_authority = unrelated.prepare_terminal_authority().unwrap();
    assert_eq!(
        completed
            .verify_handoff(&engine, &other_authority)
            .unwrap_err()
            .code(),
        ErrorCode::LogFork
    );
}

#[test]
fn terminal_head_rotation_preserves_selector_when_completed_resume_is_tampered() {
    let (fixture, mut engine) = resumed();
    let head = fixture.system().join("terminal-management/head.json");
    let original_head = fs::read(&head).unwrap();
    let complete = fixture.source_path().join("resume-complete.json");
    let original_completion = fs::read(&complete).unwrap();
    let mut changed: json_codec::Value = decode_strict(&original_completion).unwrap();
    changed["generation_hash"] = json_codec::json!("0".repeat(64));
    fs::write(&complete, json_codec::to_vec(&changed).unwrap()).unwrap();
    begin_terminal(&mut engine);
    assert!(engine.prepare_terminal_disk_exit().is_err());
    assert_eq!(fs::read(&head).unwrap(), original_head);
    fs::write(&complete, original_completion).unwrap();
    let mut exit = engine.prepare_terminal_disk_exit().unwrap();
    let management = exit.complete(&mut engine).unwrap();
    assert_eq!(
        management.operation(fixture.resume).unwrap().kind,
        "terminal-metadata-resume"
    );
}

#[test]
fn terminal_head_rotation_accepts_real_add_and_exit_descendants_of_the_resume() {
    let (fixture, mut engine) = resumed();
    let original_member = engine
        .config
        .metadata_members()
        .next()
        .unwrap()
        .disk_id
        .clone();
    let mut target = next_target(engine.config(), fixture.temporary.path(), "ordinary-add");
    target
        .disks
        .iter_mut()
        .find(|disk| disk.disk_id == original_member)
        .unwrap()
        .metadata_member = true;
    let added = target.disks.last().unwrap().clone();
    let addition = OperationId::generate().unwrap();
    engine
        .begin_membership_handoff(target, "admin", addition)
        .unwrap();
    let archive = engine.build_managed_membership_control_archive().unwrap();
    engine
        .enroll_pending_members(
            &archive.archive_path,
            &archive.root_hash,
            vec![PendingMemberDirectory {
                disk_id: added.disk_id.clone(),
                directory: File::open(&added.root).unwrap(),
            }],
        )
        .unwrap();
    engine.resume_membership_joint_decision().unwrap();
    assert_eq!(
        engine.activate_pending_membership().unwrap().operation_id,
        addition
    );
    let ordinary_exit = OperationId::generate().unwrap();
    engine
        .begin_disk_exit(
            &original_member,
            MembershipExitAction::Deregister,
            "admin",
            ordinary_exit,
        )
        .unwrap();
    let archive = engine.build_managed_membership_control_archive().unwrap();
    engine
        .enroll_pending_members(&archive.archive_path, &archive.root_hash, Vec::new())
        .unwrap();
    assert!(
        engine
            .resume_membership_joint_decision()
            .unwrap()
            .joint_quorum_accepted()
    );
    assert_eq!(
        engine.activate_pending_disk_exit().unwrap().operation_id,
        ordinary_exit
    );
    begin_terminal(&mut engine);
    let mut exit = engine.prepare_terminal_disk_exit().unwrap();
    let management = exit.complete(&mut engine).unwrap();
    assert!(management.operation(fixture.resume).is_some());
    assert!(management.operation(addition).is_some());
    assert!(management.operation(ordinary_exit).is_some());
}
