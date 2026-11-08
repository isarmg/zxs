//! Actual SYSTEM evidence and cold owner paths. Not executed in this change.
use super::*;
use crate::{Config, RepositoryName};
use std::{fs, path::PathBuf};
struct Fixture {
    temp: tempfile::TempDir,
    config: Config,
    terminal: TerminalManagement,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let mut config =
            Config::development((0..3).map(|n| temp.path().join(format!("disk-{n}")))).unwrap();
        for disk in config.disks.iter_mut().skip(1) {
            disk.metadata_member = false;
        }
        let mut engine = Engine::initialize(&temp.path().join("system"), config.clone()).unwrap();
        engine
            .create_repository(
                &RepositoryName::parse("retained").unwrap(),
                "P01",
                1,
                "admin",
                OperationId::generate().unwrap(),
            )
            .unwrap();
        engine
            .begin_disk_exit(
                &config.disks[0].disk_id,
                MembershipExitAction::Deregister,
                "admin",
                OperationId::generate().unwrap(),
            )
            .unwrap();
        let mut exit = engine.prepare_terminal_disk_exit().unwrap();
        let terminal = exit.complete(&mut engine).unwrap();
        drop(exit);
        drop(engine);
        Self {
            temp,
            config,
            terminal,
        }
    }
    fn system(&self) -> PathBuf {
        self.temp.path().join("system")
    }
    fn plan(&self) -> TerminalRetirementPlan {
        self.terminal
            .preview_terminal_retirement(
                &self.config.disks[1].disk_id,
                "admin",
                OperationId::generate().unwrap(),
            )
            .unwrap()
    }
}
#[test]
fn selection_and_cold_resume_preserve_registry_and_never_report_published_removal() {
    let mut fixture = Fixture::new();
    let plan = fixture.plan();
    let before = fixture.terminal.registry().commitment().clone();
    for disk in &fixture.config.disks {
        fs::rename(&disk.root, disk.root.with_extension("held")).unwrap();
        fs::write(&disk.root, b"must not open retained targets").unwrap();
    }
    let status = fixture.terminal.stage_terminal_retirement(&plan).unwrap();
    assert_eq!(status.phase, TerminalRetirementPhase::DecisionStored);
    assert_eq!(fixture.terminal.registry().commitment(), &before);
    assert_eq!(fixture.terminal.registered_disks().len(), 2);
    assert!(fixture.terminal.operation(plan.operation_id()).is_none());
    let system = fixture.system();
    drop(fixture.terminal);
    let mut cold = TerminalManagement::open(&system).unwrap();
    assert_eq!(
        cold.terminal_retirement_status().unwrap(),
        Some(status.clone())
    );
    assert_eq!(
        cold.resume_terminal_retirement("admin", plan.operation_id(), plan.digest())
            .unwrap(),
        status
    );
    assert_eq!(cold.registry().commitment(), &before);
    assert_eq!(cold.registered_disks().len(), 2);
    assert!(cold.operation(plan.operation_id()).is_none());
    for disk in &fixture.config.disks {
        assert_eq!(
            fs::read(&disk.root).unwrap(),
            b"must not open retained targets"
        );
    }
}
#[test]
fn intent_without_selection_recovers_only_its_exact_original_actor_id_and_digest() {
    let mut fixture = Fixture::new();
    let plan = fixture.plan();
    let directory = fixture.terminal.store.operation();
    directory
        .write_json(&directory.path().join(INTENT), &plan.intent, false)
        .unwrap();
    assert_eq!(
        fixture
            .terminal
            .terminal_retirement_status()
            .unwrap()
            .unwrap()
            .phase,
        TerminalRetirementPhase::IntentStored
    );
    assert!(
        fixture
            .terminal
            .resume_terminal_retirement("other", plan.operation_id(), plan.digest())
            .is_err()
    );
    assert!(
        fixture
            .terminal
            .resume_terminal_retirement("admin", OperationId::generate().unwrap(), plan.digest())
            .is_err()
    );
    assert!(
        fixture
            .terminal
            .resume_terminal_retirement("admin", plan.operation_id(), &"0".repeat(64))
            .is_err()
    );
    let system = fixture.system();
    drop(fixture.terminal);
    let mut cold = TerminalManagement::open(&system).unwrap();
    assert_eq!(
        cold.resume_terminal_retirement("admin", plan.operation_id(), plan.digest())
            .unwrap()
            .phase,
        TerminalRetirementPhase::DecisionStored
    );
}
#[test]
fn pending_retirement_fences_metadata_resume_before_any_selected_target_access() {
    let mut fixture = Fixture::new();
    let plan = fixture.plan();
    fixture.terminal.stage_terminal_retirement(&plan).unwrap();
    assert!(
        fixture
            .terminal
            .preview_disk_readmission(&fixture.config.disks[1].disk_id)
            .is_err()
    );
    assert_eq!(
        fixture
            .terminal
            .readmission_metadata_member(&fixture.config.disks[1].disk_id)
            .unwrap(),
        None
    );
    let system = fixture.system();
    assert!(
        fixture
            .terminal
            .prepare_metadata_resume(
                fixture.config.clone(),
                "admin",
                OperationId::generate().unwrap(),
                Vec::new()
            )
            .is_err()
    );
    assert!(TerminalManagement::open_for_resume(&system).is_err());
    let cold = TerminalManagement::open(&system).unwrap();
    assert!(cold.complete_metadata_resume_held(None).is_err());
}
#[test]
fn malformed_or_orphan_selection_never_opens_a_clean_terminal_owner() {
    for orphan in [false, true] {
        let mut fixture = Fixture::new();
        let plan = fixture.plan();
        fixture.terminal.stage_terminal_retirement(&plan).unwrap();
        let directory = fixture.terminal.store.operation().path().to_owned();
        if orphan {
            fs::remove_file(directory.join(INTENT)).unwrap();
        } else {
            fs::write(directory.join(SELECTED), b"corrupt selected retirement").unwrap();
        }
        let system = fixture.system();
        drop(fixture.terminal);
        assert!(TerminalManagement::open(&system).is_err());
        assert!(directory.join(SELECTED).is_file());
    }
}
#[test]
fn stale_source_and_other_pending_selection_cannot_reserve_a_new_operation() {
    let mut fixture = Fixture::new();
    let plan = fixture.plan();
    let mut wrong = plan.clone();
    wrong.intent.source.sequence += 1;
    wrong.digest = object_hash(DIGEST_KIND, &wrong.intent).unwrap();
    assert!(fixture.terminal.stage_terminal_retirement(&wrong).is_err());
    assert!(
        fixture
            .terminal
            .terminal_retirement_status()
            .unwrap()
            .is_none()
    );
    let alternative = fixture
        .terminal
        .preview_terminal_retirement(
            &fixture.config.disks[2].disk_id,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    fixture.terminal.stage_terminal_retirement(&plan).unwrap();
    assert!(
        fixture
            .terminal
            .stage_terminal_retirement(&alternative)
            .is_err()
    );
    assert!(
        fixture
            .terminal
            .preview_terminal_retirement(
                &fixture.config.disks[2].disk_id,
                "admin",
                OperationId::generate().unwrap()
            )
            .is_err()
    );
    assert_eq!(
        fixture
            .terminal
            .terminal_retirement_status()
            .unwrap()
            .unwrap()
            .operation_id,
        plan.operation_id()
    );
}

#[test]
fn first_terminal_successor_completes_stop_and_receipt_without_any_target_probe() {
    let mut fixture = Fixture::new();
    let plan = fixture.plan();
    let old = fixture.terminal.registry().commitment().clone();
    for disk in &fixture.config.disks {
        fs::rename(&disk.root, disk.root.with_extension("unplugged")).unwrap();
        fs::write(&disk.root, b"do not resolve this as a directory").unwrap();
    }
    let receipt = fixture
        .terminal
        .complete_terminal_retirement(&plan)
        .unwrap();
    assert_eq!(receipt.request_digest, plan.digest());
    assert_eq!(receipt.result["registration"], "detached");
    assert_eq!(receipt.result["target_io"], "closed");
    assert_eq!(fixture.terminal.registered_disks().len(), 1);
    assert_eq!(fixture.terminal.registry().historical_topology().len(), 3);
    assert_ne!(fixture.terminal.registry().commitment(), &old);
    assert_eq!(
        fixture.terminal.operation(plan.operation_id()),
        Some(&receipt)
    );
    assert!(
        fixture
            .terminal
            .terminal_retirement_status()
            .unwrap()
            .is_none()
    );
    assert!(
        fixture
            .terminal
            .preview_terminal_retirement(
                &fixture.config.disks[2].disk_id,
                "admin",
                OperationId::generate().unwrap()
            )
            .is_ok()
    );
    assert!(fixture.terminal.writer_locks().is_err());
    let root = fixture.system();
    drop(fixture.terminal);
    let mut cold = TerminalManagement::open(&root).unwrap();
    assert_eq!(cold.registered_disks().len(), 1);
    let original = cold
        .original_terminal_retirement("admin", plan.operation_id(), plan.digest())
        .unwrap();
    assert_eq!(
        cold.complete_terminal_retirement(&original).unwrap(),
        receipt
    );
    assert_eq!(
        cold.terminal_retirement_result("admin", plan.operation_id(), plan.digest())
            .unwrap(),
        Some(receipt)
    );
    drop(cold);
    assert!(TerminalManagement::open_for_resume(&root).is_err());
    for disk in &fixture.config.disks {
        assert_eq!(
            fs::read(&disk.root).unwrap(),
            b"do not resolve this as a directory"
        );
    }
}

#[test]
fn stop_without_final_marker_keeps_registration_until_exact_same_original_completion() {
    let mut fixture = Fixture::new();
    let plan = fixture.plan();
    let receipt = fixture
        .terminal
        .complete_terminal_retirement(&plan)
        .unwrap();
    let complete_path = fixture
        .terminal
        .store
        .operation()
        .path()
        .join(completion::COMPLETE);
    // Model a final marker that did not survive. The exact original Stop/plan
    // remain durable, so a cold retry must not invent another ledger operation.
    fs::remove_file(&complete_path).unwrap();
    let root = fixture.system();
    drop(fixture.terminal);
    let mut cold = TerminalManagement::open(&root).unwrap();
    assert_eq!(cold.registered_disks().len(), 2);
    assert!(cold.operation(plan.operation_id()).is_none());
    let original = cold
        .original_terminal_retirement("admin", plan.operation_id(), plan.digest())
        .unwrap();
    assert_eq!(
        cold.complete_terminal_retirement(&original).unwrap(),
        receipt
    );
    assert_eq!(cold.registered_disks().len(), 1);
}

#[test]
fn final_marker_without_its_native_stop_is_corruption_not_completed_or_empty() {
    let mut fixture = Fixture::new();
    let plan = fixture.plan();
    fixture
        .terminal
        .complete_terminal_retirement(&plan)
        .unwrap();
    let stop = fixture
        .terminal
        .store
        .operation()
        .path()
        .join(completion::STOP);
    fs::remove_file(&stop).unwrap();
    let root = fixture.system();
    drop(fixture.terminal);
    assert!(TerminalManagement::open(&root).is_err());
    assert!(TerminalManagement::requires_management(&root).unwrap());
    assert!(Engine::open(&root).is_err());
}

#[test]
fn linked_retirements_cold_resume_to_empty_registry_keep_every_exact_original_receipt() {
    let mut fixture = Fixture::new();
    let first = fixture.plan();
    let first_result = fixture
        .terminal
        .complete_terminal_retirement(&first)
        .unwrap();
    let second = fixture
        .terminal
        .preview_terminal_retirement(
            &fixture.config.disks[2].disk_id,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    fixture.terminal.stage_terminal_retirement(&second).unwrap();
    assert_eq!(fixture.terminal.registered_disks().len(), 1);
    assert_eq!(
        fixture.terminal.completed_terminal_retirement(),
        Some(&first_result)
    );
    assert_eq!(
        fixture
            .terminal
            .terminal_retirement_status()
            .unwrap()
            .unwrap()
            .operation_id,
        second.operation_id()
    );
    for disk in &fixture.config.disks {
        fs::rename(&disk.root, disk.root.with_extension("removed")).unwrap();
        fs::write(&disk.root, b"all old addresses are non-directories").unwrap();
    }
    let root = fixture.system();
    drop(fixture.terminal);
    let mut cold = TerminalManagement::open(&root).unwrap();
    assert_eq!(
        cold.terminal_retirement_result("admin", first.operation_id(), first.digest())
            .unwrap(),
        Some(first_result.clone())
    );
    let original_second = cold
        .original_terminal_retirement("admin", second.operation_id(), second.digest())
        .unwrap();
    let second_result = cold.complete_terminal_retirement(&original_second).unwrap();
    assert_eq!(second_result.sequence, first_result.sequence + 1);
    assert_eq!(cold.registered_disks().len(), 0);
    assert_eq!(cold.registry().historical_topology().len(), 3);
    let original_first = cold
        .original_terminal_retirement("admin", first.operation_id(), first.digest())
        .unwrap();
    assert_eq!(
        cold.complete_terminal_retirement(&original_first).unwrap(),
        first_result
    );
    assert_eq!(cold.state().sequence, second_result.sequence);
    assert!(
        cold.original_terminal_retirement("other", first.operation_id(), first.digest())
            .is_err()
    );
    assert!(cold.writer_locks().is_err());
    drop(cold);
    let reopened = TerminalManagement::open(&root).unwrap();
    assert_eq!(
        reopened.status().registrations,
        super::super::TerminalRegistrationState::EmptyRegistry
    );
    assert_eq!(
        reopened.operation(second.operation_id()),
        Some(&second_result)
    );
    assert_eq!(
        reopened.operation(first.operation_id()),
        Some(&first_result)
    );
}

#[test]
fn durable_next_intent_recovers_missing_child_without_choosing_another_operation() {
    let mut fixture = Fixture::new();
    let first = fixture.plan();
    fixture
        .terminal
        .complete_terminal_retirement(&first)
        .unwrap();
    let next = fixture
        .terminal
        .preview_terminal_retirement(
            &fixture.config.disks[2].disk_id,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    let directory = fixture.terminal.store.operation();
    let complete: json_codec::Value = crate::decode_strict(
        &directory
            .read_bytes(
                &directory.path().join(completion::COMPLETE),
                crate::disk::MAX_METADATA,
            )
            .unwrap(),
    )
    .unwrap();
    let link = json_codec::json!({"format":"terminal-registration-retirement",
        "predecessor_completion":object_hash("terminal-retirement-completion", &complete).unwrap(),
        "intent":next.intent,"intent_digest":next.digest});
    directory
        .write_json(&directory.path().join(chain::NEXT), &link, false)
        .unwrap();
    let root = fixture.system();
    drop(fixture.terminal);
    let mut cold = TerminalManagement::open(&root).unwrap();
    let original = cold
        .original_terminal_retirement("admin", next.operation_id(), next.digest())
        .unwrap();
    assert_eq!(cold.registered_disks().len(), 1);
    cold.complete_terminal_retirement(&original).unwrap();
    assert_eq!(cold.registered_disks().len(), 0);
}

#[test]
fn unrelated_terminal_child_is_not_adopted_as_another_registration_decision() {
    let mut fixture = Fixture::new();
    let first = fixture.plan();
    fixture
        .terminal
        .complete_terminal_retirement(&first)
        .unwrap();
    let directory = fixture.terminal.store.operation();
    let container = directory.path().join("retirement-operations");
    directory.private_dir(&container).unwrap();
    let container = directory.private_child(&container).unwrap();
    container
        .private_dir(
            &container
                .path()
                .join(OperationId::generate().unwrap().to_string()),
        )
        .unwrap();
    let root = fixture.system();
    drop(fixture.terminal);
    assert!(TerminalManagement::open(&root).is_err());
    assert!(Engine::open(&root).is_err());
}

#[test]
fn completed_retirement_requires_its_mask_graph_during_cold_open() {
    let mut fixture = Fixture::new();
    let plan = fixture.plan();
    let result = fixture
        .terminal
        .complete_terminal_retirement(&plan)
        .unwrap();
    assert_eq!(result.result["namespace_projection_verified"], true);
    assert_eq!(result.result["active_namespace_admitted"], false);
    assert_eq!(result.result["metadata_resumption_supported"], false);
    let mask = result.result["occurrence_mask_hash"]
        .as_str()
        .unwrap()
        .to_owned();
    let successor = fixture
        .terminal
        .store
        .operation()
        .path()
        .join(completion::SUCCESSOR);
    let root = fixture.system();
    drop(fixture.terminal);
    let mut body: json_codec::Value =
        json_codec::from_slice(&fs::read(&successor).unwrap()).unwrap();
    assert!(
        body["objects"]
            .as_object_mut()
            .unwrap()
            .remove(&mask)
            .is_some()
    );
    let corrupt = json_codec::to_vec(&body).unwrap();
    fs::write(&successor, &corrupt).unwrap();
    assert!(TerminalManagement::open(&root).is_err());
    assert!(Engine::open(&root).is_err());
    assert_eq!(fs::read(&successor).unwrap(), corrupt);
}

#[test]
fn unmasked_retirement_successor_is_rejected_without_implicit_upgrade() {
    let mut fixture = Fixture::new();
    let plan = fixture.plan();
    fixture
        .terminal
        .complete_terminal_retirement(&plan)
        .unwrap();
    let successor = fixture
        .terminal
        .store
        .operation()
        .path()
        .join(completion::SUCCESSOR);
    let root = fixture.system();
    drop(fixture.terminal);
    let mut body: json_codec::Value =
        json_codec::from_slice(&fs::read(&successor).unwrap()).unwrap();
    body.as_object_mut().unwrap().remove("occurrence_mask_hash");
    body["format"] = json_codec::json!("terminal-registration-retirement");
    let unmasked = json_codec::to_vec(&body).unwrap();
    fs::write(&successor, &unmasked).unwrap();
    assert!(TerminalManagement::open(&root).is_err());
    assert_eq!(fs::read(&successor).unwrap(), unmasked);
}

#[test]
fn portable_complete_chain_preserves_checkpoint_and_never_opens_retired_roots() {
    let mut fixture = Fixture::new();
    for index in [1, 2] {
        let plan = fixture
            .terminal
            .preview_terminal_retirement(
                &fixture.config.disks[index].disk_id,
                "admin",
                OperationId::generate().unwrap(),
            )
            .unwrap();
        fixture
            .terminal
            .complete_terminal_retirement(&plan)
            .unwrap();
    }
    let archive = fixture.terminal.export_retirement_archive().unwrap();
    let checkpoint = archive.checkpoint().clone();
    assert_eq!(checkpoint.completed_retirements, 2);
    assert_eq!(
        checkpoint.registry,
        *fixture.terminal.registry().commitment()
    );
    assert_eq!(checkpoint.sequence, fixture.terminal.state().sequence);
    assert!(!checkpoint.target_writer_admitted);
    assert!(!checkpoint.active_namespace_admitted);
    assert!(!checkpoint.metadata_resumption_supported);
    assert!(!checkpoint.live_io_exclusion_transferred);
    for disk in &fixture.config.disks {
        fs::rename(&disk.root, disk.root.with_extension("checkpoint-held")).unwrap();
        fs::write(&disk.root, b"must not be opened during checkpoint capture").unwrap();
    }
    let verified = fixture
        .terminal
        .verify_retirement_archive(archive.bytes(), &checkpoint.digest)
        .unwrap();
    assert_eq!(verified.digest, checkpoint.digest);
    let system = fixture.system();
    drop(fixture.terminal);
    let cold = TerminalManagement::open(&system).unwrap();
    assert_eq!(
        cold.export_retirement_archive().unwrap().bytes(),
        archive.bytes()
    );
    assert!(cold.require_no_terminal_retirement().is_err());
}
#[test]
fn pending_child_never_exports_completed_prefix_as_current() {
    let mut fixture = Fixture::new();
    let first = fixture.plan();
    fixture
        .terminal
        .complete_terminal_retirement(&first)
        .unwrap();
    let checkpoint = fixture.terminal.export_retirement_archive().unwrap();
    let next = fixture
        .terminal
        .preview_terminal_retirement(
            &fixture.config.disks[2].disk_id,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    fixture.terminal.stage_terminal_retirement(&next).unwrap();
    assert!(fixture.terminal.export_retirement_archive().is_err());
    // An independently retained older digest only verifies that old checkpoint,
    // not latest-ness and not permission to finish or discard the pending child.
    assert!(
        fixture
            .terminal
            .verify_retirement_archive(checkpoint.bytes(), &checkpoint.checkpoint().digest)
            .is_ok()
    );
    assert!(
        fixture
            .terminal
            .terminal_retirement_status()
            .unwrap()
            .is_some()
    );
}
#[test]
fn portable_checkpoint_rejects_truncation_reorder_and_rehashed_receipt_mutation() {
    let mut fixture = Fixture::new();
    for index in [1, 2] {
        let plan = fixture
            .terminal
            .preview_terminal_retirement(
                &fixture.config.disks[index].disk_id,
                "admin",
                OperationId::generate().unwrap(),
            )
            .unwrap();
        fixture
            .terminal
            .complete_terminal_retirement(&plan)
            .unwrap();
    }
    let archive = fixture.terminal.export_retirement_archive().unwrap();
    let body: json_codec::Value = json_codec::from_slice(archive.bytes()).unwrap();
    assert!(
        fixture
            .terminal
            .verify_retirement_archive(archive.bytes(), &"0".repeat(64))
            .is_err()
    );
    for change in 0..4 {
        let mut changed = body.clone();
        match change {
            0 => {
                changed["frames"].as_array_mut().unwrap().pop();
            }
            1 => {
                changed["frames"].as_array_mut().unwrap().swap(0, 1);
            }
            2 => {
                changed["frames"][1]["completed"]["result"]["sequence"] = "999".into();
            }
            _ => {
                changed["frames"][1]["predecessor"]["predecessor_completion"] =
                    "0".repeat(64).into();
            }
        }
        let digest = object_hash("terminal-retirement-chain-checkpoint", &changed).unwrap();
        assert!(
            fixture
                .terminal
                .verify_retirement_archive(&json_codec::to_vec(&changed).unwrap(), &digest)
                .is_err()
        );
    }
}
#[test]
fn portable_checkpoint_rejects_rehashed_mask_loss_and_conflicting_stop_binding() {
    let mut fixture = Fixture::new();
    let plan = fixture.plan();
    fixture
        .terminal
        .complete_terminal_retirement(&plan)
        .unwrap();
    let archive = fixture.terminal.export_retirement_archive().unwrap();
    let body: json_codec::Value = json_codec::from_slice(archive.bytes()).unwrap();
    let mut changed = body.clone();
    let hash = changed["frames"][0]["successor"]["occurrence_mask_hash"]
        .as_str()
        .unwrap()
        .to_owned();
    changed["frames"][0]["successor"]["objects"]
        .as_object_mut()
        .unwrap()
        .remove(&hash)
        .unwrap();
    let digest = object_hash("terminal-retirement-chain-checkpoint", &changed).unwrap();
    assert!(
        fixture
            .terminal
            .verify_retirement_archive(&json_codec::to_vec(&changed).unwrap(), &digest)
            .is_err()
    );
    let mut changed = body;
    changed["frames"][0]["stop"]["native"]["plan"]["operation_id"] =
        OperationId::generate().unwrap().to_string().into();
    let digest = object_hash("terminal-retirement-chain-checkpoint", &changed).unwrap();
    assert!(
        fixture
            .terminal
            .verify_retirement_archive(&json_codec::to_vec(&changed).unwrap(), &digest)
            .is_err()
    );
}
