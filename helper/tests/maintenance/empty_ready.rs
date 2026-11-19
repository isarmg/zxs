//! Read-only observation of an empty published original. Construction, rename
//! and deliberate changes below are confined to the independent test fixture.
use super::*;
use crate::maintenance::pending::ColdEmptyReady;
use rustix::fs::{RenameFlags, renameat_with};

fn published_fixture(group: &mut FreshRootLocks, s: &Path, r: &Path, d: &Path) -> Fixture {
    let original = name_original(group, s);
    for (p, label) in [s, r, d].into_iter().zip(&original.labels) {
        let dir = File::open(p).unwrap();
        fsetxattr(&dir, c"user.zxs.volume", label, XattrFlags::CREATE).unwrap();
        dir.sync_all().unwrap();
    }
    let system = File::open(s).unwrap();
    renameat_with(&system, c"INITIALIZING", &system, c"REGISTRATION", RenameFlags::NOREPLACE).unwrap();
    system.sync_all().unwrap();
    original
}

#[test]
fn cold_ready_reads_only_the_selected_original_without_resuming_or_assuming_the_other_volumes_are_available() {
    let base = data_directory(); let r = recycle_directory();
    let (mut group, s, d) = roots(base.path(), r.path());
    let original = published_fixture(&mut group, &s, r.path(), &d); drop(group);
    fs::rename(&d, base.path().join("data-now-unavailable")).unwrap();
    let ready = s.join("REGISTRATION"); let fd = File::open(&ready).unwrap();
    let cold = ColdEmptyReady::inspect(&s, original.uid, OP).unwrap();
    cold.recheck().unwrap();
    assert_eq!(flock(&fd, NonBlockingLockShared), Err(Errno::WOULDBLOCK));
    assert!(!s.join("INITIALIZING").exists()); assert!(!d.exists());
    assert_eq!(fs::read(&ready).unwrap(), original.bytes);
    drop(cold);
    flock(&fd, NonBlockingLockShared).unwrap(); drop(fd);
    assert!(ColdEmptyReady::inspect(&s, original.uid, [0xD1;16]).is_err());
    assert!(ColdPending::inspect(&s, original.uid, OP).is_err());
    ColdEmptyReady::inspect(&s, original.uid, OP).unwrap().recheck().unwrap();
}

#[test]
fn cold_ready_refuses_missing_or_wrong_system_label_dual_names_and_later_bytes_without_changes() {
    let base = data_directory(); let r = recycle_directory();
    let (mut group, s, d) = roots(base.path(), r.path());
    let o = published_fixture(&mut group, &s, r.path(), &d); drop(group);
    let read = || ColdEmptyReady::inspect(&s, o.uid, OP);
    let sf = File::open(&s).unwrap(); let ready = s.join("REGISTRATION");
    read().unwrap().recheck().unwrap();
    fremovexattr(&sf, c"user.zxs.volume").unwrap();
    assert!(read().is_err());
    assert_eq!(fgetxattr(&sf, c"user.zxs.volume", &mut [0u8;160][..]), Err(Errno::NODATA));
    let mut wrong = o.labels[0].clone(); *wrong.last_mut().unwrap() ^= 1;
    fsetxattr(&sf, c"user.zxs.volume", &wrong, XattrFlags::CREATE).unwrap();
    assert!(read().is_err()); let mut buf = [0u8;160];
    let n = fgetxattr(&sf, c"user.zxs.volume", &mut buf[..]).unwrap(); assert_eq!(&buf[..n], &wrong[..]);
    fsetxattr(&sf, c"user.zxs.volume", &o.labels[0], XattrFlags::REPLACE).unwrap();
    fs::write(s.join("INITIALIZING"), b"unknown entry").unwrap(); assert!(read().is_err());
    assert_eq!(fs::read(s.join("INITIALIZING")).unwrap(), b"unknown entry");
    fs::remove_file(s.join("INITIALIZING")).unwrap(); // our own fixture entry
    read().unwrap().recheck().unwrap();
    let fd = fs::OpenOptions::new().read(true).write(true).open(&ready).unwrap();
    fd.write_all_at(b"later record not interpreted", o.bytes.len() as u64).unwrap();
    assert_eq!(read().err().unwrap().kind(), io::ErrorKind::Unsupported);
    assert_eq!(fs::read(&ready).unwrap(), [o.bytes.as_slice(), b"later record not interpreted"].concat());
    let mut frontier = [0;48];
    assert_eq!(fgetxattr(&fd, FRONTIER, &mut frontier[..]).unwrap(), 48);
    assert_eq!(frontier, o.frontier);
    // The same genuine original can subsequently exceed the bounded empty
    // image. Record only "unsupported", preserving both ends and exact size.
    let end = crate::maintenance::genesis::MAX_IMAGE as u64;
    fd.write_all_at(b"X", end).unwrap();
    let before = fd.metadata().unwrap();
    assert_eq!(read().err().unwrap().kind(), io::ErrorKind::Unsupported);
    let after = fd.metadata().unwrap();
    assert_eq!((after.ino(), after.len(), after.mtime(), after.mtime_nsec(), after.ctime(), after.ctime_nsec()),
        (before.ino(), end + 1, before.mtime(), before.mtime_nsec(), before.ctime(), before.ctime_nsec()));
    let mut first = vec![0; o.bytes.len()]; fd.read_exact_at(&mut first, 0).unwrap(); assert_eq!(first, o.bytes);
    let mut last = [0]; fd.read_exact_at(&mut last, end).unwrap(); assert_eq!(&last, b"X");
}

#[test]
fn neither_existing_ready_observer_nor_new_open_accepts_identical_bytes_on_a_replacement_inode() {
    let base = data_directory(); let r = recycle_directory();
    let (mut group, s, d) = roots(base.path(), r.path());
    let o = published_fixture(&mut group, &s, r.path(), &d); drop(group);
    let known = ColdEmptyReady::inspect(&s, o.uid, OP).unwrap(); known.recheck().unwrap();
    let ready = s.join("REGISTRATION"); let saved = base.path().join("saved-original");
    fs::rename(&ready, &saved).unwrap();
    let copy = fs::OpenOptions::new().read(true).write(true).create_new(true).mode(0o600).open(&ready).unwrap();
    copy.write_all_at(&o.bytes, 0).unwrap();
    fsetxattr(&copy, FRONTIER, &o.frontier, XattrFlags::CREATE).unwrap();
    assert_ne!(copy.metadata().unwrap().ino(), fs::metadata(&saved).unwrap().ino());
    assert!(known.recheck().is_err()); assert!(ColdEmptyReady::inspect(&s, o.uid, OP).is_err());
    assert_eq!(fs::read(&saved).unwrap(), o.bytes); assert_eq!(fs::read(&ready).unwrap(), o.bytes);
}
