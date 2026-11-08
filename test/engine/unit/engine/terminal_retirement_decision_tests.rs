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
            .is_err()
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
