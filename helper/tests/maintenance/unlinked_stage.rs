//! Future tests only, on exclusive test directories under two qualified local filesystems.
use super::*;
use crate::pinned::storage_status::Selection;
use crate::test_area::{data_directory, recycle_directory};
use rustix::fs::{FlockOperation::NonBlockingLockShared, flock, fremovexattr};
use std::{
    ffi::OsString,
    fs::{self, File},
    os::unix::fs::{DirBuilderExt, MetadataExt},
    path::{Path, PathBuf},
};

pub(in crate::maintenance) fn roots(parent: &Path, recycle: &Path) -> (FreshRootLocks, PathBuf, PathBuf) {
    let system = parent.join("system");
    let data = parent.join("data");
    for path in [&system, &data] {
        fs::DirBuilder::new().mode(0o700).create(path).unwrap();
    }
    let args = [OsString::from("--system"), system.clone().into_os_string(),
                "--recycle".into(), recycle.as_os_str().into(), "--data".into(), data.clone().into_os_string()];
    let selection = Selection::parse(args.into_iter()).unwrap();
    let uid = fs::metadata(&system).unwrap().uid();
    (FreshRootLocks::try_lock(&selection, uid).expect(
        "needs two supported private volumes and qualified whole-mount ancestry with an observable self-parent root"), system, data)
}

fn unclaimed_and_locked(paths: &[&Path]) {
    for path in paths {
        let original = File::open(path).unwrap();
        assert_eq!(flock(&original, NonBlockingLockShared), Err(Errno::WOULDBLOCK));
        let mut one = [0u8; 1];
        assert_eq!(fgetxattr(&original, c"user.zxs.volume", &mut one[..]), Err(Errno::NODATA));
        assert_eq!(fs::read_dir(path).unwrap().count(), 0);
    }
}

#[test]
fn synchronized_genesis_and_frontier_are_read_from_the_same_still_unnamed_original() {
    let parent = data_directory();
    let recycle = recycle_directory();
    let (mut group, system, data) = roots(parent.path(), recycle.path());
    let proposal = group.prepare_unpublished([0x91; 16]).unwrap();
    let synced = SyncedUnlinked::prepare(proposal).unwrap();
    let fd = &synced.register.held;
    let m = fd.metadata().unwrap();
    assert_eq!(m.nlink(), 0);
    assert_eq!(m.len(), synced.bytes.image.len() as u64);
    assert_eq!(m.uid(), fs::metadata(&system).unwrap().uid());
    assert_eq!(m.mode() & 0o7777, 0o600);
    let mut bytes = vec![0; synced.bytes.image.len()];
    fd.read_exact_at(&mut bytes, 0).unwrap();
    assert_eq!(bytes, synced.bytes.image);
    assert_eq!(fd.read_at(&mut [0], m.len()).unwrap(), 0);
    let mut frontier = [0; 48];
    assert_eq!(fgetxattr(fd, FRONTIER, &mut frontier[..]).unwrap(), 48);
    assert_eq!(frontier, synced.bytes.frontier);
    assert_eq!(&frontier[..16], &[0; 16]);
    assert_eq!(&frontier[16..], &bytes[bytes.len()-32..]);
    assert!(synced.register.fresh_origins().is_err(), "nonempty cannot reenter the fresh writer");
    synced.recheck().unwrap();

    // Retain inode and length while changing one byte. Readback must not repair it.
    fd.write_all_at(&[bytes[0] ^ 1], 0).unwrap();
    assert!(synced.recheck().is_err());
    let mut first = [0];
    fd.read_exact_at(&mut first, 0).unwrap();
    assert_eq!(first, [bytes[0] ^ 1]);
    fd.write_all_at(&bytes[..1], 0).unwrap(); // only our test original, positive control
    synced.recheck().unwrap();

    let mut wrong = frontier;
    wrong[47] ^= 1;
    for value in [&wrong[..], &frontier[..47], &[][..]] {
        fsetxattr(fd, FRONTIER, value, XattrFlags::REPLACE).unwrap();
        assert!(synced.recheck().is_err());
        let mut actual = [0; 48];
        let got = fgetxattr(fd, FRONTIER, &mut actual[..]).unwrap();
        assert_eq!(&actual[..got], value);
    }
    fremovexattr(fd, FRONTIER).unwrap();
    assert!(synced.recheck().is_err(), "a missing frontier is not the zero frontier");
    fsetxattr(fd, FRONTIER, &frontier, XattrFlags::CREATE).unwrap();
    synced.recheck().unwrap();
    fd.write_all_at(b"x", m.len()).unwrap();
    assert!(synced.recheck().is_err(), "an extra tail cannot be accepted or trimmed");
    assert_eq!(fd.metadata().unwrap().len(), m.len() + 1);
    unclaimed_and_locked(&[system.as_path(), recycle.path(), data.as_path()]);
    drop(synced);
    unclaimed_and_locked(&[system.as_path(), recycle.path(), data.as_path()]); // group has its own lifetime
    drop(group);
    for p in [system.as_path(), recycle.path(), data.as_path()] {
        assert_eq!(fs::read_dir(p).unwrap().count(), 0);
        flock(File::open(p).unwrap(), NonBlockingLockShared).unwrap();
    }
}

#[test]
fn a_preexisting_frontier_is_not_overwritten_or_treated_as_missing_before_body_write() {
    let parent = data_directory();
    let recycle = recycle_directory();
    let (mut group, system, data) = roots(parent.path(), recycle.path());
    for existing in [&[][..], &[0x42][..], &[0x43; 48][..]] {
        let proposal = group.prepare_unpublished([0x92; 16]).unwrap();
        let (_group, held, bytes) = proposal.into_checked_parts().unwrap();
        fsetxattr(&held.held, FRONTIER, existing, XattrFlags::CREATE).unwrap(); // fixture-injected
        assert!(write_initial(&held, &bytes).is_err());
        assert_eq!(held.held.metadata().unwrap().len(), 0);
        assert_eq!(held.held.metadata().unwrap().nlink(), 0);
        let mut actual = [0; 48];
        let n = fgetxattr(&held.held, FRONTIER, &mut actual[..]).unwrap();
        assert_eq!(&actual[..n], existing);
        unclaimed_and_locked(&[system.as_path(), recycle.path(), data.as_path()]);
    }
}

#[test]
fn same_length_change_after_complete_content_sample_is_refused_at_finish() {
    use std::time::{Duration, UNIX_EPOCH};
    let data = data_directory();
    let recycle = recycle_directory();
    let (mut group, system, primary) = roots(data.path(), recycle.path());
    let synced = SyncedUnlinked::prepare(group.prepare_unpublished([0x93; 16]).unwrap()).unwrap();
    let genesis = synced.bytes.checked().unwrap();
    let fd = &synced.register.held;
    // Establish a distinct mtime for the positive control even on low-resolution
    // filesystems; no sleep and no injected/mock stat result.
    let first_time = UNIX_EPOCH + Duration::from_secs(1_600_000_000);
    fd.set_modified(first_time).unwrap();
    synced.roots.check_image_roots(&genesis).unwrap();
    let unchanged = synced.sample_original(&genesis).unwrap();
    synced.finish_original(&genesis, unchanged).unwrap();
    let before = synced.sample_original(&genesis).unwrap(); // includes actual EOF and xattr
    let original = fd.metadata().unwrap();
    fd.write_all_at(&[synced.bytes.image[0] ^ 1], 0).unwrap();
    fd.set_modified(first_time + Duration::from_secs(10)).unwrap();
    let now = fd.metadata().unwrap();
    assert_eq!((now.dev(), now.ino(), now.nlink(), now.len()),
               (original.dev(), original.ino(), original.nlink(), original.len()));
    assert_ne!((now.mtime(), now.mtime_nsec()), (original.mtime(), original.mtime_nsec()));
    assert!(synced.finish_original(&genesis, before).is_err());
    assert!(synced.recheck().is_err()); // the next real byte read also finds the altered byte
    unclaimed_and_locked(&[system.as_path(), recycle.path(), primary.as_path()]);
}

// Exercise an actual staged original after the test-only first name and cold open.
#[path = "pending.rs"]
mod pending;
