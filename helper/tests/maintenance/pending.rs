//! Isolated, source-only cold observation of a test-created first name. Production never creates it here.
use super::*;
use crate::maintenance::pending::ColdPending;
use rustix::fs::{AtFlags, linkat};
use std::os::unix::fs::OpenOptionsExt;

struct Fixture { bytes: Vec<u8>, frontier: [u8; 48], system_label: Vec<u8>, uid: u32 }
const OP: [u8; 16] = [0x8A; 16];

fn name_original(group: &mut FreshRootLocks, system: &Path) -> Fixture {
    let staged = SyncedUnlinked::prepare(group.prepare_unpublished(OP).unwrap()).unwrap();
    let fixture = Fixture { bytes: staged.bytes.image.clone(), frontier: staged.bytes.frontier,
        system_label: staged.bytes.labels[0].clone(), uid: staged.register.held.metadata().unwrap().uid() };
    linkat(&staged.register.held, c"", staged.register.system, c"INITIALIZING", AtFlags::EMPTY_PATH)
        .expect("requires permission to name our very own private anonymous test inode");
    staged.register.system.sync_all().unwrap();
    assert_eq!(ColdPending::inspect(system, fixture.uid, OP).err().unwrap().kind(), io::ErrorKind::WouldBlock,
        "an independently opened description cannot obtain a second EX while the original is held");
    fixture
}

#[test]
fn cold_pending_observes_only_its_original_system_and_never_claims_or_opens_unselected_roots() {
    let parent = data_directory(); let recycle = recycle_directory();
    let (mut group, s, d) = roots(parent.path(), recycle.path());
    let original = name_original(&mut group, &s); drop(group);
    let file = File::open(s.join("INITIALIZING")).unwrap(); let sys = File::open(&s).unwrap();
    fs::rename(&d, parent.path().join("not-at-the-recorded-spelling")).unwrap();
    let cold = ColdPending::inspect(&s, original.uid, OP).unwrap();
    cold.recheck().unwrap(); // remaining saved paths deliberately NOT opened or pronounced sound
    assert_eq!(flock(&file, NonBlockingLockShared), Err(Errno::WOULDBLOCK));
    let mut marker = [0u8; 160];
    assert_eq!(fgetxattr(&sys, c"user.zxs.volume", &mut marker[..]), Err(Errno::NODATA));
    // A non-cooperating test writer changes state under our EX. An old observation
    // does not absorb it, even when its exact label could be valid on a fresh cold read.
    fsetxattr(&sys, c"user.zxs.volume", &original.system_label, XattrFlags::CREATE).unwrap();
    assert!(cold.recheck().is_err());
    drop(cold);
    ColdPending::inspect(&s, original.uid, OP).unwrap().recheck().unwrap();
    let n = fgetxattr(&sys, c"user.zxs.volume", &mut marker[..]).unwrap();
    assert_eq!(&marker[..n], original.system_label.as_slice());
    assert_eq!(fs::read(s.join("INITIALIZING")).unwrap(), original.bytes);
    assert!(!s.join("REGISTRATION").exists());
    assert!(!d.exists());
}

#[test]
fn cold_pending_rejects_wrong_request_foreign_contents_and_frontier_without_repair() {
    let parent = data_directory(); let recycle = recycle_directory();
    let (mut group, s, _d) = roots(parent.path(), recycle.path());
    let original = name_original(&mut group, &s); drop(group);
    let path = s.join("INITIALIZING"); let sys = File::open(&s).unwrap();
    let fd = fs::OpenOptions::new().read(true).write(true).open(&path).unwrap();
    let check = || ColdPending::inspect(&s, original.uid, OP);
    check().unwrap().recheck().unwrap();
    assert!(ColdPending::inspect(&s, original.uid.wrapping_add(1), OP).is_err());
    assert!(ColdPending::inspect(&s, original.uid, [0x8B; 16]).is_err());
    for name in ["REGISTRATION", "unknown"] {
        fs::write(s.join(name), b"leave exactly this").unwrap();
        assert!(check().is_err());
        assert_eq!(fs::read(s.join(name)).unwrap(), b"leave exactly this");
        fs::remove_file(s.join(name)).unwrap(); // only our own fixture entries
    }
    for label in [&[][..], &[0xFF][..]] {
        fsetxattr(&sys, c"user.zxs.volume", label, XattrFlags::CREATE).unwrap();
        assert!(check().is_err());
        let mut buf = [0u8; 3]; let n = fgetxattr(&sys, c"user.zxs.volume", &mut buf[..]).unwrap();
        assert_eq!(&buf[..n], label);
        fremovexattr(&sys, c"user.zxs.volume").unwrap();
    }
    fremovexattr(&fd, FRONTIER).unwrap();
    assert!(check().is_err());
    let mut raw = [0; 48];
    assert_eq!(fgetxattr(&fd, FRONTIER, &mut raw[..]), Err(Errno::NODATA));
    let mut wrong = original.frontier; wrong[26] ^= 1;
    fsetxattr(&fd, FRONTIER, &wrong, XattrFlags::CREATE).unwrap();
    assert!(check().is_err()); assert_eq!(fgetxattr(&fd, FRONTIER, &mut raw[..]).unwrap(), 48);
    assert_eq!(raw, wrong);
    fsetxattr(&fd, FRONTIER, &original.frontier, XattrFlags::REPLACE).unwrap();
    check().unwrap().recheck().unwrap();
    fd.write_all_at(b"x", original.bytes.len() as u64).unwrap();
    assert!(check().is_err()); assert_eq!(fs::read(path).unwrap(), [original.bytes.as_slice(), b"x"].concat());
}

#[test]
fn byte_identical_name_replacement_never_rebinds_the_original_pending_or_its_lease() {
    let parent = data_directory(); let recycle = recycle_directory();
    let (mut group, s, _d) = roots(parent.path(), recycle.path());
    let original = name_original(&mut group, &s); drop(group);
    let observed = ColdPending::inspect(&s, original.uid, OP).unwrap();
    let p = s.join("INITIALIZING"); let parked = parent.path().join("parked-original");
    fs::rename(&p, &parked).unwrap();
    let new = fs::OpenOptions::new().read(true).write(true).create_new(true).mode(0o600).open(&p).unwrap();
    new.write_all_at(&original.bytes, 0).unwrap();
    fsetxattr(&new, FRONTIER, &original.frontier, XattrFlags::CREATE).unwrap();
    assert_ne!(fs::metadata(&parked).unwrap().ino(), new.metadata().unwrap().ino());
    assert!(observed.recheck().is_err());
    assert!(ColdPending::inspect(&s, original.uid, OP).is_err(),
        "a valid self-consistent byte string and frontier do not authenticate the copied inode");
    let old = File::open(&parked).unwrap();
    assert_eq!(flock(&old, NonBlockingLockShared), Err(Errno::WOULDBLOCK));
    drop(observed);
    flock(&old, NonBlockingLockShared).unwrap();
    assert_eq!(fs::read(&p).unwrap(), original.bytes);
    assert_eq!(fs::read(&parked).unwrap(), original.bytes);
}
