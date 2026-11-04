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
