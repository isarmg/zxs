use super::*;
use std::{
    fs,
    os::{
        fd::AsRawFd,
        unix::{fs::symlink, net::UnixListener},
    },
};

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
