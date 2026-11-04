use super::*;
use crate::{Config, disk::DiskHandle};
use std::{
    os::unix::fs::PermissionsExt,
    sync::{
        Barrier,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

fn disk() -> (tempfile::TempDir, ValidatedConfig, Arc<DiskHandle>) {
    let directory = tempfile::tempdir().unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let config =
        ValidatedConfig::try_from(Config::development([directory.path().to_path_buf()]).unwrap())
            .unwrap();
    let handle = Arc::new(DiskHandle::open(&config, &config.disks[0], false).unwrap());
    (directory, config, handle)
}

fn genesis_with_data_disks() -> (tempfile::TempDir, ValidatedConfig, Arc<DiskHandle>) {
    let directory = tempfile::tempdir().unwrap();
    let mut config =
        Config::development(["primary", "data", "backup"].map(|name| directory.path().join(name)))
            .unwrap();
    config.disks[1].metadata_member = false;
    config.disks[1].level = 1;
    config.disks[1].upstream_slot_id = None;
    config.disks[2].metadata_member = false;
    config.disks[2].level = 2;
    for disk in &config.disks {
        std::fs::create_dir(&disk.root).unwrap();
        std::fs::set_permissions(&disk.root, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let config = ValidatedConfig::try_from(config).unwrap();
    let handle = Arc::new(DiskHandle::open(&config, &config.disks[0], false).unwrap());
    (directory, config, handle)
}

#[test]
fn closing_admission_and_registering_an_operation_are_one_linearization() {
    for _ in 0..64 {
        let (_directory, _config, handle) = disk();
        let candidate = handle.root.io_binding().unwrap();
        let barrier = Barrier::new(3);
        let (admitted, transition) = std::thread::scope(|threads| {
            let reader = threads.spawn(|| {
                barrier.wait();
                candidate.admit()
            });
            let closer = threads.spawn(|| {
                barrier.wait();
                candidate.begin_quiesce().unwrap().unwrap()
            });
            barrier.wait();
            (reader.join().unwrap(), closer.join().unwrap())
        });
        let expected = match &admitted {
            Ok(_) => 1,
            Err(error) => {
                assert_eq!(error.code(), ErrorCode::DiskUnavailable);
                0
            }
        };
        assert_eq!(candidate.status().unwrap().ordinary, expected);
        assert_eq!(candidate.status().unwrap().control, 1);
        assert_eq!(
            candidate.admit().unwrap_err().code(),
            ErrorCode::DiskUnavailable
        );
        drop(transition);
        assert_eq!(candidate.status().unwrap().drained(), expected == 0);
        drop(admitted);
        assert!(candidate.status().unwrap().drained());
        assert_eq!(
            candidate.admit().unwrap_err().code(),
            ErrorCode::DiskUnavailable
        );
    }
}

#[test]
fn an_operation_admitted_before_quiescing_keeps_its_real_tail_counted() {
    let (_directory, _config, handle) = disk();
    let binding = handle.root.io_binding().unwrap();
    let entered = Barrier::new(2);
    let release = Barrier::new(2);
    std::thread::scope(|threads| {
        threads.spawn(|| {
            let lease = binding.admit().unwrap();
            entered.wait();
            release.wait();
            assert_eq!(
                lease.check_running().unwrap_err().code(),
                ErrorCode::DiskUnavailable
            );
            drop(lease);
        });
        entered.wait();
        let transition = binding.begin_quiesce().unwrap().unwrap();
        drop(transition);
        assert_eq!(binding.status().unwrap().ordinary, 1);
        assert!(!binding.status().unwrap().drained());
        release.wait();
    });
    assert!(binding.status().unwrap().drained());
}

#[test]
fn old_handles_and_private_children_share_the_retained_binding_gate() {
    let (directory, config, old) = disk();
    let path = directory.path().join("private");
    std::fs::create_dir(&path).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    let child = old.root.private_child(&path).unwrap();
    let old_lease = child.admit_io().unwrap();
    let successor = config
        .clone()
        .with_disk_roots(vec![directory.path().to_path_buf()])
        .unwrap();
    let new = DiskHandle::open(&successor, &successor.disks[0], false).unwrap();
    let old_binding = old.root.io_binding().unwrap();
    let new_binding = new.root.io_binding().unwrap();
    assert!(Arc::ptr_eq(&old_binding.gate, &new_binding.gate));
    assert!(Arc::ptr_eq(
        &old_binding.gate,
        &child.io_binding().unwrap().gate
    ));
    assert!(old_lease.owner.binding.authority.same_admission(&config));
    old_lease.check_running().unwrap();
    drop(old.root.admit_io().unwrap());
    let transition = new_binding.begin_quiesce().unwrap().unwrap();
    assert_eq!(
        old.root.admit_io().unwrap_err().code(),
        ErrorCode::DiskUnavailable
    );
    assert_eq!(
        child.admit_io().unwrap_err().code(),
        ErrorCode::DiskUnavailable
    );
    assert!(matches!(
        old.root.private_child(&path),
        Err(error) if error.code() == ErrorCode::DiskUnavailable
    ));
    assert_eq!(
        old_lease.owner.binding.gate.identity.disk_id,
        config.disks[0].disk_id
    );
    assert_eq!(
        old_lease.owner.binding.gate.identity.binding_generation,
        config.disks[0].binding_generation
    );
    drop(transition);
    assert!(!new_binding.status().unwrap().drained());
    drop(old_lease);
    assert!(new_binding.status().unwrap().drained());
}

#[test]
fn opening_an_unverified_configuration_cannot_replace_existing_control_authority() {
    let (_directory, config, old) = disk();
    let extra_directory = tempfile::tempdir().unwrap();
    let mut unverified = (*config).clone();
    let mut extra = Config::development([extra_directory.path().to_path_buf()])
        .unwrap()
        .disks
        .remove(0);
    extra.slot_id = "B01".into();
    extra.level = 2;
    extra.upstream_slot_id = Some("P01".into());
    unverified.disks.push(extra);
    let unverified = ValidatedConfig::try_from(unverified).unwrap();
    let reopened = DiskHandle::open(&unverified, &unverified.disks[0], false).unwrap();
    let original = old.root.io_binding().unwrap();
    let attempted = reopened.root.io_binding().unwrap();
    assert!(Arc::ptr_eq(&original.gate, &attempted.gate));
    assert!(matches!(
        attempted.begin_quiesce(),
        Err(error) if error.code() == ErrorCode::ConfigConflict
    ));
    assert!(matches!(
        DiskIoBinding::activate_generation(std::slice::from_ref(original), &unverified),
        Err(error) if error.code() == ErrorCode::ConfigConflict
    ));
    assert!(!original.status().unwrap().quiescing);
    let transition = original.begin_quiesce().unwrap().unwrap();
    drop(transition);
    assert!(attempted.status().unwrap().drained());
}

#[test]
fn unchanged_voters_do_not_authorize_changed_data_only_control_contexts() {
    let (_directory, original, old) = genesis_with_data_disks();
    let changes: &[(&str, fn(&mut Config))] = &[
        ("data-only addition", |config| {
            let mut disk = config.disks[1].clone();
            disk.disk_id = crate::new_id("disk").unwrap();
            disk.slot_id = "D01".into();
            disk.root = config.disks[0].root.parent().unwrap().join("added-data");
            config.disks.push(disk);
        }),
        ("data-only removal", |config| {
            config.disks.pop();
        }),
        ("data-only disk ID", |config| {
            config.disks[2].disk_id = crate::new_id("disk").unwrap()
        }),
        ("data-only slot", |config| {
            config.disks[2].slot_id = "D01".into()
        }),
        ("data-only binding", |config| {
            config.disks[2].binding_generation += 1
        }),
        ("data-only primary role", |config| {
            config.disks[2].level = 1;
            config.disks[2].upstream_slot_id = None;
        }),
        ("data-only upstream", |config| {
            config.disks[2].upstream_slot_id = Some("P01".into())
        }),
        ("data-only capacity", |config| {
            config.disks[2].capacity_bytes = Some(4096)
        }),
        ("persisted process limit", |config| config.max_entries += 1),
    ];
    let binding = old.root.io_binding().unwrap();
    for (description, change) in changes {
        let mut changed = (*original).clone();
        change(&mut changed);
        let changed = ValidatedConfig::try_from(changed).unwrap();
        assert!(original.same_metadata_membership(&changed), "{description}");
        let reopened = DiskHandle::open(&changed, &changed.disks[0], false).unwrap();
        let attempted = reopened.root.io_binding().unwrap();
        assert!(Arc::ptr_eq(&binding.gate, &attempted.gate));
        assert!(
            matches!(
                attempted.begin_quiesce(),
                Err(error) if error.code() == ErrorCode::ConfigConflict
            ),
            "changed context gained control: {description}"
        );
        assert!(!binding.status().unwrap().quiescing);
        drop(binding.admit().unwrap());
    }
    let transition = binding.begin_quiesce().unwrap().unwrap();
    drop(transition);
    assert!(binding.status().unwrap().drained());
}

#[test]
fn complete_genesis_context_survives_reopen_order_and_verified_root_relocation() {
    let (directory, original, old) = genesis_with_data_disks();
    let binding = old.root.io_binding().unwrap();
    let reopened = ValidatedConfig::try_from((*original).clone()).unwrap();
    let same = DiskHandle::open(&reopened, &reopened.disks[0], false).unwrap();
    assert!(Arc::ptr_eq(
        &binding.gate,
        &same.root.io_binding().unwrap().gate
    ));
    assert!(same_authority(&binding.authority, &reopened));
    let mut reordered = (*original).clone();
    reordered.disks.reverse();
    let reordered = ValidatedConfig::try_from(reordered).unwrap();
    let disk = reordered
        .disks
        .iter()
        .find(|disk| disk.disk_id == old.seal.disk_id)
        .unwrap();
    let same = DiskHandle::open(&reordered, disk, false).unwrap();
    assert!(Arc::ptr_eq(
        &binding.gate,
        &same.root.io_binding().unwrap().gate
    ));
    assert!(same_authority(&binding.authority, &reordered));

    let lease = old.root.admit_io().unwrap();
    let relocated = directory.path().join("relocated-primary");
    std::fs::rename(&original.disks[0].root, &relocated).unwrap();
    let roots = original
        .disks
        .iter()
        .enumerate()
        .map(|(index, disk)| {
            if index == 0 {
                relocated.clone()
            } else {
                disk.root.clone()
            }
        })
        .collect();
    let rebound = original.clone().with_disk_roots(roots).unwrap();
    let rebound = rebound
        .with_workspace_limits(Some(crate::RetainedWorkspaceLimits {
            max_retained_workspaces: original.max_retained_workspaces + 1,
            max_retained_workspaces_per_principal: original.max_retained_workspaces_per_principal
                + 1,
            max_retained_workspace_metadata_bytes: original.max_retained_workspace_metadata_bytes
                + 1,
            max_retained_workspace_metadata_bytes_per_principal: original
                .max_retained_workspace_metadata_bytes_per_principal
                + 1,
        }))
        .unwrap();
    let same = DiskHandle::open(&rebound, &rebound.disks[0], false).unwrap();
    assert!(Arc::ptr_eq(
        &binding.gate,
        &same.root.io_binding().unwrap().gate
    ));
    lease.check_running().unwrap();
    let transition = same
        .root
        .io_binding()
        .unwrap()
        .begin_quiesce()
        .unwrap()
        .unwrap();
    drop(transition);
    assert_eq!(
        lease.check_running().unwrap_err().code(),
        ErrorCode::DiskUnavailable
    );
    assert!(!binding.status().unwrap().drained());
    drop(lease);
    assert!(binding.status().unwrap().drained());
}

#[test]
fn duplicate_files_are_all_closed_even_when_the_wrappers_stay_idle() {
    let (directory, _config, handle) = disk();
    let path = directory.path().join("content");
    std::fs::write(&path, b"held content").unwrap();
    let file = handle.root.open_leased_regular(&path).unwrap();
    let copy = file.try_clone().unwrap();
    let binding = handle.root.io_binding().unwrap();
    assert_eq!(binding.status().unwrap().ordinary, 1);
    drop(file);
    assert_eq!(binding.status().unwrap().ordinary, 1);
    let transition = binding.begin_quiesce().unwrap().unwrap();
    drop(transition);
    assert!(binding.status().unwrap().drained());
    assert!(copy.resource.value.lock().unwrap().is_none());
    assert_eq!(
        copy.try_clone().unwrap_err().kind(),
        io::ErrorKind::PermissionDenied
    );
    drop(copy);
    assert!(binding.status().unwrap().drained());
}

#[test]
fn standard_file_operations_refuse_quiescing_without_retryable_interrupted() {
    let (directory, _config, handle) = disk();
    let path = directory.path().join("content");
    std::fs::write(&path, b"held content").unwrap();
    let mut file = handle.root.open_leased_regular(&path).unwrap();
    let binding = handle.root.io_binding().unwrap();
    let transition = binding.begin_quiesce().unwrap().unwrap();
    drop(transition);
    let error = file.read_to_end(&mut Vec::new()).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    assert!(error.to_string().contains(&handle.seal.disk_id));
    assert_eq!(
        file.metadata().unwrap_err().kind(),
        io::ErrorKind::PermissionDenied
    );
    assert_eq!(
        file.seek(SeekFrom::Start(0)).unwrap_err().kind(),
        io::ErrorKind::PermissionDenied
    );
    assert_eq!(
        file.read_at(&mut [0; 1], 0).unwrap_err().kind(),
        io::ErrorKind::PermissionDenied
    );
    assert_eq!(
        file.write_at(b"x", 0).unwrap_err().kind(),
        io::ErrorKind::PermissionDenied
    );
    assert_eq!(
        file.write_all(b"x").unwrap_err().kind(),
        io::ErrorKind::PermissionDenied
    );
    assert_eq!(
        file.flush().unwrap_err().kind(),
        io::ErrorKind::PermissionDenied
    );
    assert_eq!(
        file.sync_all().unwrap_err().kind(),
        io::ErrorKind::PermissionDenied
    );
    assert_eq!(
        file.sync_data().unwrap_err().kind(),
        io::ErrorKind::PermissionDenied
    );
    assert_eq!(
        file.set_len(0).unwrap_err().kind(),
        io::ErrorKind::PermissionDenied
    );
    assert_eq!(binding.status().unwrap().ordinary, 0);
    drop(file);
    assert!(binding.status().unwrap().drained());
    assert_eq!(std::fs::read(&path).unwrap(), b"held content");
}

#[test]
fn operation_cleanup_runs_before_the_last_lease_reaches_zero() {
    struct Cleanup {
        path: std::path::PathBuf,
        binding: DiskIoBinding,
        observed: Arc<AtomicBool>,
    }
    impl Drop for Cleanup {
        fn drop(&mut self) {
            // This operation's scratch cleanup is already owned work, not a
            // new admission. A disabled coordinator must still wait for it.
            let still_owned = self.binding.status().is_ok_and(|state| state.ordinary == 1);
            let removed = std::fs::remove_file(&self.path).is_ok();
            self.observed
                .store(still_owned && removed, Ordering::SeqCst);
        }
    }
    let (directory, _config, handle) = disk();
    let path = directory.path().join("scratch");
    std::fs::write(&path, b"scratch").unwrap();
    let binding = handle.root.io_binding().unwrap();
    let observed = Arc::new(AtomicBool::new(false));
    let resource = binding.admit().unwrap().hold(Cleanup {
        path: path.clone(),
        binding: binding.clone(),
        observed: Arc::clone(&observed),
    });
    let transition = binding.begin_quiesce().unwrap().unwrap();
    drop(transition);
    assert!(observed.load(Ordering::SeqCst));
    assert!(binding.status().unwrap().drained());
    drop(resource);
    assert!(observed.load(Ordering::SeqCst));
    assert!(binding.status().unwrap().drained());
    assert!(!path.exists());
}

#[test]
fn in_flight_io_keeps_its_descriptor_and_lease_until_the_actual_return() {
    struct BlockRead {
        entered: Arc<Barrier>,
        finish: Arc<Barrier>,
        closed: Arc<AtomicBool>,
    }
    impl Read for BlockRead {
        fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
            self.entered.wait();
            self.finish.wait();
            output[0] = b'x';
            Ok(1)
        }
    }
    impl Drop for BlockRead {
        fn drop(&mut self) {
            self.closed.store(true, Ordering::SeqCst);
        }
    }
    let (_directory, _config, handle) = disk();
    let binding = handle.root.io_binding().unwrap();
    let entered = Arc::new(Barrier::new(2));
    let finish = Arc::new(Barrier::new(2));
    let closed = Arc::new(AtomicBool::new(false));
    let mut reader = binding.admit().unwrap().hold(BlockRead {
        entered: Arc::clone(&entered),
        finish: Arc::clone(&finish),
        closed: Arc::clone(&closed),
    });
    let worker = std::thread::spawn(move || {
        let mut byte = [0; 1];
        assert_eq!(reader.read(&mut byte).unwrap(), 1);
        assert_eq!(byte[0], b'x');
        reader
    });
    entered.wait();
    let transition = binding.begin_quiesce().unwrap().unwrap();
    drop(transition);
    assert!(!closed.load(Ordering::SeqCst));
    assert_eq!(binding.status().unwrap().ordinary, 1);
    assert!(!binding.status().unwrap().drained());
    finish.wait();
    let mut idle_wrapper = worker.join().unwrap();
    assert!(closed.load(Ordering::SeqCst));
    assert!(binding.status().unwrap().drained());
    assert_eq!(
        idle_wrapper.read(&mut [0; 1]).unwrap_err().kind(),
        io::ErrorKind::PermissionDenied
    );
}

#[test]
fn a_file_opened_before_quiescing_but_registered_after_it_is_closed() {
    let (directory, _config, handle) = disk();
    let path = directory.path().join("content");
    std::fs::write(&path, b"late registration").unwrap();
    let binding = handle.root.io_binding().unwrap();
    let lease = binding.admit().unwrap();
    let file = File::open(path).unwrap();
    let transition = binding.begin_quiesce().unwrap().unwrap();
    drop(transition);
    assert_eq!(binding.status().unwrap().ordinary, 1);
    let resource = lease.hold(file);
    assert!(resource.resource.value.lock().unwrap().is_none());
    assert!(binding.status().unwrap().drained());
    assert_eq!(
        resource.metadata().unwrap_err().kind(),
        io::ErrorKind::PermissionDenied
    );
}

#[test]
fn concurrent_clone_cannot_escape_the_quiescing_resource_snapshot() {
    for _ in 0..64 {
        let (directory, _config, handle) = disk();
        let path = directory.path().join("content");
        std::fs::write(&path, b"clone registration").unwrap();
        let binding = handle.root.io_binding().unwrap();
        let file = binding.admit().unwrap().hold(File::open(&path).unwrap());
        let barrier = Barrier::new(3);
        let (copy, transition) = std::thread::scope(|threads| {
            let copier = threads.spawn(|| {
                barrier.wait();
                file.try_clone()
            });
            let closer = threads.spawn(|| {
                barrier.wait();
                binding.begin_quiesce().unwrap().unwrap()
            });
            barrier.wait();
            (copier.join().unwrap(), closer.join().unwrap())
        });
        drop(transition);
        assert!(binding.status().unwrap().drained());
        assert!(file.resource.value.lock().unwrap().is_none());
        match copy {
            Ok(copy) => assert!(copy.resource.value.lock().unwrap().is_none()),
            Err(error) => assert_eq!(error.kind(), io::ErrorKind::PermissionDenied),
        }
    }
}

#[test]
fn duplicate_quiescing_does_not_mint_control_or_confuse_disk_identity() {
    let (_first_dir, _first_config, first) = disk();
    let (_second_dir, _second_config, second) = disk();
    let binding = first.root.io_binding().unwrap();
    let other = second.root.io_binding().unwrap();
    let mut transition = binding.begin_quiesce().unwrap().unwrap();
    assert!(binding.begin_quiesce().unwrap().is_none());
    assert_eq!(binding.status().unwrap().control, 1);
    assert_eq!(
        transition.take_control(other).unwrap_err().code(),
        ErrorCode::DiskIdentityMismatch
    );
    let control = transition.take_control(binding).unwrap();
    control.check_running().unwrap();
    assert_eq!(
        transition.take_control(binding).unwrap_err().code(),
        ErrorCode::ConfigConflict
    );
    drop(transition);
    assert_eq!(binding.status().unwrap().control, 1);
    assert!(!binding.status().unwrap().drained());
    drop(control);
    assert!(binding.status().unwrap().drained());
    assert!(binding.begin_quiesce().unwrap().is_none());
    assert_eq!(
        binding.admit().unwrap_err().code(),
        ErrorCode::DiskUnavailable
    );
    drop(other.admit().unwrap());
    assert!(!other.status().unwrap().quiescing);
}

#[test]
fn changing_a_binding_cannot_reuse_its_existing_gate_or_control() {
    let (_directory, config, handle) = disk();
    let binding = handle.root.io_binding().unwrap();
    let mut changed = handle.seal.clone();
    changed.binding_generation += 1;
    assert_eq!(
        binding.for_authority(&changed, &config).unwrap_err().code(),
        ErrorCode::DiskIdentityMismatch,
    );
    let replacement = DiskIoBinding::new(&changed, &config).unwrap();
    let mut transition = binding.begin_quiesce().unwrap().unwrap();
    assert_eq!(
        transition.take_control(&replacement).unwrap_err().code(),
        ErrorCode::DiskIdentityMismatch
    );
    drop(replacement.admit().unwrap());
    drop(transition);
    assert!(binding.status().unwrap().drained());
}

#[test]
fn failed_open_releases_admission_and_poison_never_claims_drain() {
    let (directory, _config, handle) = disk();
    let binding = handle.root.io_binding().unwrap();
    assert!(
        handle
            .root
            .open_leased_regular(&directory.path().join("missing"))
            .is_err()
    );
    assert_eq!(binding.status().unwrap().ordinary, 0);
    let lease = binding.admit().unwrap();
    let gate = Arc::clone(&binding.gate);
    assert!(
        std::thread::spawn(move || {
            let _state = gate.state.lock().unwrap();
            panic!("deliberately poison the test admission registry");
        })
        .join()
        .is_err()
    );
    drop(lease);
    assert_eq!(
        binding.status().unwrap_err().code(),
        ErrorCode::RecoveryRequired
    );
    assert_eq!(
        binding.admit().unwrap_err().code(),
        ErrorCode::RecoveryRequired
    );
}

#[test]
fn a_quiescing_wrapper_never_calls_the_underlying_reader_again() {
    struct CountReads(Arc<AtomicUsize>);
    impl Read for CountReads {
        fn read(&mut self, _output: &mut [u8]) -> io::Result<usize> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(0)
        }
    }
    let (_directory, _config, handle) = disk();
    let binding = handle.root.io_binding().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let mut reader = binding
        .admit()
        .unwrap()
        .hold(CountReads(Arc::clone(&calls)));
    reader.read(&mut [0; 1]).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let transition = binding.begin_quiesce().unwrap().unwrap();
    drop(transition);
    assert_eq!(
        reader.read_exact(&mut [0; 1]).unwrap_err().kind(),
        io::ErrorKind::PermissionDenied
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    drop(reader);
    assert!(binding.status().unwrap().drained());
}
