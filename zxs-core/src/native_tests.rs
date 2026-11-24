use super::{Errno, Folder, Mode, fs, measure};
use crate::native_test_support as support;
use rustix::fs::inotify;
use std::{
    ffi::OsStr,
    fs as stdfs,
    io::{Write, sink},
    mem::MaybeUninit,
    os::unix::fs::{PermissionsExt, symlink},
    path::Path,
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

fn permissions(path: &Path, mode: u32) {
    stdfs::set_permissions(path, stdfs::Permissions::from_mode(mode)).unwrap();
}

#[test]
#[ignore = "requires newly allocated unprivileged ext4 fixture scratch"]
fn owned_group_write_applies_only_to_ordinary_paths() {
    let case = support::fixture("ordinary-group-mode");
    let root = case.join("work");
    let nested = root.join("sub");
    stdfs::create_dir(&root).unwrap();
    stdfs::create_dir(&nested).unwrap();
    permissions(&root, 0o775);
    permissions(&nested, 0o775);

    let work = Folder::open(&root, false).unwrap();
    assert!(Folder::open(&root, true).is_err());
    let file = nested.join("work.txt");
    stdfs::write(&file, b"earlier editor bytes").unwrap();
    permissions(&file, 0o664);
    let old = work.source(Path::new("sub/work.txt")).unwrap();
    assert_eq!(old.scan(sink()).unwrap().length, 20);

    // The same ordinary pathname is editable, but a previously reviewed inode
    // is never reused after an editor's temporary-file-and-rename save.
    let renamed = nested.join("editor.tmp");
    stdfs::write(&renamed, b"later editor bytes").unwrap();
    permissions(&renamed, 0o664);
    stdfs::rename(renamed, &file).unwrap();
    assert!(old.scan(sink()).is_err());
    let current = work.source(Path::new("sub/work.txt")).unwrap();
    let mut bytes = Vec::new();
    let fresh = current.scan(&mut bytes).unwrap();
    assert_eq!(bytes, b"later editor bytes");
    assert_eq!(
        stdfs::metadata(&file).unwrap().permissions().mode() & 0o777,
        0o664
    );

    let ignore = root.join(".zxsignore");
    stdfs::write(&ignore, b"ignored.txt\n").unwrap();
    permissions(&ignore, 0o664);
    stdfs::write(nested.join("ignored.txt"), b"excluded ordinary material").unwrap();
    assert!(
        work.names(Path::new("sub"))
            .unwrap()
            .contains(&"work.txt".into())
    );
    let bundle = crate::bundle::Review::read(&root).unwrap();
    assert_eq!(
        bundle.manifest.members.get("sub/work.txt"),
        Some(&Some(fresh))
    );
    assert!(!bundle.manifest.members.contains_key("sub/ignored.txt"));

    let out = case.join("output");
    stdfs::create_dir(&out).unwrap();
    permissions(&out, 0o775);
    let bundle_path = out.join("new.zip");
    let selected = crate::bundle::Destination::new(&bundle, &bundle_path).unwrap();
    selected.publish(&bundle, true).unwrap();
    assert_eq!(
        stdfs::metadata(bundle_path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    group_write_does_not_weaken_publication_or_private_modes(&case, &out);

    permissions(&file, 0o666);
    assert!(work.source(Path::new("sub/work.txt")).is_err());
    permissions(&file, 0o664);
    symlink("sub/work.txt", root.join("alias")).unwrap();
    assert!(work.source(Path::new("alias")).is_err());
    stdfs::hard_link(&file, root.join("hard")).unwrap();
    assert!(work.source(Path::new("sub/work.txt")).is_err());
    stdfs::remove_file(root.join("hard")).unwrap();
    stdfs::remove_file(root.join("alias")).unwrap();
    fs::mkfifoat(
        &work.fd,
        "pipe",
        Mode::RUSR | Mode::WUSR | Mode::RGRP | Mode::WGRP,
    )
    .unwrap();
    assert!(work.source(Path::new("pipe")).is_err());
    stdfs::remove_file(root.join("pipe")).unwrap();

    permissions(&nested, 0o777);
    assert!(work.names(Path::new("sub")).is_err());
    assert!(crate::bundle::Review::read(&root).is_err());
    permissions(&root, 0o777);
    assert!(work.check().is_err());
    assert!(Folder::open(&root, false).is_err());
}

fn group_write_does_not_weaken_publication_or_private_modes(case: &Path, out: &Path) {
    let destination = Folder::open(out, false).unwrap();
    let leaf = OsStr::new("late-name");
    let mut temporary = destination.unlinked().unwrap();
    temporary.write_all(b"must not displace anything").unwrap();
    let body = measure(&temporary, sink()).unwrap();
    let result = destination.publish_guarded(leaf, &temporary, &body, || {
        let now_exists = out.join(leaf);
        stdfs::write(&now_exists, b"keep the late owner entry")?;
        stdfs::set_permissions(&now_exists, stdfs::Permissions::from_mode(0o664))?;
        Ok(())
    });
    assert!(result.is_err());
    let winner = destination.source(Path::new(leaf)).unwrap();
    let mut readback = Vec::new();
    let winner_body = winner.scan(&mut readback).unwrap();
    assert_eq!(readback, b"keep the late owner entry");
    assert!(
        destination
            .remove_verified("late-name", &winner_body, winner.before, || Ok(()))
            .is_err()
    );
    winner.check().unwrap();

    let state = case.join("private");
    stdfs::create_dir(&state).unwrap();
    permissions(&state, 0o700);
    let private = Folder::open(&state, true).unwrap();
    let candidate = state.join("candidate");
    stdfs::write(&candidate, b"not admitted as private bytes").unwrap();
    permissions(&candidate, 0o664);
    assert!(private.source(Path::new("candidate")).is_err());
    assert!(private.lock("candidate").is_err());
    assert_eq!(
        stdfs::read(&candidate).unwrap(),
        b"not admitted as private bytes"
    );
    let frozen = private.unlinked().unwrap();
    assert_eq!(
        frozen.metadata().unwrap().permissions().mode() & 0o777,
        0o600
    );
    permissions(&state, 0o775);
    assert!(private.check().is_err());
    assert!(Folder::open(&state, true).is_err());
}
