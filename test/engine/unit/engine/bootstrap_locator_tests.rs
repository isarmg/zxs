//! Temp-directory source tests for the local restriction primitive. These do NOT
//! test Engine::open, publication authority, physical power loss or real disks.
use super::store::BootstrapStore;
use super::*;
use platform_io::fs_safety::PrivateDirectory;
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
};

struct Fixture {
    _temporary: tempfile::TempDir,
    control: PathBuf,
    config: Config,
    store: BootstrapStore,
    initial: LoadedLocator,
}
impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let control = temporary.path().join("control");
        private_dir(&control);
        let paths: Vec<_> = (0..3)
            .map(|index| temporary.path().join(format!("root-{index}")))
            .collect();
        for path in &paths {
            private_dir(path);
        }
        let config = Config::development(paths).unwrap();
        let roots = observe(&config);
        let mut store =
            BootstrapStore::hold(File::open(&control).unwrap(), control.clone()).unwrap();
        let initial = store.initialize(&config.pool_id, roots).unwrap();
        Self {
            _temporary: temporary,
            control,
            config,
            store,
            initial,
        }
    }
    fn binding(&self, index: usize) -> DiskRegistrationIdentity {
        binding(&self.config, index)
    }
    fn stop(&mut self) -> LoadedLocator {
        let target = self.binding(0);
        self.store
            .append_stop(&self.initial, &target, operation(1), digest())
            .unwrap()
    }
    fn record(&self, generation: u64) -> PathBuf {
        self.control.join(format!("record-{generation:020}.json"))
    }
    fn write_head(&self, head: &Head) {
        private_file(&self.control.join("head.json"), &head.bytes().unwrap());
    }
}
fn private_dir(path: &Path) {
    fs::create_dir(path).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}
fn private_file(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}
fn operation(number: usize) -> OperationId {
    format!("op_{number:032x}").parse().unwrap()
}
fn digest() -> IntentDigest {
    IntentDigest::parse(&"a".repeat(64)).unwrap()
}
fn binding(config: &Config, index: usize) -> DiskRegistrationIdentity {
    let disk = &config.disks[index];
    DiskRegistrationIdentity {
        pool_id: config.pool_id.clone(),
        disk_id: disk.disk_id.clone(),
        slot_id: disk.slot_id.clone(),
        binding_generation: disk.binding_generation,
    }
}
fn observe(config: &Config) -> Vec<RootObservation> {
    config
        .disks
        .iter()
        .enumerate()
        .map(|(index, disk)| {
            RootObservation::capture(
                &binding(config, index),
                &disk.root,
                &File::open(&disk.root).unwrap(),
                disk.metadata_member,
            )
            .unwrap()
        })
        .collect()
}
fn selected_paths(locator: &LoadedLocator, config: &Config) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    locator
        .select(config)
        .unwrap()
        .try_for_each(|probe| {
            paths.push(probe.path().to_owned());
            Ok::<_, ()>(())
        })
        .unwrap();
    paths
}

#[test]
fn bootstrap_locator_pending_stop_without_intent_never_probes_present_target_from_stale_config() {
    let mut fixture = Fixture::new();
    let unchanged = storage_contract::canonical_json(&fixture.config).unwrap();
    let target = fixture.config.disks[0].root.clone();
    let stopped = fixture.stop();
    // Root 0 was physically created and opened by fixture admission, not merely
    // a nonexistent pathname. No intent/fence/capsule is ever written here.
    let reloaded = fixture.store.reload(&fixture.config.pool_id).unwrap();
    assert_eq!(reloaded.generation(), stopped.generation());
    let mut stats = Vec::new();
    let mut opens = Vec::new();
    reloaded
        .select(&fixture.config)
        .unwrap()
        .try_for_each(|probe| {
            stats.push(probe.path().to_owned());
            let metadata = fs::symlink_metadata(probe.path()).unwrap();
            assert!(metadata.is_dir());
            opens.push(probe.path().to_owned());
            let held = File::open(probe.path()).unwrap();
            assert_eq!(
                FileIdentity::from_fd(&held).unwrap(),
                probe.expected_identity()
            );
            assert_ne!(probe.disk_id(), fixture.config.disks[0].disk_id);
            Ok::<_, ()>(())
        })
        .unwrap();
    assert_eq!(stats.len(), 2);
    assert_eq!(opens.len(), 2);
    assert!(!stats.contains(&target));
    assert!(!opens.contains(&target));
    // Locator selection has not changed registration or its fixed metadata set.
    assert_eq!(
        storage_contract::canonical_json(&fixture.config).unwrap(),
        unchanged
    );
    assert_eq!(
        fixture
            .config
            .disks
            .iter()
            .filter(|disk| disk.metadata_member)
            .count(),
        3
    );
}

#[test]
fn bootstrap_locator_distinct_directories_on_same_filesystem_are_supported() {
    let fixture = Fixture::new();
    let roots = &fixture.initial.record.roots;
    assert_eq!(roots[0].device, roots[1].device);
    assert_ne!(roots[0].inode, roots[1].inode);
    assert_eq!(fixture.initial.select(&fixture.config).unwrap().len(), 3);
}

#[test]
fn bootstrap_locator_canonical_roundtrip_keeps_prior_records_immutable() {
    let mut fixture = Fixture::new();
    let initial_bytes = fs::read(fixture.record(1)).unwrap();
    let stopped = fixture.stop();
    assert_eq!(fs::read(fixture.record(1)).unwrap(), initial_bytes);
    let bytes = fs::read(fixture.record(2)).unwrap();
    let decoded = Record::decode(&bytes).unwrap();
    assert_eq!(decoded.encode().unwrap().0, bytes);
    assert_eq!(decoded.encode().unwrap().1, stopped.head);
    assert_eq!(decoded.previous, Some(fixture.initial.head.clone()));
}

#[test]
fn bootstrap_locator_missing_head_does_not_reinitialize_or_fall_back() {
    let mut fixture = Fixture::new();
    fixture.stop();
    fs::remove_file(fixture.control.join("head.json")).unwrap();
    assert!(fixture.store.reload(&fixture.config.pool_id).is_err());
    assert!(
        fixture
            .store
            .initialize(&fixture.config.pool_id, observe(&fixture.config))
            .is_err()
    );
    assert!(fixture.record(2).is_file());
}

#[test]
fn bootstrap_locator_missing_current_payload_never_uses_previous_payload() {
    let mut fixture = Fixture::new();
    fixture.stop();
    fs::remove_file(fixture.record(2)).unwrap();
    assert!(fixture.store.reload(&fixture.config.pool_id).is_err());
    assert!(fixture.record(1).is_file());
}

#[test]
fn bootstrap_locator_old_valid_head_cannot_hide_a_newer_stop_record() {
    let mut fixture = Fixture::new();
    fixture.stop();
    fixture.write_head(&fixture.initial.head);
    assert!(fixture.store.reload(&fixture.config.pool_id).is_err());
}

#[test]
fn bootstrap_locator_partial_atomic_temp_is_not_ignored() {
    let fixture = Fixture::new();
    let temporary = fixture
        .control
        .join(".state-atomic-0123456789abcdef0123456789abcdef.tmp");
    private_file(&temporary, b"{\"pending");
    assert!(fixture.store.reload(&fixture.config.pool_id).is_err());
    assert_eq!(fs::read(temporary).unwrap(), b"{\"pending");
}

#[test]
fn bootstrap_locator_truncated_head_and_payload_are_management_only() {
    for head in [true, false] {
        let mut fixture = Fixture::new();
        fixture.stop();
        let path = if head {
            fixture.control.join("head.json")
        } else {
            fixture.record(2)
        };
        private_file(&path, b"{");
        assert!(fixture.store.reload(&fixture.config.pool_id).is_err());
    }
}

#[test]
fn bootstrap_locator_wrong_pool_generation_and_hash_are_management_only() {
    let mut fixture = Fixture::new();
    let stopped = fixture.stop();
    let other_pool = "pool_ffffffffffffffffffffffffffffffff";
    assert!(fixture.store.reload(other_pool).is_err());
    for mutation in 0..3 {
        let mut head = stopped.head.clone();
        match mutation {
            0 => head.pool_id = other_pool.into(),
            1 => head.generation += 1,
            2 => head.record_hash = "b".repeat(64).parse().unwrap(),
            _ => unreachable!(),
        }
        fixture.write_head(&head);
        assert!(fixture.store.reload(&fixture.config.pool_id).is_err());
    }
}

#[test]
fn bootstrap_locator_modified_old_record_breaks_chain_instead_of_rolling_back() {
    let mut fixture = Fixture::new();
    fixture.stop();
    let mut prior = fixture.initial.record.clone();
    prior.roots[0].inode += 1;
    private_file(&fixture.record(1), &prior.encode().unwrap().0);
    assert!(fixture.store.reload(&fixture.config.pool_id).is_err());
}

#[test]
fn bootstrap_locator_unknown_duplicate_and_noncanonical_fields_are_rejected() {
    let fixture = Fixture::new();
    let record = fs::read(fixture.record(1)).unwrap();
    let mut spaced = record.clone();
    spaced.push(b'\n');
    assert!(Record::decode(&spaced).is_err());
    let mut value: json_codec::Value = json_codec::from_slice(&record).unwrap();
    value
        .as_object_mut()
        .unwrap()
        .insert("unknown".into(), true.into());
    assert!(Record::decode(&storage_contract::canonical_json(&value).unwrap()).is_err());
    let duplicate = format!(
        "{{\"pool_id\":\"{}\",{}",
        fixture.config.pool_id,
        String::from_utf8(record).unwrap().trim_start_matches('{')
    );
    assert!(Record::decode(duplicate.as_bytes()).is_err());
}

#[test]
fn bootstrap_locator_oversized_payload_is_bounded_before_decode() {
    let fixture = Fixture::new();
    private_file(&fixture.record(1), &vec![b' '; MAX_RECORD_BYTES + 1]);
    assert!(fixture.store.reload(&fixture.config.pool_id).is_err());
    assert!(Record::decode(&vec![b' '; MAX_RECORD_BYTES + 1]).is_err());
}

#[test]
fn bootstrap_locator_generation_overflow_is_rejected_without_panic() {
    let fixture = Fixture::new();
    let mut record = fixture.initial.record.clone();
    let mut previous = fixture.initial.head.clone();
    previous.generation = u64::MAX;
    record.previous = Some(previous);
    assert!(Record::decode(&record.encode().unwrap().0).is_err());
}

#[test]
fn bootstrap_locator_uncertain_alias_or_binding_projection_causes_zero_callbacks() {
    let mut fixture = Fixture::new();
    let stopped = fixture.stop();
    for mutation in 0..4 {
        let mut projected = fixture.config.clone();
        match mutation {
            0 => projected.disks[0].root = projected.disks[1].root.clone(),
            1 => projected.disks[0].root = fixture._temporary.path().join("unknown-alias"),
            2 => projected.disks[1].binding_generation += 1,
            3 => projected.disks.push(projected.disks[1].clone()),
            _ => unreachable!(),
        }
        let mut probes = 0;
        if let Ok(selection) = stopped.select(&projected) {
            selection
                .try_for_each(|_| {
                    probes += 1;
                    Ok::<_, ()>(())
                })
                .unwrap();
            panic!("ambiguous projection must not yield a probe set");
        }
        assert_eq!(probes, 0);
    }
}

#[test]
fn bootstrap_locator_duplicate_observed_directory_identity_is_rejected() {
    let fixture = Fixture::new();
    let mut observations = observe(&fixture.config);
    observations[1] = RootObservation::capture(
        &fixture.binding(1),
        &fixture.config.disks[1].root,
        &File::open(&fixture.config.disks[0].root).unwrap(),
        true,
    )
    .unwrap();
    assert!(Record::initial(&fixture.config.pool_id, observations).is_err());
}

#[test]
fn bootstrap_locator_duplicate_or_nested_recorded_paths_are_rejected() {
    let fixture = Fixture::new();
    for nested in [false, true] {
        let mut observations = observe(&fixture.config);
        let path = if nested {
            fixture.config.disks[0].root.join("nested")
        } else {
            fixture.config.disks[0].root.clone()
        };
        observations[1].record.path = path.to_str().unwrap().into();
        assert!(Record::initial(&fixture.config.pool_id, observations).is_err());
    }
}

#[test]
fn bootstrap_locator_noncanonical_addresses_are_not_stat_candidates() {
    for path in [
        "relative", "/", "/x/../y", "/x/./y", "/x//y", "/x/", "/x\0y",
    ] {
        assert!(canonical_root(Path::new(path)).is_err(), "{path:?}");
    }
}

#[test]
fn bootstrap_locator_each_write_boundary_blocks_fence_until_successful_retry() {
    use store::WriteFault;
    for fault in [
        WriteFault::BeforeRecordBarrier,
        WriteFault::AfterRecordBarrier,
        WriteFault::BeforeHeadBarrier,
        WriteFault::AfterHeadBarrier,
    ] {
        let mut fixture = Fixture::new();
        let target = fixture.binding(0);
        fixture.store.inject(fault);
        let result = fixture
            .store
            .append_stop(&fixture.initial, &target, operation(1), digest());
        assert!(
            result.is_err(),
            "interruption must not yield durable success: {fault:?}"
        );
        let after = fixture.store.reload(&fixture.config.pool_id);
        match fault {
            WriteFault::BeforeRecordBarrier => assert_eq!(after.unwrap().generation(), 1),
            WriteFault::AfterRecordBarrier | WriteFault::BeforeHeadBarrier => {
                assert!(after.is_err())
            }
            WriteFault::AfterHeadBarrier => assert_eq!(after.unwrap().generation(), 2),
        }
        // Same request can finish an exact orphan or repeat real fsync barriers.
        // Injection models interruption at boundaries, not an executed fsync failure.
        let retry = fixture
            .store
            .append_stop(&fixture.initial, &target, operation(1), digest())
            .unwrap();
        assert_eq!(retry.generation(), 2);
        assert!(!selected_paths(&retry, &fixture.config).contains(&fixture.config.disks[0].root));
        assert_eq!(
            fixture.store.reload(&fixture.config.pool_id).unwrap().head,
            retry.head
        );
    }
}

#[test]
fn bootstrap_locator_orphan_collision_is_preserved_and_not_replaced() {
    let mut fixture = Fixture::new();
    fixture.store.inject(store::WriteFault::AfterRecordBarrier);
    let target = fixture.binding(0);
    assert!(
        fixture
            .store
            .append_stop(&fixture.initial, &target, operation(1), digest())
            .is_err()
    );
    let bytes = fs::read(fixture.record(2)).unwrap();
    assert!(
        fixture
            .store
            .append_stop(&fixture.initial, &target, operation(2), digest())
            .is_err()
    );
    assert_eq!(fs::read(fixture.record(2)).unwrap(), bytes);
    assert!(fixture.store.reload(&fixture.config.pool_id).is_err());
}

#[test]
fn bootstrap_locator_obsolete_caller_cannot_replace_new_head() {
    let mut fixture = Fixture::new();
    let stopped = fixture.stop();
    let target = fixture.binding(1);
    assert!(
        fixture
            .store
            .append_stop(&fixture.initial, &target, operation(2), digest())
            .is_err()
    );
    assert_eq!(
        fixture.store.reload(&fixture.config.pool_id).unwrap().head,
        stopped.head
    );
}

#[test]
fn bootstrap_locator_last_retained_root_cannot_be_removed() {
    let mut fixture = Fixture::new();
    let first = fixture.stop();
    let second_target = fixture.binding(1);
    let last_target = fixture.binding(2);
    let second = fixture
        .store
        .append_stop(&first, &second_target, operation(2), digest())
        .unwrap();
    assert_eq!(second.select(&fixture.config).unwrap().len(), 1);
    assert!(
        fixture
            .store
            .append_stop(&second, &last_target, operation(3), digest())
            .is_err()
    );
    assert_eq!(
        fixture.store.reload(&fixture.config.pool_id).unwrap().head,
        second.head
    );
}

#[test]
fn bootstrap_locator_symlink_hardlink_and_unsafe_permissions_are_not_read() {
    for mutation in 0..3 {
        let fixture = Fixture::new();
        let record = fixture.record(1);
        match mutation {
            0 => {
                let moved = fixture._temporary.path().join("moved-record");
                fs::rename(&record, &moved).unwrap();
                symlink(moved, record).unwrap();
            }
            1 => fs::hard_link(&record, fixture._temporary.path().join("record-hardlink")).unwrap(),
            2 => fs::set_permissions(record, fs::Permissions::from_mode(0o644)).unwrap(),
            _ => unreachable!(),
        }
        assert!(fixture.store.reload(&fixture.config.pool_id).is_err());
    }
}

#[test]
fn bootstrap_locator_second_store_cannot_write_through_same_control_directory() {
    let fixture = Fixture::new();
    assert!(
        BootstrapStore::hold(
            File::open(&fixture.control).unwrap(),
            fixture.control.clone()
        )
        .is_err()
    );
}

#[test]
fn bootstrap_locator_held_control_directory_does_not_reopen_replacement_path() {
    let mut fixture = Fixture::new();
    let moved = fixture._temporary.path().join("held-control");
    fs::rename(&fixture.control, &moved).unwrap();
    private_dir(&fixture.control);
    private_file(&fixture.control.join("unrelated"), b"do not change");
    let stopped = fixture.stop();
    assert_eq!(stopped.generation(), 2);
    assert!(moved.join("record-00000000000000000002.json").is_file());
    assert!(!fixture.control.join("head.json").exists());
    assert_eq!(
        fs::read(fixture.control.join("unrelated")).unwrap(),
        b"do not change"
    );
    assert_eq!(
        fixture
            .store
            .reload(&fixture.config.pool_id)
            .unwrap()
            .generation(),
        2
    );
}

#[test]
fn bootstrap_locator_no_implicit_system_directory_recreation() {
    let fixture = Fixture::new();
    let missing = fixture._temporary.path().join("lost-system-state");
    assert!(PrivateDirectory::open_existing(&missing).is_err());
    assert!(!missing.exists());
    // No store constructor here consumes Config or can discover storage roots.
}

#[test]
fn bootstrap_locator_stop_hints_expose_context_without_asserting_intent_existence() {
    let mut fixture = Fixture::new();
    let stopped = fixture.stop();
    let hints: Vec<_> = stopped.stop_hints().collect();
    assert_eq!(hints.len(), 1);
    let hint = hints[0];
    let target = fixture.binding(0);
    assert_eq!(hint.pool_id, target.pool_id);
    assert_eq!(hint.disk_id, target.disk_id);
    assert_eq!(hint.slot_id, target.slot_id);
    assert_eq!(hint.binding_generation, target.binding_generation);
    assert_eq!(hint.operation_id, operation(1));
    assert_eq!(hint.intent_digest, digest());
    stopped
        .select(&fixture.config)
        .unwrap()
        .try_for_each(|probe| {
            let binding = probe.binding();
            assert_eq!(binding.pool_id, fixture.config.pool_id);
            assert_ne!(binding.disk_id, target.disk_id);
            Ok::<_, ()>(())
        })
        .unwrap();
}

#[test]
fn bootstrap_locator_sync_current_requires_exact_head_without_reopening_target() {
    let mut fixture = Fixture::new();
    let stopped = fixture.stop();
    assert!(fixture.store.sync_current(&fixture.initial).is_err());
    fixture.store.sync_current(&stopped).unwrap();
    assert_eq!(
        fixture.store.reload(&fixture.config.pool_id).unwrap().head,
        stopped.head
    );
    let current_config = {
        let mut config = fixture.config.clone();
        config.disks.remove(0);
        config
    };
    assert_eq!(stopped.select(&current_config).unwrap().len(), 2);
}

#[test]
fn bootstrap_locator_control_identity_alias_is_rejected_even_at_another_address() {
    let fixture = Fixture::new();
    let other_control = fixture._temporary.path().join("another-control");
    private_dir(&other_control);
    let mut store =
        BootstrapStore::hold(File::open(&other_control).unwrap(), other_control.clone()).unwrap();
    let mut observations = observe(&fixture.config);
    // A caller may observe the same held object under a distinct diagnostic
    // address. The physical equality is known without probing that address.
    observations[0] = RootObservation::capture(
        &fixture.binding(0),
        &fixture.config.disks[0].root,
        &File::open(&other_control).unwrap(),
        true,
    )
    .unwrap();
    assert!(
        store
            .initialize(&fixture.config.pool_id, observations)
            .is_err()
    );
    assert!(!other_control.join("head.json").exists());
    assert!(
        !other_control
            .join("record-00000000000000000001.json")
            .exists()
    );
}
