use super::*;
use std::{fs, io::Write, os::unix::fs::PermissionsExt};

#[test]
fn snapshot_preserves_every_metadata_field_and_equality_component() {
    let mut file = tempfile::tempfile().unwrap();
    file.write_all(b"contents").unwrap();
    let metadata = file.metadata().unwrap();
    let stamp = FileStamp::of(&file).unwrap();
    let FileStamp {
        device,
        inode,
        size,
        links,
        mode,
        modified,
        changed,
    } = &stamp;
    assert_eq!(*device, metadata.dev());
    assert_eq!(*inode, metadata.ino());
    assert_eq!(*size, metadata.len());
    assert_eq!(*links, metadata.nlink());
    assert_eq!(*mode, metadata.mode());
    assert_eq!(*modified, (metadata.mtime(), metadata.mtime_nsec()));
    assert_eq!(*changed, (metadata.ctime(), metadata.ctime_nsec()));
    assert_eq!(stamp, FileStamp::of(&file).unwrap());
    let changes: [fn(&mut FileStamp); 9] = [
        |stamp: &mut FileStamp| stamp.device ^= 1,
        |stamp: &mut FileStamp| stamp.inode ^= 1,
        |stamp: &mut FileStamp| stamp.size ^= 1,
        |stamp: &mut FileStamp| stamp.links ^= 1,
        |stamp: &mut FileStamp| stamp.mode ^= 1,
        |stamp: &mut FileStamp| stamp.modified.0 ^= 1,
        |stamp: &mut FileStamp| stamp.modified.1 ^= 1,
        |stamp: &mut FileStamp| stamp.changed.0 ^= 1,
        |stamp: &mut FileStamp| stamp.changed.1 ^= 1,
    ];
    for change in changes {
        let mut changed = FileStamp::of(&file).unwrap();
        change(&mut changed);
        assert_ne!(stamp, changed);
    }
}

#[test]
fn held_file_snapshots_detect_metadata_and_path_replacement() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("held");
    fs::write(&path, b"contents").unwrap();
    let file = File::open(&path).unwrap();
    let original = FileStamp::of(&file).unwrap();
    file.set_permissions(fs::Permissions::from_mode(original.mode ^ 0o100))
        .unwrap();
    assert_ne!(original, FileStamp::of(&file).unwrap());
    let before_link = FileStamp::of(&file).unwrap();
    fs::hard_link(&path, root.path().join("alias")).unwrap();
    assert_ne!(before_link, FileStamp::of(&file).unwrap());
    fs::rename(&path, root.path().join("old")).unwrap();
    fs::write(&path, b"contents").unwrap();
    assert_ne!(
        FileStamp::of(&file).unwrap(),
        FileStamp::of(&File::open(path).unwrap()).unwrap()
    );
}
