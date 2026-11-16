//! Local persistence/layout fixtures only, not native authority or pool startup
//! acceptance. All roots are private temporary directories, never real disks.
use super::*;
use std::{fs, os::unix::fs::PermissionsExt};

fn root() -> (tempfile::TempDir, DiskRoot) {
    let temp = tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let held = DiskRoot::open(temp.path()).unwrap();
    let config =
        ValidatedConfig::try_from(Config::development(crate::test_disks(
            [held.path().to_owned()],
        )).unwrap()).unwrap();
    let mut seal = crate::disk::identity_at(
        &config,
        &config.disks[0],
        &held.directory(held.path()).unwrap(),
    )
    .unwrap();
    seal.format = DiskSealFormat::MembershipGeneration {
        authority_hash: "a".repeat(64),
    };
    held.write_json(&held.path().join("disk.json"), &seal, false)
        .unwrap();
    (temp, held)
}
fn selection(root: &DiskRoot, plan: &str, files: &str) -> LocalSelection {
    LocalSelection::capture(
        root,
        &SelectionInputs {
            plan_hash: plan,
            native_installation_hash: &"b".repeat(64),
            authority_hash: &"a".repeat(64),
            transition_hash: &"c".repeat(64),
            archive_hash: &"d".repeat(64),
            files_hash: files,
        },
    )
    .unwrap()
}
fn files() -> BTreeMap<PathBuf, Vec<u8>> {
    BTreeMap::from([
        (
            layout::GENESIS_CONFIGURATION.as_path().to_owned(),
            b"fixed configuration".to_vec(),
        ),
        (
            PathBuf::from("metadata/membership-generations")
                .join("a".repeat(64))
                .join("authority.json"),
            b"fixed authority".to_vec(),
        ),
    ])
}
fn inventory(files: &BTreeMap<PathBuf, Vec<u8>>) -> Inventory {
    Inventory::capture(
        files
            .iter()
            .map(|(path, bytes)| (path.clone(), bytes.as_slice())),
    )
    .unwrap()
}
fn write_files(root: &DiskRoot, files: &BTreeMap<PathBuf, Vec<u8>>) {
    for (path, bytes) in files {
        install_preparation_bytes(root, &root.path().join(path), bytes).unwrap();
    }
}

#[test]
fn observing_partial_locator_never_creates_missing_native_files() {
    let (_temp, root) = root();
    let files = files();
    let inventory = inventory(&files);
    inventory
        .verify_present(&root, Presence::Existing, &AtomicBool::new(false))
        .unwrap();
    assert!(
        inventory
            .verify_present(&root, Presence::Complete, &AtomicBool::new(false))
            .is_err()
    );
    assert!(!root.path().join("metadata").exists());
    assert!(!root.path().join("zxs-data").exists());
}

#[test]
fn unowned_matching_native_namespace_is_not_adopted() {
    let anchor_only = BTreeMap::from([(
        layout::GENESIS_CONFIGURATION.as_path().to_owned(),
        b"fixed configuration".to_vec(),
    )]);
    for files in [anchor_only, files()] {
        let (_temp, root) = root();
        let inventory = inventory(&files);
        write_files(&root, &files);
        let selected = selection(&root, &"e".repeat(64), &inventory.hash);
        assert_eq!(
            selected.select(&root).unwrap_err().code(),
            ErrorCode::ObjectCollision
        );
        assert!(!root.path().join(SELECTED).exists());
        inventory
            .verify_present(&root, Presence::Complete, &AtomicBool::new(false))
            .unwrap();
    }
}

#[test]
fn selected_complete_locator_loss_is_detected_without_repair() {
    let (_temp, root) = root();
    let files = files();
    let inventory = inventory(&files);
    let selected = selection(&root, &"e".repeat(64), &inventory.hash);
    selected.select(&root).unwrap();
    write_files(&root, &files);
    inventory
        .verify_present(&root, Presence::Complete, &AtomicBool::new(false))
        .unwrap();
    selected.complete(&root).unwrap();
    selected.require_complete(&root).unwrap();
    let missing = root.path().join(layout::GENESIS_CONFIGURATION.as_path());
    fs::remove_file(&missing).unwrap();
    assert!(
        inventory
            .verify_present(&root, Presence::Complete, &AtomicBool::new(false))
            .is_err()
    );
    assert!(!missing.exists());
    assert!(root.path().join(LOCAL_COMPLETE).is_file());
}

#[test]
fn data_only_target_inventory_rejects_an_active_metadata_journal() {
    for relative in [
        PathBuf::from("metadata/membership-generations")
            .join("a".repeat(64))
            .join("journal.bin"),
        genesis_namespace().join("unexpected.json"),
    ] {
        let (_temp, root) = root();
        let files = files();
        let inventory = inventory(&files);
        selection(&root, &"e".repeat(64), &inventory.hash)
            .select(&root)
            .unwrap();
        write_files(&root, &files);
        let path = root.path().join(relative);
        install_preparation_bytes(&root, &path, b"retain unknown bytes").unwrap();
        assert_eq!(
            inventory
                .inspect(&root, &AtomicBool::new(false))
                .unwrap_err()
                .code(),
            ErrorCode::ObjectCollision
        );
        assert_eq!(fs::read(path).unwrap(), b"retain unknown bytes");
    }
}

#[test]
fn original_locator_selection_cannot_be_relabelled() {
    let (_temp, root) = root();
    let files = files();
    let inventory = inventory(&files);
    let original = selection(&root, &"e".repeat(64), &inventory.hash);
    original.select(&root).unwrap();
    assert!(
        selection(&root, &"f".repeat(64), &inventory.hash)
            .select(&root)
            .is_err()
    );
    assert_eq!(
        fs::read(root.path().join(SELECTED)).unwrap(),
        original.bytes
    );
}

#[test]
fn copied_locator_selection_cannot_rebind_a_second_directory() {
    let (_first_temp, first) = root();
    let (_second_temp, second) = root();
    let original = selection(&first, &"e".repeat(64), &"f".repeat(64));
    original.select(&first).unwrap();
    install_preparation_bytes(&second, &second.path().join(SELECTED), &original.bytes).unwrap();
    assert!(require_staging(&second, Some(&"b".repeat(64))).is_err());
    assert!(require_staging(&first, None).is_err());
    assert!(!second.path().join("metadata").exists());
}

#[test]
fn generated_locator_paths_cannot_escape_fixed_native_namespaces() {
    for path in [
        "../disk.json",
        "/metadata/pool-membership/config.json",
        "metadata/objects/x",
        "metadata/pool-membership",
        "metadata/pool-membership/../other",
        "zxs-data/pool-membership",
        "zxs-data/pool-membership/unrelated.json",
        "zxs-data/pool-membership/config.json/extra",
        "zxs-data/pool-membership/../other",
        "zxs-data/objects/x",
        "/zxs-data/pool-membership/config.json",
    ] {
        assert!(
            Inventory::capture(std::iter::once((PathBuf::from(path), b"bytes".as_slice())))
                .is_err()
        );
    }
    let accepted = inventory(&files());
    assert!(accepted.files.contains_key(layout::GENESIS_CONFIGURATION.as_path()));
    assert!(accepted.directories.contains(genesis_namespace()));
    assert!(!accepted.directories.contains(Path::new("zxs-data")));
}

#[test]
fn changed_native_file_is_preserved_and_rejected_by_readback() {
    let (_temp, root) = root();
    let files = files();
    let inventory = inventory(&files);
    selection(&root, &"e".repeat(64), &inventory.hash)
        .select(&root)
        .unwrap();
    write_files(&root, &files);
    let path = root.path().join(layout::GENESIS_CONFIGURATION.as_path());
    root.write_atomic(&path, b"different bytes", true).unwrap();
    assert!(
        inventory
            .verify_present(&root, Presence::Complete, &AtomicBool::new(false))
            .is_err()
    );
    assert_eq!(fs::read(path).unwrap(), b"different bytes");
}
