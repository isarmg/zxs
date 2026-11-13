//! Count before each actual named-root syscall, with physically present targets.
use super::*;
use crate::engine::disk_lifecycle::bootstrap::store::{PreparedStop, WriteFault, inject_next};
use std::{fs, os::unix::fs::PermissionsExt};

#[derive(Default)]
struct CountedIo {
    stats: Vec<PathBuf>,
    opens: Vec<PathBuf>,
}
impl RootIo for CountedIo {
    fn stat(&mut self, probe: ReadProbe<'_>) -> std::io::Result<fs::Metadata> {
        self.stats.push(probe.path().to_owned());
        NamedRootIo.stat(probe)
    }
    fn open(&mut self, probe: ReadProbe<'_>) -> Result<DiskRoot> {
        self.opens.push(probe.path().to_owned());
        NamedRootIo.open(probe)
    }
}

#[test]
fn prepared_exit_stop_probes_neither_old_nor_partial_stops_at_any_real_barrier() {
    for phase in [
        WriteFault::BeforeRecord,
        WriteFault::AfterRecord,
        WriteFault::BeforeHead,
        WriteFault::AfterHead,
    ] {
        let temp = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let config =
            Config::development((0..4).map(|n| temp.path().join(format!("disk-{n}")))).unwrap();
        let system = temp.path().join("system");
        let mut engine = Engine::initialize(&system, config.clone()).unwrap();
        engine
            .create_repository(
                &RepositoryName::parse("kept").unwrap(),
                "P01",
                1,
                "admin",
                OperationId::generate().unwrap(),
            )
            .unwrap();
        let mut operation = OperationId::generate().unwrap();
        for index in 0..2 {
            operation = OperationId::generate().unwrap();
            engine
                .begin_disk_exit(
                    &config.disks[index].disk_id,
                    MembershipExitAction::PlannedOffline,
                    "admin",
                    operation,
                )
                .unwrap();
            let archive = engine.build_managed_membership_control_archive().unwrap();
            engine
                .enroll_pending_members(&archive.archive_path, &archive.root_hash, Vec::new())
                .unwrap();
            engine.resume_membership_joint_decision().unwrap();
            if index == 0 {
                engine.activate_pending_disk_exit().unwrap();
            } else {
                inject_next(phase);
                assert!(engine.activate_pending_disk_exit().is_err());
            }
        }
        let projected = engine.config.clone();
        let bytes = fs::read(
            system
                .join("membership-exit-stops")
                .join(operation.to_string())
                .join("plan.json"),
        )
        .unwrap();
        let value: json_codec::Value = decode_strict(&bytes).unwrap();
        let stop: PreparedStop = json_codec::from_value(value["stop"].clone()).unwrap();
        let authority: PortableAuthority =
            json_codec::from_value(value["authority"].clone()).unwrap();
        drop(engine);
        let mut io = CountedIo::default();
        assert!(discover_startup_with_io(&system, projected.clone(), &mut io).is_err());
        assert!(
            io.stats.is_empty() && io.opens.is_empty(),
            "ordinary startup must reject a pre-record stop plan before probing"
        );
        let bootstrap = open_store(&system).unwrap();
        let resumed =
            verify_prepared_exit_stop_with_io(&bootstrap, &stop, &authority, &projected, &mut io)
                .unwrap();
        assert_eq!(io.stats.len(), 2);
        assert_eq!(io.opens.len(), 2);
        for disk in &config.disks[..2] {
            assert!(disk.root.is_dir(), "test targets remain physically present");
            assert!(!io.stats.contains(&disk.root));
            assert!(!io.opens.contains(&disk.root));
            assert!(resumed.excludes_io_disk(&disk.disk_id));
        }
    }
}

struct ReadmittedFixture {
    temp: tempfile::TempDir,
    original: Config,
    projected: ValidatedConfig,
    operation: OperationId,
    authority: PortableAuthority,
    stop: PreparedStop,
    before: Arc<State>,
}
impl ReadmittedFixture {
    fn new(phase: WriteFault) -> Self {
        Self::new_observed(phase, |_| {})
    }
    fn new_observed(phase: WriteFault, observe: impl Fn(&str)) -> Self {
        observe("fixture.begin");
        let temp = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let original =
            Config::development((0..3).map(|n| temp.path().join(format!("disk-{n}")))).unwrap();
        let system = temp.path().join("system");
        let mut engine = Engine::initialize(&system, original.clone()).unwrap();
        engine
            .create_repository(
                &RepositoryName::parse("retained-readmission").unwrap(),
                "P01",
                1,
                "admin",
                OperationId::generate().unwrap(),
            )
            .unwrap();
        observe("initialize_repository.end");
        for (index, disk) in original.disks.iter().enumerate() {
            observe(&format!("disk{index}.exit_prepare.begin"));
            prepare_exit(&mut engine, &disk.disk_id, OperationId::generate().unwrap());
            observe(&format!("disk{index}.exit_prepare.end"));
            observe(&format!("disk{index}.exit_activate.begin"));
            engine.activate_pending_disk_exit().unwrap();
            observe(&format!("disk{index}.exit_activate.end"));
            let directory = || PendingMemberDirectory {
                disk_id: disk.disk_id.clone(),
                directory: File::open(&disk.root).unwrap(),
            };
            observe(&format!("disk{index}.readmission_begin.begin"));
            engine
                .begin_disk_readmission(
                    &disk.disk_id,
                    "admin",
                    OperationId::generate().unwrap(),
                    directory(),
                )
                .unwrap();
            observe(&format!("disk{index}.readmission_begin.end"));
            observe(&format!("disk{index}.readmission_archive.begin"));
            let archive = engine.build_managed_membership_control_archive().unwrap();
            observe(&format!("disk{index}.readmission_archive.end"));
            observe(&format!("disk{index}.readmission_activate.begin"));
            engine
                .activate_disk_readmission(&archive.archive_path, &archive.root_hash, directory())
                .unwrap();
            observe(&format!("disk{index}.readmission_activate.end"));
        }
        let before = Arc::clone(&engine.state);
        let operation = OperationId::generate().unwrap();
        observe("fault_exit_prepare.begin");
        prepare_exit(&mut engine, &original.disks[0].disk_id, operation);
        observe("fault_exit_prepare.end");
        observe("fault_exit_activate.begin");
        inject_next(phase);
        assert!(engine.activate_pending_disk_exit().is_err(), "{phase:?}");
        observe("fault_exit_activate.end");
        assert!(!engine.state.operations.contains_key(&operation));
        let projected = engine.config.clone();
        let value: json_codec::Value = decode_strict(
            &fs::read(
                system
                    .join("membership-exit-stops")
                    .join(operation.to_string())
                    .join("plan.json"),
            )
            .unwrap(),
        )
        .unwrap();
        let stop = json_codec::from_value(value["stop"].clone()).unwrap();
        let authority = json_codec::from_value(value["authority"].clone()).unwrap();
        observe("original_engine_drop.begin");
        drop(engine);
        observe("original_engine_drop.end");
        observe("fixture.end");
        Self {
            temp,
            original,
            projected,
            operation,
            authority,
            stop,
            before,
        }
    }
    fn system(&self) -> PathBuf {
        self.temp.path().join("system")
    }
    fn verify(&self, io: &mut CountedIo) -> Result<ValidatedConfig> {
        let store = open_store(&self.system())?;
        verify_prepared_exit_stop_with_io(&store, &self.stop, &self.authority, &self.projected, io)
    }
}
fn prepare_exit(engine: &mut Engine, disk: &str, operation: OperationId) {
    engine
        .begin_disk_exit(
            disk,
            MembershipExitAction::PlannedOffline,
            "admin",
            operation,
        )
        .unwrap();
    let archive = engine.build_managed_membership_control_archive().unwrap();
    engine
        .enroll_pending_members(&archive.archive_path, &archive.root_hash, Vec::new())
        .unwrap();
    let decision = engine.resume_membership_joint_decision().unwrap();
    assert!(decision.original_quorum_accepted());
    assert!(decision.target_quorum_accepted());
    assert!(decision.all_certificates_synchronized());
}

#[test]
fn prepared_exit_stop_uses_readmitted_survivors_at_every_locator_barrier() {
    use std::io::Write;
    let started = std::time::Instant::now();
    for phase in [
        WriteFault::BeforeRecord,
        WriteFault::AfterRecord,
        WriteFault::BeforeHead,
        WriteFault::AfterHead,
    ] {
        let progress = |stage: &str| {
            let message = format!("\nZXS_EXIT_STOP phase={phase:?} stage={stage} elapsed_ms={}\n",
                started.elapsed().as_millis());
            let mut stderr = std::io::stderr().lock();
            stderr.write_all(message.as_bytes()).unwrap();
            stderr.flush().unwrap();
        };
        let fixture = ReadmittedFixture::new_observed(phase, &progress);
        progress("verify_survivors.begin");
        let mut io = CountedIo::default();
        let verified = fixture.verify(&mut io).unwrap();
        assert_eq!(
            (
                verified.metadata_members().len(),
                verified.metadata_quorum()
            ),
            (2, 2)
        );
        assert_eq!(io.stats.len(), 2);
        assert_eq!(io.opens.len(), 2);
        assert!(!io.stats.contains(&fixture.original.disks[0].root));
        assert!(!io.opens.contains(&fixture.original.disks[0].root));
        for disk in &fixture.original.disks[1..] {
            assert!(io.stats.contains(&disk.root));
            assert!(io.opens.contains(&disk.root));
            let epoch = fixture.projected.io_admission_epoch(&disk.disk_id).unwrap();
            assert!(epoch.is_some());
            assert_eq!(verified.io_admission_epoch(&disk.disk_id).unwrap(), epoch);
        }
        progress("verify_survivors.end");
        progress("resume.begin");
        let mut resumed = Engine::resume_membership_activation(&fixture.system()).unwrap();
        progress("resume.end");
        progress("activate.begin");
        let result = resumed.activate_pending_disk_exit().unwrap();
        progress("activate.end");
        assert_eq!(result.operation_id, fixture.operation);
        for (operation, result) in fixture.before.operations.iter() {
            assert_eq!(
                resumed.operation_record(*operation).unwrap().result(),
                result
            );
        }
        assert!(
            resumed
                .config
                .excludes_io_disk(&fixture.original.disks[0].disk_id)
        );
        progress("resumed_drop.begin");
        drop(resumed);
        progress("resumed_drop.end");
        progress("cold.begin");
        let reopened = Engine::open(&fixture.system()).unwrap();
        progress("cold.end");
        assert_eq!(
            reopened
                .operation_record(fixture.operation)
                .unwrap()
                .result(),
            &result
        );
        assert_eq!(
            (
                reopened.config.metadata_members().len(),
                reopened.config.metadata_quorum()
            ),
            (2, 2)
        );
        progress("cleanup.begin");
        drop(reopened);
        drop(result);
        drop(verified);
        drop(io);
        drop(fixture);
        progress("cleanup.end");
        progress("phase.end");
    }
}

#[test]
fn prepared_exit_stop_requires_surviving_epoch_proofs_before_storage_io() {
    for corrupt in [false, true] {
        let fixture = ReadmittedFixture::new(WriteFault::AfterRecord);
        let epoch = fixture
            .projected
            .io_admission_epoch(&fixture.original.disks[1].disk_id)
            .unwrap()
            .unwrap();
        let certificate = fixture
            .system()
            .join("readmission-probe-admissions")
            .join(format!("{epoch}.json"));
        let saved = fs::read(&certificate).unwrap();
        if corrupt {
            fs::write(&certificate, b"{truncated retained probe").unwrap();
        } else {
            fs::rename(
                &certificate,
                fixture.temp.path().join("preserved-probe.json"),
            )
            .unwrap();
        }
        let mut io = CountedIo::default();
        assert!(fixture.verify(&mut io).is_err());
        assert!(io.stats.is_empty() && io.opens.is_empty());
        fs::write(&certificate, saved).unwrap();
        let mut resumed = Engine::resume_membership_activation(&fixture.system()).unwrap();
        assert_eq!(
            resumed.activate_pending_disk_exit().unwrap().operation_id,
            fixture.operation
        );
    }
}

#[test]
fn prepared_exit_stop_readmission_proof_never_substitutes_for_actual_retained_quorum() {
    let fixture = ReadmittedFixture::new(WriteFault::AfterRecord);
    let missing = &fixture.original.disks[1].root;
    let preserved = fixture.temp.path().join("preserved-missing-voter");
    fs::rename(missing, &preserved).unwrap();
    let mut io = CountedIo::default();
    assert_eq!(
        fixture.verify(&mut io).unwrap_err().code(),
        ErrorCode::MetadataQuorumUnavailable
    );
    assert!(io.stats.contains(missing));
    assert!(!io.opens.contains(missing));
    assert!(!io.stats.contains(&fixture.original.disks[0].root));
    assert!(!io.opens.contains(&fixture.original.disks[0].root));
    fs::rename(preserved, missing).unwrap();
    let mut io = CountedIo::default();
    let verified = fixture.verify(&mut io).unwrap();
    assert_eq!(
        (
            verified.metadata_members().len(),
            verified.metadata_quorum()
        ),
        (2, 2)
    );
}
