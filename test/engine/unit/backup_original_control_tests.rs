//! Scope/inventory fixtures only. These fake bytes are not native authority or
//! Publication receipts; no test claims to admit a journal by its pathname.
use super::*;
use std::{fs, os::unix::fs::PermissionsExt};

fn root() -> (tempfile::TempDir, DiskRoot) {
    let directory = tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let root = DiskRoot::open(directory.path()).unwrap();
    (directory, root)
}
fn scopes() -> [PathBuf; 3] {
    [
        PathBuf::from("metadata/membership-pending/t/control-archive"),
        PathBuf::from("metadata/membership-generations/g"),
        PathBuf::from("metadata/membership-history"),
    ]
}
fn files() -> BTreeMap<PathBuf, Vec<u8>> {
    BTreeMap::from([
        (
            scopes()[0].join("archive-root.json"),
            b"exact archive".to_vec(),
        ),
        (
            scopes()[1].join("authority.json"),
            b"exact authority".to_vec(),
        ),
        (scopes()[2].join("candidate.json"), b"exact proof".to_vec()),
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
fn install(root: &DiskRoot, files: &BTreeMap<PathBuf, Vec<u8>>) {
    for (path, bytes) in files {
        install_preparation_bytes(root, &root.path().join(path), bytes).unwrap();
    }
}

#[test]
fn original_control_observation_never_creates_a_missing_archive() {
    let (_directory, root) = root();
    let files = files();
    let inventory = inventory(&files);
    inventory
        .inspect_under(&root, &scopes(), &[], &AtomicBool::new(false))
        .unwrap();
    inventory
        .verify_files(&root, Presence::Existing, &AtomicBool::new(false))
        .unwrap();
    assert!(
        inventory
            .verify_files(&root, Presence::Complete, &AtomicBool::new(false))
            .is_err()
    );
    assert!(!root.path().join("metadata").exists());
}

#[test]
fn original_generation_preserves_but_does_not_admit_the_separately_verified_journal() {
    let (_directory, root) = root();
    let files = files();
    let inventory = inventory(&files);
    let journal = scopes()[1].join("journal.bin");
    install_preparation_bytes(
        &root,
        &root.path().join(&journal),
        b"not authority: observed elsewhere",
    )
    .unwrap();
    inventory
        .inspect_under(
            &root,
            &scopes(),
            std::slice::from_ref(&journal),
            &AtomicBool::new(false),
        )
        .unwrap();
    install(&root, &files);
    inventory
        .verify_files(&root, Presence::Complete, &AtomicBool::new(false))
        .unwrap();
    assert_eq!(
        fs::read(root.path().join(journal)).unwrap(),
        b"not authority: observed elsewhere"
    );
}

#[test]
fn original_data_only_root_cannot_adopt_a_generation_journal() {
    let (_directory, root) = root();
    let files = files();
    let inventory = inventory(&files);
    let journal = root.path().join(scopes()[1].join("journal.bin"));
    install_preparation_bytes(&root, &journal, b"unowned vote").unwrap();
    assert_eq!(
        inventory
            .inspect_under(&root, &scopes(), &[], &AtomicBool::new(false))
            .unwrap_err()
            .code(),
        ErrorCode::ObjectCollision
    );
    assert_eq!(fs::read(journal).unwrap(), b"unowned vote");
}

#[test]
fn equal_unselected_original_control_bytes_are_not_adopted() {
    let (_directory, root) = root();
    let files = files();
    let inventory = inventory(&files);
    install(&root, &files);
    assert_eq!(
        inventory
            .require_absent_files(&root, &AtomicBool::new(false))
            .unwrap_err()
            .code(),
        ErrorCode::ObjectCollision
    );
    inventory
        .verify_files(&root, Presence::Complete, &AtomicBool::new(false))
        .unwrap();
}

#[test]
fn selected_scope_preserves_original_anchor_seal_and_unrelated_predecessor() {
    let (_directory, root) = root();
    for path in [
        "disk.json",
        "metadata/pool-membership/config.json",
        "metadata/pool-membership/generation.json",
        "metadata/membership-generations/old/authority.json",
    ] {
        install_preparation_bytes(&root, &root.path().join(path), path.as_bytes()).unwrap();
    }
    let files = files();
    let inventory = inventory(&files);
    install(&root, &files);
    inventory
        .inspect_under(&root, &scopes(), &[], &AtomicBool::new(false))
        .unwrap();
    for path in [
        "disk.json",
        "metadata/pool-membership/config.json",
        "metadata/pool-membership/generation.json",
        "metadata/membership-generations/old/authority.json",
    ] {
        assert_eq!(fs::read(root.path().join(path)).unwrap(), path.as_bytes());
    }
}

#[test]
fn completed_original_control_missing_file_is_not_rebuilt() {
    let (_directory, root) = root();
    let files = files();
    let inventory = inventory(&files);
    install(&root, &files);
    let missing = root.path().join(scopes()[0].join("archive-root.json"));
    fs::remove_file(&missing).unwrap();
    assert!(
        inventory
            .verify_files(&root, Presence::Complete, &AtomicBool::new(false))
            .is_err()
    );
    assert!(!missing.exists());
}

#[test]
fn unknown_original_archive_entry_is_retained_and_rejected() {
    let (_directory, root) = root();
    let files = files();
    let inventory = inventory(&files);
    install(&root, &files);
    let extra = root.path().join(scopes()[0].join("unexpected.json"));
    install_preparation_bytes(&root, &extra, b"do not remove").unwrap();
    assert!(
        inventory
            .inspect_under(&root, &scopes(), &[], &AtomicBool::new(false))
            .is_err()
    );
    assert_eq!(fs::read(extra).unwrap(), b"do not remove");
}

#[test]
fn cancelled_original_control_inventory_is_read_only() {
    let (_directory, root) = root();
    let files = files();
    let inventory = inventory(&files);
    install(&root, &files);
    assert!(
        inventory
            .inspect_under(&root, &scopes(), &[], &AtomicBool::new(true))
            .is_err()
    );
    inventory
        .verify_files(&root, Presence::Complete, &AtomicBool::new(false))
        .unwrap();
}
