use super::*;
use data_serialization::Serializer;
use std::{cell::Cell, sync::Barrier};

fn disk() -> (tempfile::TempDir, Arc<DiskHandle>) {
    let directory = tempfile::tempdir().unwrap();
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let config =
        ValidatedConfig::try_from(Config::development([directory.path().to_path_buf()]).unwrap())
            .unwrap();
    let handle = Arc::new(DiskHandle::open(&config, &config.disks[0], false).unwrap());
    (directory, handle)
}

fn unavailable<T>(result: Result<T>) {
    match result {
        Err(error) => assert_eq!(error.code(), ErrorCode::DiskUnavailable),
        Ok(_) => panic!("quiescing root admitted ordinary I/O"),
    }
}

#[test]
fn every_bound_root_entry_rejects_before_opening_or_mutating() {
    let (directory, handle) = disk();
    let root = &handle.root;
    let child = directory.path().join("child");
    root.private_dir(&child).unwrap();
    let derived = root.private_child(&child).unwrap();
    let retained = root
        .operation()
        .unwrap()
        .retained_root(directory.path().to_path_buf())
        .unwrap();
    let file = child.join("value");
    root.write_json(&file, &17u64, false).unwrap();
    let binding = root.io_binding().unwrap();
    drop(binding.begin_quiesce().unwrap().unwrap());

    unavailable(root.directory(directory.path()));
    unavailable(root.entries(&child));
    unavailable(root.open_regular(&file));
    unavailable(root.open_optional_read_regular(&file));
    unavailable(root.open_leased_regular(&file));
    unavailable(root.open_read_regular(&file));
    unavailable(root.private_directory(&child));
    unavailable(root.lock_file());
    unavailable(root.optional_metadata(directory.path()));
    unavailable(root.optional_metadata(&file));
    unavailable(root.read_bytes(&file, 64));
    unavailable(root.read_bytes_observed(&file, 64, &mut |_| Ok(())));
    unavailable(root.read_json_observed::<u64>(&file, &mut |_| Ok(())));
    unavailable(root.private_dir(directory.path()));
    unavailable(root.private_dir(&child.join("new")));
    unavailable(root.write_atomic(&file, b"changed", true));
    unavailable(root.write_json(&file, &18u64, true));
    unavailable(root.sync_dir(&child));
    unavailable(root.remove_file_synced(&file));
    unavailable(root.available());
    unavailable(root.private_child(&child));
    unavailable(derived.read_bytes(&file, 64));
    unavailable(retained.read_bytes(&file, 64));

    assert_eq!(fs::read(&file).unwrap(), b"17");
    assert!(!child.join("new").exists());
    assert!(!directory.path().join(".writer.lock").exists());
    assert!(binding.status().unwrap().drained());
}

fn open_descriptors_for(path: &Path) -> usize {
    fs::read_dir("/proc/self/fd")
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|entry| match fs::read_link(entry) {
            Ok(target) => target == path,
            // The directory enumeration descriptor can close before read_link.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => panic!("cannot inspect test descriptor: {error}"),
        })
        .count()
}

#[test]
fn synchronous_read_success_retires_its_descriptor_and_count_before_return() {
    let (directory, handle) = disk();
    let path = directory.path().join("value");
    handle.root.write_json(&path, &17u64, false).unwrap();
    let binding = handle.root.io_binding().unwrap();
    let mut observations = 0;
    let value = handle
        .root
        .read_json_observed::<u64>(&path, &mut |_| {
            observations += 1;
            assert_eq!(binding.status().unwrap().ordinary, 1);
            assert_eq!(open_descriptors_for(&path), 1);
            Ok(())
        })
        .unwrap();
    assert_eq!(value, 17);
    assert!(observations > 0);
    assert_eq!(binding.status().unwrap().ordinary, 0);
    assert!(!binding.status().unwrap().quiescing);
    assert_eq!(open_descriptors_for(&path), 0);
    assert_eq!(handle.root.read_bytes(&path, 64).unwrap(), b"17");
}

#[test]
fn synchronous_read_keeps_its_descriptor_and_count_until_cancellation_or_error_tail() {
    for reject in [false, true] {
        let (directory, handle) = disk();
        let path = directory.path().join("value");
        handle.root.write_json(&path, &17u64, false).unwrap();
        let binding = handle.root.io_binding().unwrap();
        let entered = Barrier::new(2);
        let release = Barrier::new(2);
        std::thread::scope(|threads| {
            let reader = threads.spawn(|| {
                let mut first = true;
                handle.root.read_json_observed::<u64>(&path, &mut |_| {
                    assert_eq!(binding.status().unwrap().ordinary, 1);
                    if first {
                        first = false;
                        entered.wait();
                        release.wait();
                        if reject {
                            return Err(Error::new(ErrorCode::ResourceLimit, "observer stopped"));
                        }
                    }
                    Ok(())
                })
            });
            entered.wait();
            assert_eq!(open_descriptors_for(&path), 1);
            let mut transition = binding.begin_quiesce().unwrap().unwrap();
            assert_eq!(binding.status().unwrap().ordinary, 1);
            assert!(!binding.status().unwrap().drained());
            unavailable(handle.root.with_transition_io::<()>(&mut transition, |_| {
                panic!("control callback ran before ordinary read cleanup")
            }));
            unavailable(handle.root.read_bytes(&path, 64));
            release.wait();
            let result = reader.join().unwrap();
            if reject {
                let error = result.unwrap_err();
                assert_eq!(error.code(), ErrorCode::ResourceLimit);
                assert_eq!(error.message(), "observer stopped");
            } else {
                unavailable(result);
            }
            assert_eq!(open_descriptors_for(&path), 0);
            assert_eq!(binding.status().unwrap().ordinary, 0);
            assert_eq!(
                handle
                    .root
                    .with_transition_io(&mut transition, |control| control.read_metadata(&path))
                    .unwrap(),
                b"17"
            );
        });
        assert!(binding.status().unwrap().drained());
    }
}

struct QuiescingValue<'a> {
    binding: &'a DiskIoBinding,
    quiesced: Cell<bool>,
}

impl Serialize for QuiescingValue<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        assert_eq!(self.binding.status().unwrap().ordinary, 1);
        // Canonical encoding validates and serializes separately. Only the first
        // pass initiates quiescing; both passes must retain the same operation.
        if !self.quiesced.replace(true) {
            drop(self.binding.begin_quiesce().unwrap().unwrap());
        }
        assert!(!self.binding.status().unwrap().drained());
        serializer.serialize_u64(17)
    }
}

#[test]
fn nested_atomic_write_does_not_readmit_after_quiescing_during_encoding() {
    for collision in [false, true] {
        let (directory, handle) = disk();
        let binding = handle.root.io_binding().unwrap();
        let path = directory.path().join("new/nested/value");
        if collision {
            handle.root.write_atomic(&path, b"existing", false).unwrap();
        }
        let value = QuiescingValue {
            binding,
            quiesced: Cell::new(false),
        };
        let result = handle.root.write_json(&path, &value, false);
        if collision {
            assert_eq!(result.unwrap_err().code(), ErrorCode::ObjectCollision);
            assert_eq!(fs::read(&path).unwrap(), b"existing");
        } else {
            result.unwrap();
            assert_eq!(fs::read(&path).unwrap(), b"17");
        }
        assert_eq!(open_descriptors_for(&path), 0);
        assert!(binding.status().unwrap().drained());
        unavailable(handle.root.write_json(&path, &18u64, true));
    }
}

#[test]
fn atomic_error_cleanup_precedes_drain_and_control_io() {
    let (directory, handle) = disk();
    let binding = handle.root.io_binding().unwrap();
    let path = directory.path().join("collision");
    handle.root.write_atomic(&path, b"existing", false).unwrap();
    let operation = handle.root.operation().unwrap();
    let held = operation.private_directory(directory.path()).unwrap();
    let name = EntryName::new("collision").unwrap();
    // The actual platform atomic owner stages and synchronizes a file, then
    // fails no-clobber publication. Its real unlink cleanup runs under the same
    // borrowed operation scope used by RootOperation::write_atomic.
    let prepared = AtomicFile::stage(&held, &name)
        .unwrap()
        .prepare(b"replacement")
        .unwrap();
    let mut transition = binding.begin_quiesce().unwrap().unwrap();
    unavailable(handle.root.with_transition_io::<()>(&mut transition, |_| {
        panic!("control callback ran while atomic cleanup was outstanding")
    }));
    assert_eq!(binding.status().unwrap().ordinary, 1);
    assert!(prepared.create().is_err());
    let names = fs::read_dir(directory.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<Vec<_>>();
    assert_eq!(names, [std::ffi::OsString::from("collision")]);
    assert_eq!(fs::read(&path).unwrap(), b"existing");
    drop(held);
    assert_eq!(binding.status().unwrap().ordinary, 1);
    unavailable(handle.root.with_transition_io::<()>(&mut transition, |_| {
        panic!("control callback ran before the ordinary owner retired")
    }));
    drop(operation);
    handle
        .root
        .with_transition_io(&mut transition, |control| {
            assert_eq!(binding.status()?.ordinary, 0);
            assert_eq!(binding.status()?.control, 1);
            control.write_json(&path, &19u64, true)?;
            assert_eq!(control.read_metadata(&path)?, b"19");
            Ok(())
        })
        .unwrap();
    assert!(binding.status().unwrap().drained());
    assert_eq!(
        handle
            .root
            .with_transition_io(&mut transition, |_| Ok(()))
            .unwrap_err()
            .code(),
        ErrorCode::ConfigConflict
    );
}

#[test]
fn transition_scope_rejects_another_binding_without_consuming_its_control() {
    let (directory, handle) = disk();
    let (_other_directory, other) = disk();
    let binding = handle.root.io_binding().unwrap();
    let mut transition = binding.begin_quiesce().unwrap().unwrap();
    let error = other
        .root
        .with_transition_io(&mut transition, |_| Ok(()))
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::DiskIdentityMismatch);
    assert_eq!(binding.status().unwrap().control, 1);
    let missing = directory.path().join("missing");
    assert!(
        handle
            .root
            .with_transition_io(&mut transition, |control| control.read_metadata(&missing))
            .is_err()
    );
    assert!(binding.status().unwrap().drained());
    assert_eq!(
        handle
            .root
            .with_transition_io(&mut transition, |_| Ok(()))
            .unwrap_err()
            .code(),
        ErrorCode::ConfigConflict
    );
}

#[test]
fn retained_root_identity_failure_releases_its_original_operation() {
    let (directory, handle) = disk();
    let (_replacement_directory, replacement) = disk();
    let binding = handle.root.io_binding().unwrap();
    let result = handle
        .root
        .operation()
        .unwrap()
        .retained_root(replacement.root.path().to_path_buf());
    assert!(matches!(
        result,
        Err(error) if error.code() == ErrorCode::DiskIdentityMismatch
    ));
    assert_eq!(binding.status().unwrap().ordinary, 0);
    handle
        .root
        .write_atomic(&directory.path().join("still-active"), b"value", false)
        .unwrap();
}

#[test]
fn binding_adoption_keeps_bootstrap_alias_and_retained_generation_on_one_root_fd() {
    let directory = tempfile::tempdir().unwrap();
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let config =
        ValidatedConfig::try_from(Config::development([directory.path().to_owned()]).unwrap())
            .unwrap();
    let bootstrap = DiskRoot::open(directory.path()).unwrap();
    let adopted = bootstrap.retained(directory.path().to_owned()).unwrap();
    let seal = identity_at(
        &config,
        &config.disks[0],
        bootstrap.operation().unwrap().held().directory(),
    )
    .unwrap();
    let handle = DiskHandle::new(adopted, seal, &config).unwrap();
    let retained = handle.root.retained(directory.path().to_owned()).unwrap();
    assert!(Arc::ptr_eq(&bootstrap.directory, &handle.root.directory));
    assert!(Arc::ptr_eq(&retained.directory, &handle.root.directory));
    assert_eq!(open_descriptors_for(directory.path()), 1);
    let path = directory.path().join("value");
    bootstrap.write_atomic(&path, b"retained", false).unwrap();
    let binding = handle.root.io_binding().unwrap();
    assert!(bootstrap.io.is_none());
    drop(binding.begin_quiesce().unwrap().unwrap());
    assert!(binding.status().unwrap().drained());
    assert_eq!(open_descriptors_for(directory.path()), 0);
    unavailable(bootstrap.open_read_regular(&path));
    unavailable(bootstrap.retained(directory.path().to_owned()));
    unavailable(retained.directory(directory.path()));
    assert_eq!(fs::read(path).unwrap(), b"retained");
}

#[test]
fn derived_root_registration_racing_quiesce_never_leaves_a_live_descriptor() {
    for _ in 0..32 {
        let (directory, handle) = disk();
        let child = directory.path().join("child");
        handle.root.private_dir(&child).unwrap();
        let barrier = Barrier::new(2);
        let binding = handle.root.io_binding().unwrap();
        let derived = std::thread::scope(|threads| {
            let deriving = threads.spawn(|| {
                barrier.wait();
                handle.root.private_child(&child)
            });
            barrier.wait();
            drop(binding.begin_quiesce().unwrap().unwrap());
            deriving.join().unwrap()
        });
        assert!(binding.status().unwrap().drained());
        assert_eq!(open_descriptors_for(&child), 0);
        assert_eq!(open_descriptors_for(directory.path()), 0);
        match derived {
            Ok(root) => unavailable(root.entries(&child)),
            Err(error) => assert_eq!(error.code(), ErrorCode::DiskUnavailable),
        }
    }
}

#[test]
fn admitted_resources_require_exact_closure_domain_to_remove_a_handle() {
    let (_directory, target) = disk();
    let (_other_directory, other) = disk();
    let resources = DiskResources {
        disks: [(target.seal.disk_id.clone(), Arc::clone(&target))].into(),
    };
    let binding = target.root.io_binding().unwrap();
    drop(binding.begin_quiesce().unwrap().unwrap());
    let proof = binding.closed_proof().unwrap();
    assert_eq!(
        resources
            .excluding_closed(&proof)
            .unwrap()
            .handles()
            .count(),
        0
    );
    let other_resources = DiskResources {
        disks: [(other.seal.disk_id.clone(), Arc::clone(&other))].into(),
    };
    assert_eq!(
        other_resources
            .excluding_closed(&proof)
            .err()
            .unwrap()
            .code(),
        ErrorCode::DiskIdentityMismatch
    );
}
