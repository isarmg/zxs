//! Real independent disks, published history and held-directory read consumers.
use platform_io::fs_safety::PrivateDirectory;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};
use storage_contract::OperationId;
use storage_engine::disk_authority::{
    Digest, DiskSelection, FileWrite, Identity, IndependentDisk, IndependentSourceReader, Origin,
    Role, SourceInventory, Token, source_inventory_display,
};

fn token(value: &str) -> Token {
    Token::new(value).unwrap()
}
fn create(root: &Path, name: &str, role: Role) -> IndependentDisk {
    IndependentDisk::initialize(
        &root.join(name),
        Identity {
            origin: Origin {
                pool_id: token("origin_pool"),
                disk_id: token(name),
                slot_id: token(name),
                binding_generation: 17,
            },
            incarnation: Digest::body(name.as_bytes()),
            role,
        },
    )
    .unwrap()
}
fn write(disk: &IndependentDisk, bytes: &[u8]) {
    let review = disk
        .review_file(FileWrite {
            operation: OperationId::generate().unwrap(),
            expected_base: disk.report().unwrap().checkpoint,
            history: &token("history"),
            file: &token("file"),
            bytes,
        })
        .unwrap();
    disk.execute(&review).unwrap();
}
fn backup(root: &Path, target: &IndependentDisk, sources: &[&IndependentDisk]) {
    let selections: Vec<_> = sources
        .iter()
        .map(|source| DiskSelection {
            path: root.join(source.identity().origin.disk_id.as_str()),
            identity: source.identity().clone(),
            known_checkpoint: source.report().unwrap().checkpoint,
        })
        .collect();
    let review = target
        .review_backup(
            OperationId::generate().unwrap(),
            target.report().unwrap().checkpoint,
            &selections,
        )
        .unwrap();
    target.execute(&review).unwrap();
}
fn reader(path: &Path) -> IndependentSourceReader {
    IndependentSourceReader::from_directory(PrivateDirectory::open_existing(path).unwrap()).unwrap()
}
fn inventory(path: &Path) -> SourceInventory {
    reader(path).source_inventory().unwrap()
}
fn snapshot(path: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(root: &Path, path: &Path, result: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                walk(root, &entry.path(), result);
            } else {
                result.insert(
                    entry.path().strip_prefix(root).unwrap().to_owned(),
                    fs::read(entry.path()).unwrap(),
                );
            }
        }
    }
    let mut result = BTreeMap::new();
    walk(path, path, &mut result);
    result
}

#[test]
fn primary_inventory_contains_only_its_history_without_downstream_or_system() {
    let root = tempfile::tempdir().unwrap();
    let primary = create(root.path(), "primary", Role::Primary);
    let other = create(root.path(), "other", Role::Primary);
    let secondary = create(root.path(), "secondary", Role::Secondary);
    write(&primary, b"first");
    write(&primary, b"second");
    write(&other, b"unrelated");
    backup(root.path(), &secondary, &[&primary]);
    fs::remove_dir_all(root.path().join("secondary")).unwrap();
    fs::remove_dir_all(root.path().join("other")).unwrap();
    let before = snapshot(root.path());
    let captured = inventory(&root.path().join("primary"));
    assert_eq!(captured.versions().len(), 2);
    assert!(
        captured
            .versions()
            .iter()
            .all(|version| version.owner() == primary.identity())
    );
    assert_eq!(captured.topology().nodes().len(), 1);
    assert!(captured.topology().relations().is_empty());
    assert_eq!(snapshot(root.path()), before);
}

#[test]
fn moved_backup_keeps_portable_original_identity_without_source_disks() {
    let root = tempfile::tempdir().unwrap();
    let primary = create(root.path(), "primary", Role::Primary);
    let secondary = create(root.path(), "secondary", Role::Secondary);
    write(&primary, b"first");
    write(&primary, b"second");
    backup(root.path(), &secondary, &[&primary]);
    let captured = inventory(&root.path().join("secondary"));
    let old_display = source_inventory_display(&captured).unwrap();
    let moved = root.path().join("another_host_mount");
    fs::rename(root.path().join("secondary"), &moved).unwrap();
    fs::remove_dir_all(root.path().join("primary")).unwrap();
    let before = snapshot(&moved);
    let moved_inventory = inventory(&moved);
    assert_eq!(
        source_inventory_display(&moved_inventory).unwrap(),
        old_display
    );
    assert_eq!(moved_inventory.versions().len(), 2);
    assert!(
        moved_inventory
            .versions()
            .iter()
            .all(|version| version.owner() == primary.identity())
    );
    assert_eq!(snapshot(&moved), before);
}

#[test]
fn tertiary_inventory_distinguishes_same_named_histories_from_original_primaries() {
    let root = tempfile::tempdir().unwrap();
    let left = create(root.path(), "left", Role::Primary);
    let right = create(root.path(), "right", Role::Primary);
    let secondary = create(root.path(), "secondary", Role::Secondary);
    let tertiary = create(root.path(), "tertiary", Role::Tertiary);
    write(&left, b"same bytes");
    write(&right, b"same bytes");
    backup(root.path(), &secondary, &[&left, &right]);
    backup(root.path(), &tertiary, &[&secondary]);
    for name in ["left", "right", "secondary"] {
        fs::remove_dir_all(root.path().join(name)).unwrap();
    }
    let before = snapshot(root.path());
    let captured = inventory(&root.path().join("tertiary"));
    let rows = captured.versions();
    assert_eq!(rows.len(), 2);
    assert_ne!(rows[0].owner(), rows[1].owner());
    assert_eq!(rows[0].history_id(), rows[1].history_id());
    assert_eq!(rows[0].file_id(), rows[1].file_id());
    assert_eq!(rows[0].body(), rows[1].body());
    assert_ne!(rows[0].commit(), rows[1].commit());
    assert_eq!(
        source_inventory_display(&captured)
            .unwrap()
            .versions()
            .len(),
        2
    );
    assert_eq!(snapshot(root.path()), before);
}

#[test]
fn backup_inventory_retains_prior_source_history_beyond_current_topology() {
    let root = tempfile::tempdir().unwrap();
    let old = create(root.path(), "old", Role::Primary);
    let current = create(root.path(), "current", Role::Primary);
    let secondary = create(root.path(), "secondary", Role::Secondary);
    write(&old, b"old");
    write(&current, b"current");
    backup(root.path(), &secondary, &[&old]);
    backup(root.path(), &secondary, &[&current]);
    fs::remove_dir_all(root.path().join("old")).unwrap();
    fs::remove_dir_all(root.path().join("current")).unwrap();
    let captured = inventory(&root.path().join("secondary"));
    assert_eq!(captured.versions().len(), 2);
    assert!(
        captured
            .versions()
            .iter()
            .any(|version| version.owner() == old.identity())
    );
    assert!(
        !captured
            .topology()
            .nodes()
            .any(|node| node.identity() == old.identity())
    );
    assert_eq!(
        source_inventory_display(&captured)
            .unwrap()
            .versions()
            .len(),
        2
    );
}

#[test]
fn inventory_does_not_read_full_bodies_but_rejects_corrupt_metadata_and_replacement() {
    let root = tempfile::tempdir().unwrap();
    let primary = create(root.path(), "primary", Role::Primary);
    write(&primary, b"first");
    write(&primary, b"second");
    let path = root.path().join("primary");
    let original = source_inventory_display(&inventory(&path)).unwrap();
    for entry in fs::read_dir(path.join("bodies")).unwrap() {
        fs::write(entry.unwrap().path(), b"corrupt FULL bytes").unwrap();
    }
    assert_eq!(
        source_inventory_display(&inventory(&path)).unwrap(),
        original
    );
    for entry in fs::read_dir(path.join("bodies")).unwrap() {
        fs::remove_file(entry.unwrap().path()).unwrap();
    }
    assert_eq!(
        source_inventory_display(&inventory(&path)).unwrap(),
        original
    );
    let held = reader(&path);
    fs::rename(&path, root.path().join("moved")).unwrap();
    create(root.path(), "primary", Role::Primary);
    assert!(held.source_inventory().is_err());
    let moved = root.path().join("moved");
    let object = fs::read_dir(moved.join("objects"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    fs::write(object, b"corrupt metadata").unwrap();
    let before = snapshot(&moved);
    assert!(reader(&moved).source_inventory().is_err());
    assert_eq!(snapshot(&moved), before);
}

#[test]
fn initialized_empty_disks_do_not_invent_retained_versions() {
    let root = tempfile::tempdir().unwrap();
    for (name, role) in [
        ("primary", Role::Primary),
        ("secondary", Role::Secondary),
        ("tertiary", Role::Tertiary),
    ] {
        create(root.path(), name, role);
        let capture = inventory(&root.path().join(name));
        assert!(capture.versions().is_empty());
        let display = source_inventory_display(&capture).unwrap();
        assert!(display.root().publication.is_none());
        assert!(display.versions().is_empty());
    }
}

#[test]
fn held_inventory_rejects_disconnect_and_a_logically_identical_replacement() {
    fn copy_private(source: &Path, destination: &Path) {
        use std::os::unix::fs::PermissionsExt;
        fs::create_dir(destination).unwrap();
        fs::set_permissions(destination, fs::Permissions::from_mode(0o700)).unwrap();
        for entry in fs::read_dir(source).unwrap() {
            let entry = entry.unwrap();
            let target = destination.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_private(&entry.path(), &target);
            } else {
                fs::copy(entry.path(), target).unwrap();
            }
        }
    }
    let root = tempfile::tempdir().unwrap();
    let primary = create(root.path(), "primary", Role::Primary);
    write(&primary, b"source");
    let path = root.path().join("primary");
    let moved = root.path().join("moved");
    let held = reader(&path);
    let original = source_inventory_display(&held.source_inventory().unwrap()).unwrap();
    fs::rename(&path, &moved).unwrap();
    let missing = held.source_inventory().unwrap_err();
    assert!(matches!(
        &missing,
        storage_engine::disk_authority::Error::Filesystem(
            platform_io::fs_safety::Error::Io(error)
        ) | storage_engine::disk_authority::Error::Io(error)
            if error.kind() == std::io::ErrorKind::NotFound
    ));
    copy_private(&moved, &path);
    let before = snapshot(&path);
    assert!(matches!(
        held.source_inventory().unwrap_err(),
        storage_engine::disk_authority::Error::Identity
    ));
    assert_eq!(snapshot(&path), before);
    // A new selection can observe the portable facts. It proves no unique
    // current disk, writer eligibility or global checkpoint freshness.
    assert_eq!(
        source_inventory_display(&inventory(&path)).unwrap(),
        original
    );
}

#[test]
fn held_inventory_classifies_replaced_metadata_directory_as_identity_change() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let primary = create(root.path(), "primary", Role::Primary);
    write(&primary, b"source");
    let path = root.path().join("primary");
    let held = reader(&path);
    let saved = path.join("objects-retained");
    fs::rename(path.join("objects"), &saved).unwrap();
    fs::create_dir(path.join("objects")).unwrap();
    fs::set_permissions(path.join("objects"), fs::Permissions::from_mode(0o700)).unwrap();
    let before = snapshot(&path);
    let error = held.source_inventory().unwrap_err();
    assert!(
        matches!(
            error,
            storage_engine::disk_authority::Error::Filesystem(
                platform_io::fs_safety::Error::IdentityChanged(_)
            )
        ),
        "{error:?}"
    );
    assert_eq!(snapshot(&path), before);
}

#[test]
fn same_body_versions_of_one_file_keep_distinct_commit_identities() {
    let root = tempfile::tempdir().unwrap();
    let primary = create(root.path(), "primary", Role::Primary);
    write(&primary, b"unchanged content");
    write(&primary, b"unchanged content");
    let captured = inventory(&root.path().join("primary"));
    let rows = captured.versions();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].owner(), rows[1].owner());
    assert_eq!(rows[0].history_id(), rows[1].history_id());
    assert_eq!(rows[0].file_id(), rows[1].file_id());
    assert_eq!(rows[0].body(), rows[1].body());
    assert_ne!(rows[0].commit(), rows[1].commit());
    let display = source_inventory_display(&captured).unwrap();
    assert_eq!(display.versions().len(), 2);
    assert_ne!(display.versions()[0].commit, display.versions()[1].commit);
    assert_eq!(display.versions()[0].body, display.versions()[1].body);
}
