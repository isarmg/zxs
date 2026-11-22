//! Kernel-backed future cases only in dedicated temporary data and recycle locations under
//! the restricted test worker. Dropping a guard models process interruption, not power failure.
use super::*;
use crate::{
    maintenance::{
        pending::ColdPending,
        register_flock::staging::{SyncedUnlinked, first_name::fixture_first, fixture_roots},
    },
    pinned::storage_status::Selection,
    test_area::{data_directory, recycle_directory},
};
use rustix::{fs::{FlockOperation::NonBlockingLockShared, fgetxattr, flock}, io::Errno};
use std::{ffi::OsString, fs::{self, File}, os::unix::fs::MetadataExt, path::Path};

const OP: [u8; 16] = [0xC1; 16];
fn selection(s: &Path, r: &Path, d: &Path) -> Selection {
    Selection::parse([OsString::from("--system"), s.as_os_str().into(), "--recycle".into(), r.as_os_str().into(), "--data".into(), d.as_os_str().into()].into_iter()).unwrap()
}
fn label(p: &Path) -> Option<Vec<u8>> {
    let mut buf = [0; 160];
    match fgetxattr(File::open(p).unwrap(), c"user.zxs.volume", &mut buf[..]) {
        Ok(n) => Some(buf[..n].to_vec()),
        Err(Errno::NODATA) => None,
        Err(e) => panic!("cannot inspect explicit original fixture volume: {e}"),
    }
}

#[test]
fn warm_first_label_then_cold_same_pending_finishes_all_original_labels_and_never_publishes_ready() {
    let parent = data_directory(); let r = recycle_directory();
    let (mut group, s, d) = fixture_roots(parent.path(), r.path());
    let policy = WorkerPolicy::fixture(); let uid = fs::metadata(&s).unwrap().uid();
    let stage = SyncedUnlinked::prepare(group.prepare_unpublished(OP).unwrap()).unwrap();
    let first = fixture_first(stage, OP);
    let mut warm = PendingRootSet::from_first(first).unwrap();
    let before = fs::read(s.join("INITIALIZING")).unwrap(); let was = fs::metadata(s.join("INITIALIZING")).unwrap().ino();
    let expected = warm.prepare_labels(OP, &policy).unwrap();
    warm.apply_one(0, &expected[0], &policy).unwrap();
    assert_eq!(label(&s).as_deref(), Some(expected[0].as_slice()));
    assert_eq!(label(r.path()), None); assert_eq!(label(&d), None);
    assert!(!s.join("REGISTRATION").exists());
    drop(warm); drop(group); // partial original survives; no automatic completion
    let selected = selection(&s, r.path(), &d);
    let open = || PendingRootSet::inspect(ColdPending::inspect(&s, uid, OP).unwrap(), &selected).unwrap();
    let labelled = open().apply_labels(OP, &policy).unwrap();
    labelled.recheck().unwrap();
    for (i,path) in [s.as_path(), r.path(), d.as_path()].into_iter().enumerate() {
        assert_eq!(label(path).as_deref(), Some(expected[i].as_slice()));
        assert_eq!(flock(File::open(path).unwrap(), NonBlockingLockShared), Err(Errno::WOULDBLOCK));
    }
    assert_eq!(flock(File::open(s.join("INITIALIZING")).unwrap(), NonBlockingLockShared), Err(Errno::WOULDBLOCK));
    drop(labelled);
    // The exact existing labels may be resynchronized, never replaced. A
    // filesystem's ctime is not our proof of label contents; reread the attributes.
    let ctimes = || [s.as_path(), r.path(), d.as_path()].map(|p| {
        let m = fs::metadata(p).unwrap(); (m.ctime(), m.ctime_nsec())
    });
    let t = ctimes();
    open().apply_labels(OP, &policy).unwrap().recheck().unwrap();
    assert_eq!(ctimes(), t);
    assert_eq!(fs::metadata(s.join("INITIALIZING")).unwrap().ino(), was);
    assert_eq!(fs::read(s.join("INITIALIZING")).unwrap(), before);
    assert!(!s.join("REGISTRATION").exists());
}

#[test]
fn wrong_operation_or_late_foreign_data_prevents_every_first_label_and_preserves_the_source() {
    let parent = data_directory(); let r = recycle_directory();
    let (mut group, s, d) = fixture_roots(parent.path(), r.path());
    let policy = WorkerPolicy::fixture(); let uid = fs::metadata(&s).unwrap().uid();
    let fresh = SyncedUnlinked::prepare(group.prepare_unpublished(OP).unwrap()).unwrap();
    let warm = PendingRootSet::from_first(fixture_first(fresh, OP)).unwrap();
    assert!(warm.apply_labels([0xAB; 16], &policy).is_err());
    drop(group);
    let selected = selection(&s, r.path(), &d);
    let cold = PendingRootSet::inspect(ColdPending::inspect(&s, uid, OP).unwrap(), &selected).unwrap();
    let image = fs::read(s.join("INITIALIZING")).unwrap();
    let foreign = d.join("untouched");
    fs::write(&foreign, b"original material").unwrap();
    assert!(cold.apply_labels(OP, &policy).is_err());
    for p in [s.as_path(), r.path(), d.as_path()] { assert_eq!(label(p), None); }
    assert_eq!(fs::read(&foreign).unwrap(), b"original material");
    assert_eq!(fs::read(s.join("INITIALIZING")).unwrap(), image);
    assert!(!s.join("REGISTRATION").exists());
}

#[test]
fn after_a_real_completed_row_a_foreign_claim_is_never_replaced_or_rolled_back() {
    let parent = data_directory(); let r = recycle_directory();
    let (mut group, s, d) = fixture_roots(parent.path(), r.path());
    let policy = WorkerPolicy::fixture(); let uid = fs::metadata(&s).unwrap().uid();
    let stage = SyncedUnlinked::prepare(group.prepare_unpublished(OP).unwrap()).unwrap();
    let mut warm = PendingRootSet::from_first(fixture_first(stage, OP)).unwrap();
    let labels = warm.prepare_labels(OP, &policy).unwrap();
    warm.apply_one(0, &labels[0], &policy).unwrap();
    let rf = File::open(r.path()).unwrap();
    fsetxattr(&rf, c"user.zxs.volume", b"another original", XattrFlags::CREATE).unwrap();
    assert!(warm.apply_one(1, &labels[1], &policy).is_err());
    drop(warm); drop(group);
    let selected = selection(&s, r.path(), &d);
    assert!(PendingRootSet::inspect(ColdPending::inspect(&s, uid, OP).unwrap(), &selected).is_err());
    assert_eq!(label(&s).as_deref(), Some(labels[0].as_slice()));
    assert_eq!(label(r.path()).as_deref(), Some(&b"another original"[..]));
    assert_eq!(label(&d), None);
    assert!(s.join("INITIALIZING").is_file()); assert!(!s.join("REGISTRATION").exists());
}
