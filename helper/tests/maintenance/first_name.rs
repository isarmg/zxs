//! Source controls for the first persistent name, not a power-loss simulation or a public command.
//! Needs an isolated, non-root no-new-privileges storage worker with no capabilities and two
//! qualified filesystems. An unavailable prerequisite is not a passing result.
use super::*;
use crate::maintenance::register_flock::staging::tests::roots;
use crate::test_area::{data_directory, recycle_directory};
use rustix::fs::{FlockOperation::NonBlockingLockShared, XattrFlags, fgetxattr, flock, fsetxattr};
use rustix::io::Errno;
use std::{fs, os::unix::fs::{FileExt, MetadataExt, OpenOptionsExt}};

const OP: [u8; 16] = [0xB1; 16];

#[test]
fn first_name_keeps_original_register_ex_and_unclaimed_volume_locks_until_handoff() {
    let parent = data_directory(); let recycle = recycle_directory();
    let (mut group, s, d) = roots(parent.path(), recycle.path());
    let uid = fs::metadata(&s).unwrap().uid();
    let stage = SyncedUnlinked::prepare(group.prepare_unpublished(OP).unwrap()).unwrap();
    let original = stage.register.held.metadata().unwrap();
    let bytes = stage.bytes.image.clone();
    let pending = stage.name_initializing(OP, &WorkerPolicy::fixture()).expect("requires qualified restricted worker and kernel support for direct O_TMPFILE first link");
    pending.recheck().unwrap();
    let name = s.join("INITIALIZING");
    let independent = File::open(&name).unwrap();
    let named = independent.metadata().unwrap();
    assert_eq!((named.ino(), named.dev(), named.nlink(), original.nlink()), (original.ino(), original.dev(), 1, 0));
    assert_eq!(fs::read(&name).unwrap(), bytes);
    assert_eq!(flock(&independent, NonBlockingLockShared), Err(Errno::WOULDBLOCK));
    for path in [s.as_path(), recycle.path(), d.as_path()] {
        let dir = File::open(path).unwrap();
        assert_eq!(flock(&dir, NonBlockingLockShared), Err(Errno::WOULDBLOCK));
        assert_eq!(fgetxattr(&dir, c"user.zxs.volume", &mut [0u8; 1][..]), Err(Errno::NODATA));
    }
    assert!(!s.join("REGISTRATION").exists());
    drop(pending); drop(group);
    ColdPending::inspect(&s, uid, OP).unwrap().recheck().unwrap();
    assert!(ColdPending::inspect(&s, uid, [0xB2; 16]).is_err());
}

#[test]
fn wrong_operation_or_known_occupant_never_replaces_or_writes_a_first_name() {
    let parent = data_directory(); let recycle = recycle_directory();
    let (mut group, s, _d) = roots(parent.path(), recycle.path());
    let stage = SyncedUnlinked::prepare(group.prepare_unpublished(OP).unwrap()).unwrap();
    assert!(matches!(stage.name_initializing([0xB3; 16], &WorkerPolicy::fixture()), Err(NameError::NotAttempted(_))));
    assert_eq!(fs::read_dir(&s).unwrap().count(), 0);
    let stage = SyncedUnlinked::prepare(group.prepare_unpublished(OP).unwrap()).unwrap();
    let name = s.join("INITIALIZING");
    fs::write(&name, b"belongs to another operation").unwrap();
    let was = fs::metadata(&name).unwrap().ino();
    assert!(matches!(stage.name_initializing(OP, &WorkerPolicy::fixture()), Err(NameError::NotAttempted(_))));
    assert_eq!(fs::metadata(&name).unwrap().ino(), was);
    assert_eq!(fs::read(&name).unwrap(), b"belongs to another operation");
    assert!(!s.join("REGISTRATION").exists());
}

#[test]
fn losing_the_original_name_after_link_never_adopts_or_cleans_an_identical_substitute() {
    let parent = data_directory(); let recycle = recycle_directory();
    let (mut group, s, _d) = roots(parent.path(), recycle.path());
    let uid = fs::metadata(&s).unwrap().uid();
    let stage = SyncedUnlinked::prepare(group.prepare_unpublished(OP).unwrap()).unwrap();
    let bytes = stage.bytes.image.clone(); let frontier = stage.bytes.frontier;
    let linked = stage.link_first(OP, &WorkerPolicy::fixture()).expect("requires the restricted first-name worker");
    let name = s.join("INITIALIZING"); let parked = parent.path().join("our-original");
    fs::rename(&name, &parked).unwrap();
    let other = fs::OpenOptions::new().create_new(true).read(true).write(true).mode(0o600).open(&name).unwrap();
    other.write_all_at(&bytes, 0).unwrap();
    fsetxattr(&other, c"user.zxs.confirmed-frontier", &frontier, XattrFlags::CREATE).unwrap();
    assert_ne!(fs::metadata(&parked).unwrap().ino(), other.metadata().unwrap().ino());
    assert!(linked.confirm().is_err());
    assert_eq!(fs::read(&parked).unwrap(), bytes);
    assert_eq!(fs::read(&name).unwrap(), bytes);
    assert!(!s.join("REGISTRATION").exists());
    drop(group);
    assert!(ColdPending::inspect(&s, uid, OP).is_err());
}
