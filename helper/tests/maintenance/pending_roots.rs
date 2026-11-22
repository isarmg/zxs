//! The first-name fixture is authentic. No test here publishes READY or releases a root on the caller's behalf.
use super::*;
use crate::maintenance::root_set::PendingRootSet;
use rustix::fs::FlockOperation::NonBlockingLockExclusive;

fn selected(system: &Path, recycle: &Path, data: &Path) -> Selection {
    let a = [OsString::from("--system"), system.as_os_str().into(), "--recycle".into(),
             recycle.as_os_str().into(), "--data".into(), data.as_os_str().into()];
    Selection::parse(a.into_iter()).unwrap()
}
fn observe(system: &Path, uid: u32, selection: &Selection) -> io::Result<PendingRootSet<'static>> {
    PendingRootSet::inspect(ColdPending::inspect(system, uid, OP)?, selection)
}
fn no_tag(dir: &File) {
    let mut buf = [0; 1];
    assert_eq!(fgetxattr(dir, c"user.zxs.volume", &mut buf[..]), Err(Errno::NODATA));
}
fn available(path: &Path) { flock(File::open(path).unwrap(), NonBlockingLockExclusive).unwrap(); }

#[test]
fn partial_original_labels_require_all_locks_and_do_not_turn_into_automatic_completion() {
    let parent = data_directory(); let r = recycle_directory();
    let (mut group, s, d) = roots(parent.path(), r.path());
    let original = name_original(&mut group, &s); drop(group);
    let sf = File::open(&s).unwrap(); let rf = File::open(r.path()).unwrap(); let df = File::open(&d).unwrap();
    fsetxattr(&rf, c"user.zxs.volume", &original.labels[1], XattrFlags::CREATE).unwrap();
    let selection = selected(&s, r.path(), &d);
    let observed = observe(&s, original.uid, &selection).unwrap();
    observed.recheck().unwrap();
    for path in [s.as_path(), r.path(), d.as_path(), &s.join("INITIALIZING")] {
        assert_eq!(flock(File::open(path).unwrap(), NonBlockingLockShared), Err(Errno::WOULDBLOCK));
    }
    no_tag(&sf); no_tag(&df);
    let mut actual = [0; 160]; let got = fgetxattr(&rf, c"user.zxs.volume", &mut actual[..]).unwrap();
    assert_eq!(&actual[..got], original.labels[1].as_slice());
    // Emulate an uncooperative change after taking the first complete sample.
    // The old observer must not upgrade an originally missing label, even to an exact one.
    fsetxattr(&df, c"user.zxs.volume", &original.labels[2], XattrFlags::CREATE).unwrap();
    assert!(observed.recheck().is_err()); no_tag(&sf);
    drop(observed); observe(&s, original.uid, &selection).unwrap().recheck().unwrap();
    no_tag(&sf); assert_eq!(fs::read(s.join("INITIALIZING")).unwrap(), original.bytes);
    assert!(!s.join("REGISTRATION").exists());
}

#[test]
fn a_late_foreign_row_is_preserved_and_no_earlier_volume_is_touched() {
    let parent = data_directory(); let r = recycle_directory();
    let (mut group, s, d) = roots(parent.path(), r.path());
    let original = name_original(&mut group, &s); drop(group);
    let selection = selected(&s, r.path(), &d);
    let sf = File::open(&s).unwrap(); let rf = File::open(r.path()).unwrap(); let df = File::open(&d).unwrap();
    let evidence = d.join("not-ours");
    fs::write(&evidence, b"unchanged").unwrap();
    assert!(observe(&s, original.uid, &selection).is_err());
    assert_eq!(fs::read(&evidence).unwrap(), b"unchanged"); no_tag(&sf); no_tag(&rf); no_tag(&df);
    fs::remove_file(&evidence).unwrap(); // only our own fixture
    for foreign in [&original.labels[1][..], &[][..]] {
        fsetxattr(&df, c"user.zxs.volume", foreign, XattrFlags::CREATE).unwrap();
        assert!(observe(&s, original.uid, &selection).is_err());
        let mut b = [0; 160];
        let n = fgetxattr(&df, c"user.zxs.volume", &mut b[..]).unwrap();
        assert_eq!(&b[..n], foreign); no_tag(&sf); no_tag(&rf);
        fremovexattr(&df, c"user.zxs.volume").unwrap();
    }
    let ancestor = File::open(parent.path()).unwrap();
    fsetxattr(&ancestor, c"user.zxs.volume", &original.labels[2], XattrFlags::CREATE).unwrap();
    assert!(observe(&s, original.uid, &selection).is_err(), "even an authentic leaf label is forbidden at an ancestor");
    no_tag(&sf); no_tag(&rf); no_tag(&df);
    assert_eq!(fs::read(s.join("INITIALIZING")).unwrap(), original.bytes);
}

#[test]
fn one_busy_root_or_a_replaced_named_directory_cannot_be_skipped_and_all_acquired_locks_close() {
    let parent = data_directory(); let r = recycle_directory();
    let (mut group, s, d) = roots(parent.path(), r.path());
    let original = name_original(&mut group, &s); drop(group);
    let selection = selected(&s, r.path(), &d);
    let held = File::open(r.path()).unwrap();
    flock(&held, NonBlockingLockExclusive).unwrap();
    assert_eq!(observe(&s, original.uid, &selection).err().unwrap().kind(), io::ErrorKind::WouldBlock);
    for path in [s.as_path(), d.as_path(), &s.join("INITIALIZING")] { available(path); }
    assert_eq!(flock(File::open(r.path()).unwrap(), NonBlockingLockShared), Err(Errno::WOULDBLOCK));
    drop(held);
    observe(&s, original.uid, &selection).unwrap().recheck().unwrap();

    // An unselected, absent spelling should be refused as an original-request mismatch,
    // *before* attempting to open the path and returning NotFound.
    let unrelated = selected(&s, r.path(), &parent.path().join("not-requested"));
    assert_eq!(observe(&s, original.uid, &unrelated).err().unwrap().kind(), io::ErrorKind::InvalidInput);
    let parked = parent.path().join("old-data");
    fs::rename(&d, &parked).unwrap(); fs::DirBuilder::new().mode(0o700).create(&d).unwrap();
    assert!(observe(&s, original.uid, &selection).is_err());
    for path in [s.as_path(), r.path(), d.as_path(), parked.as_path(), &s.join("INITIALIZING")] { available(path); }
    assert_eq!(fs::read_dir(&d).unwrap().count(), 0);
    assert_eq!(fs::read_dir(&parked).unwrap().count(), 0);
    assert_eq!(fs::read(s.join("INITIALIZING")).unwrap(), original.bytes);
}

#[test]
fn cold_system_and_group_row_must_agree_on_the_same_mount_and_label_observation() {
    use crate::maintenance::identity::ObservedOriginal;
    let parent = data_directory(); let r = recycle_directory();
    let (mut group, s, _d) = roots(parent.path(), r.path());
    let original = name_original(&mut group, &s); drop(group);
    let sf = File::open(&s).unwrap();
    let cold = ColdPending::inspect(&s, original.uid, OP).unwrap();
    let mut row = ObservedOriginal::root(&sf, original.uid).unwrap();
    cold.check_system_observation(&row, false).unwrap();
    assert!(cold.check_system_observation(&row, true).is_err(), "missing versus exact label is a different observation");
    let mount = row.at.mount;
    row.at.mount = if mount == 1 { 2 } else { 1 }; // comparison-only: same native birth/entity, different mount
    assert!(cold.check_system_observation(&row, false).is_err());
    drop(cold);
    fsetxattr(&sf, c"user.zxs.volume", &original.labels[0], XattrFlags::CREATE).unwrap();
    let labelled = ColdPending::inspect(&s, original.uid, OP).unwrap();
    row.at.mount = mount;
    labelled.check_system_observation(&row, true).unwrap();
    assert!(labelled.check_system_observation(&row, false).is_err());
}
