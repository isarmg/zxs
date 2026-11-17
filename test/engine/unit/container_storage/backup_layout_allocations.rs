use super::*;
use std::{fs, os::unix::fs::PermissionsExt, path::Path};

fn count_directories(root: &Path) -> u64 {
    fs::read_dir(root).unwrap().map(|entry| {
        let entry = entry.unwrap();
        assert!(entry.file_type().unwrap().is_dir());
        1 + count_directories(&entry.path())
    }).sum()
}

#[test]
fn native_layout_directory_demand_includes_real_versions_and_each_source_area() {
    let temporary = tempfile::Builder::new().permissions(fs::Permissions::from_mode(0o700))
        .tempdir().unwrap();
    let management = DiskRoot::open(temporary.path()).unwrap();
    // B/A on C: each exclusive prefix has all five physical areas. A FILE's
    // versions directory is present even though it is not retained as an FD.
    for _ in 0..2 {
        let id = DirectoryId::generate().unwrap();
        let (node, _) = prefix::create_exclusive(&management, &layout::container::prefix_preparation(id)).unwrap();
        areas::HeldAreas::create(&node).unwrap();
    }
    let id = DirectoryId::generate().unwrap();
    prefix::create_exclusive(&management, &layout::container::namespace_preparation(id)).unwrap();
    for _ in 0..2 {
        let id = DirectoryId::generate().unwrap();
        let (file, _) = prefix::create_exclusive(&management, &layout::container::preparation(id)).unwrap();
        file.private_dir(&file.path().join(layout::VERSIONS.as_path())).unwrap();
    }
    let actual = count_directories(temporary.path());
    assert_eq!(actual, 17);
    assert_eq!(directory_count(2, 1, 2, 0).unwrap(), actual);
    assert_eq!(directory_count(2, 1, 2, 5).unwrap(), actual + 5);
}
