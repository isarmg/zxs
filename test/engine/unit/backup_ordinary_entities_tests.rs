//! Storage/locator boundary fixtures only, not a native Backup Publication,
//! SQLite semantic admission or active Engine cutover acceptance test.
use super::*;
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
};

fn fixture() -> (tempfile::TempDir, DiskRoot, Image<'static>, PathBuf) {
    let temporary = tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let root = DiskRoot::open(temporary.path()).unwrap();
    let history = format!("history_{}", "1".repeat(32));
    let file = format!("file_{}", "2".repeat(32));
    let home = PathBuf::from("histories")
        .join(&history)
        .join("file-containers")
        .join(&file)
        .join("retained");
    let mut image = Image {
        files: BTreeMap::new(),
        directories: BTreeSet::new(),
        histories: BTreeSet::from([history.clone()]),
        directory_homes: BTreeMap::new(),
        file_homes: BTreeMap::new(),
        locator_paths: BTreeSet::new(),
        binding: LayoutBinding::parse(&"a".repeat(64)).unwrap(),
        manifest: b"test-only immutable recipe".to_vec(),
        metadata_bytes: 0,
        bodies: 0,
        body_bytes: 0,
        directory_repositories: 0,
    };
    image
        .add_home(FileHome {
            history,
            id: file,
            path: home.clone(),
            address: FileAddress::Stable {
                relative_path: StableRelative::parse("retained").unwrap(),
            },
        })
        .unwrap();
    let metadata = home.join(".zxs-file.sqlite");
    image
        .add_file(
            metadata.clone(),
            Data::Bytes(b"fixture metadata, not a database authority".to_vec()),
        )
        .unwrap();
    (temporary, root, image, metadata)
}

#[test]
fn ordinary_observation_does_not_create_missing_entities() {
    let (_temporary, root, image, _) = fixture();
    assert!(image.verify(&root, &AtomicBool::new(false)).is_err());
    assert!(!root.path().join("histories").exists());
}

#[test]
fn ordinary_file_locator_resolves_the_exact_complete_entity() {
    let (_temporary, root, image, _) = fixture();
    image
        .require_absent(&root, &AtomicBool::new(false))
        .unwrap();
    image.install(&root, &AtomicBool::new(false)).unwrap();
    let first = image.verify(&root, &AtomicBool::new(false)).unwrap();
    let home = image.file_homes.values().next().unwrap();
    let history = root.path().join("histories").join(&home.history);
    let anchor = HistoryAnchor::new(&root, &history, &home.history).unwrap();
    let intent = layout_locator::load_file(&anchor, &home.id, &mut |_| Ok(())).unwrap();
    assert_eq!(
        layout_locator::locate_file(&anchor, &intent, &mut |_| Ok(())).unwrap(),
        root.path().join(&home.path)
    );
    image.install(&root, &AtomicBool::new(false)).unwrap();
    assert_eq!(image.verify(&root, &AtomicBool::new(false)).unwrap(), first);
}

#[test]
fn unselected_equal_material_is_not_adopted() {
    let (_temporary, root, image, _) = fixture();
    image.install(&root, &AtomicBool::new(false)).unwrap();
    assert!(
        image
            .require_absent(&root, &AtomicBool::new(false))
            .is_err()
    );
    assert!(image.verify(&root, &AtomicBool::new(false)).is_ok());
}

#[test]
fn unknown_ordinary_content_is_preserved_and_blocks_resume() {
    let (_temporary, root, image, metadata) = fixture();
    image.install(&root, &AtomicBool::new(false)).unwrap();
    let unknown = root.path().join(metadata.parent().unwrap()).join("unowned");
    root.write_atomic(&unknown, b"keep this", false).unwrap();
    assert!(image.install(&root, &AtomicBool::new(false)).is_err());
    assert_eq!(fs::read(unknown).unwrap(), b"keep this");
}

#[test]
fn changed_metadata_is_not_overwritten() {
    let (_temporary, root, image, metadata) = fixture();
    image.install(&root, &AtomicBool::new(false)).unwrap();
    let path = root.path().join(metadata);
    root.write_atomic(&path, b"changed", true).unwrap();
    assert!(image.verify(&root, &AtomicBool::new(false)).is_err());
    assert!(image.install(&root, &AtomicBool::new(false)).is_err());
    assert_eq!(fs::read(path).unwrap(), b"changed");
}

#[test]
fn same_bytes_on_a_new_inode_change_the_completed_binding() {
    let (_temporary, root, image, metadata) = fixture();
    image.install(&root, &AtomicBool::new(false)).unwrap();
    let before = image.verify(&root, &AtomicBool::new(false)).unwrap();
    let path = root.path().join(metadata);
    let original = root.open_read_regular(&path).unwrap();
    let bytes = fs::read(&path).unwrap();
    root.write_atomic(&path, &bytes, true).unwrap();
    assert_ne!(
        FileIdentity::from_metadata(&original.metadata().unwrap()),
        FileIdentity::from_metadata(&regular(&root, &path).unwrap())
    );
    assert_ne!(
        image.verify(&root, &AtomicBool::new(false)).unwrap(),
        before
    );
}

#[test]
fn missing_completed_locator_is_not_repaired_by_observation() {
    let (_temporary, root, image, _) = fixture();
    image.install(&root, &AtomicBool::new(false)).unwrap();
    let path = root.path().join(image.locator_paths.iter().next().unwrap());
    root.remove_file_synced(&path).unwrap();
    assert!(image.verify(&root, &AtomicBool::new(false)).is_err());
    assert!(!path.exists());
}

#[test]
fn source_packages_and_other_histories_remain_untouched() {
    let (_temporary, root, image, _) = fixture();
    let history = image.histories.iter().next().unwrap();
    let source = root
        .path()
        .join("histories")
        .join(history)
        .join("versions/original-package");
    install_preparation_bytes(&root, &source, b"original package bytes").unwrap();
    let other = root.path().join("histories/other/untouched");
    install_preparation_bytes(&root, &other, b"unrelated").unwrap();
    image.install(&root, &AtomicBool::new(false)).unwrap();
    image.verify(&root, &AtomicBool::new(false)).unwrap();
    assert_eq!(fs::read(source).unwrap(), b"original package bytes");
    assert_eq!(fs::read(other).unwrap(), b"unrelated");
}

#[test]
fn symbolic_ordinary_namespace_is_not_followed() {
    let (_temporary, root, image, _) = fixture();
    let outside = tempfile::tempdir().unwrap();
    let history = root
        .path()
        .join("histories")
        .join(image.histories.iter().next().unwrap());
    root.private_dir(&history).unwrap();
    symlink(outside.path(), history.join("file-containers")).unwrap();
    assert!(image.install(&root, &AtomicBool::new(false)).is_err());
    assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
}

#[test]
fn canceled_ordinary_install_does_not_create_namespace() {
    let (_temporary, root, image, _) = fixture();
    assert!(image.install(&root, &AtomicBool::new(true)).is_err());
    assert!(!root.path().join("histories").exists());
}

#[test]
fn ordinary_capacity_rejects_overflow_instead_of_saturating() {
    assert!(capacity_before_choice(u64::MAX).is_err());
}

#[test]
fn prefixed_managed_entity_stays_in_the_normal_absence_inventory() {
    let (_temporary, root, mut image, _) = fixture();
    let history = image.histories.iter().next().unwrap().clone();
    let directory = DirectoryId::parse(&format!("repo_{}", "3".repeat(32))).unwrap();
    let ordinary =
        HistoryRelative::parse(&format!("directory-repositories/{}", directory.as_str())).unwrap();
    let to = target_directory(&ordinary, &["Primary".into(), "Second".into()]).unwrap();
    assert_eq!(
        to.as_str(),
        format!(
            "directory-repositories/{}/Primary/Second",
            directory.as_str()
        )
    );
    let anchor = PathBuf::from("histories").join(&history);
    image.add_directory(anchor.join(to.as_path())).unwrap();
    image
        .add_locator(
            anchor
                .join(".zxs-layout/directories")
                .join(format!("{}.json", directory.as_str())),
        )
        .unwrap();
    image.directory_homes.insert(
        (history.clone(), directory.clone()),
        Directory {
            history: history.clone(),
            id: directory.clone(),
            to: to.clone(),
        },
    );
    let file = format!("file_{}", "4".repeat(32));
    let home = anchor.join(to.as_path()).join("child.txt");
    image
        .add_home(FileHome {
            history: history.clone(),
            id: file.clone(),
            path: home.clone(),
            address: FileAddress::Directory {
                directory_id: directory,
                relative_path: HistoryRelative::parse("child.txt").unwrap(),
            },
        })
        .unwrap();
    image.install(&root, &AtomicBool::new(false)).unwrap();
    image.verify(&root, &AtomicBool::new(false)).unwrap();
    root.remove_file_synced(
        &root
            .path()
            .join(&anchor)
            .join(".zxs-layout/files")
            .join(format!("{file}.json")),
    )
    .unwrap();
    assert!(
        super::super::super::container_at(
            &root,
            &root.path().join(&anchor),
            Some(&history),
            &file,
            &mut |_| Ok(())
        )
        .is_err()
    );
    assert!(root.path().join(home).is_dir());
}
