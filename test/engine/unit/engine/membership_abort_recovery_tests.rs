//! Recovery boundary definitions. These exercise real files, not an authority
//! constructor standing in for local slots or an Engine startup result.
use super::*;
use crate::engine::{
    disk_lifecycle::bootstrap::{RootObservation, store::BootstrapStore},
    disk_registry::DiskRegistrationIdentity,
    membership_transition::abort::tests::authority_reads::{ReadFixture, read_fixture},
};
use std::fs;

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
impl CountedIo {
    fn assert_original_only(&self, fixture: &ReadFixture) {
        let expected = fixture
            .original
            .disks
            .iter()
            .flat_map(|disk| [disk.root.clone(), disk.root.clone()])
            .collect::<Vec<_>>();
        let mut expected = expected;
        expected.sort();
        let mut actual = self.touches.clone();
        actual.sort();
        assert_eq!(actual, expected);
    }
}

fn copy_tree(from: &Path, to: &Path) {
    private_dir(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let destination = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &destination);
        } else {
            fs::copy(entry.path(), destination).unwrap();
        }
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

fn install_archives(fixture: &ReadFixture) {
    for disk in &fixture.original.disks {
        copy_tree(
            &fixture.source,
            &member_directory(disk, &fixture.reference.transition_hash)
                .join(CONTROL_ARCHIVE_DIRECTORY),
        );
    }
}

fn install_marker(disk: &DiskConfig, authority: &PortableAuthority) {
    let directory = generation_directory(disk, &authority.hash().unwrap());
    private_dir(&directory).unwrap();
    fs::write(directory.join("journal.bin"), []).unwrap();
    fs::write(
        disk.root.join(GENERATION_FILE),
        canonical_json(authority).unwrap(),
    )
    .unwrap();
}

fn forbid_rejected_target(fixture: &ReadFixture) {
    let intent = fixture.authority.intent().unwrap();
    let rejected = &intent.target.disks.last().unwrap().root;
    fs::rename(rejected, fixture.temp.path().join("preserved-target")).unwrap();
    // Even statting this rejected locator violates the discovery contract.
    // disk_unavailable would return UnsafePath if discovery followed the hint.
    std::os::unix::fs::symlink("/cancelled-target-must-not-be-probed", rejected).unwrap();
}

fn locator(fixture: &ReadFixture) -> BootstrapStore {
    let path = fixture.temp.path().join("bootstrap");
    private_dir(&path).unwrap();
    let mut store = BootstrapStore::hold(File::open(&path).unwrap(), path).unwrap();
    let observations = fixture
        .original
        .disks
        .iter()
        .map(|disk| {
            let root = DiskRoot::open(&disk.root).unwrap();
            RootObservation::capture(
                &DiskRegistrationIdentity {
                    pool_id: fixture.original.pool_id.clone(),
                    disk_id: disk.disk_id.clone(),
                    slot_id: disk.slot_id.clone(),
                    binding_generation: disk.binding_generation,
                },
                &disk.root,
                &root.directory(root.path()).unwrap(),
                disk.metadata_member,
            )
            .unwrap()
        })
        .collect();
    store
        .initialize(&fixture.original.pool_id, observations)
        .unwrap();
    store
}

#[test]
fn abort_discovery_authenticates_every_original_marker_before_any_fence_hint() {
    for marker_index in 0..3 {
        let fixture = read_fixture();
        install_archives(&fixture);
        install_marker(&fixture.original.disks[marker_index], &fixture.authority);
        forbid_rejected_target(&fixture);
        let before = fixture
            .original
            .disks
            .iter()
            .map(|disk| files(&disk.root))
            .collect::<Vec<_>>();
        let config =
            discover(ValidatedConfig::try_from(fixture.original.clone()).unwrap()).unwrap();
        assert_eq!(
            config.generation_hash(),
            Some(fixture.authority.hash().unwrap().as_str())
        );
        assert_eq!(config.disks.len(), 3);
        assert_eq!(config.metadata_members().len(), 3);
        assert_eq!(config.metadata_quorum(), 2);
        assert_eq!(
            config.generation().unwrap().base.sequence,
            fixture.base.sequence
        );
        for (disk, saved) in fixture.original.disks.iter().zip(before) {
            assert_eq!(files(&disk.root), saved);
        }
    }
}

#[test]
fn abort_discovery_never_hides_an_invalid_marker_behind_a_valid_later_marker() {
    let fixture = read_fixture();
    install_archives(&fixture);
    install_marker(&fixture.original.disks[2], &fixture.authority);
    fs::write(fixture.original.disks[0].root.join(GENERATION_FILE), b"{}").unwrap();
    forbid_rejected_target(&fixture);
    let saved = files(&fixture.original.disks[0].root);
    assert!(discover(ValidatedConfig::try_from(fixture.original.clone()).unwrap()).is_err());
    assert_eq!(files(&fixture.original.disks[0].root), saved);
}

#[test]
fn abort_discovery_same_generation_accepts_different_valid_old_quorum_witnesses() {
    let fixture = read_fixture();
    install_archives(&fixture);
    for (index, disk) in fixture.original.disks.iter().enumerate() {
        install_marker(
            disk,
            if index == 1 {
                &fixture.alternate
            } else {
                &fixture.authority
            },
        );
    }
    forbid_rejected_target(&fixture);
    let config = discover(ValidatedConfig::try_from(fixture.original.clone()).unwrap()).unwrap();
    assert_eq!(
        config.generation_hash(),
        Some(fixture.authority.hash().unwrap().as_str())
    );
    assert_eq!(
        config.metadata_members().len(),
        fixture.original.disks.len()
    );
    assert_eq!(config.generation().unwrap().history.len(), 1);
}

#[test]
fn restricted_abort_discovery_keeps_original_quorum_and_never_follows_target() {
    let fixture = read_fixture();
    install_archives(&fixture);
    for (index, disk) in fixture.original.disks.iter().enumerate() {
        install_marker(
            disk,
            if index == 0 {
                &fixture.authority
            } else {
                &fixture.alternate
            },
        );
    }
    let store = locator(&fixture);
    forbid_rejected_target(&fixture);
    let config = ValidatedConfig::try_from(fixture.original.clone()).unwrap();
    let before = fixture
        .original
        .disks
        .iter()
        .map(|disk| files(&disk.root))
        .collect::<Vec<_>>();
    let mut io = CountedIo::default();
    let evidence = discover_store_with_io(&store, &config, &mut io).unwrap();
    io.assert_original_only(&fixture);
    assert_eq!(evidence.authority_hash(), fixture.authority.hash().unwrap());
    assert_eq!(evidence.metadata_members(), 3);
    assert_eq!(evidence.metadata_quorum(), 2);
    assert_eq!(evidence.verified_members().len(), 3);
    assert!(evidence.rejected_members().is_empty());
    for (disk, saved) in fixture.original.disks.iter().zip(before) {
        assert_eq!(files(&disk.root), saved);
    }
}

#[test]
fn restricted_abort_discovery_does_not_replace_missing_local_archives_from_healthy_f() {
    let fixture = read_fixture();
    install_archives(&fixture);
    for disk in &fixture.original.disks {
        install_marker(disk, &fixture.authority);
    }
    let store = locator(&fixture);
    let config = ValidatedConfig::try_from(fixture.original.clone()).unwrap();
    for disk in fixture.original.disks.iter().take(2) {
        let path = member_directory(disk, &fixture.reference.transition_hash)
            .join(CONTROL_ARCHIVE_DIRECTORY);
        fs::rename(&path, path.with_extension("preserved")).unwrap();
    }
    let before = fixture
        .original
        .disks
        .iter()
        .map(|disk| files(&disk.root))
        .collect::<Vec<_>>();
    let mut io = CountedIo::default();
    let error = discover_store_with_io(&store, &config, &mut io).unwrap_err();
    io.assert_original_only(&fixture);
    let RestrictedDiscoveryFailure::Verification(error) = error else {
        panic!("the locator remains valid")
    };
    assert_eq!(error.code(), ErrorCode::MetadataQuorumUnavailable);
    assert!(
        error
            .to_string()
            .contains("fixed 3 metadata members; quorum is 2")
    );
    for (disk, saved) in fixture.original.disks.iter().zip(before) {
        assert_eq!(files(&disk.root), saved);
    }
}

#[test]
fn abort_missing_current_journal_cannot_fall_back_to_the_intact_frozen_archive() {
    let fixture = read_fixture();
    install_archives(&fixture);
    for disk in &fixture.original.disks {
        install_marker(disk, &fixture.authority);
    }
    let store = locator(&fixture);
    let config = discover(ValidatedConfig::try_from(fixture.original.clone()).unwrap()).unwrap();
    for disk in fixture.original.disks.iter().take(2) {
        let journal =
            generation_directory(disk, &fixture.authority.hash().unwrap()).join("journal.bin");
        fs::rename(&journal, journal.with_extension("preserved")).unwrap();
        let root = DiskRoot::open(&disk.root).unwrap();
        assert_eq!(
            activation_namespace_present(&root, disk, &config)
                .unwrap_err()
                .code(),
            ErrorCode::RecoveryRequired,
        );
    }
    let before = fixture
        .original
        .disks
        .iter()
        .map(|disk| files(&disk.root))
        .collect::<Vec<_>>();
    let mut io = CountedIo::default();
    let error = discover_store_with_io(&store, &config, &mut io).unwrap_err();
    io.assert_original_only(&fixture);
    let RestrictedDiscoveryFailure::Verification(error) = error else {
        panic!("the locator remains valid")
    };
    assert_eq!(error.code(), ErrorCode::MetadataQuorumUnavailable);
    for (disk, saved) in fixture.original.disks.iter().zip(before) {
        assert_eq!(files(&disk.root), saved);
    }
}

#[test]
fn abort_unmarked_staging_namespace_can_resume_but_does_not_count_as_current() {
    let fixture = read_fixture();
    install_archives(&fixture);
    install_marker(&fixture.original.disks[0], &fixture.authority);
    let config = discover(ValidatedConfig::try_from(fixture.original.clone()).unwrap()).unwrap();
    let disk = &fixture.original.disks[1];
    private_dir(&generation_directory(
        disk,
        &fixture.authority.hash().unwrap(),
    ))
    .unwrap();
    let root = DiskRoot::open(&disk.root).unwrap();
    let before = files(&disk.root);
    assert!(!activation_namespace_present(&root, disk, &config).unwrap());
    assert_eq!(files(&disk.root), before);
}

fn addition_target(config: &Config, temp: &Path, label: &str) -> Config {
    let mut target = config.clone();
    let extra = Config::development(
        (0..=config.disks.len()).map(|index| temp.join(format!("{label}-{index}"))),
    )
    .unwrap()
    .disks
    .pop()
    .unwrap();
    private_dir(&extra.root).unwrap();
    target.disks.push(extra);
    target
}

fn publish_addition(engine: &mut Engine, target: Config) -> PortableAuthority {
    let added = target.disks.last().unwrap().clone();
    engine
        .begin_membership_handoff(target, "admin", OperationId::generate().unwrap())
        .unwrap();
    let archive = engine.build_managed_membership_control_archive().unwrap();
    engine
        .enroll_pending_members(
            &archive.archive_path,
            &archive.root_hash,
            vec![PendingMemberDirectory {
                disk_id: added.disk_id,
                directory: File::open(&added.root).unwrap(),
            }],
        )
        .unwrap();
    engine.commit_membership_joint_decision().unwrap();
    engine.activate_pending_membership().unwrap();
    engine.config.generation().unwrap().portable.clone()
}

fn publish_cancellation(
    engine: &mut Engine,
    target: Config,
    preserved: &Path,
) -> PortableAuthority {
    let rejected = target.disks.last().unwrap().root.clone();
    engine
        .begin_membership_handoff(target, "admin", OperationId::generate().unwrap())
        .unwrap();
    let archive = engine.build_managed_membership_control_archive().unwrap();
    fs::rename(&rejected, preserved).unwrap();
    std::os::unix::fs::symlink("/cancelled-ancestor-must-not-be-probed", &rejected).unwrap();
    engine
        .cancel_membership_handoff(&archive.archive_path, &archive.root_hash)
        .unwrap();
    engine.config.generation().unwrap().portable.clone()
}

#[test]
fn add_then_abort_mixed_predecessor_markers_select_abort_before_old_fence_hints() {
    let temp = tempfile::tempdir().unwrap();
    let original =
        Config::development((0..3).map(|n| temp.path().join(format!("old-{n}")))).unwrap();
    let system = temp.path().join("system");
    let mut engine = Engine::initialize(&system, original.clone()).unwrap();
    let effective = addition_target(&original, temp.path(), "added");
    let predecessor = publish_addition(&mut engine, effective.clone());
    let predecessor_seals = effective
        .disks
        .iter()
        .map(|disk| fs::read(disk.root.join("disk.json")).unwrap())
        .collect::<Vec<_>>();
    let target = addition_target(&effective, temp.path(), "cancelled");
    let cancellation = publish_cancellation(
        &mut engine,
        target,
        &temp.path().join("preserved-cancelled-target"),
    );
    drop(engine);
    // The current marker is deliberately last in projection order. Every
    // earlier root still presents the old Add and its later cancellation fence.
    for (disk, seal) in effective.disks.iter().zip(predecessor_seals).take(3) {
        fs::write(
            disk.root.join(GENERATION_FILE),
            canonical_json(&predecessor).unwrap(),
        )
        .unwrap();
        fs::write(disk.root.join("disk.json"), seal).unwrap();
    }
    let before = effective
        .disks
        .iter()
        .map(|disk| files(&disk.root))
        .collect::<Vec<_>>();
    let recovered = discover(ValidatedConfig::try_from(effective.clone()).unwrap()).unwrap();
    assert_eq!(
        recovered.generation_hash(),
        Some(cancellation.hash().unwrap().as_str())
    );
    assert_eq!(recovered.disks.len(), 4);
    assert_eq!(recovered.metadata_quorum(), 3);
    assert!(
        recovered
            .generation()
            .unwrap()
            .portable
            .contains(&predecessor.hash().unwrap())
            .unwrap()
    );
    for (disk, saved) in effective.disks.iter().zip(before) {
        assert_eq!(files(&disk.root), saved);
    }
}

#[test]
fn successor_add_head_filters_rejected_fence_using_its_complete_abort_lineage() {
    let temp = tempfile::tempdir().unwrap();
    let original =
        Config::development((0..3).map(|n| temp.path().join(format!("old-{n}")))).unwrap();
    let mut engine = Engine::initialize(&temp.path().join("system"), original.clone()).unwrap();
    let genesis_seal = fs::read(original.disks[0].root.join("disk.json")).unwrap();
    let rejected = addition_target(&original, temp.path(), "cancelled");
    let cancellation = publish_cancellation(
        &mut engine,
        rejected,
        &temp.path().join("preserved-cancelled-target"),
    );
    let cancelled_seal = fs::read(original.disks[1].root.join("disk.json")).unwrap();
    let effective = addition_target(&original, temp.path(), "successor");
    let successor = publish_addition(&mut engine, effective.clone());
    drop(engine);
    let first = &original.disks[0];
    fs::rename(
        first.root.join(GENERATION_FILE),
        first.root.join("preserved-generation.json"),
    )
    .unwrap();
    fs::write(first.root.join("disk.json"), genesis_seal).unwrap();
    let second = &original.disks[1];
    fs::write(
        second.root.join(GENERATION_FILE),
        canonical_json(&cancellation).unwrap(),
    )
    .unwrap();
    fs::write(second.root.join("disk.json"), cancelled_seal).unwrap();
    // The first root has no candidate marker. Only the selected later Add's
    // complete lineage can reject its old fence before it queues that locator.
    let before = effective
        .disks
        .iter()
        .map(|disk| files(&disk.root))
        .collect::<Vec<_>>();
    let recovered = discover(ValidatedConfig::try_from(original).unwrap()).unwrap();
    assert_eq!(
        recovered.generation_hash(),
        Some(successor.hash().unwrap().as_str())
    );
    assert_eq!(recovered.disks.len(), 4);
    assert!(
        recovered
            .generation()
            .unwrap()
            .portable
            .rejects_transition(&object_hash(INTENT_KIND, &cancellation.intent().unwrap()).unwrap())
            .unwrap()
    );
    for (disk, saved) in effective.disks.iter().zip(before) {
        assert_eq!(files(&disk.root), saved);
    }
}
