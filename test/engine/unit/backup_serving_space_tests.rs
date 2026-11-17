use super::*;
use crate::ErrorCode;
use std::{fs, os::unix::fs::PermissionsExt};

#[test]
fn ordinary_pending_allocation_rounds_every_file_and_counts_distinct_directories_and_controls() {
    let unit = NonZeroU64::new(65_536).unwrap();
    let build = || {
        let mut work = Demand::at(unit, 2).unwrap();
        for bytes in [0, 1, unit.get() + 1] { work.missing_file(bytes).unwrap(); }
        let mut controls = Needed::default();
        controls.charge(EntryCost::File(9), unit).unwrap();
        controls.charge(EntryCost::File(0), unit).unwrap();
        (work, controls)
    };
    // Two dirs (4 units), three regular files (1+2+3), largest rounded
    // scratch (2), two missing records (2+1); plus original caller reserve.
    let exact = 15 * unit.get() + 13;
    let check = |bytes, inodes| {
        let (work, controls) = build();
        work.require_counts(&Space { unit, bytes, inodes }, controls, 3, 13, 0)
    };
    check(exact, Some(7)).unwrap();
    assert_eq!(check(exact, Some(6)).unwrap_err().code(), ErrorCode::NoSpace);
    assert_eq!(check(exact - 1, Some(7)).unwrap_err().code(), ErrorCode::NoSpace);
    check(exact, None).unwrap();
}

#[test]
fn partial_serving_keeps_original_scratch_metadata_and_small_block_floors() {
    let unit = NonZeroU64::new(1024).unwrap();
    let check = |available| {
        let mut work = Demand::at(unit, 0).unwrap();
        work.missing_file(1025).unwrap();
        work.require_counts(&Space { unit, bytes: available, inodes: Some(1) }, Needed::default(),
            50, 17, 33 * 1024 * 1024)
    };
    let exact = 50 * 8192 + 1025 * 2 + 17 + 33 * 1024 * 1024;
    check(exact).unwrap();
    assert_eq!(check(exact - 1).unwrap_err().code(), ErrorCode::NoSpace);
}

#[test]
fn ordinary_root_records_charge_only_this_held_target_and_never_adopt_linked_material() {
    let temp = tempfile::Builder::new().permissions(fs::Permissions::from_mode(0o700))
        .tempdir().unwrap();
    let root = DiskRoot::open(temp.path()).unwrap();
    let selected = root.path().join("selected.json");
    root.write_atomic(&selected, b"only a size observation here", false).unwrap();
    let work = BackupServingSpace::capture(&root, 0).unwrap();
    let fields = [("selected.json", usize::MAX), ("local.json", 8193), ("set.json", 11)];
    let actual = work.controls(fields).unwrap();
    assert_eq!((actual.files, actual.directories), (2, 0));
    assert_eq!(actual.bytes, EntryCost::File(8193).allocation(work.demand.unit).unwrap()
        + EntryCost::File(11).allocation(work.demand.unit).unwrap());
    assert!(!root.path().join("local.json").exists());
    assert!(!root.path().join("set.json").exists());
    fs::hard_link(&selected, root.path().join("alias")).unwrap();
    assert!(work.controls(fields).is_err());
    assert_eq!(fs::read(selected).unwrap(), b"only a size observation here");
}
