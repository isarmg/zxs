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
        Self::with_action(MembershipExitAction::Deregister)
    }
    // Historical PlannedOffline evidence remains a valid recovery fixture;
    // this does not add a new user-facing planned-offline action.
    fn with_action(action: MembershipExitAction) -> Self {
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
                action,
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

#[test]
fn complete_retirement_chain_prepares_retained_graph_without_root_io_or_admission() {
    let mut fixture = Fixture::with_action(MembershipExitAction::PlannedOffline);
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
    let operation = OperationId::generate().unwrap();
    let original = fixture.terminal.registry().commitment().clone();
    for disk in &fixture.config.disks {
        fs::rename(&disk.root, disk.root.with_extension("proposal-held")).unwrap();
        fs::write(&disk.root, b"proposal must not open any data root").unwrap();
    }
    let proposal = fixture
        .terminal
        .prepare_chain_readmission_proposal(&fixture.config.disks[0].disk_id, "admin", operation)
        .unwrap();
    let report = proposal.preparation();
    assert_eq!(report.source.completed_retirements, 2);
    assert_eq!(report.source.registry, original);
    assert_ne!(report.proposed_registry, original);
    assert!(report.complete_namespace_projection_prepared);
    assert!(!report.target_directory_verified);
    assert!(!report.target_decision_recorded);
    assert!(!report.active_namespace_admitted);
    assert!(!report.metadata_resumption_supported);
    assert!(fixture.terminal.operation(operation).is_none());
    assert_eq!(*fixture.terminal.registry().commitment(), original);
    assert_eq!(fixture.terminal.registered_disks().len(), 1);
    assert!(fixture.terminal.require_no_terminal_retirement().is_err());
    fixture
        .terminal
        .verify_chain_readmission_proposal(proposal.bytes(), &report.proposal_digest)
        .unwrap();
    let system = fixture.system();
    drop(fixture.terminal);
    let cold = TerminalManagement::open(&system).unwrap();
    let repeated = cold
        .prepare_chain_readmission_proposal(&fixture.config.disks[0].disk_id, "admin", operation)
        .unwrap();
    assert_eq!(repeated.bytes(), proposal.bytes());
    assert!(Engine::open(&system).is_err());
}

#[test]
fn chain_readmission_proposal_requires_complete_chain_and_disabled_metadata_identity() {
    let mut fixture = Fixture::with_action(MembershipExitAction::PlannedOffline);
    let target = fixture.config.disks[0].disk_id.clone();
    assert!(
        fixture
            .terminal
            .prepare_chain_readmission_proposal(&target, "admin", OperationId::generate().unwrap(),)
            .is_err()
    );
    let retired = fixture.plan();
    fixture
        .terminal
        .complete_terminal_retirement(&retired)
        .unwrap();
    for id in [
        &fixture.config.disks[1].disk_id,
        &fixture.config.disks[2].disk_id,
    ] {
        assert!(
            fixture
                .terminal
                .prepare_chain_readmission_proposal(id, "admin", OperationId::generate().unwrap(),)
                .is_err()
        );
    }
    assert!(
        fixture
            .terminal
            .prepare_chain_readmission_proposal(&target, "admin", retired.operation_id(),)
            .is_err()
    );
    let mut detached = Fixture::new();
    let retired = detached.plan();
    detached
        .terminal
        .complete_terminal_retirement(&retired)
        .unwrap();
    assert!(
        detached
            .terminal
            .prepare_chain_readmission_proposal(
                &detached.config.disks[0].disk_id,
                "admin",
                OperationId::generate().unwrap(),
            )
            .is_err()
    );
}

#[test]
fn chain_proposal_rejects_rehashed_target_stop_configuration_and_graph_changes() {
    let mut fixture = Fixture::with_action(MembershipExitAction::PlannedOffline);
    let retired = fixture.plan();
    fixture
        .terminal
        .complete_terminal_retirement(&retired)
        .unwrap();
    let proposal = fixture
        .terminal
        .prepare_chain_readmission_proposal(
            &fixture.config.disks[0].disk_id,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    let original: json_codec::Value = json_codec::from_slice(proposal.bytes()).unwrap();
    assert!(
        fixture
            .terminal
            .verify_chain_readmission_proposal(proposal.bytes(), &"0".repeat(64))
            .is_err()
    );
    for change in 0..5 {
        let mut body = original.clone();
        match change {
            0 => body["target"]["slot_id"] = "P99".into(),
            1 => {
                body["stopped"]["operation_id"] =
                    OperationId::generate().unwrap().to_string().into()
            }
            2 => body["configuration"]["disks"][1]["metadata_member"] = true.into(),
            3 => {
                body["objects"].as_object_mut().unwrap().clear();
            }
            _ => {
                body["source"]["frames"].as_array_mut().unwrap().clear();
            }
        }
        let digest = object_hash("terminal-chain-retained-readmission-proposal", &body).unwrap();
        assert!(
            fixture
                .terminal
                .verify_chain_readmission_proposal(&json_codec::to_vec(&body).unwrap(), &digest,)
                .is_err()
        );
    }
}

#[test]
fn chain_proposal_is_not_latest_or_reserved_after_another_retirement_starts() {
    let mut fixture = Fixture::with_action(MembershipExitAction::PlannedOffline);
    let retired = fixture.plan();
    fixture
        .terminal
        .complete_terminal_retirement(&retired)
        .unwrap();
    let proposal = fixture
        .terminal
        .prepare_chain_readmission_proposal(
            &fixture.config.disks[0].disk_id,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    let next = fixture
        .terminal
        .preview_terminal_retirement(
            &fixture.config.disks[2].disk_id,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    fixture.terminal.stage_terminal_retirement(&next).unwrap();
    assert!(
        fixture
            .terminal
            .prepare_chain_readmission_proposal(
                &fixture.config.disks[0].disk_id,
                "admin",
                proposal.preparation().operation_id,
            )
            .is_err()
    );
    assert!(
        fixture
            .terminal
            .verify_chain_readmission_proposal(
                proposal.bytes(),
                &proposal.preparation().proposal_digest,
            )
            .is_ok()
    );
    assert!(
        fixture
            .terminal
            .terminal_retirement_status()
            .unwrap()
            .is_some()
    );
    assert!(fixture.terminal.require_no_terminal_retirement().is_err());
}

#[test]
fn resume_inventory_recognizes_retirement_ownership_without_accepting_unknown_material() {
    let mut fixture = Fixture::with_action(MembershipExitAction::PlannedOffline);
    let retired = fixture.plan();
    fixture
        .terminal
        .complete_terminal_retirement(&retired)
        .unwrap();
    fixture.terminal.export_retirement_archive().unwrap();
    let unknown = fixture
        .terminal
        .store
        .operation()
        .path()
        .join("retirement-unselected.json");
    fs::write(&unknown, b"{}").unwrap();
    assert!(fixture.terminal.export_retirement_archive().is_err());
    let system = fixture.system();
    drop(fixture.terminal);
    assert!(TerminalManagement::open(&system).is_err());
    assert_eq!(fs::read(unknown).unwrap(), b"{}");
}

fn chain_target_fixture() -> (
    Fixture,
    OperationId,
    crate::TerminalChainReadmissionProposal,
) {
    let mut fixture = Fixture::with_action(MembershipExitAction::PlannedOffline);
    let retired = fixture.plan();
    fixture
        .terminal
        .complete_terminal_retirement(&retired)
        .unwrap();
    let operation = OperationId::generate().unwrap();
    let proposal = fixture
        .terminal
        .prepare_chain_readmission_proposal(&fixture.config.disks[0].disk_id, "admin", operation)
        .unwrap();
    (fixture, operation, proposal)
}
fn retained_chain_directory(fixture: &Fixture) -> crate::PendingMemberDirectory {
    crate::PendingMemberDirectory {
        disk_id: fixture.config.disks[0].disk_id.clone(),
        directory: fs::File::open(&fixture.config.disks[0].root).unwrap(),
    }
}

#[test]
fn actual_chain_target_decision_preserves_old_seal_and_never_admits_writer() {
    let (mut fixture, operation, proposal) = chain_target_fixture();
    let root = fixture.config.disks[0].root.clone();
    let seal = fs::read(root.join("disk.json")).unwrap();
    let before = fixture.terminal.registry().commitment().clone();
    let directory = retained_chain_directory(&fixture);
    let status = fixture
        .terminal
        .synchronize_chain_readmission_target(
            "admin",
            operation,
            &proposal.preparation().proposal_digest,
            directory,
        )
        .unwrap();
    assert_eq!(
        status.phase,
        crate::TerminalChainReadmissionPhase::TargetDecisionPreserved
    );
    assert!(status.target_reverified_this_call);
    assert!(status.source_request_reserved);
    assert!(status.target_decision_hash.is_some());
    assert!(!status.active_writer_admitted);
    assert!(!status.metadata_resumption_supported);
    assert_eq!(fs::read(root.join("disk.json")).unwrap(), seal);
    assert_eq!(*fixture.terminal.registry().commitment(), before);
    assert!(fixture.terminal.operation(operation).is_none());
    assert!(
        fixture
            .terminal
            .preview_terminal_retirement(
                &fixture.config.disks[2].disk_id,
                "admin",
                OperationId::generate().unwrap(),
            )
            .is_err()
    );
    assert!(fixture.terminal.export_retirement_archive().is_err());
    let vote_path = root
        .join("metadata/membership-decisions")
        .join(&status.selection_hash)
        .join("decision.json");
    let vote: json_codec::Value = json_codec::from_slice(&fs::read(&vote_path).unwrap()).unwrap();
    assert_eq!(vote["choice"]["kind"], "TerminalChainReadmission");
    let system = fixture.system();
    drop(fixture.terminal);
    let mut cold = TerminalManagement::open(&system).unwrap();
    let observed = cold.chain_readmission_status().unwrap().unwrap();
    assert_eq!(observed.selection_hash, status.selection_hash);
    assert!(!observed.target_reverified_this_call);
    let repeated = cold
        .synchronize_chain_readmission_target(
            "admin",
            operation,
            &proposal.preparation().proposal_digest,
            crate::PendingMemberDirectory {
                disk_id: fixture.config.disks[0].disk_id.clone(),
                directory: fs::File::open(root).unwrap(),
            },
        )
        .unwrap();
    assert_eq!(repeated.target_decision_hash, status.target_decision_hash);
    assert!(cold.writer_locks().is_err());
    drop(cold);
    assert!(Engine::open(&system).is_err());
}

#[test]
fn chain_target_status_does_not_open_missing_target_or_claim_fresh_durability() {
    let (mut fixture, operation, proposal) = chain_target_fixture();
    let directory = retained_chain_directory(&fixture);
    let result = fixture
        .terminal
        .synchronize_chain_readmission_target(
            "admin",
            operation,
            &proposal.preparation().proposal_digest,
            directory,
        )
        .unwrap();
    let system = fixture.system();
    drop(fixture.terminal);
    let root = &fixture.config.disks[0].root;
    fs::rename(root, root.with_extension("selected-missing")).unwrap();
    fs::write(root, b"do not probe from source-only status").unwrap();
    let cold = TerminalManagement::open(&system).unwrap();
    let status = cold.chain_readmission_status().unwrap().unwrap();
    assert_eq!(status.selection_hash, result.selection_hash);
    assert!(!status.target_reverified_this_call);
    assert!(!status.active_writer_admitted);
    assert_eq!(
        fs::read(root).unwrap(),
        b"do not probe from source-only status"
    );
}

#[test]
fn chain_target_original_request_survives_target_archive_failure_and_only_exact_retry() {
    let (mut fixture, operation, proposal) = chain_target_fixture();
    let root = fixture.config.disks[0].root.clone();
    // Replace only the target metadata namespace, not any source/archive data.
    fs::rename(root.join("metadata"), root.join("held-metadata")).unwrap();
    fs::write(root.join("metadata"), b"target copy unavailable").unwrap();
    let directory = retained_chain_directory(&fixture);
    assert!(
        fixture
            .terminal
            .synchronize_chain_readmission_target(
                "admin",
                operation,
                &proposal.preparation().proposal_digest,
                directory,
            )
            .is_err()
    );
    let selected = fixture
        .terminal
        .chain_readmission_status()
        .unwrap()
        .unwrap();
    assert_eq!(
        selected.phase,
        crate::TerminalChainReadmissionPhase::SourceSelected
    );
    assert!(!selected.target_reverified_this_call);
    assert!(selected.target_decision_hash.is_none());
    let directory = retained_chain_directory(&fixture);
    assert!(
        fixture
            .terminal
            .synchronize_chain_readmission_target(
                "other",
                operation,
                &proposal.preparation().proposal_digest,
                directory,
            )
            .is_err()
    );
    let directory = retained_chain_directory(&fixture);
    assert!(
        fixture
            .terminal
            .synchronize_chain_readmission_target(
                "admin",
                OperationId::generate().unwrap(),
                &proposal.preparation().proposal_digest,
                directory,
            )
            .is_err()
    );
    fs::remove_file(root.join("metadata")).unwrap();
    fs::rename(root.join("held-metadata"), root.join("metadata")).unwrap();
    let directory = retained_chain_directory(&fixture);
    let resumed = fixture
        .terminal
        .synchronize_chain_readmission_target(
            "admin",
            operation,
            &proposal.preparation().proposal_digest,
            directory,
        )
        .unwrap();
    assert_eq!(resumed.selection_hash, selected.selection_hash);
    assert_eq!(
        resumed.phase,
        crate::TerminalChainReadmissionPhase::TargetDecisionPreserved
    );
}

#[test]
fn chain_target_cannot_replace_the_original_decision_choice_or_physical_root() {
    let (mut fixture, operation, proposal) = chain_target_fixture();
    let directory = retained_chain_directory(&fixture);
    let status = fixture
        .terminal
        .synchronize_chain_readmission_target(
            "admin",
            operation,
            &proposal.preparation().proposal_digest,
            directory,
        )
        .unwrap();
    let root = fixture.config.disks[0].root.clone();
    let vote_path = root
        .join("metadata/membership-decisions")
        .join(&status.selection_hash)
        .join("decision.json");
    let mut vote: json_codec::Value =
        json_codec::from_slice(&fs::read(&vote_path).unwrap()).unwrap();
    vote["choice"]["kind"] = "TerminalResume".into();
    let corrupt = json_codec::to_vec(&vote).unwrap();
    fs::write(&vote_path, &corrupt).unwrap();
    let directory = retained_chain_directory(&fixture);
    assert!(
        fixture
            .terminal
            .synchronize_chain_readmission_target(
                "admin",
                operation,
                &proposal.preparation().proposal_digest,
                directory,
            )
            .is_err()
    );
    assert_eq!(fs::read(vote_path).unwrap(), corrupt);
    fs::rename(&root, root.with_extension("selected-original")).unwrap();
    fs::create_dir(&root).unwrap();
    let directory = retained_chain_directory(&fixture);
    assert!(
        fixture
            .terminal
            .synchronize_chain_readmission_target(
                "admin",
                operation,
                &proposal.preparation().proposal_digest,
                directory,
            )
            .is_err()
    );
    assert!(fixture.terminal.operation(operation).is_none());
}

fn prepare_chain_first_fixture() -> (
    Fixture,
    OperationId,
    crate::TerminalChainReadmissionProposal,
    crate::TerminalChainReadmissionStatus,
) {
    let (mut fixture, operation, proposal) = chain_target_fixture();
    let directory = retained_chain_directory(&fixture);
    fixture
        .terminal
        .synchronize_chain_readmission_target(
            "admin",
            operation,
            &proposal.preparation().proposal_digest,
            directory,
        )
        .unwrap();
    let directory = retained_chain_directory(&fixture);
    let status = fixture
        .terminal
        .prepare_chain_readmission_activation(
            "admin",
            operation,
            &proposal.preparation().proposal_digest,
            directory,
        )
        .unwrap();
    (fixture, operation, proposal, status)
}
fn chain_first_target(
    fixture: &Fixture,
    status: &crate::TerminalChainReadmissionStatus,
) -> PathBuf {
    fixture.config.disks[0]
        .root
        .join("metadata/membership-pending")
        .join(&status.selection_hash)
        .join("first-candidate")
}
fn chain_first_object(root: &std::path::Path, kind: &str, hash: &str) -> PathBuf {
    root.join("objects")
        .join(kind)
        .join(&hash[..2])
        .join(format!("{hash}.json"))
}

#[test]
fn chain_first_native_candidate_preserves_full_receipts_but_never_admits_live_state() {
    let (fixture, operation, _, status) = prepare_chain_first_fixture();
    assert_eq!(
        status.phase,
        crate::TerminalChainReadmissionPhase::FirstCandidatePrepared
    );
    assert!(!status.active_writer_admitted);
    assert!(!status.metadata_resumption_supported);
    let first = status.activation.as_ref().unwrap();
    assert!(first.source_material_reverified_this_call);
    assert!(first.target_material_reverified_this_call);
    assert!(!first.first_publication_recorded);
    let root = chain_first_target(&fixture, &status);
    let core = fs::read(chain_first_object(
        &root,
        "membership-generation",
        &first.core_hash,
    ))
    .unwrap();
    assert!(
        crate::decode_strict::<
            crate::engine::membership_transition::pending::activation::PortableAuthority,
        >(&core)
        .is_err()
    );
    let candidate: crate::journal::Candidate = crate::decode_strict(
        &fs::read(chain_first_object(
            &root,
            "candidate",
            &first.candidate_hash,
        ))
        .unwrap(),
    )
    .unwrap();
    assert_eq!(candidate.sequence, fixture.terminal.state().sequence + 1);
    assert_eq!(candidate.generation(), Some(first.core_hash.as_str()));
    assert_eq!(
        candidate.metadata_disk_ids,
        vec![fixture.config.disks[0].disk_id.clone()]
    );
    assert!(candidate.versions.is_empty());
    let read = |kind: &'static str, hash: &str| {
        let bytes = fs::read(chain_first_object(&root, kind, hash)).unwrap();
        crate::journal::AuthenticatedMetadataObject::authenticate(
            kind,
            hash,
            crate::strict_json(&bytes).unwrap(),
        )
    };
    let snapshot: crate::journal::Snapshot = json_codec::from_value(
        read("snapshot", &candidate.snapshot_hash)
            .unwrap()
            .into_value(),
    )
    .unwrap();
    let next = crate::journal::resolve_admitted_snapshot(snapshot, read).unwrap();
    for (id, result) in &fixture.terminal.state().operations {
        assert_eq!(next.state().operations.get(id), Some(result));
        assert_eq!(
            next.state().operation_principals.get(id),
            fixture.terminal.state().operation_principals.get(id)
        );
    }
    assert_eq!(
        next.state().operations.len(),
        fixture.terminal.state().operations.len() + 1
    );
    assert!(next.state().operations.contains_key(&operation));
    assert!(fixture.terminal.operation(operation).is_none());
    assert_eq!(
        next.state().business_generation,
        fixture.terminal.state().business_generation
    );
    assert_eq!(
        next.state().topology_generation,
        fixture.terminal.state().topology_generation + 1
    );
    assert!(fixture.terminal.writer_locks().is_err());
    assert!(
        !fixture.config.disks[0]
            .root
            .join("metadata/membership-generations")
            .join(&first.core_hash)
            .exists()
    );
}

#[test]
fn chain_first_cold_source_status_never_probes_missing_target_and_repeats_exact_inputs() {
    let (fixture, operation, proposal, status) = prepare_chain_first_fixture();
    let system = fixture.system();
    let target = fixture.config.disks[0].root.clone();
    let disk_id = fixture.config.disks[0].disk_id.clone();
    let first_hash = status.activation.as_ref().unwrap().candidate_hash.clone();
    let original_seal = fs::read(target.join("disk.json")).unwrap();
    drop(fixture.terminal);
    fs::rename(&target, target.with_extension("first-input-missing")).unwrap();
    fs::write(&target, b"source-only query must not probe").unwrap();
    let mut cold = TerminalManagement::open(&system).unwrap();
    let observed = cold.chain_readmission_status().unwrap().unwrap();
    assert_eq!(
        observed.phase,
        crate::TerminalChainReadmissionPhase::FirstCandidatePrepared
    );
    let first = observed.activation.unwrap();
    assert_eq!(first.candidate_hash, first_hash);
    assert!(first.source_material_reverified_this_call);
    assert!(!first.target_material_reverified_this_call);
    assert!(!observed.target_reverified_this_call);
    fs::remove_file(&target).unwrap();
    fs::rename(target.with_extension("first-input-missing"), &target).unwrap();
    let repeated = cold
        .prepare_chain_readmission_activation(
            "admin",
            operation,
            &proposal.preparation().proposal_digest,
            crate::PendingMemberDirectory {
                disk_id,
                directory: fs::File::open(&target).unwrap(),
            },
        )
        .unwrap();
    assert_eq!(repeated.activation.unwrap().candidate_hash, first_hash);
    assert_eq!(fs::read(target.join("disk.json")).unwrap(), original_seal);
    assert!(cold.operation(operation).is_none());
    drop(cold);
    assert!(Engine::open(&system).is_err());
}

#[test]
fn completed_chain_first_target_damage_is_not_repaired_from_system_copy() {
    let (mut fixture, operation, proposal, status) = prepare_chain_first_fixture();
    let root = chain_first_target(&fixture, &status);
    let path = chain_first_object(
        &root,
        "candidate",
        &status.activation.as_ref().unwrap().candidate_hash,
    );
    fs::remove_file(&path).unwrap();
    let observed = fixture
        .terminal
        .chain_readmission_status()
        .unwrap()
        .unwrap();
    assert!(
        !observed
            .activation
            .unwrap()
            .target_material_reverified_this_call
    );
    let directory = retained_chain_directory(&fixture);
    assert!(
        fixture
            .terminal
            .prepare_chain_readmission_activation(
                "admin",
                operation,
                &proposal.preparation().proposal_digest,
                directory,
            )
            .is_err()
    );
    assert!(!path.exists());
    assert!(fixture.terminal.operation(operation).is_none());
}

#[test]
fn chain_first_partial_inputs_retain_exact_selection_and_only_original_retry() {
    let (mut fixture, operation, proposal) = chain_target_fixture();
    let directory = retained_chain_directory(&fixture);
    let status = fixture
        .terminal
        .synchronize_chain_readmission_target(
            "admin",
            operation,
            &proposal.preparation().proposal_digest,
            directory,
        )
        .unwrap();
    let path = chain_first_target(&fixture, &status);
    fs::write(&path, b"leave obstruction intact").unwrap();
    let directory = retained_chain_directory(&fixture);
    assert!(
        fixture
            .terminal
            .prepare_chain_readmission_activation(
                "admin",
                operation,
                &proposal.preparation().proposal_digest,
                directory,
            )
            .is_err()
    );
    assert_eq!(fs::read(&path).unwrap(), b"leave obstruction intact");
    let selected = fixture
        .terminal
        .chain_readmission_status()
        .unwrap()
        .unwrap();
    assert_eq!(
        selected.phase,
        crate::TerminalChainReadmissionPhase::ActivationSelected
    );
    assert!(
        !selected
            .activation
            .as_ref()
            .unwrap()
            .source_material_reverified_this_call
    );
    let directory = retained_chain_directory(&fixture);
    assert!(
        fixture
            .terminal
            .prepare_chain_readmission_activation(
                "other",
                operation,
                &proposal.preparation().proposal_digest,
                directory,
            )
            .is_err()
    );
    fs::remove_file(&path).unwrap();
    let system = fixture.system();
    let target = fixture.config.disks[0].root.clone();
    let disk_id = fixture.config.disks[0].disk_id.clone();
    drop(fixture.terminal);
    let mut cold = TerminalManagement::open(&system).unwrap();
    let continued = cold
        .prepare_chain_readmission_activation(
            "admin",
            operation,
            &proposal.preparation().proposal_digest,
            crate::PendingMemberDirectory {
                disk_id,
                directory: fs::File::open(target).unwrap(),
            },
        )
        .unwrap();
    assert_eq!(
        continued.activation.unwrap().candidate_hash,
        selected.activation.unwrap().candidate_hash
    );
    assert!(cold.operation(operation).is_none());
}

#[test]
fn chain_first_changed_source_selection_rejects_cold_recovery_without_overwrite() {
    let (fixture, _, _, _) = prepare_chain_first_fixture();
    let selected = fixture
        .terminal
        .store
        .operation()
        .path()
        .join("chain-first-selected.json");
    let mut value: json_codec::Value = crate::decode_strict(&fs::read(&selected).unwrap()).unwrap();
    value["core_hash"] = "0".repeat(64).into();
    let bytes = crate::canonical_json(&value).unwrap();
    fs::write(&selected, &bytes).unwrap();
    let system = fixture.system();
    drop(fixture.terminal);
    assert!(TerminalManagement::open(&system).is_err());
    assert_eq!(fs::read(selected).unwrap(), bytes);
}

fn published_chain_first_fixture() -> (
    Fixture,
    OperationId,
    crate::TerminalChainReadmissionProposal,
    crate::TerminalChainReadmissionStatus,
) {
    let (mut fixture, operation, proposal, _) = prepare_chain_first_fixture();
    let directory = retained_chain_directory(&fixture);
    let status = fixture
        .terminal
        .publish_chain_readmission_first(
            "admin",
            operation,
            &proposal.preparation().proposal_digest,
            directory,
        )
        .unwrap();
    (fixture, operation, proposal, status)
}
#[test]
fn chain_first_publication_uses_actual_native_three_frames_but_not_active_writer() {
    let (fixture, operation, _, status) = published_chain_first_fixture();
    assert_eq!(
        status.phase,
        crate::TerminalChainReadmissionPhase::FirstPublicationPreserved
    );
    assert!(
        status
            .activation
            .as_ref()
            .unwrap()
            .first_publication_recorded
    );
    let publication = status.publication.as_ref().unwrap();
    assert!(publication.target_reverified_this_call);
    assert!(!publication.active_current_admitted);
    assert!(!status.active_writer_admitted);
    assert!(!status.metadata_resumption_supported);
    let root = chain_first_target(&fixture, &status);
    let disk = crate::disk::DiskRoot::open(&root).unwrap();
    let scan =
        crate::journal::scan_path_at(&disk, &root.join("first-publication/journal.bin")).unwrap();
    assert!(!scan.torn_tail);
    assert_eq!(scan.records.len(), 3);
    assert_eq!(
        scan.records[0].0.record_type,
        crate::journal::RecordType::Accept
    );
    assert_eq!(
        scan.records[1].0.record_type,
        crate::journal::RecordType::Finalize
    );
    assert_eq!(
        scan.records[2].0.record_type,
        crate::journal::RecordType::Publication
    );
    assert_eq!(scan.records[0].1, publication.accept_frame_hash);
    assert_eq!(scan.records[1].1, publication.finalize_frame_hash);
    assert_eq!(scan.records[2].1, publication.publication_frame_hash);
    assert_eq!(scan.records[2].0.object_hash, publication.publication_hash);
    assert!(fixture.terminal.operation(operation).is_none());
    assert!(fixture.terminal.writer_locks().is_err());
    assert!(
        !fixture.config.disks[0]
            .root
            .join("metadata/membership-generations")
            .join(&status.activation.unwrap().core_hash)
            .exists()
    );
}
#[test]
fn chain_publication_cold_source_status_is_historical_and_original_retry_does_not_append_again() {
    let (fixture, operation, proposal, status) = published_chain_first_fixture();
    let system = fixture.system();
    let target = fixture.config.disks[0].root.clone();
    let disk_id = fixture.config.disks[0].disk_id.clone();
    let log_path = chain_first_target(&fixture, &status).join("first-publication/journal.bin");
    let bytes = fs::read(&log_path).unwrap();
    let seal = fs::read(target.join("disk.json")).unwrap();
    drop(fixture.terminal);
    fs::rename(&target, target.with_extension("published-missing")).unwrap();
    fs::write(&target, b"publication status must not open target").unwrap();
    let mut cold = TerminalManagement::open(&system).unwrap();
    let observed = cold.chain_readmission_status().unwrap().unwrap();
    assert!(!observed.target_reverified_this_call);
    assert!(
        !observed
            .publication
            .as_ref()
            .unwrap()
            .target_reverified_this_call
    );
    assert_eq!(
        observed.publication.unwrap().publication_hash,
        status.publication.as_ref().unwrap().publication_hash
    );
    fs::remove_file(&target).unwrap();
    fs::rename(target.with_extension("published-missing"), &target).unwrap();
    let repeated = cold
        .publish_chain_readmission_first(
            "admin",
            operation,
            &proposal.preparation().proposal_digest,
            crate::PendingMemberDirectory {
                disk_id,
                directory: fs::File::open(&target).unwrap(),
            },
        )
        .unwrap();
    assert!(repeated.publication.unwrap().target_reverified_this_call);
    assert_eq!(fs::read(&log_path).unwrap(), bytes);
    assert_eq!(fs::read(target.join("disk.json")).unwrap(), seal);
    assert!(cold.operation(operation).is_none());
    drop(cold);
    assert!(Engine::open(&system).is_err());
}
#[test]
fn chain_publication_repairs_only_the_exact_unfinished_third_frame() {
    let (fixture, operation, proposal, status) = published_chain_first_fixture();
    let system = fixture.system();
    let target = fixture.config.disks[0].root.clone();
    let disk_id = fixture.config.disks[0].disk_id.clone();
    let target_bundle = chain_first_target(&fixture, &status);
    let source_operation = fixture.terminal.store.operation().path().to_path_buf();
    let source_bundle = source_operation.join("chain-first");
    let log = target_bundle.join("first-publication/journal.bin");
    let bytes = fs::read(&log).unwrap();
    let mut reader = crate::journal::JournalFrameReader::new(std::io::Cursor::new(bytes.clone()));
    reader.next_record().unwrap().unwrap();
    reader.next_record().unwrap().unwrap();
    let through_finalize = reader.valid_bytes as usize;
    // Model a crash during the known third frame, before seal/target/SYSTEM receipts.
    fs::write(&log, &bytes[..through_finalize + 7]).unwrap();
    let seal_hash = &status.publication.as_ref().unwrap().seal_hash;
    for bundle in [&source_bundle, &target_bundle] {
        fs::remove_file(chain_first_object(bundle, "publication-seal", seal_hash)).unwrap();
    }
    fs::remove_file(target_bundle.join("published.json")).unwrap();
    fs::remove_file(source_operation.join("chain-publication.json")).unwrap();
    drop(fixture.terminal);
    let mut cold = TerminalManagement::open(&system).unwrap();
    assert_eq!(
        cold.chain_readmission_status().unwrap().unwrap().phase,
        crate::TerminalChainReadmissionPhase::FirstPublicationPending
    );
    let resumed = cold
        .publish_chain_readmission_first(
            "admin",
            operation,
            &proposal.preparation().proposal_digest,
            crate::PendingMemberDirectory {
                disk_id,
                directory: fs::File::open(target).unwrap(),
            },
        )
        .unwrap();
    assert_eq!(
        resumed.publication.unwrap().publication_hash,
        status.publication.unwrap().publication_hash
    );
    assert_eq!(fs::read(log).unwrap(), bytes);
    assert!(cold.operation(operation).is_none());
}
#[test]
fn completed_chain_publication_missing_journal_is_not_recreated() {
    let (mut fixture, operation, proposal, status) = published_chain_first_fixture();
    let log = chain_first_target(&fixture, &status).join("first-publication/journal.bin");
    fs::remove_file(&log).unwrap();
    let directory = retained_chain_directory(&fixture);
    assert!(
        fixture
            .terminal
            .publish_chain_readmission_first(
                "admin",
                operation,
                &proposal.preparation().proposal_digest,
                directory,
            )
            .is_err()
    );
    assert!(!log.exists());
    let directory = retained_chain_directory(&fixture);
    assert!(
        fixture
            .terminal
            .prepare_chain_readmission_activation(
                "admin",
                operation,
                &proposal.preparation().proposal_digest,
                directory,
            )
            .is_err()
    );
    assert!(!log.exists());
    let observed = fixture
        .terminal
        .chain_readmission_status()
        .unwrap()
        .unwrap();
    assert!(!observed.publication.unwrap().target_reverified_this_call);
    assert!(fixture.terminal.operation(operation).is_none());
}
#[test]
fn chain_publication_requires_exact_completed_inputs_before_selecting_publication() {
    let (mut fixture, operation, proposal) = chain_target_fixture();
    let directory = retained_chain_directory(&fixture);
    fixture
        .terminal
        .synchronize_chain_readmission_target(
            "admin",
            operation,
            &proposal.preparation().proposal_digest,
            directory,
        )
        .unwrap();
    let directory = retained_chain_directory(&fixture);
    assert!(
        fixture
            .terminal
            .publish_chain_readmission_first(
                "admin",
                operation,
                &proposal.preparation().proposal_digest,
                directory,
            )
            .is_err()
    );
    assert!(
        !fixture
            .terminal
            .store
            .operation()
            .path()
            .join("chain-publication-selected.json")
            .exists()
    );
    assert!(
        fixture
            .terminal
            .chain_readmission_status()
            .unwrap()
            .unwrap()
            .publication
            .is_none()
    );
}

#[test]
fn chain_published_current_reads_actual_projection_but_preserves_stop_and_live_state() {
    let (fixture, operation, proposal, status) = published_chain_first_fixture();
    let root = chain_first_target(&fixture, &status);
    let journal = root.join("first-publication/journal.bin");
    let prior_log = fs::read(&journal).unwrap();
    let prior_seal = fs::read(fixture.config.disks[0].root.join("disk.json")).unwrap();
    let branch_ids: Vec<_> = fixture.terminal.state().branches.keys().cloned().collect();
    let directory_ids: Vec<_> = fixture
        .terminal
        .state()
        .directory_branches
        .keys()
        .cloned()
        .collect();
    let directory = retained_chain_directory(&fixture);
    let reader = fixture
        .terminal
        .open_chain_published_current(
            "admin",
            operation,
            &proposal.preparation().proposal_digest,
            directory,
        )
        .unwrap();
    let report = reader.report().unwrap();
    assert_eq!(report.registry, status.preparation.proposed_registry);
    assert_eq!(
        report.publication_hash,
        status.publication.as_ref().unwrap().publication_hash
    );
    assert_eq!(
        report.sequence,
        status.activation.as_ref().unwrap().sequence
    );
    assert_eq!(report.file_branch_count, branch_ids.len());
    assert!(report.frozen_namespace_admitted);
    assert!(report.target_reverified_this_call);
    assert!(!report.active_engine_admitted);
    assert!(!report.target_writer_admitted);
    assert!(!report.stop_consumed);
    let mut files = 0_u64;
    for branch in branch_ids {
        files += reader.branch_entries(&branch).unwrap().len() as u64;
    }
    let mut directories = 0_u64;
    for branch in directory_ids {
        directories += reader.directory_entries(&branch).unwrap().len() as u64;
    }
    assert_eq!(report.visible_file_branch_entries, files);
    assert_eq!(report.visible_directory_branch_entries, directories);
    assert_eq!(
        reader.branch_entries("absent").unwrap_err().code(),
        ErrorCode::NotFound
    );
    drop(reader);
    assert_eq!(fs::read(journal).unwrap(), prior_log);
    assert_eq!(
        fs::read(fixture.config.disks[0].root.join("disk.json")).unwrap(),
        prior_seal
    );
    assert!(fixture.terminal.operation(operation).is_none());
    assert!(fixture.terminal.writer_locks().is_err());
    assert!(
        !fixture.config.disks[0]
            .root
            .join("metadata/membership-generations")
            .join(report.core_hash)
            .exists()
    );
}

#[test]
fn chain_published_current_never_promotes_prepared_candidate_or_repairs_missing_log() {
    let (fixture, operation, proposal, status) = prepare_chain_first_fixture();
    let directory = retained_chain_directory(&fixture);
    assert!(
        fixture
            .terminal
            .open_chain_published_current(
                "admin",
                operation,
                &proposal.preparation().proposal_digest,
                directory,
            )
            .is_err()
    );
    let root = chain_first_target(&fixture, &status);
    assert!(!root.join("first-publication").exists());
    drop(fixture);
    let (fixture, operation, proposal, status) = published_chain_first_fixture();
    let root = chain_first_target(&fixture, &status);
    let journal = root.join("first-publication/journal.bin");
    fs::remove_file(&journal).unwrap();
    let directory = retained_chain_directory(&fixture);
    assert!(
        fixture
            .terminal
            .open_chain_published_current(
                "admin",
                operation,
                &proposal.preparation().proposal_digest,
                directory,
            )
            .is_err()
    );
    assert!(!journal.exists());
    assert!(fixture.terminal.operation(operation).is_none());
}

#[test]
fn chain_published_current_has_no_source_only_fallback_and_reopens_original_cold_target() {
    let (fixture, operation, proposal, status) = published_chain_first_fixture();
    let system = fixture.system();
    let root = fixture.config.disks[0].root.clone();
    let disk_id = fixture.config.disks[0].disk_id.clone();
    drop(fixture.terminal);
    fs::rename(&root, root.with_extension("published-current-held")).unwrap();
    fs::write(&root, b"not the selected original directory").unwrap();
    let cold = TerminalManagement::open(&system).unwrap();
    assert_eq!(
        cold.chain_readmission_status().unwrap().unwrap().phase,
        crate::TerminalChainReadmissionPhase::FirstPublicationPreserved
    );
    assert!(
        cold.open_chain_published_current(
            "admin",
            operation,
            &proposal.preparation().proposal_digest,
            crate::PendingMemberDirectory {
                disk_id: disk_id.clone(),
                directory: fs::File::open(root.with_extension("published-current-held")).unwrap(),
            },
        )
        .is_err()
    );
    fs::remove_file(&root).unwrap();
    fs::rename(root.with_extension("published-current-held"), &root).unwrap();
    let reader = cold
        .open_chain_published_current(
            "admin",
            operation,
            &proposal.preparation().proposal_digest,
            crate::PendingMemberDirectory {
                disk_id,
                directory: fs::File::open(&root).unwrap(),
            },
        )
        .unwrap();
    assert_eq!(
        reader.report().unwrap().seal_hash,
        status.publication.unwrap().seal_hash
    );
    drop(reader);
    assert!(cold.operation(operation).is_none());
    drop(cold);
    assert!(Engine::open(&system).is_err());
}

#[test]
fn chain_published_current_rejects_changed_native_inputs_without_overwriting_them() {
    let (fixture, operation, proposal, status) = published_chain_first_fixture();
    let root = chain_first_target(&fixture, &status);
    let path = chain_first_object(&root, "snapshot", &status.activation.unwrap().snapshot_hash);
    fs::write(&path, b"damaged actual native snapshot").unwrap();
    let directory = retained_chain_directory(&fixture);
    assert!(
        fixture
            .terminal
            .open_chain_published_current(
                "admin",
                operation,
                &proposal.preparation().proposal_digest,
                directory,
            )
            .is_err()
    );
    assert_eq!(fs::read(path).unwrap(), b"damaged actual native snapshot");
    assert!(fixture.terminal.operation(operation).is_none());
}

#[test]
fn chain_published_current_holds_exact_target_and_rejects_other_actor() {
    let (fixture, operation, proposal, _) = published_chain_first_fixture();
    let directory = retained_chain_directory(&fixture);
    assert!(
        fixture
            .terminal
            .open_chain_published_current(
                "other",
                operation,
                &proposal.preparation().proposal_digest,
                directory,
            )
            .is_err()
    );
    let directory = retained_chain_directory(&fixture);
    let reader = fixture
        .terminal
        .open_chain_published_current(
            "admin",
            operation,
            &proposal.preparation().proposal_digest,
            directory,
        )
        .unwrap();
    let root = fixture.config.disks[0].root.clone();
    fs::rename(&root, root.with_extension("reader-held")).unwrap();
    fs::create_dir(&root).unwrap();
    assert!(reader.report().is_err());
    assert!(reader.branch_entries("absent").is_err());
    drop(reader);
    fs::remove_dir(&root).unwrap();
    fs::rename(root.with_extension("reader-held"), &root).unwrap();
}

fn locator_bytes(system: &std::path::Path) -> std::collections::BTreeMap<String, Vec<u8>> {
    fs::read_dir(system.join("bootstrap-discovery"))
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            (
                entry.file_name().into_string().unwrap(),
                fs::read(entry.path()).unwrap(),
            )
        })
        .collect()
}

#[test]
fn chain_epoch_plan_selects_exact_new_lifetime_but_never_updates_locator_or_stop() {
    let (mut fixture, operation, proposal, before) = published_chain_first_fixture();
    let system = fixture.system();
    let locator = locator_bytes(&system);
    let old_seal = fs::read(fixture.config.disks[0].root.join("disk.json")).unwrap();
    let directory = retained_chain_directory(&fixture);
    let status = fixture
        .terminal
        .prepare_chain_activation_plan(
            "admin",
            operation,
            &proposal.preparation().proposal_digest,
            directory,
        )
        .unwrap();
    assert_eq!(
        status.phase,
        crate::TerminalChainReadmissionPhase::ActivationPlanSelected
    );
    let plan = status.activation_plan.unwrap();
    assert_eq!(plan.core_hash, before.activation.unwrap().core_hash);
    assert_eq!(
        plan.publication_hash,
        before.publication.unwrap().publication_hash
    );
    assert_ne!(
        Some(plan.planned_io_epoch.as_str()),
        plan.previous_io_epoch.as_deref()
    );
    assert_eq!(
        plan.planned_locator_generation,
        plan.previous_locator_generation + 1
    );
    assert_ne!(plan.previous_locator_hash, plan.planned_locator_hash);
    assert!(plan.target_reverified_this_call);
    assert!(!plan.locator_installed);
    assert!(!plan.stop_consumed);
    assert!(!plan.active_writer_admitted);
    assert_eq!(locator_bytes(&system), locator);
    assert_eq!(
        fs::read(fixture.config.disks[0].root.join("disk.json")).unwrap(),
        old_seal
    );
    assert!(fixture.terminal.operation(operation).is_none());
    assert!(fixture.terminal.writer_locks().is_err());
    let selected: json_codec::Value = crate::decode_strict(
        &fs::read(
            fixture
                .terminal
                .store
                .operation()
                .path()
                .join("chain-activation-plan.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(selected["planned_epoch"], plan.planned_io_epoch);
    let event = selected["bootstrap"]["next"]["readmissions"]
        .as_array()
        .unwrap()
        .last()
        .unwrap();
    assert_eq!(event["intent_digest"], plan.planned_io_epoch);
    assert_eq!(event["stopped_intent"], plan.stopped_transition);
}

#[test]
fn chain_epoch_plan_cold_query_is_source_only_and_exact_target_retry_keeps_bytes() {
    let (mut fixture, operation, proposal, _) = published_chain_first_fixture();
    let directory = retained_chain_directory(&fixture);
    let status = fixture
        .terminal
        .prepare_chain_activation_plan(
            "admin",
            operation,
            &proposal.preparation().proposal_digest,
            directory,
        )
        .unwrap();
    let path = fixture
        .terminal
        .store
        .operation()
        .path()
        .join("chain-activation-plan.json");
    let bytes = fs::read(&path).unwrap();
    let system = fixture.system();
    let locator = locator_bytes(&system);
    let root = fixture.config.disks[0].root.clone();
    let disk_id = fixture.config.disks[0].disk_id.clone();
    drop(fixture.terminal);
    fs::rename(&root, root.with_extension("epoch-plan-held")).unwrap();
    fs::write(&root, b"source query must not probe").unwrap();
    let mut cold = TerminalManagement::open(&system).unwrap();
    let queried = cold.chain_readmission_status().unwrap().unwrap();
    let report = queried.activation_plan.unwrap();
    assert_eq!(
        report.planned_io_epoch,
        status.activation_plan.unwrap().planned_io_epoch
    );
    assert!(!report.target_reverified_this_call);
    assert!(!report.locator_installed);
    fs::remove_file(&root).unwrap();
    fs::rename(root.with_extension("epoch-plan-held"), &root).unwrap();
    let repeated = cold
        .prepare_chain_activation_plan(
            "admin",
            operation,
            &proposal.preparation().proposal_digest,
            crate::PendingMemberDirectory {
                disk_id,
                directory: fs::File::open(&root).unwrap(),
            },
        )
        .unwrap();
    assert!(
        repeated
            .activation_plan
            .unwrap()
            .target_reverified_this_call
    );
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert_eq!(locator_bytes(&system), locator);
    assert!(cold.operation(operation).is_none());
    drop(cold);
    assert!(Engine::open(&system).is_err());
}

#[test]
fn chain_epoch_plan_needs_actual_publication_and_does_not_recreate_a_missing_target_log() {
    let (mut fixture, operation, proposal, _) = prepare_chain_first_fixture();
    let directory = retained_chain_directory(&fixture);
    assert!(
        fixture
            .terminal
            .prepare_chain_activation_plan(
                "admin",
                operation,
                &proposal.preparation().proposal_digest,
                directory,
            )
            .is_err()
    );
    assert!(
        !fixture
            .terminal
            .store
            .operation()
            .path()
            .join("chain-activation-plan.json")
            .exists()
    );
    drop(fixture);
    let (mut fixture, operation, proposal, status) = published_chain_first_fixture();
    let directory = retained_chain_directory(&fixture);
    fixture
        .terminal
        .prepare_chain_activation_plan(
            "admin",
            operation,
            &proposal.preparation().proposal_digest,
            directory,
        )
        .unwrap();
    let journal = chain_first_target(&fixture, &status).join("first-publication/journal.bin");
    fs::remove_file(&journal).unwrap();
    let locator = locator_bytes(&fixture.system());
    let directory = retained_chain_directory(&fixture);
    assert!(
        fixture
            .terminal
            .prepare_chain_activation_plan(
                "admin",
                operation,
                &proposal.preparation().proposal_digest,
                directory,
            )
            .is_err()
    );
    assert!(!journal.exists());
    assert_eq!(locator_bytes(&fixture.system()), locator);
}

#[test]
fn chain_epoch_plan_cold_recovery_rejects_changed_old_stops_or_future_epoch() {
    for field in ["old-stops", "epoch"] {
        let (mut fixture, operation, proposal, _) = published_chain_first_fixture();
        let directory = retained_chain_directory(&fixture);
        fixture
            .terminal
            .prepare_chain_activation_plan(
                "admin",
                operation,
                &proposal.preparation().proposal_digest,
                directory,
            )
            .unwrap();
        let path = fixture
            .terminal
            .store
            .operation()
            .path()
            .join("chain-activation-plan.json");
        let mut plan: json_codec::Value = crate::decode_strict(&fs::read(&path).unwrap()).unwrap();
        if field == "old-stops" {
            plan["bootstrap"]["next"]["stops"] = json_codec::json!([]);
        } else {
            plan["planned_epoch"] = "0".repeat(64).into();
        }
        let bytes = crate::canonical_json(&plan).unwrap();
        fs::write(&path, &bytes).unwrap();
        let system = fixture.system();
        let locator = locator_bytes(&system);
        drop(fixture.terminal);
        assert!(TerminalManagement::open(&system).is_err());
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert_eq!(locator_bytes(&system), locator);
    }
}

#[test]
fn chain_epoch_plan_refuses_other_actor_and_never_mints_an_active_probe() {
    let (mut fixture, operation, proposal, _) = published_chain_first_fixture();
    let directory = retained_chain_directory(&fixture);
    assert!(
        fixture
            .terminal
            .prepare_chain_activation_plan(
                "other",
                operation,
                &proposal.preparation().proposal_digest,
                directory,
            )
            .is_err()
    );
    let path = fixture
        .terminal
        .store
        .operation()
        .path()
        .join("chain-activation-plan.json");
    assert!(!path.exists());
    assert!(fixture.terminal.operation(operation).is_none());
}

fn planned_chain_io_fixture() -> (
    Fixture,
    OperationId,
    crate::TerminalChainReadmissionProposal,
    crate::TerminalChainReadmissionStatus,
) {
    let (mut fixture, operation, proposal, _) = published_chain_first_fixture();
    let directory = retained_chain_directory(&fixture);
    let status = fixture
        .terminal
        .prepare_chain_activation_plan(
            "admin",
            operation,
            &proposal.preparation().proposal_digest,
            directory,
        )
        .unwrap();
    (fixture, operation, proposal, status)
}

#[test]
fn chain_io_decision_is_actual_native_choice_but_preserves_stop_log_and_live_state() {
    let (mut fixture, operation, proposal, planned) = planned_chain_io_fixture();
    let root = fixture.config.disks[0].root.clone();
    let original_vote = root
        .join("metadata/membership-decisions")
        .join(&planned.selection_hash)
        .join("decision.json");
    let original_vote_bytes = fs::read(&original_vote).unwrap();
    let journal = chain_first_target(&fixture, &planned).join("first-publication/journal.bin");
    let journal_bytes = fs::read(&journal).unwrap();
    let seal = fs::read(root.join("disk.json")).unwrap();
    let locator = locator_bytes(&fixture.system());
    let directory = retained_chain_directory(&fixture);
    let status = fixture
        .terminal
        .decide_chain_io_admission(
            "admin",
            operation,
            &proposal.preparation().proposal_digest,
            directory,
        )
        .unwrap();
    assert_eq!(
        status.phase,
        crate::TerminalChainReadmissionPhase::IoDecisionPreserved
    );
    let report = status.io_decision.unwrap();
    let plan = planned.activation_plan.unwrap();
    assert_eq!(report.io_epoch, plan.planned_io_epoch);
    assert_eq!(report.publication_hash, plan.publication_hash);
    assert!(report.target_reverified_this_call);
    assert!(!report.locator_installed && !report.stop_consumed && !report.active_writer_admitted);
    assert!(!status.active_writer_admitted && !status.metadata_resumption_supported);
    let actual: json_codec::Value = crate::decode_strict(
        &fs::read(
            root.join("metadata/membership-decisions")
                .join(&report.io_epoch)
                .join("decision.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(actual["choice"]["kind"], "TerminalChainIoAdmission");
    assert_eq!(actual["choice"]["core_hash"], report.plan_hash);
    assert_eq!(actual["transition_hash"], report.io_epoch);
    assert_eq!(fs::read(&original_vote).unwrap(), original_vote_bytes);
    assert_eq!(fs::read(&journal).unwrap(), journal_bytes);
    assert_eq!(fs::read(root.join("disk.json")).unwrap(), seal);
    assert_eq!(locator_bytes(&fixture.system()), locator);
    assert!(fixture.terminal.operation(operation).is_none());
    assert!(fixture.terminal.writer_locks().is_err());
}

#[test]
fn chain_io_decision_cold_status_does_not_probe_and_exact_retry_keeps_vote() {
    let (mut fixture, operation, proposal, _) = planned_chain_io_fixture();
    let directory = retained_chain_directory(&fixture);
    let status = fixture
        .terminal
        .decide_chain_io_admission(
            "admin",
            operation,
            &proposal.preparation().proposal_digest,
            directory,
        )
        .unwrap();
    let report = status.io_decision.unwrap();
    let root = fixture.config.disks[0].root.clone();
    let disk = fixture.config.disks[0].disk_id.clone();
    let vote = root
        .join("metadata/membership-decisions")
        .join(&report.io_epoch)
        .join("decision.json");
    let bytes = fs::read(&vote).unwrap();
    let system = fixture.system();
    let locator = locator_bytes(&system);
    drop(fixture.terminal);
    fs::rename(&root, root.with_extension("io-decided-held")).unwrap();
    fs::write(&root, b"source-only status must not probe this replacement").unwrap();
    let mut cold = TerminalManagement::open(&system).unwrap();
    let status = cold.chain_readmission_status().unwrap().unwrap();
    assert_eq!(
        status.phase,
        crate::TerminalChainReadmissionPhase::IoDecisionPreserved
    );
    assert!(!status.io_decision.unwrap().target_reverified_this_call);
    fs::remove_file(&root).unwrap();
    fs::rename(root.with_extension("io-decided-held"), &root).unwrap();
    let repeated = cold
        .decide_chain_io_admission(
            "admin",
            operation,
            &proposal.preparation().proposal_digest,
            crate::PendingMemberDirectory {
                disk_id: disk,
                directory: fs::File::open(&root).unwrap(),
            },
        )
        .unwrap();
    assert_eq!(
        repeated.io_decision.unwrap().decision_hash,
        report.decision_hash
    );
    assert_eq!(fs::read(&vote).unwrap(), bytes);
    assert_eq!(locator_bytes(&system), locator);
    assert!(cold.operation(operation).is_none());
    drop(cold);
    assert!(Engine::open(&system).is_err());
}

#[test]
fn chain_io_decision_cold_retry_finishes_only_the_missing_source_receipt() {
    let (mut fixture, operation, proposal, _) = planned_chain_io_fixture();
    let directory = retained_chain_directory(&fixture);
    let status = fixture
        .terminal
        .decide_chain_io_admission(
            "admin",
            operation,
            &proposal.preparation().proposal_digest,
            directory,
        )
        .unwrap();
    let report = status.io_decision.unwrap();
    let complete = fixture
        .terminal
        .store
        .operation()
        .path()
        .join("chain-io-decision.json");
    let expected = fs::read(&complete).unwrap();
    fs::remove_file(&complete).unwrap();
    let root = fixture.config.disks[0].root.clone();
    let disk = fixture.config.disks[0].disk_id.clone();
    let vote = root
        .join("metadata/membership-decisions")
        .join(&report.io_epoch)
        .join("decision.json");
    let bytes = fs::read(&vote).unwrap();
    let system = fixture.system();
    drop(fixture.terminal);
    let mut cold = TerminalManagement::open(&system).unwrap();
    let status = cold.chain_readmission_status().unwrap().unwrap();
    assert_eq!(
        status.phase,
        crate::TerminalChainReadmissionPhase::IoDecisionPending
    );
    assert!(status.io_decision.is_none());
    assert!(!complete.exists());
    cold.decide_chain_io_admission(
        "admin",
        operation,
        &proposal.preparation().proposal_digest,
        crate::PendingMemberDirectory {
            disk_id: disk,
            directory: fs::File::open(&root).unwrap(),
        },
    )
    .unwrap();
    assert_eq!(fs::read(&complete).unwrap(), expected);
    assert_eq!(fs::read(&vote).unwrap(), bytes);
}

#[test]
fn chain_io_decision_completed_missing_evidence_is_not_recreated_or_revoted() {
    for missing in [
        "target-plan",
        "target-completion",
        "native-vote",
        "published-log",
    ] {
        let (mut fixture, operation, proposal, _) = planned_chain_io_fixture();
        let directory = retained_chain_directory(&fixture);
        let status = fixture
            .terminal
            .decide_chain_io_admission(
                "admin",
                operation,
                &proposal.preparation().proposal_digest,
                directory,
            )
            .unwrap();
        let report = status.io_decision.as_ref().unwrap();
        let root = fixture.config.disks[0].root.clone();
        let parent = root
            .join("metadata/membership-pending")
            .join(&status.selection_hash);
        let missing = match missing {
            "target-plan" => parent.join("io-admission-plan.json"),
            "target-completion" => parent.join("io-admission-decision.json"),
            "native-vote" => root
                .join("metadata/membership-decisions")
                .join(&report.io_epoch)
                .join("decision.json"),
            "published-log" => {
                chain_first_target(&fixture, &status).join("first-publication/journal.bin")
            }
            _ => unreachable!(),
        };
        fs::remove_file(&missing).unwrap();
        let locator = locator_bytes(&fixture.system());
        let directory = retained_chain_directory(&fixture);
        assert!(
            fixture
                .terminal
                .decide_chain_io_admission(
                    "admin",
                    operation,
                    &proposal.preparation().proposal_digest,
                    directory,
                )
                .is_err()
        );
        assert!(!missing.exists());
        assert_eq!(locator_bytes(&fixture.system()), locator);
        let historical = fixture
            .terminal
            .chain_readmission_status()
            .unwrap()
            .unwrap();
        assert!(!historical.io_decision.unwrap().target_reverified_this_call);
    }
}

#[test]
fn chain_io_decision_requires_original_plan_and_actor_and_rejects_mixed_vote_kind() {
    let (mut fixture, operation, proposal, _) = published_chain_first_fixture();
    let directory = retained_chain_directory(&fixture);
    assert!(
        fixture
            .terminal
            .decide_chain_io_admission(
                "admin",
                operation,
                &proposal.preparation().proposal_digest,
                directory,
            )
            .is_err()
    );
    assert!(
        !fixture
            .terminal
            .store
            .operation()
            .path()
            .join("chain-io-decision-request.json")
            .exists()
    );
    drop(fixture);
    let (mut fixture, operation, proposal, _) = planned_chain_io_fixture();
    let directory = retained_chain_directory(&fixture);
    assert!(
        fixture
            .terminal
            .decide_chain_io_admission(
                "other",
                operation,
                &proposal.preparation().proposal_digest,
                directory,
            )
            .is_err()
    );
    assert!(
        !fixture
            .terminal
            .store
            .operation()
            .path()
            .join("chain-io-decision-request.json")
            .exists()
    );
    let directory = retained_chain_directory(&fixture);
    let status = fixture
        .terminal
        .decide_chain_io_admission(
            "admin",
            operation,
            &proposal.preparation().proposal_digest,
            directory,
        )
        .unwrap();
    let report = status.io_decision.unwrap();
    let vote = fixture.config.disks[0]
        .root
        .join("metadata/membership-decisions")
        .join(&report.io_epoch)
        .join("decision.json");
    let mut body: json_codec::Value = crate::decode_strict(&fs::read(&vote).unwrap()).unwrap();
    body["choice"]["kind"] = "TerminalChainReadmission".into();
    let wrong = crate::canonical_json(&body).unwrap();
    fs::write(&vote, &wrong).unwrap();
    let directory = retained_chain_directory(&fixture);
    assert!(
        fixture
            .terminal
            .decide_chain_io_admission(
                "admin",
                operation,
                &proposal.preparation().proposal_digest,
                directory,
            )
            .is_err()
    );
    assert_eq!(fs::read(&vote).unwrap(), wrong);
}
