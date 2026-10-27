use super::*;
use std::{
    fs,
    os::{
        fd::AsRawFd,
        unix::{fs::symlink, net::UnixListener},
    },
};

#[test]
fn same_mount_private_leaf_open_keeps_the_held_parent_and_file_constraints() {
    let temporary = tempfile::tempdir().unwrap();
    let address = temporary.path().join("private");
    let private = PrivateDirectory::create(&address).unwrap();
    fs::write(address.join("file"), b"held").unwrap();
    let moved = temporary.path().join("moved");
    fs::rename(&address, &moved).unwrap();
    PrivateDirectory::create(&address).unwrap();
    fs::write(address.join("file"), b"replacement").unwrap();
    let mut file = open_private_regular(&private, &EntryName::new("file").unwrap()).unwrap();
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut file, &mut bytes).unwrap();
    assert_eq!(bytes, b"held");

    symlink("file", moved.join("link")).unwrap();
    assert!(open_private_regular(&private, &EntryName::new("link").unwrap()).is_err());
    fs::hard_link(moved.join("file"), moved.join("hard")).unwrap();
    assert!(matches!(
        open_private_regular(&private, &EntryName::new("file").unwrap()),
        Err(Error::MultipleLinks(_))
    ));
    assert!(matches!(
        open_private_regular(&private, &EntryName::new("missing").unwrap()),
        Err(Error::Io(ref source)) if source.kind() == io::ErrorKind::NotFound
    ));
    assert_eq!(fs::read(address.join("file")).unwrap(), b"replacement");
}

#[test]
fn same_mount_private_leaf_open_preserves_administrative_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("private");
    PrivateDirectory::create(&path).unwrap();
    let private = PrivateDirectory::open_for_administration(&path).unwrap();
    let name = EntryName::new("file").unwrap();
    let file = path.join(name.as_path());
    fs::write(&file, b"bytes").unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
    open_private_regular(&private, &name).unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o400)).unwrap();
    assert!(matches!(
        open_private_regular(&private, &name),
        Err(Error::UnsafePermissions(_))
    ));
    assert_eq!(
        fs::metadata(file).unwrap().permissions().mode() & 0o777,
        0o400
    );
}

#[test]
fn admitted_directory_conversion_keeps_original_handle_after_path_rebinding() {
    for mounts in [MountPolicy::SameMount, MountPolicy::AllowNestedMounts] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("root");
        let private = PrivateDirectory::create(&path).unwrap();
        fs::write(path.join("file"), b"original").unwrap();
        let descriptor = private.directory_handle().as_raw_fd();
        let identity = FileIdentity::from_fd(private.directory_handle()).unwrap();

        let moved = temp.path().join("moved");
        fs::rename(&path, &moved).unwrap();
        let replacement = PrivateDirectory::create(&path).unwrap();
        fs::write(path.join("file"), b"replacement").unwrap();

        let root = OpenAt2Root::from_private_directory(private, mounts).unwrap();
        assert_eq!(root.directory().as_raw_fd(), descriptor);
        assert_eq!(root.identity(), identity);
        assert_eq!(
            root.identity(),
            FileIdentity::from_metadata(&fs::metadata(&moved).unwrap())
        );
        assert_ne!(
            root.identity(),
            FileIdentity::from_fd(replacement.directory_handle()).unwrap()
        );
        let expected = ResolveFlags::BENEATH | ResolveFlags::NO_MAGICLINKS;
        assert_eq!(
            root.resolve,
            match mounts {
                MountPolicy::SameMount => expected | ResolveFlags::NO_XDEV,
                MountPolicy::AllowNestedMounts => expected,
            }
        );
        let mut file = root
            .open_file(&RelativePath::new("file").unwrap(), false)
            .unwrap();
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut file, &mut bytes).unwrap();
        assert_eq!(bytes, b"original");
        assert_eq!(fs::read(path.join("file")).unwrap(), b"replacement");
    }
}

#[test]
fn directory_constructor_rejects_regular_files_for_both_mount_policies() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("file");
    fs::write(&path, b"original").unwrap();
    for mounts in [MountPolicy::SameMount, MountPolicy::AllowNestedMounts] {
        let file = File::open(&path).unwrap();
        assert!(matches!(
            OpenAt2Root::from_directory(file, mounts),
            Err(Error::UnsafeFileType(_))
        ));
    }
    assert_eq!(fs::read(path).unwrap(), b"original");
}

#[test]
fn rejects_root_and_intermediate_links_and_all_non_regular_files() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("file"), b"original").unwrap();
    fs::create_dir(temp.path().join("dir")).unwrap();
    symlink(temp.path(), temp.path().join("alias")).unwrap();
    assert!(OpenAt2Root::open(&temp.path().join("alias"), MountPolicy::SameMount).is_err());
    let root = OpenAt2Root::open(temp.path(), MountPolicy::SameMount).unwrap();
    symlink("file", temp.path().join("link")).unwrap();
    fs::hard_link(temp.path().join("file"), temp.path().join("hard")).unwrap();
    rustix::fs::mknodat(
        root.directory(),
        "fifo",
        FileType::Fifo,
        Mode::from_raw_mode(0o600),
        0,
    )
    .unwrap();
    let _socket = UnixListener::bind(temp.path().join("socket")).unwrap();
    for name in [
        "dir",
        "link",
        "hard",
        "file",
        "alias/file",
        "fifo",
        "socket",
    ] {
        assert!(
            root.open_file(&RelativePath::new(name).unwrap(), false)
                .is_err(),
            "{name}"
        );
    }
    assert_eq!(fs::read(temp.path().join("file")).unwrap(), b"original");
}
#[test]
fn root_identity_and_exclusive_lock_survive_directory_rebinding() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("root");
    fs::create_dir(&path).unwrap();
    fs::write(path.join("file"), b"old").unwrap();
    let root = OpenAt2Root::open(&path, MountPolicy::AllowNestedMounts).unwrap();
    let lock = AdvisoryLock::directory(root.directory()).unwrap();
    let second = OpenAt2Root::open(&path, MountPolicy::AllowNestedMounts).unwrap();
    assert!(AdvisoryLock::directory(second.directory()).is_err());
    let moved = temp.path().join("moved");
    fs::rename(&path, &moved).unwrap();
    fs::create_dir(&path).unwrap();
    fs::write(path.join("file"), b"new").unwrap();
    assert_eq!(
        root.identity(),
        FileIdentity::from_metadata(&fs::metadata(moved).unwrap())
    );
    let mut old = root
        .open_file(&RelativePath::new("file").unwrap(), false)
        .unwrap();
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut old, &mut bytes).unwrap();
    assert_eq!(bytes, b"old");
    drop(lock);
    drop(root);
    AdvisoryLock::directory(second.directory()).unwrap();
}

#[test]
fn directory_lock_ownership_is_independent_of_anchor_handles() {
    let root = tempfile::tempdir().unwrap();
    let anchor = std::fs::File::open(root.path()).unwrap();
    let retained_anchor = anchor.try_clone().unwrap();
    let lock = AdvisoryLock::directory(&anchor).unwrap();
    assert!(matches!(
        AdvisoryLock::directory(&anchor),
        Err(Error::AlreadyLocked(_))
    ));
    drop(lock);
    let next = AdvisoryLock::directory(&anchor).unwrap();
    drop(anchor);
    drop(retained_anchor);
    let contender = std::fs::File::open(root.path()).unwrap();
    assert!(matches!(
        AdvisoryLock::directory(&contender),
        Err(Error::AlreadyLocked(_))
    ));
    drop(next);
    AdvisoryLock::directory(&contender).unwrap();
}

#[test]
fn descendant_directory_and_enumeration_keep_the_held_root_after_rebinding() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("root");
    fs::create_dir_all(path.join("nested/child")).unwrap();
    fs::write(path.join("nested/child/original"), b"held").unwrap();
    let root = OpenAt2Root::open(&path, MountPolicy::SameMount).unwrap();
    let child = RelativePath::new("nested/child").unwrap();
    let identity = FileIdentity::from_fd(root.open_directory(&child).unwrap()).unwrap();

    let moved = temp.path().join("moved");
    fs::rename(&path, &moved).unwrap();
    fs::create_dir_all(path.join("nested/child")).unwrap();
    fs::write(path.join("nested/child/replacement"), b"new").unwrap();

    assert_eq!(
        FileIdentity::from_fd(root.open_directory(&child).unwrap()).unwrap(),
        identity
    );
    assert_ne!(
        FileIdentity::from_metadata(&fs::metadata(path.join("nested/child")).unwrap()),
        identity
    );
    assert_eq!(
        root.entries_at(&child)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap(),
        vec![EntryName::new("original").unwrap()]
    );
    assert_eq!(
        root.entries()
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap(),
        vec![EntryName::new("nested").unwrap()]
    );
    assert_eq!(
        fs::read(path.join("nested/child/replacement")).unwrap(),
        b"new"
    );
}

#[test]
fn directory_open_and_enumeration_reject_links_and_non_directories() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir_all(temp.path().join("nested/child")).unwrap();
    fs::write(temp.path().join("file"), b"original").unwrap();
    symlink("nested", temp.path().join("alias")).unwrap();
    symlink("child", temp.path().join("nested/link")).unwrap();
    let root = OpenAt2Root::open(temp.path(), MountPolicy::SameMount).unwrap();
    rustix::fs::mknodat(
        root.directory(),
        "fifo",
        FileType::Fifo,
        Mode::from_raw_mode(0o600),
        0,
    )
    .unwrap();
    for path in ["alias", "alias/child", "nested/link", "file", "fifo"] {
        let path = RelativePath::new(path).unwrap();
        assert!(root.open_directory(&path).is_err(), "{path:?}");
        assert!(root.entries_at(&path).is_err(), "{path:?}");
    }
    root.open_directory(&RelativePath::new("nested/child").unwrap())
        .unwrap();
}

#[test]
fn directory_enumerators_have_independent_cursors_even_with_path_only_root() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join("nested")).unwrap();
    fs::write(temp.path().join("alpha"), b"a").unwrap();
    fs::write(temp.path().join("beta"), b"b").unwrap();
    fs::write(temp.path().join("nested/child"), b"c").unwrap();
    let file = File::from(
        openat(
            rustix::fs::CWD,
            temp.path(),
            OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .unwrap(),
    );
    let root = OpenAt2Root::from_directory(file, MountPolicy::SameMount).unwrap();
    let expected = ["alpha", "beta", "nested"]
        .map(|name| EntryName::new(name).unwrap())
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();
    let mut first = root.entries().unwrap();
    let first_name = first.next().unwrap().unwrap();
    assert_eq!(
        root.entries()
            .unwrap()
            .collect::<Result<std::collections::BTreeSet<_>, _>>()
            .unwrap(),
        expected
    );
    let mut rest = first
        .collect::<Result<std::collections::BTreeSet<_>, _>>()
        .unwrap();
    assert!(rest.insert(first_name));
    assert_eq!(rest, expected);
    let nested = RelativePath::new("nested").unwrap();
    for _ in 0..2 {
        assert_eq!(
            root.entries_at(&nested)
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap(),
            vec![EntryName::new("child").unwrap()]
        );
    }
}

#[test]
fn descendant_directory_access_preserves_same_mount_policy() {
    let proc = RelativePath::new("proc").unwrap();
    let confined = OpenAt2Root::open(Path::new("/"), MountPolicy::SameMount).unwrap();
    assert!(matches!(
        confined.open_directory(&proc),
        Err(Error::Io(error)) if error.raw_os_error() == Some(rustix::io::Errno::XDEV.raw_os_error())
    ));
    assert!(confined.entries_at(&proc).is_err());
    let nested = OpenAt2Root::open(Path::new("/"), MountPolicy::AllowNestedMounts).unwrap();
    nested.open_directory(&proc).unwrap();
    nested.entries_at(&proc).unwrap();
}
