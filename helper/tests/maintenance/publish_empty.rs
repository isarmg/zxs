//! Future isolated tests. Losing a caller or dropping a held result is not a
//! power cut. No test starts a service or changes host credentials.
use super::*;
use crate::maintenance::{
    genesis::FreshGenesis,
    pending::ColdPending,
    register_flock::staging::{SyncedUnlinked, first_name::fixture_first, fixture_roots},
    root_set::FreshRootLocks,
};
use crate::test_area::{data_directory, recycle_directory};
use rustix::{fs::{FlockOperation::NonBlockingLockShared, XattrFlags, fgetxattr, flock, fsetxattr}, io::Errno};
use std::{ffi::OsString, fs::{self, File}, os::unix::fs::{FileExt, MetadataExt, OpenOptionsExt}, path::Path};

const OP: [u8; 16] = [0xDE; 16];
fn selected(s: &Path, r: &Path, d: &Path) -> Selection {
    Selection::parse([OsString::from("--system"), s.as_os_str().into(), "--recycle".into(),
        r.as_os_str().into(), "--data".into(), d.as_os_str().into()].into_iter()).unwrap()
}
fn first<'r>(group: &'r mut FreshRootLocks, worker: &WorkerPolicy) -> SyncedLabels<'r> {
    let stage = SyncedUnlinked::prepare(group.prepare_unpublished(OP).unwrap()).unwrap();
    PendingRootSet::from_first(fixture_first(stage, OP)).unwrap().apply_labels(OP, worker).unwrap()
}
fn attr(file: &File, name: &std::ffi::CStr) -> Vec<u8> {
    let mut buf = [0u8;160]; let n = fgetxattr(file, name, &mut buf[..]).unwrap(); buf[..n].to_vec()
}
fn confirm_existing(s: &Path, uid: u32, selected: &Selection, worker: &WorkerPolicy) -> io::Result<ConfirmedEmpty<'static>> {
    ConfirmedEmpty::confirm_existing(ColdEmptyReady::inspect(s, uid, OP)?, selected, OP, worker)
}

#[test]
fn publishes_and_cold_confirms_same_initial_inode_all_exact_labels_and_the_real_ex_lifetime() {
    let base = data_directory(); let r = recycle_directory();
    let (mut group, s, d) = fixture_roots(base.path(), r.path());
    let worker = WorkerPolicy::fixture(); let uid = fs::metadata(&s).unwrap().uid();
    let pending = s.join("INITIALIZING"); let ready = s.join("REGISTRATION");
    let all = first(&mut group, &worker);
    let independent = File::open(&pending).unwrap(); let inode = independent.metadata().unwrap().ino();
    let bytes = fs::read(&pending).unwrap();
    let frontier = attr(&independent, c"user.zxs.confirmed-frontier");
    let receipt = all.publish(OP, &worker).unwrap(); receipt.recheck().unwrap();
    assert!(!pending.exists()); assert_eq!(fs::read_dir(&s).unwrap().count(), 1);
    assert_eq!(fs::metadata(&ready).unwrap().ino(), inode); assert_eq!(fs::read(&ready).unwrap(), bytes);
    let g = FreshGenesis::decode(&bytes).unwrap(); g.check_initial_frontier(Some(&frontier)).unwrap();
    assert_eq!(attr(&File::open(&ready).unwrap(), c"user.zxs.confirmed-frontier"), frontier);
    for (i,p) in [s.as_path(), r.path(), d.as_path()].into_iter().enumerate() {
        let fd = File::open(p).unwrap(); g.check_label(i, &attr(&fd,c"user.zxs.volume")).unwrap();
        assert_eq!(flock(&fd, NonBlockingLockShared), Err(Errno::WOULDBLOCK));
    }
    assert_eq!(flock(&independent, NonBlockingLockShared), Err(Errno::WOULDBLOCK));
    assert_eq!(ColdEmptyReady::inspect(&s, uid, OP).err().unwrap().kind(), io::ErrorKind::WouldBlock);
    drop(receipt); drop(group);
    let selection = selected(&s, r.path(), &d);
    let after = confirm_existing(&s, uid, &selection, &worker).unwrap(); after.recheck().unwrap();
    assert_eq!(flock(&independent, NonBlockingLockShared), Err(Errno::WOULDBLOCK));
    assert_eq!(fs::metadata(&ready).unwrap().ino(), inode); assert_eq!(fs::read(&ready).unwrap(), bytes);
    drop(after); flock(&independent, NonBlockingLockShared).unwrap();
}

#[test]
fn wrong_operation_and_an_occupied_ready_name_do_not_publish_replace_or_remove_anything() {
    let base = data_directory(); let r = recycle_directory();
    let (mut group, s, d) = fixture_roots(base.path(), r.path());
    let worker = WorkerPolicy::fixture(); let uid = fs::metadata(&s).unwrap().uid();
    let pending = s.join("INITIALIZING"); let ready = s.join("REGISTRATION");
    let all = first(&mut group, &worker); let before = fs::read(&pending).unwrap();
    let ino = fs::metadata(&pending).unwrap().ino();
    assert!(matches!(all.publish([0xDF;16], &worker), Err(PublishError::NotAttempted(_))));
    assert!(!ready.exists()); drop(group);
    let cold = PendingRootSet::inspect(ColdPending::inspect(&s, uid, OP).unwrap(), &selected(&s,r.path(),&d)).unwrap();
    let all = cold.apply_labels(OP, &worker).unwrap();
    fs::write(&ready, b"unrelated occupant").unwrap(); let other = fs::metadata(&ready).unwrap().ino();
    assert!(matches!(all.publish(OP, &worker), Err(PublishError::NotAttempted(_))));
    assert_eq!(fs::metadata(&pending).unwrap().ino(), ino); assert_eq!(fs::read(&pending).unwrap(), before);
    assert_eq!(fs::metadata(&ready).unwrap().ino(), other); assert_eq!(fs::read(&ready).unwrap(), b"unrelated occupant");
    assert!(ColdEmptyReady::inspect(&s, uid, OP).is_err());
}

#[test]
fn visible_rename_without_return_can_only_be_confirmed_as_exact_ready_and_never_repairs_a_bad_volume() {
    let base = data_directory(); let r = recycle_directory();
    let (mut group, s, d) = fixture_roots(base.path(), r.path());
    let worker = WorkerPolicy::fixture(); let uid = fs::metadata(&s).unwrap().uid();
    let pending = s.join("INITIALIZING"); let ready = s.join("REGISTRATION");
    let all = first(&mut group, &worker); let ino = fs::metadata(&pending).unwrap().ino();
    let bytes = fs::read(&pending).unwrap();
    let renamed = all.rename_original(OP, &worker).unwrap();
    assert!(!pending.exists()); assert_eq!(fs::metadata(&ready).unwrap().ino(), ino);
    drop(renamed); drop(group); // no parent confirmation; do not infer what survived a power loss
    let selection = selected(&s, r.path(), &d);
    assert!(ColdPending::inspect(&s, uid, OP).is_err());
    let rf = File::open(r.path()).unwrap(); let df = File::open(&d).unwrap();
    let original_r = attr(&rf,c"user.zxs.volume"); let original_d = attr(&df,c"user.zxs.volume");
    fsetxattr(&df,c"user.zxs.volume",&original_r,XattrFlags::REPLACE).unwrap(); // foreign role in this fixture
    ColdEmptyReady::inspect(&s, uid, OP).unwrap().recheck().unwrap(); // S-only query does not open or repair D
    assert!(confirm_existing(&s, uid, &selection, &worker).is_err());
    assert_eq!(attr(&df,c"user.zxs.volume"), original_r); assert_eq!(fs::read(&ready).unwrap(), bytes);
    fsetxattr(&df,c"user.zxs.volume",&original_d,XattrFlags::REPLACE).unwrap(); // restore only our fixture
    let other_op = ColdEmptyReady::inspect(&s, uid, OP).unwrap();
    assert!(ConfirmedEmpty::confirm_existing(other_op,&selection,[0xE0;16],&worker).is_err());
    let current = confirm_existing(&s, uid, &selection, &worker).unwrap(); current.recheck().unwrap();
    assert_eq!(fs::metadata(&ready).unwrap().ino(), ino); assert!(!pending.exists());
}

#[test]
fn substituted_ready_after_rename_is_not_accepted_moved_back_or_cleaned() {
    let base = data_directory(); let r = recycle_directory();
    let (mut group, s, _d) = fixture_roots(base.path(), r.path());
    let worker = WorkerPolicy::fixture();
    let pending = s.join("INITIALIZING"); let ready = s.join("REGISTRATION");
    let all = first(&mut group, &worker);
    let bytes = fs::read(&pending).unwrap(); let original = File::open(&pending).unwrap();
    let frontier = attr(&original,c"user.zxs.confirmed-frontier");
    let renamed = all.rename_original(OP,&worker).unwrap();
    let kept = base.path().join("our-original"); fs::rename(&ready, &kept).unwrap();
    let alternate = fs::OpenOptions::new().create_new(true).read(true).write(true).mode(0o600).open(&ready).unwrap();
    alternate.write_all_at(&bytes,0).unwrap(); fsetxattr(&alternate,c"user.zxs.confirmed-frontier",&frontier,XattrFlags::CREATE).unwrap();
    assert_ne!(alternate.metadata().unwrap().ino(),original.metadata().unwrap().ino());
    assert!(renamed.confirm().is_err());
    assert_eq!(fs::read(&kept).unwrap(), bytes); assert_eq!(fs::read(&ready).unwrap(), bytes);
    assert!(!pending.exists());
}

#[test]
fn a_partially_labelled_original_can_finish_by_cold_handoff_without_changing_the_register_inode_or_releasing_its_new_ex() {
    let base = data_directory(); let r = recycle_directory();
    let (mut group, s, d) = fixture_roots(base.path(), r.path());
    let worker = WorkerPolicy::fixture(); let uid = fs::metadata(&s).unwrap().uid();
    let stage = SyncedUnlinked::prepare(group.prepare_unpublished(OP).unwrap()).unwrap();
    let mut warm = PendingRootSet::from_first(fixture_first(stage, OP)).unwrap();
    let expected = warm.prepare_labels(OP, &worker).unwrap();
    warm.apply_one(0, &expected[0], &worker).unwrap();
    let pending = s.join("INITIALIZING"); let ready = s.join("REGISTRATION");
    let independent = File::open(&pending).unwrap(); let inode = independent.metadata().unwrap().ino();
    let bytes = fs::read(&pending).unwrap();
    for p in [r.path(), d.as_path()] {
        assert_eq!(fgetxattr(File::open(p).unwrap(), c"user.zxs.volume", &mut [0u8;160][..]), Err(Errno::NODATA));
    }
    drop(warm); drop(group); // prior process no longer owns the register or volume locks
    let selection = selected(&s,r.path(),&d);
    let cold = PendingRootSet::inspect(ColdPending::inspect(&s,uid,OP).unwrap(),&selection).unwrap();
    let all = cold.apply_labels(OP,&worker).unwrap();
    assert_eq!(flock(&independent,NonBlockingLockShared),Err(Errno::WOULDBLOCK));
    let renamed = all.rename_original(OP,&worker).unwrap();
    assert!(!pending.exists()); assert_eq!(fs::metadata(&ready).unwrap().ino(),inode);
    assert_eq!(flock(&independent,NonBlockingLockShared),Err(Errno::WOULDBLOCK));
    let confirmed = renamed.confirm().unwrap(); confirmed.recheck().unwrap();
    let fresh_reader = File::open(&ready).unwrap();
    assert_eq!(flock(&fresh_reader,NonBlockingLockShared),Err(Errno::WOULDBLOCK));
    for (i,p) in [s.as_path(),r.path(),d.as_path()].into_iter().enumerate() {
        assert_eq!(attr(&File::open(p).unwrap(),c"user.zxs.volume"),expected[i]);
    }
    assert_eq!(fs::read(&ready).unwrap(),bytes);
    drop(confirmed); flock(&fresh_reader,NonBlockingLockShared).unwrap();
}
