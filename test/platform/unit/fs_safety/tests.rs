use super::*;

#[test]
fn batch_removal_cleans_only_named_files_and_accepts_missing_entries() {
    let temp = tempfile::tempdir().unwrap();
    let directory = PrivateDirectory::create(temp.path().join("private")).unwrap();
    let names = ["source", "source.work.json", "source.budget.json"]
        .map(|name| EntryName::new(name).unwrap());
    for name in &names {
        AtomicFile::create(&directory, name, b"retained").unwrap();
    }
    let unrelated = EntryName::new("unrelated").unwrap();
    AtomicFile::create(&directory, &unrelated, b"keep").unwrap();
    directory.remove_files(&names).unwrap();
    directory.remove_files(&names).unwrap();
    for name in &names {
        assert!(!directory.path().join(name.as_path()).exists());
    }
    assert_eq!(
        fs::read(directory.path().join("unrelated")).unwrap(),
        b"keep"
    );
}

#[test]
fn batch_removal_stops_at_unsafe_entry_without_deleting_later_files() {
    let temp = tempfile::tempdir().unwrap();
    let directory = PrivateDirectory::create(temp.path().join("private")).unwrap();
    let names = ["source", "source.work.json", "source.budget.json"]
        .map(|name| EntryName::new(name).unwrap());
    AtomicFile::create(&directory, &names[0], b"source").unwrap();
    directory.create_child(&names[1]).unwrap();
    AtomicFile::create(&directory, &names[2], b"budget").unwrap();
    assert!(matches!(
        directory.remove_files(&names),
        Err(Error::UnsafeFileType(_))
    ));
    assert!(!directory.path().join("source").exists());
    assert!(directory.path().join("source.work.json").is_dir());
    assert_eq!(
        fs::read(directory.path().join("source.budget.json")).unwrap(),
        b"budget"
    );
}

#[test]
fn batch_removal_rejects_replaced_named_directory_without_removing_either_copy() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("private");
    let held = PrivateDirectory::create(&path).unwrap();
    let name = EntryName::new("source").unwrap();
    AtomicFile::create(&held, &name, b"original").unwrap();
    let moved = temp.path().join("moved");
    fs::rename(&path, &moved).unwrap();
    let replacement = PrivateDirectory::create(&path).unwrap();
    AtomicFile::create(&replacement, &name, b"replacement").unwrap();
    assert!(matches!(
        held.remove_files(&[name]),
        Err(Error::UnsafeFileType(_))
    ));
    assert_eq!(fs::read(moved.join("source")).unwrap(), b"original");
    assert_eq!(fs::read(path.join("source")).unwrap(), b"replacement");
}

#[test]
fn batch_removal_rejects_symbolic_and_multiple_links() {
    use std::os::unix::fs::symlink;
    for symbolic in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let directory = PrivateDirectory::create(temp.path().join("private")).unwrap();
        let source = EntryName::new("source").unwrap();
        AtomicFile::create(&directory, &source, b"keep").unwrap();
        let alias = EntryName::new("alias").unwrap();
        let source_path = directory.path().join(source.as_path());
        let alias_path = directory.path().join(alias.as_path());
        if symbolic {
            symlink(&source_path, &alias_path).unwrap();
        } else {
            fs::hard_link(&source_path, &alias_path).unwrap();
        }
        assert!(directory.remove_files(&[alias]).is_err());
        assert_eq!(fs::read(&source_path).unwrap(), b"keep");
        assert!(fs::symlink_metadata(&alias_path).is_ok());
    }
}

#[test]
fn admitted_relative_paths_split_into_typed_parents_and_entries() {
    for (input, parent, name) in [
        ("entry", None, "entry"),
        ("one/two/leaf", Some("one/two"), "leaf"),
        ("目录/名字", Some("目录"), "名字"),
    ] {
        let path = RelativePath::new(input).unwrap();
        let (actual_parent, actual_name) = path.split_parent();
        assert_eq!(
            actual_parent,
            parent.map(|path| RelativePath::new(path).unwrap())
        );
        assert_eq!(actual_name, EntryName::new(name).unwrap());
    }
    let long_name = "x".repeat(300);
    let path = RelativePath::new(format!("parent/{long_name}")).unwrap();
    assert_eq!(path.split_parent().1.as_os_str(), OsStr::new(&long_name));
    for invalid in ["", "/entry", ".", "parent/../entry", "a//b", "a/./b", "a/"] {
        assert!(RelativePath::new(invalid).is_err(), "{invalid:?}");
    }
}

#[cfg(unix)]
#[test]
fn splitting_an_admitted_path_preserves_non_utf8_entry_bytes() {
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    let path = PathBuf::from(std::ffi::OsString::from_vec(b"parent/\xff".to_vec()));
    let (parent, name) = RelativePath::new(path).unwrap().split_parent();
    assert_eq!(parent.unwrap().as_path(), Path::new("parent"));
    assert_eq!(name.as_os_str().as_bytes(), b"\xff");
}

#[test]
fn lower_hex_preserves_fixed_vectors() {
    assert_eq!(crate::lower_hex(&[]), "");
    assert_eq!(
        crate::lower_hex(&[0x00, 0x01, 0x0f, 0x10, 0x7f, 0x80, 0xfe, 0xff]),
        "00010f107f80feff"
    );
    assert_eq!(
        crate::lower_hex(&[0; 16]),
        "00000000000000000000000000000000"
    );
}

#[test]
fn lower_hex_matches_previous_format_for_every_byte() {
    let bytes: Vec<u8> = (0..=u8::MAX).collect();
    let previous: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    assert_eq!(crate::lower_hex(&bytes), previous);
}

#[test]
fn temporary_name_retains_its_exact_hex_shape() {
    let name = temporary_name().unwrap();
    assert_eq!(name.len(), 50);
    let nonce = name
        .strip_prefix(".state-atomic-")
        .unwrap()
        .strip_suffix(".tmp")
        .unwrap();
    assert_eq!(nonce.len(), 32);
    assert!(
        nonce
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    );
    assert!(AtomicFile::is_temporary_name(
        &EntryName::new(name).unwrap()
    ));
}

#[cfg(unix)]
#[test]
fn open_existing_never_creates_or_repairs_a_directory() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("private");
    assert!(PrivateDirectory::open_existing(&path).is_err());
    assert!(!path.exists());
    PrivateDirectory::create(&path).unwrap();
    let held = PrivateDirectory::open_existing(&path).unwrap();
    let moved = temp.path().join("moved");
    fs::rename(&path, &moved).unwrap();
    symlink(&moved, &path).unwrap();
    assert!(PrivateDirectory::open_existing(&path).is_err());
    fs::remove_file(&path).unwrap();
    PrivateDirectory::create(&path).unwrap();
    let name = EntryName::new("entry").unwrap();
    AtomicFile::create(&held, &name, b"anchored").unwrap();
    assert_eq!(fs::read(moved.join("entry")).unwrap(), b"anchored");
    let child = held
        .create_child(&EntryName::new("child").unwrap())
        .unwrap();
    AtomicFile::create(&child, &name, b"child anchored").unwrap();
    assert_eq!(
        fs::read(moved.join("child/entry")).unwrap(),
        b"child anchored"
    );
    assert!(!path.join("entry").exists());
    assert!(!path.join("child").exists());
    fs::set_permissions(&moved, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(
        PrivateDirectory::open_existing(&moved),
        Err(Error::UnsafePermissions(_))
    ));
    assert_eq!(
        fs::metadata(&moved).unwrap().permissions().mode() & 0o777,
        0o755
    );
}

#[test]
fn rejects_escape_and_atomically_publishes() {
    assert!(RelativePath::new("../escape").is_err());
    let temp = tempfile::tempdir().unwrap();
    let root = PrivateDirectory::create(temp.path().join("state")).unwrap();
    let name = RelativePath::new("current").unwrap();
    AtomicFile::replace(&root, &name, b"one").unwrap();
    AtomicFile::replace(&root, &name, b"two").unwrap();
    assert_eq!(fs::read(root.resolve(&name)).unwrap(), b"two");
}
#[test]
fn atomic_replace_keeps_the_held_parent_after_its_path_is_replaced() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("state");
    let held = PrivateDirectory::create(&path).unwrap();
    let name = RelativePath::new("progress.json").unwrap();
    AtomicFile::replace(&held, &name, b"old").unwrap();
    let moved = temp.path().join("moved");
    fs::rename(&path, &moved).unwrap();
    let replacement = PrivateDirectory::create(&path).unwrap();
    AtomicFile::replace(&replacement, &name, b"replacement").unwrap();

    AtomicFile::replace(&held, &name, b"updated").unwrap();
    assert_eq!(fs::read(moved.join("progress.json")).unwrap(), b"updated");
    assert_eq!(
        fs::read(path.join("progress.json")).unwrap(),
        b"replacement"
    );
    assert_eq!(fs::read_dir(&moved).unwrap().count(), 1);
    assert_eq!(fs::read_dir(&path).unwrap().count(), 1);
}

#[test]
fn atomic_replace_can_retain_the_exact_published_descriptor() {
    use std::os::unix::fs::MetadataExt;
    let temp = tempfile::tempdir().unwrap();
    let directory = PrivateDirectory::create(temp.path().join("private")).unwrap();
    let name = EntryName::new("record").unwrap();
    let first = AtomicFile::replace_held(&directory, &name.as_relative(), b"one").unwrap();
    directory.verify_payload(&name, &first).unwrap();
    let second = AtomicFile::replace_held(&directory, &name.as_relative(), b"two").unwrap();
    assert_ne!(
        first.metadata().unwrap().ino(),
        second.metadata().unwrap().ino()
    );
    assert!(matches!(
        directory.verify_payload(&name, &first),
        Err(Error::IdentityChanged(_))
    ));
    directory.verify_payload(&name, &second).unwrap();
    assert_eq!(directory.read_bounded(&name, 3).unwrap(), b"two");
    fs::remove_file(directory.path().join(name.as_path())).unwrap();
    fs::create_dir(directory.path().join(name.as_path())).unwrap();
    assert!(matches!(
        AtomicFile::replace_held(&directory, &name.as_relative(), b"bad"),
        Err(Error::UnsafeFileType(_))
    ));
}

#[test]
fn raw_directory_admission_and_atomic_writes_keep_the_supplied_handle() {
    use std::os::fd::AsRawFd;
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("state");
    PrivateDirectory::create(&path).unwrap();
    let file = File::open(&path).unwrap();
    let descriptor = file.as_raw_fd();
    let moved = temp.path().join("moved");
    fs::rename(&path, &moved).unwrap();
    let replacement = PrivateDirectory::create(&path).unwrap();
    let name = EntryName::new("progress.json").unwrap();
    AtomicFile::create(&replacement, &name, b"replacement").unwrap();

    let held = PrivateDirectory::from_directory(file, path.clone()).unwrap();
    assert_eq!(held.directory_handle().as_raw_fd(), descriptor);
    assert_eq!(held.path(), path);
    assert!(held.verify_identity().is_err());
    AtomicFile::create(&held, &name, b"created").unwrap();
    AtomicFile::replace(&held, &name.as_relative(), b"updated").unwrap();
    assert_eq!(held.read_bounded(&name, 7).unwrap(), b"updated");
    assert_eq!(fs::read(moved.join(name.as_path())).unwrap(), b"updated");
    assert_eq!(fs::read(path.join(name.as_path())).unwrap(), b"replacement");
    assert_eq!(fs::read_dir(&moved).unwrap().count(), 1);
    assert_eq!(fs::read_dir(&path).unwrap().count(), 1);
}

#[test]
fn raw_directory_admission_rejects_file_type_and_permissions_without_repair() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("file");
    fs::write(&path, b"untouched").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(matches!(
        PrivateDirectory::from_directory(File::open(&path).unwrap(), path.clone()),
        Err(Error::UnsafeFileType(_))
    ));
    assert_eq!(fs::read(&path).unwrap(), b"untouched");

    let path = temp.path().join("directory");
    fs::create_dir(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(
        PrivateDirectory::from_directory(File::open(&path).unwrap(), path.clone()),
        Err(Error::UnsafePermissions(_))
    ));
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o7777,
        0o755
    );
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(matches!(
        PrivateDirectory::from_directory(File::open(&path).unwrap(), "relative".into()),
        Err(Error::AbsolutePathRequired(_))
    ));
    assert!(matches!(
        PrivateDirectory::from_directory(File::open(&path).unwrap(), path.join("../label")),
        Err(Error::UnsafeRelativePath(_))
    ));
    PrivateDirectory::from_directory(File::open(&path).unwrap(), path).unwrap();
}

#[cfg(unix)]
#[test]
fn inventory_rejects_symlinks_and_budgets() {
    use std::os::unix::fs::symlink;
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("a"), b"abc").unwrap();
    assert!(
        bounded_directory_inventory(
            temp.path(),
            InventoryLimits {
                max_entries: 0,
                max_total_bytes: 10
            }
        )
        .is_err()
    );
    symlink("a", temp.path().join("b")).unwrap();
    assert!(matches!(
        bounded_directory_inventory(
            temp.path(),
            InventoryLimits {
                max_entries: 10,
                max_total_bytes: 10
            }
        ),
        Err(Error::UnsafeFileType(_))
    ));
}

#[cfg(unix)]
#[test]
fn inventory_rejects_hardlinks_and_advisory_lock_is_exclusive() {
    let temp = tempfile::tempdir().unwrap();
    let state = PrivateDirectory::create(temp.path().join("state")).unwrap();
    let lock_name = RelativePath::new("instance.lock").unwrap();
    let first = AdvisoryLock::acquire(&state, &lock_name).unwrap();
    assert!(matches!(
        AdvisoryLock::acquire(&state, &lock_name),
        Err(Error::AlreadyLocked(_))
    ));
    drop(first);
    AdvisoryLock::acquire(&state, &lock_name).unwrap();

    let original = state.path().join("original");
    fs::write(&original, b"secret").unwrap();
    fs::hard_link(&original, state.path().join("alias")).unwrap();
    assert!(matches!(
        bounded_directory_inventory(
            state.path(),
            InventoryLimits {
                max_entries: 10,
                max_total_bytes: 1024,
            }
        ),
        Err(Error::MultipleLinks(_))
    ));
}

#[cfg(unix)]
#[test]
fn trusted_spool_rejects_replaceable_ancestry_without_repairing_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let parent = root.path().join("writable");
    fs::create_dir(&parent).unwrap();
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o777)).unwrap();
    let private = PrivateDirectory::create(parent.join("private")).unwrap();
    assert!(PrivateDirectory::open_trusted(private.path()).is_err());
    assert!(PrivateDirectory::create_trusted(parent.join("new")).is_err());
    assert!(!parent.join("new").exists());
    assert_eq!(
        fs::metadata(&parent).unwrap().permissions().mode() & 0o7777,
        0o777
    );
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o1777)).unwrap();
    assert!(PrivateDirectory::open_trusted(private.path()).is_ok());
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(PrivateDirectory::open_trusted(private.path()).is_ok());
    assert!(PrivateDirectory::create_trusted(&parent).is_err());
    assert_eq!(
        fs::metadata(&parent).unwrap().permissions().mode() & 0o7777,
        0o755
    );
}

#[cfg(unix)]
#[test]
fn payload_open_modes_create_exclusively_and_preserve_existing_bytes() {
    use std::io::{Read, Write};
    use std::os::unix::fs::MetadataExt;

    let root = tempfile::tempdir().unwrap();
    let directory = PrivateDirectory::create(root.path().join("private")).unwrap();
    let name = EntryName::new("data").unwrap();
    assert!(matches!(
        directory.open_payload(&name, PayloadOpenMode::Existing),
        Err(Error::Io(error)) if error.kind() == io::ErrorKind::NotFound
    ));
    assert!(!directory.path().join("data").exists());

    let mut created = directory
        .open_payload(&name, PayloadOpenMode::CreateNew)
        .unwrap();
    created.write_all(b"original").unwrap();
    let original = created.metadata().unwrap();
    assert_eq!(original.mode() & 0o7777, 0o600);
    assert_eq!(original.nlink(), 1);
    assert!(matches!(
        directory.open_payload(&name, PayloadOpenMode::CreateNew),
        Err(Error::Io(error)) if error.kind() == io::ErrorKind::AlreadyExists
    ));

    let mut existing = directory
        .open_payload(&name, PayloadOpenMode::Existing)
        .unwrap();
    let reopened = existing.metadata().unwrap();
    assert_eq!(reopened.dev(), original.dev());
    assert_eq!(reopened.ino(), original.ino());
    assert_eq!(reopened.len(), 8);
    assert_eq!(reopened.mode(), original.mode());
    let mut bytes = Vec::new();
    existing.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"original");
    existing.write_all(b" tail").unwrap();
    assert_eq!(
        fs::read(directory.path().join("data")).unwrap(),
        b"original tail"
    );

    let empty = EntryName::new("empty").unwrap();
    directory
        .open_payload(&empty, PayloadOpenMode::CreateNew)
        .unwrap();
    assert_eq!(
        directory
            .open_payload(&empty, PayloadOpenMode::Existing)
            .unwrap()
            .metadata()
            .unwrap()
            .len(),
        0
    );
}

#[cfg(unix)]
#[test]
fn payload_open_modes_preserve_link_and_permission_error_order() {
    use std::io::Write;
    use std::os::unix::fs::{PermissionsExt, symlink};

    let root = tempfile::tempdir().unwrap();
    let directory = PrivateDirectory::create(root.path().join("private")).unwrap();
    let target = EntryName::new("target").unwrap();
    let target_path = directory.path().join("target");
    directory
        .open_payload(&target, PayloadOpenMode::CreateNew)
        .unwrap()
        .write_all(b"retained")
        .unwrap();
    let symbolic = EntryName::new("symbolic").unwrap();
    symlink("target", directory.path().join("symbolic")).unwrap();
    assert!(matches!(
        directory.open_payload(&symbolic, PayloadOpenMode::CreateNew),
        Err(Error::Io(error)) if error.kind() == io::ErrorKind::AlreadyExists
    ));
    assert!(matches!(
        directory.open_payload(&symbolic, PayloadOpenMode::Existing),
        Err(Error::Io(error)) if error.raw_os_error() == Some(libc::ELOOP)
    ));

    let alias = directory.path().join("alias");
    fs::hard_link(&target_path, &alias).unwrap();
    fs::set_permissions(&target_path, fs::Permissions::from_mode(0o640)).unwrap();
    assert!(matches!(
        directory.open_payload(&target, PayloadOpenMode::CreateNew),
        Err(Error::Io(error)) if error.kind() == io::ErrorKind::AlreadyExists
    ));
    assert!(matches!(
        directory.open_payload(&target, PayloadOpenMode::Existing),
        Err(Error::UnsafeFileType(_))
    ));
    fs::remove_file(alias).unwrap();
    assert!(matches!(
        directory.open_payload(&target, PayloadOpenMode::Existing),
        Err(Error::UnsafePermissions(_))
    ));
    assert_eq!(
        fs::metadata(&target_path).unwrap().permissions().mode() & 0o7777,
        0o640
    );
    assert_eq!(fs::read(target_path).unwrap(), b"retained");
}

#[cfg(unix)]
#[test]
fn payload_create_new_has_one_winner_for_concurrent_creators() {
    use std::io::Write;
    use std::sync::Barrier;

    let root = tempfile::tempdir().unwrap();
    let directory = PrivateDirectory::create(root.path().join("private")).unwrap();
    let name = EntryName::new("data").unwrap();
    let barrier = Barrier::new(2);
    let (first, second) = std::thread::scope(|scope| {
        let worker = scope.spawn(|| {
            barrier.wait();
            directory.open_payload(&name, PayloadOpenMode::CreateNew)
        });
        barrier.wait();
        let second = directory.open_payload(&name, PayloadOpenMode::CreateNew);
        (worker.join().unwrap(), second)
    });
    let mut winner = match (first, second) {
        (Ok(file), Err(Error::Io(error))) | (Err(Error::Io(error)), Ok(file)) => {
            assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
            file
        }
        result => panic!("expected one exclusive creator: {result:?}"),
    };
    winner.write_all(b"winner").unwrap();
    assert_eq!(fs::read(directory.path().join("data")).unwrap(), b"winner");
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[cfg(unix)]
#[test]
fn trusted_spool_payloads_remain_anchored_and_replacements_are_rejected() {
    use std::io::{Read, Write};
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("private");
    let held = PrivateDirectory::create_trusted(&path).unwrap();
    let name = EntryName::new("data").unwrap();
    let mut file = held
        .open_payload(&name, PayloadOpenMode::CreateNew)
        .unwrap();
    file.write_all(b"original").unwrap();
    held.verify_payload(&name, &file).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(
        held.verify_payload(&name, &file),
        Err(Error::UnsafePermissions(rejected)) if rejected == path
    ));
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    held.verify_payload(&name, &file).unwrap();
    fs::rename(path.join("data"), path.join("old")).unwrap();
    let mut replacement = held
        .open_payload(&name, PayloadOpenMode::CreateNew)
        .unwrap();
    replacement.write_all(b"replacement").unwrap();
    assert!(matches!(
        held.verify_payload(&name, &file),
        Err(Error::IdentityChanged(rejected)) if rejected == path.join("data")
    ));
    held.verify_payload(&name, &replacement).unwrap();
    fs::rename(&path, root.path().join("moved")).unwrap();
    PrivateDirectory::create_trusted(&path).unwrap();
    assert!(held.verify_identity().is_err());
    assert!(matches!(
        held.verify_payload(&name, &replacement),
        Err(Error::UnsafeFileType(rejected)) if rejected == path
    ));
    let mut bytes = Vec::new();
    held.open_payload(&name, PayloadOpenMode::Existing)
        .unwrap()
        .read_to_end(&mut bytes)
        .unwrap();
    assert_eq!(bytes, b"replacement");
    held.open_payload(
        &EntryName::new("anchored").unwrap(),
        PayloadOpenMode::CreateNew,
    )
    .unwrap();
    assert!(root.path().join("moved/anchored").exists());
    assert!(!path.join("anchored").exists());
}

#[cfg(unix)]
#[test]
fn payload_verification_rechecks_leaf_constraints_in_existing_error_order() {
    use std::io::Write;
    use std::os::unix::fs::{PermissionsExt, symlink};

    let root = tempfile::tempdir().unwrap();
    let directory = PrivateDirectory::create(root.path().join("private")).unwrap();
    let name = EntryName::new("data").unwrap();
    let path = directory.path().join(name.as_path());
    let mut held = directory
        .open_payload(&name, PayloadOpenMode::CreateNew)
        .unwrap();
    held.write_all(b"original").unwrap();
    directory.verify_payload(&name, &held).unwrap();

    let alias = directory.path().join("alias");
    fs::hard_link(&path, &alias).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    assert!(matches!(
        directory.verify_payload(&name, &held),
        Err(Error::UnsafeFileType(rejected)) if rejected == path
    ));
    fs::remove_file(alias).unwrap();
    assert!(matches!(
        directory.verify_payload(&name, &held),
        Err(Error::UnsafePermissions(rejected)) if rejected == path
    ));
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    directory.verify_payload(&name, &held).unwrap();

    let retained = directory.path().join("retained");
    fs::rename(&path, &retained).unwrap();
    assert!(matches!(
        directory.verify_payload(&name, &held),
        Err(Error::Io(error)) if error.kind() == io::ErrorKind::NotFound
    ));
    symlink("retained", &path).unwrap();
    assert!(matches!(
        directory.verify_payload(&name, &held),
        Err(Error::Io(error)) if error.raw_os_error() == Some(libc::ELOOP)
    ));
    fs::remove_file(&path).unwrap();
    rustix::fs::mknodat(
        directory.directory_handle(),
        name.as_os_str(),
        rustix::fs::FileType::Fifo,
        rustix::fs::Mode::from_raw_mode(0o600),
        0,
    )
    .unwrap();
    assert!(matches!(
        directory.verify_payload(&name, &held),
        Err(Error::UnsafeFileType(rejected)) if rejected == path
    ));
    assert_eq!(fs::read(retained).unwrap(), b"original");
}

#[test]
fn randomness_failure_preserves_its_entropy_source() {
    let error = Error::Randomness(system_entropy::Error::UNSUPPORTED);
    let source = std::error::Error::source(&error).expect("entropy source must survive");
    assert!(source.downcast_ref::<system_entropy::Error>().is_some());
}

#[test]
fn create_and_public_publish_preserve_existing_destinations_and_sources() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let directory = PrivateDirectory::create(temp.path().join("private")).unwrap();
    let destination = EntryName::new("destination").unwrap();
    AtomicFile::create(&directory, &destination, b"original").unwrap();
    assert!(matches!(
        AtomicFile::create(&directory, &destination, b"replacement"),
        Err(Error::DestinationExists(_))
    ));
    let source = RelativePath::new("source").unwrap();
    fs::write(directory.resolve(&source), b"staged").unwrap();
    assert!(matches!(
        NoClobberPublish::publish(&directory, &source, &destination.as_relative()),
        Err(Error::DestinationExists(_))
    ));
    assert_eq!(fs::read(directory.resolve(&source)).unwrap(), b"staged");
    assert_eq!(
        fs::read(directory.path().join("destination")).unwrap(),
        b"original"
    );
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 2);
    assert_eq!(
        fs::metadata(directory.path().join("destination"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}

#[test]
#[ignore = "requires atomic-io.sh isolated LD_PRELOAD children"]
fn atomic_io_barriers_and_failures() {
    use std::os::unix::fs::PermissionsExt;
    let root = std::path::PathBuf::from(std::env::var_os("ZXS_ATOMIC_ROOT").unwrap());
    let log = std::path::PathBuf::from(std::env::var_os("ZXS_ATOMIC_LOG").unwrap());
    let mode = std::env::var("ZXS_ATOMIC_MODE").unwrap();
    let stage = std::env::var("ZXS_ATOMIC_STAGE").unwrap();
    let directory = PrivateDirectory::open_existing(&root).unwrap();
    let destination = EntryName::new("destination").unwrap();
    let source = RelativePath::new("source").unwrap();
    let replaces = matches!(mode.as_str(), "replace" | "replace-held");
    if replaces {
        fs::write(root.join("destination"), b"old").unwrap();
    } else if mode == "public" {
        fs::write(root.join("source"), b"new").unwrap();
        fs::set_permissions(root.join("source"), fs::Permissions::from_mode(0o600)).unwrap();
    }
    fs::write(root.join(".inject"), b"").unwrap();
    let result = match mode.as_str() {
        "create" => AtomicFile::create(&directory, &destination, b"new"),
        "recovery-intent" => AtomicFile::create_recovery_intent(&directory, &destination, b"new"),
        "replace" => AtomicFile::replace(&directory, &destination.as_relative(), b"new"),
        "replace-held" => {
            AtomicFile::replace_held(&directory, &destination.as_relative(), b"new").map(drop)
        }
        "public" => NoClobberPublish::publish(&directory, &source, &destination.as_relative()),
        _ => panic!("unknown test mode"),
    };
    fs::remove_file(root.join(".inject")).unwrap();
    let events = fs::read_to_string(log).unwrap();
    match stage.as_str() {
        "success" | "directory-sync" => {
            if stage == "success" {
                result.unwrap();
            } else {
                assert!(
                    matches!(result, Err(Error::PublishedDurabilityUnknown(ref error)) if error.raw_os_error() == Some(5))
                );
            }
            assert_eq!(events, "file-sync\ndirectory-sync\n");
            assert_eq!(fs::read(root.join("destination")).unwrap(), b"new");
            assert_eq!(
                fs::metadata(root.join("destination"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
            assert!(!root.join("source").exists());
        }
        "file-sync" if mode == "recovery-intent" => {
            assert!(
                matches!(result, Err(Error::PublishedDurabilityUnknown(ref error)) if error.raw_os_error() == Some(5))
            );
            assert_eq!(events, "file-sync\n");
            assert_eq!(fs::read(root.join("destination")).unwrap(), b"new");
        }
        "write" | "file-sync" => {
            assert!(matches!(result, Err(Error::Io(ref error)) if error.raw_os_error() == Some(5)));
            assert_eq!(
                events,
                if stage == "write" {
                    "write-failure\n"
                } else {
                    "file-sync\n"
                }
            );
            if replaces {
                assert_eq!(fs::read(root.join("destination")).unwrap(), b"old");
            } else {
                assert!(!root.join("destination").exists());
            }
        }
        "publish" => {
            assert_eq!(
                events,
                if mode == "recovery-intent" {
                    "publish-conflict\n"
                } else {
                    "file-sync\n"
                }
            );
            if replaces {
                assert!(
                    matches!(result, Err(Error::UnsafeFileType(ref path)) if path == std::path::Path::new("destination"))
                );
                assert!(root.join("destination").is_dir());
            } else {
                assert!(matches!(result, Err(Error::DestinationExists(_))));
                assert_eq!(fs::read(root.join("destination")).unwrap(), b"");
            }
        }
        _ => panic!("unknown test stage"),
    }
    if mode == "public" && matches!(stage.as_str(), "file-sync" | "publish") {
        assert_eq!(fs::read(root.join("source")).unwrap(), b"new");
    }
    assert!(root.is_dir());
    assert!(fs::read_dir(&root).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".state-atomic-")
    }));
    println!("\nZXS_ATOMIC_IO_COMPLETE mode={mode} stage={stage}");
}

#[test]
fn recovery_intent_creation_stays_on_held_owner_after_path_replacement() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("state");
    let directory = PrivateDirectory::create(&path).unwrap();
    let moved = temp.path().join("held");
    fs::rename(&path, &moved).unwrap();
    let replacement = PrivateDirectory::create(&path).unwrap();
    let name = EntryName::new("intent").unwrap();
    AtomicFile::create_recovery_intent(&directory, &name, b"complete intent").unwrap();
    assert_eq!(fs::read(moved.join("intent")).unwrap(), b"complete intent");
    assert_eq!(fs::read_dir(replacement.path()).unwrap().count(), 0);
    assert!(matches!(
        AtomicFile::create_recovery_intent(&directory, &name, b"replacement"),
        Err(Error::DestinationExists(_))
    ));
    assert_eq!(fs::read(moved.join("intent")).unwrap(), b"complete intent");
    assert_eq!(fs::read_dir(moved).unwrap().count(), 1);
}

#[test]
fn public_publish_rejects_unsafe_sources_and_cross_directory_names() {
    use std::os::unix::fs::symlink;
    let temp = tempfile::tempdir().unwrap();
    let directory = PrivateDirectory::create(temp.path().join("private")).unwrap();
    fs::write(temp.path().join("outside"), b"original").unwrap();
    symlink(
        temp.path().join("outside"),
        directory.path().join("symlink"),
    )
    .unwrap();
    fs::hard_link(
        temp.path().join("outside"),
        directory.path().join("hardlink"),
    )
    .unwrap();
    fs::create_dir(directory.path().join("nested")).unwrap();
    fs::write(directory.path().join("nested/source"), b"nested").unwrap();
    let destination = RelativePath::new("destination").unwrap();
    for source in ["symlink", "hardlink", "nested/source"] {
        assert!(
            NoClobberPublish::publish(
                &directory,
                &RelativePath::new(source).unwrap(),
                &destination
            )
            .is_err()
        );
        assert!(!directory.path().join("destination").exists());
        assert!(fs::symlink_metadata(directory.path().join(source)).is_ok());
    }
    fs::write(directory.path().join("source"), b"staged").unwrap();
    assert!(
        NoClobberPublish::publish(
            &directory,
            &RelativePath::new("source").unwrap(),
            &RelativePath::new("nested/destination").unwrap()
        )
        .is_err()
    );
    assert_eq!(
        fs::read(directory.path().join("source")).unwrap(),
        b"staged"
    );
    assert_eq!(fs::read(temp.path().join("outside")).unwrap(), b"original");
    assert_eq!(
        fs::read(directory.path().join("nested/source")).unwrap(),
        b"nested"
    );
    assert!(!directory.path().join("nested/destination").exists());
}

#[test]
fn io_error_source_preserves_errno_without_changing_display() {
    use std::error::Error as _;
    let source = io::Error::from_raw_os_error(13);
    let message = source.to_string();
    let error = Error::from(source);
    assert_eq!(error.to_string(), message);
    assert_eq!(
        error
            .source()
            .unwrap()
            .downcast_ref::<io::Error>()
            .unwrap()
            .raw_os_error(),
        Some(13)
    );
}

#[test]
fn available_bytes_uses_the_held_directory_after_its_name_is_replaced() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("private");
    let directory = PrivateDirectory::create(&path).unwrap();
    let moved = temp.path().join("retained");
    fs::rename(&path, &moved).unwrap();
    let missing = temp.path().join("missing-filesystem");
    std::os::unix::fs::symlink(&missing, &path).unwrap();
    // A pathname-based statistic cannot traverse the deliberately broken path.
    // Space changes independently, so compare the held filesystem's geometry
    // rather than requiring two sequential free-space samples to be identical.
    assert!(rustix::fs::statvfs(&path).is_err());
    let capacity = rustix::fs::fstatvfs(directory.directory_handle()).unwrap();
    let available = directory.available_bytes().unwrap();
    assert!(available <= capacity.f_blocks.saturating_mul(capacity.f_frsize));
    assert!(directory.verify_identity().is_err());
    assert_eq!(fs::read_link(&path).unwrap(), missing);
}

#[test]
fn streaming_inventory_uses_independent_held_cursors_and_relative_metadata() {
    use std::{collections::BTreeSet, ffi::OsString, os::unix::ffi::OsStringExt};
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("private");
    let directory = PrivateDirectory::create(&path).unwrap();
    let payload = EntryName::new("payload").unwrap();
    let raw = EntryName::new(OsString::from_vec(vec![b'u', 0xff])).unwrap();
    AtomicFile::create(&directory, &payload, b"old").unwrap();
    AtomicFile::create(&directory, &raw, b"raw").unwrap();
    directory
        .create_child(&EntryName::new("child").unwrap())
        .unwrap();
    let first = directory.entries().unwrap();
    let second = directory.entries().unwrap();
    fs::rename(&path, temp.path().join("retained")).unwrap();
    let replacement = PrivateDirectory::create(&path).unwrap();
    replacement.create_child(&payload).unwrap();
    AtomicFile::create(&replacement, &EntryName::new("unrelated").unwrap(), b"new").unwrap();
    assert_eq!(
        directory.entry_metadata(&payload).unwrap(),
        EntryMetadata::File { bytes: 3 }
    );
    assert_eq!(
        directory.entry_metadata(&raw).unwrap(),
        EntryMetadata::File { bytes: 3 }
    );
    assert_eq!(
        replacement.entry_metadata(&payload).unwrap(),
        EntryMetadata::Directory
    );
    assert!(directory.verify_identity().is_err());
    let expected = BTreeSet::from([payload, raw, EntryName::new("child").unwrap()]);
    let first = first.collect::<Result<BTreeSet<_>, _>>().unwrap();
    assert_eq!(first, expected);
    // The independently owned cursor remains valid after the original owner ends.
    drop(directory);
    assert_eq!(
        second.collect::<Result<BTreeSet<_>, _>>().unwrap(),
        expected
    );
}

#[test]
fn relative_inventory_metadata_rejects_links_special_files_and_missing_entries() {
    use std::os::unix::fs::symlink;
    let temp = tempfile::tempdir().unwrap();
    let directory = PrivateDirectory::create(temp.path().join("private")).unwrap();
    let source = directory.path().join("source");
    fs::write(&source, b"data").unwrap();
    fs::hard_link(&source, directory.path().join("hardlink")).unwrap();
    symlink(&source, directory.path().join("symlink")).unwrap();
    let fifo = EntryName::new("fifo").unwrap();
    rustix::fs::mkfifoat(
        directory.directory_handle(),
        fifo.as_os_str(),
        rustix::fs::Mode::from_raw_mode(0o600),
    )
    .unwrap();
    for name in ["source", "hardlink"] {
        assert!(matches!(
            directory.entry_metadata(&EntryName::new(name).unwrap()),
            Err(Error::MultipleLinks(_))
        ));
    }
    for name in ["symlink", "fifo"] {
        assert!(matches!(
            directory.entry_metadata(&EntryName::new(name).unwrap()),
            Err(Error::UnsafeFileType(_))
        ));
    }
    assert!(
        matches!(directory.entry_metadata(&EntryName::new("missing").unwrap()),
        Err(Error::Io(error)) if error.kind() == io::ErrorKind::NotFound)
    );
    assert_eq!(fs::read(&source).unwrap(), b"data");
    assert!(fs::symlink_metadata(directory.path().join("fifo")).is_ok());
}
