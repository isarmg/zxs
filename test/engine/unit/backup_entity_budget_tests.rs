//! Exercise the complete image planner's actual entry limit without disk I/O.
use super::*;

fn near_entry_limit(remaining: usize) -> Image<'static> {
    let mut image = Image {
        root_name: ROOT,
        directories: BTreeSet::new(),
        files: BTreeMap::new(),
        metadata_bytes: 0,
        manifest: Vec::new(),
        placement: EntityPlacement::empty(),
        file_units: 0,
        directory_repositories: 0,
        bodies: 0,
        body_bytes: 0,
    };
    for index in 0..MAX_ENTRIES - remaining {
        image
            .add_directory(PathBuf::from(format!("retained-{index}")))
            .unwrap();
    }
    image
}

#[test]
fn missing_ancestors_cannot_consume_the_reserved_file_entry() {
    let mut image = near_entry_limit(2);
    let path = PathBuf::from("nested/child/payload");
    let error = image
        .add_file(path.clone(), Data::Bytes(vec![1]))
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::ResourceLimit);
    assert!(!image.files.contains_key(&path));
    assert_eq!(image.directories.len() + image.files.len(), MAX_ENTRIES);
}

#[test]
fn nested_file_accepts_the_exact_complete_entry_limit() {
    let mut image = near_entry_limit(3);
    let path = PathBuf::from("nested/child/payload");
    image
        .add_file(path.clone(), Data::Bytes(vec![1]))
        .unwrap();
    assert!(image.files.contains_key(&path));
    assert_eq!(image.directories.len() + image.files.len(), MAX_ENTRIES);
    let error = image
        .add_file(PathBuf::from("nested/child/extra"), Data::Bytes(vec![2]))
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::ResourceLimit);
}

#[test]
fn existing_ancestors_do_not_consume_another_entry() {
    let mut image = near_entry_limit(2);
    image.add_directory(PathBuf::from("nested")).unwrap();
    image
        .add_file(PathBuf::from("nested/payload"), Data::Bytes(vec![1]))
        .unwrap();
    assert_eq!(image.directories.len() + image.files.len(), MAX_ENTRIES);
}
