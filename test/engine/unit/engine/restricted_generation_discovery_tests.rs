//! Real directory I/O behind the same first-stat/open boundary as production.
//! This is not an Engine::open or real deregistration acceptance suite.
use super::*;
use crate::engine::{
    disk_lifecycle::bootstrap::{IntentDigest, LoadedLocator, RootObservation},
    disk_registry::DiskRegistrationIdentity,
};
use std::{fs, io::Write, os::unix::fs::PermissionsExt};

#[derive(Debug, PartialEq, Eq)]
enum Touch {
    Stat(PathBuf),
    Open(PathBuf),
}
#[derive(Default)]
struct CountedIo {
    touches: Vec<Touch>,
}
impl RootIo for CountedIo {
    fn stat(&mut self, probe: ReadProbe<'_>) -> std::io::Result<fs::Metadata> {
        // Count BEFORE the actual first target symlink_metadata syscall, not
        // after DiskResources admission or after a successful root open.
        self.touches.push(Touch::Stat(probe.path().to_owned()));
        NamedRootIo.stat(probe)
    }
    fn open(&mut self, probe: ReadProbe<'_>) -> Result<DiskRoot> {
        self.touches.push(Touch::Open(probe.path().to_owned()));
        NamedRootIo.open(probe)
    }
}
impl CountedIo {
    fn assert_only(&self, permitted: &[PathBuf]) {
        assert_eq!(self.touches.len(), permitted.len() * 2);
        for path in permitted {
            assert_eq!(
                self.touches
                    .iter()
                    .filter(|touch| **touch == Touch::Stat(path.clone()))
                    .count(),
                1,
            );
        }
        let (pairs, remainder) = self.touches.as_chunks::<2>();
        assert!(remainder.is_empty());
        for pair in pairs {
            let Touch::Stat(path) = &pair[0] else {
                panic!("stat must come first")
            };
            assert!(permitted.contains(path));
            assert_eq!(&pair[1], &Touch::Open(path.clone()));
        }
    }
}

struct Fixture {
    temp: tempfile::TempDir,
    original: ValidatedConfig,
    target: Config,
    control: PathBuf,
    store: BootstrapStore,
    stopped: LoadedLocator,
    authority: String,
    transition: String,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let original =
            Config::development((0..3).map(|n| temp.path().join(format!("old-{n}")))).unwrap();
        let mut engine = Engine::initialize(&temp.path().join("system"), original.clone()).unwrap();
        engine
            .create_repository(
                &RepositoryName::parse("preserved").unwrap(),
                "P01",
                1,
                "admin",
                OperationId::generate().unwrap(),
            )
            .unwrap();
        let mut target = original.clone();
        let additions =
            Config::development((0..4).map(|n| temp.path().join(format!("new-{n}")))).unwrap();
        let added = additions.disks.into_iter().nth(3).unwrap();
        private_dir(&added.root).unwrap();
        target.disks.push(added);
        engine
            .begin_membership_handoff(target.clone(), "admin", OperationId::generate().unwrap())
            .unwrap();
        let archive_dir = temp.path().join("archive");
        private_dir(&archive_dir).unwrap();
        let archive = engine
            .build_membership_control_archive(&archive_dir)
            .unwrap();
        engine
            .enroll_pending_members(
                &archive_dir,
                &archive.root_hash,
                vec![PendingMemberDirectory {
                    disk_id: target.disks[3].disk_id.clone(),
                    directory: File::open(&target.disks[3].root).unwrap(),
                }],
            )
            .unwrap();
        engine.resume_membership_joint_decision().unwrap();
        engine.activate_pending_membership().unwrap();
        let generation = engine.config.generation().unwrap();
        let authority = generation.hash.clone();
        let transition = object_hash(INTENT_KIND, &generation.intent).unwrap();
        let control = temp.path().join("bootstrap");
        private_dir(&control).unwrap();
        let roots = target
            .disks
            .iter()
            .map(|disk| {
                RootObservation::capture(
                    &binding(&target, disk),
                    &disk.root,
                    &engine
                        .disk_resources
                        .get(disk)
                        .unwrap()
                        .root
                        .directory(&disk.root)
                        .unwrap(),
                    disk.metadata_member,
                )
                .unwrap()
            })
            .collect();
        let mut store =
            BootstrapStore::hold(File::open(&control).unwrap(), control.clone()).unwrap();
        let initial = store.initialize(&target.pool_id, roots).unwrap();
        let stopped = store
            .append_stop(
                &initial,
                &binding(&target, &target.disks[3]),
                OperationId::generate().unwrap(),
                IntentDigest::parse(&"a".repeat(64)).unwrap(),
            )
            .unwrap();
        drop(engine);
        Self {
            temp,
            original: ValidatedConfig::try_from(original).unwrap(),
            target,
            control,
            store,
            stopped,
            authority,
            transition,
        }
    }
    fn allowed(&self) -> Vec<PathBuf> {
        self.original
            .disks
            .iter()
            .map(|disk| disk.root.clone())
            .collect()
    }
    fn discover(
        &self,
        io: &mut CountedIo,
    ) -> std::result::Result<RestrictedGenerationEvidence, RestrictedDiscoveryFailure> {
        discover_store_with_io(&self.store, &self.original, io)
    }
    fn joint_decision(&self, index: usize) -> PathBuf {
        self.target.disks[index]
            .root
            .join("metadata/membership-decisions")
            .join(&self.transition)
            .join("decision.json")
    }
}
fn binding(config: &Config, disk: &DiskConfig) -> DiskRegistrationIdentity {
    DiskRegistrationIdentity {
        pool_id: config.pool_id.clone(),
        disk_id: disk.disk_id.clone(),
        slot_id: disk.slot_id.clone(),
        binding_generation: disk.binding_generation,
    }
}
fn snapshot(paths: &[PathBuf]) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(path: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                walk(&entry.path(), files);
            } else {
                files.insert(entry.path(), fs::read(entry.path()).unwrap());
            }
        }
    }
    let mut files = BTreeMap::new();
    for path in paths {
        walk(path, &mut files);
    }
    files
}
fn verification(error: RestrictedDiscoveryFailure) -> Error {
    match error {
        RestrictedDiscoveryFailure::Verification(error) => error,
        RestrictedDiscoveryFailure::Bootstrap(error) => panic!("unexpected locator error: {error}"),
    }
}

#[test]
fn restricted_discovery_stale_projection_and_fence_hints_never_probe_present_stopped_root() {
    let fixture = Fixture::new();
    let stopped = &fixture.target.disks[3];
    assert!(stopped.root.is_dir());
    // The authentic old log names this added root; the authentic generation
    // markers also include it. Neither kind of hint can extend RestrictedProbes.
    let disk = &fixture.original.disks[0];
    let scan = crate::journal::scan_at(&DiskRoot::open(&disk.root).unwrap(), disk).unwrap();
    assert!(
        scan.records
            .iter()
            .any(|(record, _)| record.record_type == RecordType::MembershipFence)
    );
    fs::write(
        stopped.root.join(GENERATION_FILE),
        b"excluded root must never be read",
    )
    .unwrap();
    let before = snapshot(
        &fixture
            .target
            .disks
            .iter()
            .map(|disk| disk.root.clone())
            .collect::<Vec<_>>(),
    );
    let projection = canonical_json(&*fixture.original).unwrap();
    let mut io = CountedIo::default();
    let evidence = fixture.discover(&mut io).unwrap();
    io.assert_only(&fixture.allowed());
    assert_eq!(evidence.authority_hash(), fixture.authority);
    assert_eq!(evidence.metadata_members(), 4);
    assert_eq!(evidence.metadata_quorum(), 3);
    assert_eq!(evidence.verified_members().len(), 3);
    assert!(evidence.rejected_members().is_empty());
    assert_eq!(canonical_json(&*fixture.original).unwrap(), projection);
    assert_eq!(fixture.original.metadata_members().len(), 3);
    assert_eq!(
        snapshot(
            &fixture
                .target
                .disks
                .iter()
                .map(|disk| disk.root.clone())
                .collect::<Vec<_>>()
        ),
        before
    );
}

#[test]
fn restricted_discovery_fixed_quorum_does_not_shrink_to_probe_count() {
    let mut fixture = Fixture::new();
    fixture
        .store
        .append_stop(
            &fixture.stopped,
            &binding(&fixture.target, &fixture.target.disks[0]),
            OperationId::generate().unwrap(),
            IntentDigest::parse(&"b".repeat(64)).unwrap(),
        )
        .unwrap();
    let mut io = CountedIo::default();
    let error = verification(fixture.discover(&mut io).unwrap_err());
    assert_eq!(error.code(), ErrorCode::MetadataQuorumUnavailable);
    io.assert_only(&fixture.allowed()[1..]);
    assert!(error.message().contains("fixed 4"));
    assert!(error.message().contains("quorum is 3"));
}

#[test]
fn restricted_discovery_missing_archive_cannot_be_replaced_by_excluded_target_archive() {
    let fixture = Fixture::new();
    let missing = member_directory(&fixture.target.disks[0], &fixture.transition)
        .join(CONTROL_ARCHIVE_DIRECTORY);
    fs::remove_dir_all(&missing).unwrap();
    let mut io = CountedIo::default();
    let error = verification(fixture.discover(&mut io).unwrap_err());
    assert_eq!(error.code(), ErrorCode::MetadataQuorumUnavailable);
    io.assert_only(&fixture.allowed());
    assert!(!missing.exists());
}

#[test]
fn restricted_discovery_torn_joint_decision_is_not_repaired_or_synchronized() {
    let fixture = Fixture::new();
    let journal = fixture.joint_decision(0);
    fs::OpenOptions::new()
        .append(true)
        .open(&journal)
        .unwrap()
        .write_all(b"torn")
        .unwrap();
    let before = fs::read(&journal).unwrap();
    let mut io = CountedIo::default();
    let error = verification(fixture.discover(&mut io).unwrap_err());
    assert_eq!(error.code(), ErrorCode::MetadataQuorumUnavailable);
    io.assert_only(&fixture.allowed());
    assert_eq!(fs::read(&journal).unwrap(), before);
}

#[test]
fn restricted_discovery_corrupt_visible_marker_does_not_hide_behind_valid_marker() {
    let fixture = Fixture::new();
    let last = fixture
        .original
        .disks
        .iter()
        .max_by(|a, b| a.disk_id.cmp(&b.disk_id))
        .unwrap();
    fs::write(last.root.join(GENERATION_FILE), b"{").unwrap();
    let mut io = CountedIo::default();
    assert!(fixture.discover(&mut io).is_err());
    assert!(
        !io.touches
            .contains(&Touch::Stat(fixture.target.disks[3].root.clone()))
    );
}

#[test]
fn restricted_discovery_checks_held_identity_before_reading_replacement_contents() {
    let fixture = Fixture::new();
    let root = &fixture
        .original
        .disks
        .iter()
        .min_by(|a, b| a.disk_id.cmp(&b.disk_id))
        .unwrap()
        .root;
    fs::rename(root, fixture.temp.path().join("displaced")).unwrap();
    private_dir(root).unwrap();
    fs::write(
        root.join("disk.json"),
        b"malformed contents must not be read",
    )
    .unwrap();
    let mut io = CountedIo::default();
    let error = verification(fixture.discover(&mut io).unwrap_err());
    assert_eq!(error.code(), ErrorCode::DiskIdentityMismatch);
    assert!(error.message().contains("locator's held observation"));
    assert_eq!(
        io.touches,
        vec![Touch::Stat(root.clone()), Touch::Open(root.clone())]
    );
}

#[test]
fn restricted_discovery_missing_and_corrupt_locator_stop_before_any_storage_stat() {
    for head in [None, Some(b"{".as_slice())] {
        let fixture = Fixture::new();
        let path = fixture.control.join("head.json");
        match head {
            None => fs::remove_file(path).unwrap(),
            Some(bytes) => fs::write(path, bytes).unwrap(),
        }
        let mut io = CountedIo::default();
        assert!(matches!(
            fixture.discover(&mut io),
            Err(RestrictedDiscoveryFailure::Bootstrap(_))
        ));
        assert!(io.touches.is_empty());
    }
}

#[test]
fn restricted_discovery_lost_system_state_cannot_reinitialize_from_old_projection() {
    let fixture = Fixture::new();
    let Fixture {
        temp: _temp,
        control,
        store,
        original,
        ..
    } = fixture;
    drop(store);
    fs::remove_dir_all(&control).unwrap();
    assert_eq!(
        File::open(&control).unwrap_err().kind(),
        std::io::ErrorKind::NotFound
    );
    // Even an explicitly recreated empty control directory is management-only.
    private_dir(&control).unwrap();
    let store = BootstrapStore::hold(File::open(&control).unwrap(), control).unwrap();
    let mut io = CountedIo::default();
    assert!(matches!(
        discover_store_with_io(&store, &original, &mut io),
        Err(RestrictedDiscoveryFailure::Bootstrap(_))
    ));
    assert!(io.touches.is_empty());
}

#[test]
fn restricted_discovery_probe_cannot_be_reused_with_another_projection_before_stat() {
    let fixture = Fixture::new();
    let probes = fixture.stopped.select(&fixture.original).unwrap();
    let mut changed = (*fixture.original).clone();
    changed.disks[0].root = fixture.temp.path().join("unknown-root");
    let changed = ValidatedConfig::try_from(changed).unwrap();
    let mut io = CountedIo::default();
    let error = discover_with_io(probes, &changed, &mut io).unwrap_err();
    assert_eq!(error.code(), ErrorCode::RecoveryRequired);
    assert!(io.touches.is_empty());
}

#[test]
fn restricted_discovery_missing_retained_roots_never_falls_back_to_config() {
    let fixture = Fixture::new();
    for (index, disk) in fixture.original.disks.iter().enumerate() {
        fs::rename(
            &disk.root,
            fixture.temp.path().join(format!("missing-{index}")),
        )
        .unwrap();
    }
    let mut io = CountedIo::default();
    let error = verification(fixture.discover(&mut io).unwrap_err());
    assert_eq!(error.code(), ErrorCode::RecoveryRequired);
    assert_eq!(io.touches.len(), 3);
    assert!(
        io.touches
            .iter()
            .all(|touch| matches!(touch, Touch::Stat(_)))
    );
    assert!(
        !io.touches
            .contains(&Touch::Stat(fixture.target.disks[3].root.clone()))
    );
}

struct StartupFixture {
    _temp: tempfile::TempDir,
    system: PathBuf,
    config: ValidatedConfig,
}
impl StartupFixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let system = temp.path().join("system");
        let config =
            Config::development((0..3).map(|index| temp.path().join(format!("disk-{index}"))))
                .unwrap();
        let engine = Engine::initialize(&system, config).unwrap();
        let config = engine.config.clone();
        drop(engine);
        Self {
            _temp: temp,
            system,
            config,
        }
    }
    fn locator(&self) -> PathBuf {
        self.system
            .join(crate::engine::disk_lifecycle::bootstrap::runtime::DIRECTORY)
    }
    fn stop_first(&self) {
        let mut store = open_store(&self.system).unwrap();
        let before = store.reload(&self.config.pool_id).unwrap();
        let stopped = store
            .append_stop(
                &before,
                &binding(&self.config, &self.config.disks[0]),
                OperationId::generate().unwrap(),
                IntentDigest::parse(&"a".repeat(64)).unwrap(),
            )
            .unwrap();
        store.sync_current(&stopped).unwrap();
    }
}

#[test]
fn production_startup_gate_precedes_first_stat_with_stale_config_and_present_target() {
    let fixture = StartupFixture::new();
    let unchanged = storage_contract::canonical_json(&fixture.config).unwrap();
    fixture.stop_first();
    let target = &fixture.config.disks[0].root;
    assert!(target.is_dir());
    // A target marker which would fail any accidental contents access remains
    // physically present. The callback counts stat as well as open before I/O.
    fs::write(target.join("disk.json"), b"must not be read").unwrap();
    let mut io = CountedIo::default();
    let error = discover_startup_with_io(&fixture.system, fixture.config.clone(), &mut io)
        .err()
        .unwrap();
    assert_eq!(error.code(), ErrorCode::RecoveryRequired);
    io.assert_only(
        &fixture.config.disks[1..]
            .iter()
            .map(|disk| disk.root.clone())
            .collect::<Vec<_>>(),
    );
    assert_eq!(
        storage_contract::canonical_json(&fixture.config).unwrap(),
        unchanged
    );
    assert_eq!(
        Engine::open(&fixture.system).err().unwrap().code(),
        ErrorCode::RecoveryRequired
    );
}

#[test]
fn production_open_installs_and_consumes_genesis_locator_without_changing_fixed_membership() {
    let fixture = StartupFixture::new();
    let engine = Engine::open(&fixture.system).unwrap();
    assert_eq!(engine.config.metadata_members().len(), 3);
    assert_eq!(engine.config.metadata_quorum(), 2);
    assert_eq!(engine.config.io_disks().count(), 3);
}

#[test]
fn production_startup_missing_corrupt_or_absent_system_locator_never_stats_storage() {
    for failure in 0..4 {
        let fixture = StartupFixture::new();
        match failure {
            0 => fs::remove_file(fixture.locator().join("head.json")).unwrap(),
            1 => fs::write(fixture.locator().join("head.json"), b"{").unwrap(),
            2 => fs::remove_dir_all(fixture.locator()).unwrap(),
            3 => fs::remove_dir_all(&fixture.system).unwrap(),
            _ => unreachable!(),
        }
        let mut io = CountedIo::default();
        let error = discover_startup_with_io(&fixture.system, fixture.config.clone(), &mut io)
            .err()
            .unwrap();
        assert_eq!(error.code(), ErrorCode::RecoveryRequired);
        assert!(io.touches.is_empty());
        assert_eq!(
            Engine::open(&fixture.system).err().unwrap().code(),
            ErrorCode::RecoveryRequired
        );
    }
}

#[test]
fn production_startup_arbitrary_valid_hint_edit_cannot_lower_authority_quorum() {
    let fixture = StartupFixture::new();
    let record_path = fixture.locator().join("record-00000000000000000001.json");
    let mut record: json_codec::Value = decode_strict(&fs::read(&record_path).unwrap()).unwrap();
    let retained_id = fixture.config.disks[0].disk_id.clone();
    for root in record["roots"].as_array_mut().unwrap() {
        root["metadata_member"] = (root["disk_id"].as_str() == Some(retained_id.as_str())).into();
    }
    record["retained"] = json_codec::Value::Array(vec![retained_id.into()]);
    let (bytes, hash) =
        storage_contract::encode_hashed_object("bootstrap-locator", &record).unwrap();
    fs::write(record_path, bytes).unwrap();
    let head_path = fixture.locator().join("head.json");
    let mut head: json_codec::Value = decode_strict(&fs::read(&head_path).unwrap()).unwrap();
    head["record_hash"] = hash.into();
    fs::write(head_path, storage_contract::canonical_json(&head).unwrap()).unwrap();
    let mut io = CountedIo::default();
    let error = discover_startup_with_io(&fixture.system, fixture.config.clone(), &mut io)
        .err()
        .unwrap();
    assert_eq!(error.code(), ErrorCode::MetadataQuorumUnavailable);
    io.assert_only(&[fixture.config.disks[0].root.clone()]);
    assert_eq!(fixture.config.metadata_members().len(), 3);
    assert_eq!(fixture.config.metadata_quorum(), 2);
}

#[test]
fn production_explicit_restore_never_expands_to_saved_config_root_addresses() {
    let fixture = StartupFixture::new();
    let unrequested = &fixture.config.disks[0].root;
    assert!(unrequested.is_dir());
    fs::write(
        unrequested.join("disk.json"),
        b"unrequested retained root must not be opened",
    )
    .unwrap();
    let restored_system = fixture._temp.path().join("restored-system");
    let retained: Vec<_> = fixture.config.disks[1..]
        .iter()
        .map(|disk| disk.root.clone())
        .collect();
    let restored = Engine::restore_from_disks(&restored_system, &retained).unwrap();
    assert_eq!(restored.config.disks.len(), 3);
    assert_eq!(restored.config.metadata_quorum(), 2);
    assert_eq!(restored.config.io_disks().count(), 2);
    assert!(
        restored
            .config
            .excludes_io_disk(&fixture.config.disks[0].disk_id)
    );
    drop(restored);
    let reopened = Engine::open(&restored_system).unwrap();
    assert_eq!(reopened.config.io_disks().count(), 2);
    assert_eq!(
        fs::read(unrequested.join("disk.json")).unwrap(),
        b"unrequested retained root must not be opened"
    );
}

#[test]
fn production_startup_wrong_pool_hint_never_selects_another_pool_or_stats_roots() {
    let fixture = StartupFixture::new();
    let head_path = fixture.locator().join("head.json");
    let mut head: json_codec::Value = decode_strict(&fs::read(&head_path).unwrap()).unwrap();
    head["pool_id"] = format!("pool_{}", "f".repeat(32)).into();
    fs::write(head_path, storage_contract::canonical_json(&head).unwrap()).unwrap();
    let mut io = CountedIo::default();
    assert_eq!(
        discover_startup_with_io(&fixture.system, fixture.config.clone(), &mut io)
            .err()
            .unwrap()
            .code(),
        ErrorCode::RecoveryRequired
    );
    assert!(io.touches.is_empty());
}

#[test]
fn production_unstopped_relocation_is_identity_checked_and_frozen_before_first_stop() {
    let fixture = StartupFixture::new();
    let moved: Vec<_> = fixture
        .config
        .disks
        .iter()
        .enumerate()
        .map(|(index, disk)| {
            let path = fixture._temp.path().join(format!("relocated-{index}"));
            fs::rename(&disk.root, &path).unwrap();
            path
        })
        .collect();
    let config = fixture
        .config
        .clone()
        .with_disk_roots(moved.clone())
        .unwrap();
    write_json(&fixture.system.join("config.json"), &config, true).unwrap();
    let mut engine = Engine::open(&fixture.system).unwrap();
    assert_eq!(
        engine
            .config
            .disks
            .iter()
            .map(|disk| disk.root.clone())
            .collect::<Vec<_>>(),
        moved
    );
    engine
        .create_repository(
            &RepositoryName::parse("relocated").unwrap(),
            "P01",
            1,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    let target = binding(&config, &config.disks[0]);
    let operation = OperationId::generate().unwrap();
    engine
        .begin_disk_exit(
            &target.disk_id,
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
    let result = engine.activate_pending_disk_exit().unwrap();
    assert_eq!(result.operation_id, operation);
    drop(engine);
    let mut io = CountedIo::default();
    let (config, _) = discover_startup_with_io(&fixture.system, config, &mut io).unwrap();
    assert!(config.excludes_io_disk(&target.disk_id));
    assert_eq!(config.metadata_members().len(), 2);
    assert_eq!(config.metadata_quorum(), 2);
    io.assert_only(&moved[1..]);
}
