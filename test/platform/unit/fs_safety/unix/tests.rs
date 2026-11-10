use super::*;
use crate::fs_safety::AtomicFile;
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
};

#[test]
fn administrative_atomic_files_keep_service_owner_and_refuse_unsafe_targets() {
    use std::os::unix::fs::{MetadataExt, chown};
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("state");
    fs::create_dir(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    let uid = if geteuid().as_raw() == 0 {
        65534
    } else {
        geteuid().as_raw()
    };
    let gid = if geteuid().as_raw() == 0 {
        65534
    } else {
        rustix::process::getegid().as_raw()
    };
    if geteuid().as_raw() == 0 {
        chown(&path, Some(uid), Some(gid)).unwrap();
        assert!(PrivateDirectory::open_existing(&path).is_err());
        assert!(matches!(
            PrivateDirectory::from_directory(File::open(&path).unwrap(), path.clone()),
            Err(Error::UnsafePermissions(_))
        ));
    }
    let directory = PrivateDirectory::open_for_administration(&path).unwrap();
    let pending = RelativePath::new("pending.json").unwrap();
    AtomicFile::replace(&directory, &pending, b"first").unwrap();
    AtomicFile::replace(&directory, &pending, b"second").unwrap();
    let metadata = fs::metadata(path.join("pending.json")).unwrap();
    assert_eq!(
        (
            metadata.uid(),
            metadata.gid(),
            metadata.mode() & 0o7777,
            metadata.nlink()
        ),
        (uid, gid, 0o600, 1)
    );
    assert_eq!(
        directory
            .read_bounded(&EntryName::new("pending.json").unwrap(), 6)
            .unwrap(),
        b"second"
    );
    verify_administrative_payloads(&directory, &path, (uid, gid));
    let link = path.join("link");
    symlink(path.join("pending.json"), &link).unwrap();
    assert!(AtomicFile::replace(&directory, &RelativePath::new("link").unwrap(), b"bad").is_err());
    fs::set_permissions(path.join("pending.json"), fs::Permissions::from_mode(0o640)).unwrap();
    assert!(AtomicFile::replace(&directory, &pending, b"bad").is_err());
    assert_eq!(fs::read(path.join("pending.json")).unwrap(), b"second");
    fs::set_permissions(path.join("pending.json"), fs::Permissions::from_mode(0o600)).unwrap();
    directory
        .remove_file(&EntryName::new("pending.json").unwrap())
        .unwrap();
    assert!(!path.join("pending.json").exists());
}

fn verify_administrative_payloads(directory: &PrivateDirectory, path: &Path, owner: (u32, u32)) {
    use std::os::unix::fs::{MetadataExt, chown};
    let (uid, gid) = owner;
    // Exercise the same administrative lock/readback used by first-admin setup.
    let setup = EntryName::new("setup.lock").unwrap();
    let _lock =
        AdvisoryLock::acquire(directory, &RelativePath::new("setup.lock").unwrap()).unwrap();
    let held = directory
        .open_payload(&setup, PayloadOpenMode::Existing)
        .unwrap();
    directory.verify_payload(&setup, &held).unwrap();
    assert_eq!(
        (
            held.metadata().unwrap().uid(),
            held.metadata().unwrap().gid()
        ),
        (uid, gid)
    );
    let entry = EntryName::new("created-payload").unwrap();
    let created = directory
        .open_payload(&entry, PayloadOpenMode::CreateNew)
        .unwrap();
    let metadata = created.metadata().unwrap();
    assert_eq!(
        (metadata.uid(), metadata.gid(), metadata.mode() & 0o7777),
        (uid, gid, 0o600)
    );
    directory.verify_payload(&entry, &created).unwrap();
    fs::set_permissions(
        path.join(entry.as_path()),
        fs::Permissions::from_mode(0o640),
    )
    .unwrap();
    assert!(
        directory
            .open_payload(&entry, PayloadOpenMode::Existing)
            .is_err()
    );
    assert!(directory.verify_payload(&entry, &created).is_err());
    assert_eq!(
        fs::metadata(path.join(entry.as_path())).unwrap().mode() & 0o7777,
        0o640
    );
    fs::set_permissions(
        path.join(entry.as_path()),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    if geteuid().as_raw() == 0 {
        for (wrong_uid, wrong_gid) in [(0, gid), (uid, 0)] {
            chown(path.join(entry.as_path()), Some(wrong_uid), Some(wrong_gid)).unwrap();
            assert!(
                directory
                    .open_payload(&entry, PayloadOpenMode::Existing)
                    .is_err()
            );
            assert!(directory.verify_payload(&entry, &created).is_err());
            let metadata = created.metadata().unwrap();
            assert_eq!((metadata.uid(), metadata.gid()), (wrong_uid, wrong_gid));
        }
        chown(path.join(entry.as_path()), Some(uid), Some(gid)).unwrap();
    }
    fs::rename(path.join(entry.as_path()), path.join("retained-payload")).unwrap();
    let replacement = directory
        .open_payload(&entry, PayloadOpenMode::CreateNew)
        .unwrap();
    assert!(matches!(
        directory.verify_payload(&entry, &created),
        Err(Error::IdentityChanged(_))
    ));
    directory.verify_payload(&entry, &replacement).unwrap();
}

#[test]
fn rejecting_private_directory_does_not_chmod_a_symlink_target_or_existing_directory() {
    let temp = tempfile::tempdir().unwrap();
    let victim = temp.path().join("victim");
    fs::create_dir(&victim).unwrap();
    fs::set_permissions(&victim, fs::Permissions::from_mode(0o755)).unwrap();
    let link = temp.path().join("link");
    symlink(&victim, &link).unwrap();
    assert!(PrivateDirectory::create(&link).is_err());
    assert!(PrivateDirectory::create(&victim).is_err());
    assert_eq!(
        fs::metadata(&victim).unwrap().permissions().mode() & 0o7777,
        0o755
    );
    assert!(PrivateDirectory::create(link.join("nested")).is_err());
    assert!(!victim.join("nested").exists());
}

#[test]
fn held_private_directory_survives_path_rebinding_without_touching_replacement() {
    let temp = tempfile::tempdir().unwrap();
    let original = temp.path().join("state");
    let root = PrivateDirectory::create(&original).unwrap();
    let moved = temp.path().join("moved");
    fs::rename(&original, &moved).unwrap();
    fs::create_dir(&original).unwrap();
    AtomicFile::replace(&root, &RelativePath::new("entry").unwrap(), b"private").unwrap();
    assert_eq!(fs::read(moved.join("entry")).unwrap(), b"private");
    assert!(!original.join("entry").exists());
    assert_eq!(
        fs::metadata(moved.join("entry"))
            .unwrap()
            .permissions()
            .mode()
            & 0o7777,
        0o600
    );
    AdvisoryLock::acquire(&root, &RelativePath::new("lock").unwrap()).unwrap();
    assert!(!original.join("lock").exists());
}

#[test]
fn writes_and_locks_refuse_links_and_nested_aliases() {
    let temp = tempfile::tempdir().unwrap();
    let root = PrivateDirectory::create(temp.path().join("state")).unwrap();
    let victim = temp.path().join("victim");
    fs::write(&victim, b"untouched").unwrap();
    symlink(&victim, root.path().join("link")).unwrap();
    fs::hard_link(&victim, root.path().join("hard")).unwrap();
    symlink(temp.path(), root.path().join("escape")).unwrap();
    for name in ["link", "hard", "escape/victim"] {
        let name = RelativePath::new(name).unwrap();
        assert!(AtomicFile::replace(&root, &name, b"changed").is_err());
        assert!(AdvisoryLock::acquire(&root, &name).is_err());
        assert!(crate::fs_safety::sync_file_and_parent(&root.resolve(&name)).is_err());
    }
    assert_eq!(fs::read(victim).unwrap(), b"untouched");
}

#[test]
fn typed_entry_io_is_bounded_and_create_never_replaces_existing_content() {
    let temp = tempfile::tempdir().unwrap();
    let directory = PrivateDirectory::create(temp.path().join("state")).unwrap();
    for invalid in ["", ".", "..", "a/b", "a/", "a\0b"] {
        assert!(EntryName::new(invalid).is_err(), "{invalid:?}");
    }
    let entry = EntryName::new("current").unwrap();
    AtomicFile::create(&directory, &entry, b"original").unwrap();
    assert!(AtomicFile::create(&directory, &entry, b"replace").is_err());
    assert_eq!(directory.read_bounded(&entry, 8).unwrap(), b"original");
    assert!(matches!(
        directory.read_bounded(&entry, 7),
        Err(Error::BudgetExceeded)
    ));
    let entries = directory
        .files(InventoryLimits {
            max_entries: 1,
            max_total_bytes: 8,
        })
        .unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].bytes, 8);
    assert!(matches!(
        directory.files(InventoryLimits {
            max_entries: 0,
            max_total_bytes: 8
        }),
        Err(Error::BudgetExceeded)
    ));
    assert!(matches!(
        directory.files(InventoryLimits {
            max_entries: 1,
            max_total_bytes: 7
        }),
        Err(Error::BudgetExceeded)
    ));
    let path = directory.path().join(entry.as_path());
    fs::OpenOptions::new()
        .write(true)
        .open(path)
        .unwrap()
        .set_len(8 * 1024 * 1024 * 1024)
        .unwrap();
    assert!(matches!(
        directory.read_bounded(&entry, 1024),
        Err(Error::BudgetExceeded)
    ));
    directory.remove_file(&entry).unwrap();
    assert!(
        directory
            .files(InventoryLimits {
                max_entries: 0,
                max_total_bytes: 0
            })
            .unwrap()
            .is_empty()
    );
}

#[test]
fn no_clobber_has_one_winner_and_never_replaces_an_occupant() {
    let temp = tempfile::tempdir().unwrap();
    let root = std::sync::Arc::new(PrivateDirectory::create(temp.path().join("state")).unwrap());
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let workers: Vec<_> = ["first", "second"]
        .into_iter()
        .map(|value| {
            let root = root.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let source = RelativePath::new(value).unwrap();
                AtomicFile::replace(&root, &source, value.as_bytes()).unwrap();
                barrier.wait();
                crate::fs_safety::NoClobberPublish::publish(
                    &root,
                    &source,
                    &RelativePath::new("current").unwrap(),
                    &File::open(root.resolve(&source)).unwrap(),
                )
                .is_ok()
            })
        })
        .collect();
    assert_eq!(
        workers
            .into_iter()
            .map(|worker| usize::from(worker.join().unwrap()))
            .sum::<usize>(),
        1
    );
    let bytes = fs::read(root.path().join("current")).unwrap();
    assert!(bytes == b"first" || bytes == b"second");
    let victim = temp.path().join("victim");
    fs::write(&victim, b"untouched").unwrap();
    symlink(&victim, root.path().join("occupied")).unwrap();
    let current = RelativePath::new("current").unwrap();
    assert!(
        crate::fs_safety::NoClobberPublish::publish(
            &root,
            &current,
            &RelativePath::new("occupied").unwrap(),
            &File::open(root.resolve(&current)).unwrap(),
        )
        .is_err()
    );
    assert_eq!(fs::read(victim).unwrap(), b"untouched");
    assert_eq!(fs::read(root.resolve(&current)).unwrap(), bytes);
}

#[test]
fn inventory_rejects_linked_root_special_files_and_excessive_depth() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    fs::create_dir(&root).unwrap();
    let alias = temp.path().join("alias");
    symlink(&root, &alias).unwrap();
    let limits = crate::fs_safety::InventoryLimits {
        max_entries: 1000,
        max_total_bytes: 16 * 1024 * 1024,
    };
    assert!(bounded_inventory(&alias, limits).is_err());
    let mut parent = root.clone();
    for _ in 0..128 {
        parent = parent.join("d");
        fs::create_dir(&parent).unwrap();
    }
    assert!(matches!(
        bounded_inventory(&root, limits),
        Err(Error::BudgetExceeded)
    ));
    let specials = tempfile::tempdir().unwrap();
    let _listener = std::os::unix::net::UnixListener::bind(specials.path().join("socket")).unwrap();
    assert!(matches!(
        bounded_inventory(specials.path(), limits),
        Err(Error::UnsafeFileType(_))
    ));
}

#[test]
fn ancestor_owner_and_sticky_policy() {
    assert!(trusted_ancestor(0, 0o755, 1000));
    assert!(trusted_ancestor(0, 0o1777, 1000));
    assert!(trusted_ancestor(1000, 0o700, 1000));
    assert!(!trusted_ancestor(1000, 0o777, 1000));
    assert!(!trusted_ancestor(2000, 0o700, 1000));
    assert!(!trusted_ancestor(2000, 0o700, 0));
    assert!(!trusted_ancestor(2000, 0o1777, 1000));
}

#[test]
fn staging_cleanup_reports_failure_and_disarms_before_drop() {
    let root = tempfile::tempdir().unwrap();
    let directory = File::open(root.path()).unwrap();
    std::fs::create_dir(root.path().join("staged")).unwrap();
    let mut cleanup = StagingCleanup {
        directory: &directory,
        name: "staged".into(),
        armed: true,
    };
    assert_eq!(
        cleanup.cleanup().unwrap_err().raw_os_error(),
        Some(libc::EISDIR)
    );
    assert!(!cleanup.armed);
    drop(cleanup);
    assert!(root.path().join("staged").is_dir());
}
