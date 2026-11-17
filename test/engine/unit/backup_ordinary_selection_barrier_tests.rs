use super::*;
use std::{fs, os::unix::fs::{MetadataExt, PermissionsExt}};

fn root() -> (tempfile::TempDir, DiskRoot) {
    let temp = tempfile::Builder::new().permissions(fs::Permissions::from_mode(0o700))
        .tempdir().unwrap();
    let held = DiskRoot::open(temp.path()).unwrap();
    (temp, held)
}

#[test]
fn ordinary_target_cannot_recreate_the_system_intent_or_forge_a_local_selector() {
    let (_temp, target) = root();
    let selector = b"exact selection for this target";
    let selected = target.path().join(SELECTED);
    let complete = target.path().join(LOCAL_COMPLETE);
    assert_eq!(markers(&target, selector, false).unwrap(), Markers::None);
    target.write_atomic(&complete, b"retained later evidence", false).unwrap();
    assert!(markers(&target, selector, true).is_err());
    assert!(!selected.exists());
    target.write_atomic(&selected, selector, false).unwrap();
    let original = fs::metadata(&selected).unwrap().ino();
    assert!(markers(&target, selector, false).is_err()); // even both exact target names
    assert!(markers(&target, b"another chosen target", true).is_err());
    assert_eq!(markers(&target, selector, true).unwrap(), Markers::LocalComplete);
    assert_eq!(fs::metadata(&selected).unwrap().ino(), original);
    assert_eq!(fs::read(selected).unwrap(), selector);
    assert_eq!(fs::read(complete).unwrap(), b"retained later evidence");
}

#[test]
fn ordinary_partial_selector_batch_can_resume_only_before_any_target_started() {
    let (_first, one) = root();
    let (_second, two) = root();
    one.write_atomic(&one.path().join(SELECTED), b"first", false).unwrap();
    assert_eq!(markers(&one, b"first", true).unwrap(), Markers::Selected);
    assert_eq!(markers(&two, b"second", true).unwrap(), Markers::None);
    require_barrier(&[Progress::SelectedEmpty, Progress::Unselected]).unwrap();
    assert!(require_barrier(&[Progress::Unselected, Progress::Started]).is_err());
    assert!(require_barrier(&[Progress::Started, Progress::Unselected]).is_err());
    assert!(require_barrier(&[Progress::LocalComplete, Progress::Unselected]).is_err());
    assert!(!two.path().join(SELECTED).exists());
    require_barrier(&[Progress::Started, Progress::SelectedEmpty]).unwrap();
}
