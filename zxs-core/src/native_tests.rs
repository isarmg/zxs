use super::{Errno, Folder, Mode, fs, measure};
use crate::native_test_support as support;
use rustix::fs::inotify;
use std::{
    ffi::OsStr,
    fs as stdfs,
    io::{Write, sink},
    mem::MaybeUninit,
    os::unix::fs::{PermissionsExt, symlink},
};

#[test]
#[ignore = "requires newly allocated unprivileged ext4 fixture scratch"]
fn private_lease_proves_type_before_data_open() {
    let root = support::fixture("lease");
    let folder = Folder::open(&root, true).unwrap();
    let new = folder.lock("lease-normal").unwrap();
    let first = new.before;
    let busy = match folder.lock("lease-normal") {
        Ok(_) => panic!("another process description acquired the held lease"),
        Err(error) => error,
    };
    assert_eq!(busy.downcast_ref::<Errno>(), Some(&Errno::WOULDBLOCK));
    drop(new);
    let reopened = folder.lock("lease-normal").unwrap();
    reopened.check().unwrap();
    assert_eq!(reopened.before, first);

    let sentinel = root.join("ordinary");
    stdfs::write(&sentinel, b"synthetic ordinary content").unwrap();
    stdfs::set_permissions(&sentinel, stdfs::Permissions::from_mode(0o600)).unwrap();
    stdfs::hard_link(&sentinel, root.join("lease-hard")).unwrap();
    symlink("ordinary", root.join("lease-sym")).unwrap();
    stdfs::write(root.join("lease-mode"), b"wrong mode").unwrap();
    stdfs::set_permissions(
        root.join("lease-mode"),
        stdfs::Permissions::from_mode(0o640),
    )
    .unwrap();
    fs::mkfifoat(&folder.fd, "lease-fifo", Mode::RUSR | Mode::WUSR).unwrap();
    stdfs::create_dir(root.join("lease-dir")).unwrap();

    let watch =
        inotify::init(inotify::CreateFlags::NONBLOCK | inotify::CreateFlags::CLOEXEC).unwrap();
    inotify::add_watch(&watch, &root, inotify::WatchFlags::OPEN).unwrap();
    let mut buffer = [MaybeUninit::uninit(); 512];
    let mut events = inotify::Reader::new(watch, &mut buffer);
    // A real data open must register. The forbidden O_PATH probes below must not.
    drop(stdfs::File::open(&sentinel).unwrap());
    assert_eq!(
        events.next().unwrap().file_name().unwrap().to_bytes(),
        b"ordinary"
    );
    for name in [
        "lease-fifo",
        "lease-dir",
        "lease-sym",
        "lease-hard",
        "lease-mode",
    ] {
        assert!(folder.lock(name).is_err(), "admitted {name}");
    }
    assert!(
        matches!(events.next(), Err(Errno::AGAIN)),
        "at least one rejected lease acquired a data-open descriptor"
    );
    assert_eq!(
        stdfs::read(sentinel).unwrap(),
        b"synthetic ordinary content"
    );
}

#[test]
#[ignore = "requires newly allocated unprivileged ext4 fixture scratch"]
fn final_native_link_refuses_a_post_check_occupant() {
    let root = support::fixture("link");
    let folder = Folder::open(&root, true).unwrap();
    let leaf = OsStr::new("collision.zip");
    folder.absent(leaf).unwrap();
    let mut staged = folder.unlinked().unwrap();
    staged.write_all(b"losing anonymous object").unwrap();
    staged.sync_all().unwrap();
    let before = measure(&staged, sink()).unwrap();

    // Deterministically occupy the name AFTER the preflight and completed body.
    let mut occupied = folder.new_private("collision.zip").unwrap();
    occupied.write_all(b"keep this exact occupant").unwrap();
    occupied.sync_all().unwrap();
    let identity = folder.lease_stamp(&occupied).unwrap();
    let refused = folder
        .link_unpublished(leaf, &staged, || Ok(()))
        .unwrap_err();
    assert_eq!(refused.downcast_ref::<Errno>(), Some(&Errno::EXIST));
    let winner = folder.source(std::path::Path::new(leaf)).unwrap();
    assert_eq!(winner.before, identity);
    assert_eq!(
        stdfs::read(root.join(leaf)).unwrap(),
        b"keep this exact occupant"
    );
    assert_eq!(measure(&staged, sink()).unwrap(), before);
}

#[test]
#[ignore = "requires newly allocated unprivileged ext4 fixture scratch"]
fn final_native_link_rechecks_held_lease() {
    let root = support::fixture("lease-link");
    let folder = Folder::open(&root, true).unwrap();
    let lease = folder.lock("lease-original").unwrap();
    let mut staged = folder.unlinked().unwrap();
    staged
        .write_all(b"must not escape after lease replacement")
        .unwrap();
    let body = measure(&staged, sink()).unwrap();
    let leaf = OsStr::new("unpublished");
    let refused = folder.publish_guarded(leaf, &staged, &body, || {
        lease.check()?;
        stdfs::rename(root.join("lease-original"), root.join("detached")).unwrap();
        let replacement = folder.lock("lease-original").unwrap();
        replacement.check()?;
        lease.check()
    });
    assert!(refused.is_err());
    folder.absent(leaf).unwrap();
}
