//! Real ordinary-Add consumers and filesystem barriers. These are source tests,
//! not a claim that physical power loss or a deployment was exercised.
use super::*;
use crate::engine::disk_lifecycle::bootstrap::store::{
    PreparedRootExtension, WriteFault, inject_next,
};
use std::{fs, os::unix::fs::PermissionsExt};

struct Fixture {
    temp: tempfile::TempDir,
    engine: Engine,
    original: Config,
    target: Config,
    operation: OperationId,
    version: String,
    preserved: BTreeMap<PathBuf, Vec<u8>>,
    stopped: Option<PathBuf>,
}
impl Fixture {
    fn new(added: usize, prior_stop: bool) -> Self {
        let temp = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let original =
            Config::development((0..3).map(|n| temp.path().join(format!("old-{n}")))).unwrap();
        let mut engine = Engine::initialize(&temp.path().join("system"), original.clone()).unwrap();
        let repository = engine
            .create_repository(
                &RepositoryName::parse("preserved").unwrap(),
                "P01",
                1,
                "admin",
                OperationId::generate().unwrap(),
            )
            .unwrap();
        let workspace = engine
            .create_workspace(&repository.main_branch_id, "admin")
            .unwrap();
        engine
            .stage_bytes(
                &workspace.workspace_id,
                "body.txt",
                b"old payload and history remain unchanged",
            )
            .unwrap();
        engine
            .commit_workspace(CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: OperationId::generate().unwrap(),
                principal: "admin".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: String::new(),
                request_digest: None,
            })
            .unwrap();
        let version = engine.list_tree(&repository.main_branch_id).unwrap()[0]
            .version_id
            .clone();
        engine
            .export_history_package(
                &repository.history_tree_id,
                &temp.path().join("old-package"),
            )
            .unwrap();
        let stopped = if prior_stop {
            engine
                .begin_disk_exit(
                    &original.disks[0].disk_id,
                    MembershipExitAction::PlannedOffline,
                    "admin",
                    OperationId::generate().unwrap(),
                )
                .unwrap();
            let archive = engine.build_managed_membership_control_archive().unwrap();
            engine
                .enroll_pending_members(&archive.archive_path, &archive.root_hash, Vec::new())
                .unwrap();
            engine.resume_membership_joint_decision().unwrap();
            engine.activate_pending_disk_exit().unwrap();
            Some(original.disks[0].root.clone())
        } else {
            None
        };
        let mut target = engine.config().clone();
        let count = target.disks.len();
        let additions =
            Config::development((0..count + added).map(|n| temp.path().join(format!("new-{n}"))))
                .unwrap();
        let mut directories = Vec::new();
        for mut disk in additions.disks.into_iter().skip(count) {
            if prior_stop {
                // The old primary is stopped. A new independent primary must
                // not borrow that disabled disk as its backup-chain ancestor.
                disk.level = 1;
                disk.upstream_slot_id = None;
                disk.slot_id = format!("P{:02}", target.disks.len() - count + 2);
            }
            private_dir(&disk.root).unwrap();
            directories.push(PendingMemberDirectory {
                disk_id: disk.disk_id.clone(),
                directory: File::open(&disk.root).unwrap(),
            });
            target.disks.push(disk);
        }
        let operation = OperationId::generate().unwrap();
        engine
            .begin_membership_handoff(target.clone(), "admin", operation)
            .unwrap();
        let archive = engine.build_managed_membership_control_archive().unwrap();
        engine
            .enroll_pending_members(&archive.archive_path, &archive.root_hash, directories)
            .unwrap();
        engine.resume_membership_joint_decision().unwrap();
        let mut preserved = BTreeMap::new();
        if !prior_stop {
            for disk in &original.disks {
                let journal = crate::journal::log_path(disk);
                preserved.insert(journal.clone(), fs::read(journal).unwrap());
                let root = &engine.disk_resources.get(disk).unwrap().root;
                for (record, _) in crate::journal::scan_at(root, disk).unwrap().records {
                    if record.record_type == RecordType::Publication {
                        let path =
                            crate::journal::object_path(disk, "candidate", &record.candidate_hash);
                        preserved.insert(path.clone(), fs::read(path).unwrap());
                    }
                }
            }
        }
        Self {
            temp,
            engine,
            original,
            target,
            operation,
            version,
            preserved,
            stopped,
        }
    }
    fn system(&self) -> PathBuf {
        self.temp.path().join("system")
    }
    fn crash(&mut self, phase: WriteFault) {
        inject_next(phase);
        assert!(
            self.engine.activate_pending_membership().is_err(),
            "must interrupt the real Add writer at {phase:?}"
        );
        assert!(!self.engine.state.operations.contains_key(&self.operation));
    }
    fn plan(&self) -> (PortableAuthority, PreparedRootExtension) {
        let value: json_codec::Value = decode_strict(
            &fs::read(
                self.system()
                    .join("membership-root-extensions")
                    .join(self.operation.to_string())
                    .join("plan.json"),
            )
            .unwrap(),
        )
        .unwrap();
        (
            json_codec::from_value(value["authority"].clone()).unwrap(),
            json_codec::from_value(value["extension"].clone()).unwrap(),
        )
    }
    fn verify(&self, io: &mut CountedIo) -> Result<ValidatedConfig> {
        let (authority, plan) = self.plan();
        let store = open_store(&self.system()).unwrap();
        let locator = store
            .root_extension_locator(&plan)
            .map_err(management_error)?;
        verify_prepared_add_with_io(&locator, authority, &self.engine.config, &[], io)
    }
}

#[derive(Default)]
struct CountedIo {
    touches: Vec<PathBuf>,
}
impl RootIo for CountedIo {
    fn stat(&mut self, probe: ReadProbe<'_>) -> std::io::Result<fs::Metadata> {
        self.touches.push(probe.path().to_path_buf());
        NamedRootIo.stat(probe)
    }
    fn open(&mut self, probe: ReadProbe<'_>) -> Result<DiskRoot> {
        self.touches.push(probe.path().to_path_buf());
        NamedRootIo.open(probe)
    }
}

fn files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut saved = BTreeMap::new();
    for entry in fs::read_dir(root).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            saved.extend(files(&entry.path()));
        } else {
            saved.insert(entry.path(), fs::read(entry.path()).unwrap());
        }
    }
    saved
}

#[test]
fn add_extension_real_record_and_head_barriers_resume_without_rewriting_old_candidates() {
    for phase in [
        WriteFault::BeforeRecord,
        WriteFault::AfterRecord,
        WriteFault::BeforeHead,
        WriteFault::AfterHead,
    ] {
        let mut fixture = Fixture::new(1, false);
        fixture.crash(phase);
        let store = open_store(&fixture.system()).unwrap();
        let strict = store.reload(&fixture.target.pool_id);
        assert_eq!(
            strict.is_err(),
            matches!(phase, WriteFault::AfterRecord | WriteFault::BeforeHead)
        );
        drop(store);
        let Fixture {
            temp,
            engine,
            original,
            operation,
            version,
            preserved,
            ..
        } = fixture;
        drop(engine);
        let system = temp.path().join("system");
        let mut resumed = Engine::resume_membership_activation(&system).unwrap();
        let result = resumed
            .operation_record(operation)
            .unwrap()
            .result()
            .clone();
        assert_eq!(result.kind, "membership-add");
        assert_eq!(resumed.activate_pending_membership().unwrap(), result);
        for (path, bytes) in preserved {
            assert_eq!(fs::read(path).unwrap(), bytes);
        }
        let export = temp.path().join("preserved-body");
        crate::offline_export(&original.disks[0].root, &version, &export).unwrap();
        assert_eq!(
            fs::read(export).unwrap(),
            b"old payload and history remain unchanged"
        );
        crate::history_package::offline_check_history_package(&temp.path().join("old-package"))
            .unwrap();
        let ledger =
            files(&system.join(crate::engine::disk_lifecycle::bootstrap::runtime::DIRECTORY));
        drop(resumed);
        let again = Engine::resume_membership_activation(&system).unwrap();
        assert_eq!(again.operation_record(operation).unwrap().result(), &result);
        assert_eq!(
            files(&system.join(crate::engine::disk_lifecycle::bootstrap::runtime::DIRECTORY)),
            ledger
        );
    }
}

#[test]
fn add_extension_same_operation_retries_the_real_pending_source_after_each_barrier() {
    for phase in [
        WriteFault::BeforeRecord,
        WriteFault::AfterRecord,
        WriteFault::BeforeHead,
        WriteFault::AfterHead,
    ] {
        let mut fixture = Fixture::new(1, false);
        fixture.crash(phase);
        let result = fixture.engine.activate_pending_membership().unwrap();
        assert_eq!(result.operation_id, fixture.operation);
        assert_eq!(
            fixture.engine.activate_pending_membership().unwrap(),
            result
        );
        let store = open_store(&fixture.system()).unwrap();
        assert_eq!(
            store.reload(&fixture.target.pool_id).unwrap().generation(),
            2
        );
    }
}

#[test]
fn add_extension_unknown_staging_stays_closed_before_any_storage_probe() {
    let mut fixture = Fixture::new(1, false);
    fixture.crash(WriteFault::AfterRecord);
    let locator = fixture
        .system()
        .join(crate::engine::disk_lifecycle::bootstrap::runtime::DIRECTORY);
    fs::write(locator.join("unrelated-staging"), b"preserve this evidence").unwrap();
    let before = files(&locator);
    let mut io = CountedIo::default();
    assert!(fixture.verify(&mut io).is_err());
    assert!(io.touches.is_empty());
    let system = fixture.system();
    drop(fixture.engine);
    assert!(Engine::resume_membership_activation(&system).is_err());
    assert_eq!(files(&locator), before);
}

#[test]
fn add_extension_wrong_system_authority_or_operation_is_not_rebound() {
    for field in ["authority_hash", "operation_id"] {
        let mut fixture = Fixture::new(1, false);
        fixture.crash(WriteFault::AfterRecord);
        let head_path = fixture
            .system()
            .join("membership-root-extensions/head.json");
        let mut head: json_codec::Value = decode_strict(&fs::read(&head_path).unwrap()).unwrap();
        head[field] = if field == "authority_hash" {
            "a".repeat(64).into()
        } else {
            OperationId::generate().unwrap().to_string().into()
        };
        fs::write(&head_path, canonical_json(&head).unwrap()).unwrap();
        let ledger = fixture
            .system()
            .join(crate::engine::disk_lifecycle::bootstrap::runtime::DIRECTORY);
        let before = files(&ledger);
        let system = fixture.system();
        drop(fixture.engine);
        assert!(Engine::resume_membership_activation(&system).is_err());
        assert_eq!(files(&ledger), before);
    }
}

#[test]
fn add_extension_physical_replacement_fails_before_its_seal_is_read() {
    let mut fixture = Fixture::new(1, false);
    fixture.crash(WriteFault::AfterRecord);
    let added = fixture.target.disks.last().unwrap();
    fs::rename(
        &added.root,
        fixture.temp.path().join("preserved-added-root"),
    )
    .unwrap();
    private_dir(&added.root).unwrap();
    fs::write(
        added.root.join("disk.json"),
        b"unparseable; identity must fail first",
    )
    .unwrap();
    let mut io = CountedIo::default();
    let error = fixture.verify(&mut io).unwrap_err();
    assert_eq!(error.code(), ErrorCode::DiskIdentityMismatch);
    assert!(io.touches.contains(&added.root));
}

#[test]
fn add_extension_pre_marker_admission_requires_actual_fixed_new_quorum_and_complete_archives() {
    let mut fixture = Fixture::new(4, false);
    fixture.crash(WriteFault::AfterRecord);
    let (authority, _) = fixture.plan();
    let intent = authority.intent().unwrap();
    let transition = object_hash(INTENT_KIND, &intent).unwrap();
    let mut io = CountedIo::default();
    let verified = fixture.verify(&mut io).unwrap();
    assert_eq!(
        (
            verified.metadata_members().len(),
            verified.metadata_quorum()
        ),
        (7, 4)
    );
    // Seven locator hints and a complete portable certificate do not replace
    // four actual independent current member archives.
    for disk in fixture.target.disks.iter().skip(3) {
        let archive = member_directory(disk, &transition)
            .join(CONTROL_ARCHIVE_DIRECTORY)
            .join("archive-root.json");
        fs::rename(&archive, archive.with_extension("preserved")).unwrap();
    }
    let mut io = CountedIo::default();
    assert_eq!(
        fixture.verify(&mut io).unwrap_err().code(),
        ErrorCode::MetadataQuorumUnavailable
    );
    let system = fixture.system();
    let ledger = files(&system.join(crate::engine::disk_lifecycle::bootstrap::runtime::DIRECTORY));
    drop(fixture.engine);
    assert!(Engine::resume_membership_activation(&system).is_err());
    assert_eq!(
        files(&system.join(crate::engine::disk_lifecycle::bootstrap::runtime::DIRECTORY)),
        ledger
    );
}

#[test]
fn add_extension_recovery_preserves_authenticated_old_stop_with_zero_stat_or_open() {
    let mut fixture = Fixture::new(1, true);
    let stopped = fixture.stopped.clone().unwrap();
    let before = files(&stopped);
    fixture.crash(WriteFault::AfterRecord);
    let mut io = CountedIo::default();
    let config = fixture.verify(&mut io).unwrap();
    assert!(!io.touches.contains(&stopped));
    assert!(!config.io_disks().any(|disk| disk.root == stopped));
    let system = fixture.system();
    let stop_disk = fixture.original.disks[0].disk_id.clone();
    drop(fixture.engine);
    let resumed = Engine::resume_membership_activation(&system).unwrap();
    assert!(resumed.config.excludes_io_disk(&stop_disk));
    assert_eq!(files(&stopped), before);
}

#[test]
fn add_extension_without_bound_plan_or_lost_system_requires_explicit_retained_root_recovery() {
    for lose_system in [false, true] {
        let mut fixture = Fixture::new(1, false);
        fixture.crash(WriteFault::AfterRecord);
        let system = fixture.system();
        let storage = fixture
            .target
            .disks
            .iter()
            .flat_map(|disk| files(&disk.root))
            .collect::<BTreeMap<_, _>>();
        drop(fixture.engine);
        if lose_system {
            fs::rename(&system, fixture.temp.path().join("preserved-system")).unwrap();
        } else {
            fs::rename(
                system.join("membership-root-extensions"),
                fixture.temp.path().join("preserved-plan"),
            )
            .unwrap();
        }
        let error = Engine::resume_membership_activation(&system).err().unwrap();
        assert_eq!(error.code(), ErrorCode::RecoveryRequired);
        assert!(Engine::open(&system).is_err());
        for (path, bytes) in storage {
            assert_eq!(fs::read(path).unwrap(), bytes);
        }
    }
}
