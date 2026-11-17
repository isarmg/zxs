use super::*;
use std::{fs, os::unix::fs::PermissionsExt};

#[test]
fn only_the_original_empty_versions_is_unstarted_on_the_exact_held_root() {
    let temporary = tempfile::Builder::new().permissions(fs::Permissions::from_mode(0o700))
        .tempdir().unwrap();
    let held = DiskRoot::open(temporary.path()).unwrap();
    let versions = held.path().join(layout::VERSIONS.as_path());
    let expected = BTreeSet::from([PathBuf::from("versions"), PathBuf::from("history")]);
    let stop = AtomicBool::new(false);
    let inspect = || {
        let mut read = Inventory::default();
        read.read(&held, &expected, Some(&layout::VERSIONS), &stop)?;
        Ok::<_, crate::Error>(read.started)
    };
    assert!(inspect().is_err());
    held.private_dir(&versions).unwrap();
    assert!(!inspect().unwrap());
    // Empty known metadata parent (no FILE bytes at all) still crosses the
    // all-target selection barrier. No inspection fills the empty directory.
    let history = held.path().join("history");
    held.private_dir(&history).unwrap();
    assert!(inspect().unwrap());
    assert_eq!(fs::read_dir(&history).unwrap().count(), 0);
    fs::remove_dir(history).unwrap();
    let temporary_leaf = versions.join(format!(".state-atomic-{}.tmp", "a".repeat(32)));
    fs::write(&temporary_leaf, b"unresolved").unwrap();
    assert!(inspect().unwrap());
    assert_eq!(fs::read(temporary_leaf).unwrap(), b"unresolved");
}
