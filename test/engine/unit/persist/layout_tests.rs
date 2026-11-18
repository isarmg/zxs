use crate::disk::DiskRoot;
use crate::persist::layout;
use crate::{Config, Engine};
use platform_io::fs_safety::{AdvisoryLock, PrivateDirectory};
use std::{
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt, symlink},
    path::Path,
};

fn private(path: &Path) {
    fs::create_dir(path).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn initialization_and_recovery_bind_the_fresh_roots_and_preserve_their_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let system = temp.path().join("system");
    let disk = temp.path().join("disk");
    let config = Config::development(crate::test_disks([disk.clone()])).unwrap();
    let engine = Engine::initialize(&system, config).unwrap();
    let histories = layout::histories(&disk);
    assert_eq!(histories, disk.join("common-data"));
    assert!(!disk.join("histories").exists());
    let history_root = fs::metadata(&histories).unwrap();
    assert!(history_root.is_dir());
    let history_identity = (history_root.dev(), history_root.ino());
    let seal = fs::read(disk.join("disk.json")).unwrap();
    let journal = fs::read(disk.join("metadata/application-log/journal.bin")).unwrap();
    assert!(!seal.is_empty());
    assert_eq!(layout::DISK_IDENTITY.as_path(), Path::new("disk.json"));
    assert_eq!(layout::JOURNAL.as_path(), Path::new("journal.bin"));
    for path in [system.join("writer.lock"), disk.join(".writer.lock")] {
        assert_eq!(fs::read(&path).unwrap(), b"");
        assert_eq!(fs::metadata(path).unwrap().mode() & 0o777, 0o600);
    }
    drop(engine);

    let _reopened = Engine::open(&system).unwrap();
    let reopened_history = fs::metadata(layout::histories(&disk)).unwrap();
    assert_eq!(
        (reopened_history.dev(), reopened_history.ino()),
        history_identity,
    );
    assert_eq!(fs::read(disk.join("disk.json")).unwrap(), seal);
    assert_eq!(
        fs::read(disk.join("metadata/application-log/journal.bin")).unwrap(),
        journal,
    );
    drop(_reopened);
    let mut old_seal: json_codec::Value = crate::decode_strict(&seal).unwrap();
    old_seal["format"] = json_codec::Value::String(
        "container-current-name-managed-directory".into(),
    );
    let old_bytes = crate::canonical_json(&old_seal).unwrap();
    fs::write(disk.join("disk.json"), &old_bytes).unwrap();
    assert_eq!(Engine::open(&system).err().unwrap().code(), crate::ErrorCode::DiskIdentityMismatch);
    assert_eq!(fs::read(disk.join("disk.json")).unwrap(), old_bytes);
    fs::write(disk.join("disk.json"), &seal).unwrap();
    let restored = Engine::open(&system).unwrap();
    assert!(!restored.is_read_only());
}

#[test]
fn control_and_disk_writer_locks_keep_independent_names_and_ownership() {
    let temp = tempfile::tempdir().unwrap();
    let directory = PrivateDirectory::create(temp.path().join("locks")).unwrap();
    let control = AdvisoryLock::acquire(
        &directory,
        &layout::CONTROL_WRITER_LOCK.as_relative(),
    )
    .unwrap();
    let disk = AdvisoryLock::acquire(&directory, &layout::DISK_WRITER_LOCK.as_relative()).unwrap();
    let control_path = directory.path().join("writer.lock");
    let disk_path = directory.path().join(".writer.lock");
    assert_ne!(
        fs::metadata(&control_path).unwrap().ino(),
        fs::metadata(&disk_path).unwrap().ino(),
    );
    assert!(AdvisoryLock::acquire(&directory, &layout::CONTROL_WRITER_LOCK.as_relative()).is_err());
    assert!(AdvisoryLock::acquire(&directory, &layout::DISK_WRITER_LOCK.as_relative()).is_err());
    drop(control);
    let _control = AdvisoryLock::acquire(
        &directory,
        &layout::CONTROL_WRITER_LOCK.as_relative(),
    )
    .unwrap();
    assert!(AdvisoryLock::acquire(&directory, &layout::DISK_WRITER_LOCK.as_relative()).is_err());
    drop(disk);
    assert!(control_path.is_file());
    assert!(disk_path.is_file());
}

#[test]
fn typed_entry_reads_and_writes_stay_on_the_held_directory_after_rename() {
    let temp = tempfile::tempdir().unwrap();
    let named = temp.path().join("root");
    let retained = temp.path().join("retained");
    private(&named);
    let root = DiskRoot::open(&named).unwrap();
    root.write_entry_json(&layout::DISK_IDENTITY, &17u64, false).unwrap();
    fs::rename(&named, &retained).unwrap();
    private(&named);
    fs::write(named.join("disk.json"), b"99").unwrap();
    fs::set_permissions(named.join("disk.json"), fs::Permissions::from_mode(0o600)).unwrap();

    assert_eq!(root.read_entry_bytes(&layout::DISK_IDENTITY, 64).unwrap(), b"17");
    let original = fs::metadata(retained.join("disk.json")).unwrap().ino();
    assert_eq!(
        root.open_regular_entry(&layout::DISK_IDENTITY).unwrap().metadata().unwrap().ino(),
        original,
    );
    assert_eq!(
        root.open_read_entry(&layout::DISK_IDENTITY).unwrap().metadata().unwrap().ino(),
        original,
    );
    assert_eq!(
        root.open_optional_read_entry(&layout::DISK_IDENTITY)
            .unwrap()
            .unwrap()
            .metadata()
            .unwrap()
            .ino(),
        original,
    );
    root.write_entry_json(&layout::DISK_IDENTITY, &18u64, true).unwrap();
    assert_eq!(fs::read(retained.join("disk.json")).unwrap(), b"18");
    assert_eq!(fs::read(named.join("disk.json")).unwrap(), b"99");
}

#[test]
fn typed_entry_openers_preserve_absence_link_rejection_and_read_limits() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("root");
    private(&path);
    let root = DiskRoot::open(&path).unwrap();
    assert!(root.open_optional_read_entry(&layout::DISK_IDENTITY).unwrap().is_none());
    let outside = temp.path().join("outside");
    fs::write(&outside, b"preserved").unwrap();
    fs::set_permissions(&outside, fs::Permissions::from_mode(0o600)).unwrap();
    symlink(&outside, path.join("disk.json")).unwrap();
    assert!(root.open_regular_entry(&layout::DISK_IDENTITY).is_err());
    assert!(root.open_read_entry(&layout::DISK_IDENTITY).is_err());
    assert!(root.open_optional_read_entry(&layout::DISK_IDENTITY).is_err());
    assert!(root.write_entry_json(&layout::DISK_IDENTITY, &17u64, false).is_err());
    assert_eq!(fs::read(&outside).unwrap(), b"preserved");
    fs::remove_file(path.join("disk.json")).unwrap();
    fs::hard_link(&outside, path.join("disk.json")).unwrap();
    assert!(root.open_regular_entry(&layout::DISK_IDENTITY).is_err());
    assert!(root.open_optional_read_entry(&layout::DISK_IDENTITY).is_err());
    fs::remove_file(path.join("disk.json")).unwrap();
    root.write_entry_json(&layout::DISK_IDENTITY, &17u64, false).unwrap();
    assert!(root.read_entry_bytes(&layout::DISK_IDENTITY, 1).is_err());
    assert_eq!(fs::read(path.join("disk.json")).unwrap(), b"17");
}

#[test]
fn ordinary_namespace_reserves_a_complete_component_not_the_archive_extension() {
    use name_policy::PortablePath;
    for path in ["report.zxs", "Folder.ZXS/bytes.zip", "Demo/ARchive.ZxS", ".zxs-other"] {
        assert!(crate::persist::layout::namespace::ordinary_path(&PortablePath::parse(path).unwrap()));
    }
    for path in [".zxs", "Folder/.ZxS/internal", "folder/.ZXS"] {
        assert!(!crate::persist::layout::namespace::ordinary_path(&PortablePath::parse(path).unwrap()));
    }
}
