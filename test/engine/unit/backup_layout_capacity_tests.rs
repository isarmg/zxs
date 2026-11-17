use super::*;
use crate::ErrorCode;
use std::{fs, num::NonZeroU64, os::unix::fs::{MetadataExt, PermissionsExt}};

#[test]
fn fresh_layout_cannot_borrow_bytes_or_inodes_and_keeps_original_reserves() {
    let layout = 16 * 1024 * 1024;
    let fixed = RESERVE + layout as u64 + 8192;
    let entries = 22;
    for (unit, per_directory) in [(1024, 8192), (4096, 8192), (32768, 65536)] {
        let mut target = Space {
            unit: NonZeroU64::new(unit).unwrap(),
            bytes: fixed + entries * per_directory,
            inodes: Some(entries),
        };
        require_directories(&target, entries, layout).unwrap();
        target.inodes = Some(entries - 1);
        assert_eq!(require_directories(&target, entries, layout).unwrap_err().code(), ErrorCode::NoSpace);
        target.inodes = None;
        require_directories(&target, entries, layout).unwrap();
        target.bytes -= 1;
        assert_eq!(require_directories(&target, entries, layout).unwrap_err().code(), ErrorCode::NoSpace);
        assert_eq!(require_directories(&target, u64::MAX, layout).unwrap_err().code(), ErrorCode::ResourceLimit);
    }
}

#[test]
fn fresh_voter_feedback_probe_does_not_create_parents_or_adopt_an_existing_copy() {
    let temp = tempfile::Builder::new().permissions(fs::Permissions::from_mode(0o700))
        .tempdir().unwrap();
    let root = DiskRoot::open(temp.path()).unwrap();
    let relative = PathBuf::from("metadata/backup-layout-preparations/sample.json");
    let root_feedback = PathBuf::from("metadata/backup-root-preparations/sample.json");
    let records = [(root_feedback, 4096), (relative.clone(), 8192)];
    let census = feedback_census(&root, records.clone()).unwrap();
    assert_eq!((census.needed.files, census.needed.directories), (2, 3));
    require_new_feedback_capacity(&root, records).unwrap();
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
    let path = root.path().join(&relative);
    root.private_dir(path.parent().unwrap()).unwrap();
    root.write_atomic(&path, b"this is not an accepted canonical feedback", false).unwrap();
    let ino = fs::metadata(&path).unwrap().ino();
    let bytes = fs::read(&path).unwrap();
    assert_eq!(require_new_feedback_capacity(&root, [(relative, 8192)]).unwrap_err().code(),
        ErrorCode::RecoveryRequired);
    assert_eq!(fs::metadata(&path).unwrap().ino(), ino);
    assert_eq!(fs::read(&path).unwrap(), bytes);
}
