//! Terminal completion uses only its original SYSTEM authority and saved plan.
use super::*;
use crate::engine::disk_lifecycle::bootstrap::store::{WriteFault, inject_next};
use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};

struct Fixture {
    temp: tempfile::TempDir,
    engine: Engine,
    exit: TerminalDiskExit,
    config: crate::Config,
    operation: OperationId,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let mut config =
            crate::Config::development((0..3).map(|n| temp.path().join(format!("disk-{n}"))))
                .unwrap();
        for disk in config.disks.iter_mut().skip(1) {
            disk.metadata_member = false;
        }
        let mut engine = Engine::initialize(&temp.path().join("system"), config.clone()).unwrap();
        engine
            .create_repository(
                &crate::RepositoryName::parse("unchanged").unwrap(),
                "P01",
                1,
                "admin",
                OperationId::generate().unwrap(),
            )
            .unwrap();
        let operation = OperationId::generate().unwrap();
        engine
            .begin_disk_exit(
                &config.disks[0].disk_id,
                MembershipExitAction::Deregister,
                "admin",
                operation,
            )
            .unwrap();
        let exit = engine.prepare_terminal_disk_exit().unwrap();
        Self {
            temp,
            engine,
            exit,
            config,
            operation,
        }
    }
    fn system(&self) -> PathBuf {
        self.temp.path().join("system")
    }
    fn plan(&self) -> PathBuf {
        self.system()
            .join("terminal-management")
            .join(self.operation.to_string())
            .join("bootstrap-stop.json")
    }
    fn interrupt(&mut self, phase: WriteFault) {
        inject_next(phase);
        assert!(self.exit.complete(&mut self.engine).is_err());
        assert!(
            !self
                .system()
                .join("terminal-management")
                .join(self.operation.to_string())
                .join("complete.json")
                .is_file()
        );
    }
}

#[test]
fn terminal_stop_all_four_real_barriers_resume_live_and_cold_without_storage_roots() {
    for phase in [
        WriteFault::BeforeRecord,
        WriteFault::AfterRecord,
        WriteFault::BeforeHead,
        WriteFault::AfterHead,
    ] {
        for cold in [false, true] {
            let mut fixture = Fixture::new();
            let target = Arc::clone(
                fixture
                    .engine
                    .disk_resources
                    .get(&fixture.config.disks[0])
                    .unwrap(),
            );
            fixture.interrupt(phase);
            assert!(
                target.root.admit_io().is_err(),
                "a durable exact plan closes uncertain target admission"
            );
            let system = fixture.system();
            let plan_path = fixture.plan();
            let plan = fs::read(&plan_path).unwrap();
            for (index, disk) in fixture.config.disks.iter().enumerate() {
                fs::rename(
                    &disk.root,
                    fixture.temp.path().join(format!("parked-{index}")),
                )
                .unwrap();
                // Even a present wrong type at every old address is irrelevant.
                fs::write(&disk.root, b"must never be a storage probe").unwrap();
            }
            let management = if cold {
                drop(fixture.exit);
                drop(fixture.engine);
                TerminalManagement::open(&system).unwrap()
            } else {
                fixture.exit.complete(&mut fixture.engine).unwrap()
            };
            assert_eq!(management.result().operation_id, fixture.operation);
            assert!(!management.status().writable);
            assert_eq!(fs::read(&plan_path).unwrap(), plan);
        }
    }
}

#[test]
fn terminal_stop_missing_plan_and_unknown_staging_do_not_adopt_an_orphan_record() {
    for corrupt in ["missing", "staging", "authority", "operation", "root"] {
        let mut fixture = Fixture::new();
        fixture.interrupt(WriteFault::AfterRecord);
        let plan = fixture.plan();
        match corrupt {
            "missing" => fs::remove_file(&plan).unwrap(),
            "staging" => {
                fs::write(plan.parent().unwrap().join("unknown-staging"), b"retained").unwrap()
            }
            _ => {
                let mut value: json_codec::Value =
                    crate::decode_strict(&fs::read(&plan).unwrap()).unwrap();
                match corrupt {
                    "authority" => value["authority_hash"] = "a".repeat(64).into(),
                    "operation" => {
                        value["stop"]["operation_id"] =
                            OperationId::generate().unwrap().to_string().into()
                    }
                    "root" => value["stop"]["stopped"]["roots"][0]["inode"] = "1".into(),
                    _ => unreachable!(),
                }
                fs::write(&plan, crate::canonical_json(&value).unwrap()).unwrap();
            }
        }
        let system = fixture.system();
        drop(fixture.exit);
        drop(fixture.engine);
        assert!(
            TerminalManagement::open(&system).is_err(),
            "must reject {corrupt}"
        );
        if corrupt == "staging" {
            assert_eq!(
                fs::read(plan.parent().unwrap().join("unknown-staging")).unwrap(),
                b"retained"
            );
        }
    }
}

#[test]
fn terminal_stop_preparation_failure_keeps_live_target_admission() {
    let mut fixture = Fixture::new();
    let target = Arc::clone(
        fixture
            .engine
            .disk_resources
            .get(&fixture.config.disks[0])
            .unwrap(),
    );
    let operation = fixture
        .system()
        .join("terminal-management")
        .join(fixture.operation.to_string());
    fs::write(operation.join("unknown-staging"), b"retained").unwrap();
    assert!(fixture.exit.complete(&mut fixture.engine).is_err());
    assert!(
        target.root.admit_io().is_ok(),
        "unprepared failure must not quiesce the target"
    );
}

#[test]
fn terminal_stop_known_chain_names_still_require_their_original_source() {
    for name in [
        "chain-readmission.json",
        "chain-readmission-target.json",
        "chain-first-selected.json",
        "chain-publication-selected.json",
        "chain-activation-plan.json",
        "chain-io-decision-request.json",
        "chain-generation-log-selected.json",
        "chain-generation-selector-request.json",
    ] {
        let mut fixture = Fixture::new();
        let management = fixture.exit.complete(&mut fixture.engine).unwrap();
        let path = fixture.plan().parent().unwrap().join(name);
        fs::write(&path, b"{}").unwrap();
        let system = fixture.system();
        drop(management);
        drop(fixture.exit);
        drop(fixture.engine);
        let error = TerminalManagement::open(&system)
            .err()
            .expect("a known chain filename is not authenticated evidence");
        assert!(
            !error
                .to_string()
                .contains("unknown or unfinished terminal stop write"),
            "{name} must reach its protocol validator: {error}"
        );
        assert_eq!(fs::read(&path).unwrap(), b"{}");
    }
}
