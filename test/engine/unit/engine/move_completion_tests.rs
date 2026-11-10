//! Completion archive absence is a normal first-move state, not a damaged file.
use super::*;

fn binding() -> CompletionBinding {
    CompletionBinding {
        format: Format::MoveCompletion,
        pool_id: format!("pool_{}", "1".repeat(32)),
        operation: OperationId::generate().unwrap(),
        sequence: 1,
        candidate_hash: "a".repeat(64),
        snapshot_hash: "b".repeat(64),
        previous_snapshot: None,
        request_digest: "c".repeat(64),
        metadata_disk_ids: vec![format!("disk_{}", "2".repeat(32))],
        config_hash: "d".repeat(64),
    }
}

#[test]
fn first_move_retains_and_retires_without_a_preexisting_completion_archive() {
    let temp = tempfile::tempdir().unwrap();
    let root = DiskRoot::open(temp.path()).unwrap();
    let original = binding();
    assert!(read_completed(&root, original.operation).unwrap().is_none());
    assert!(!temp.path().join(COMPLETED).exists());
    original.retain_at(&root).unwrap();
    original.retain_at(&root).unwrap();
    original
        .same(&read(&root, &root.path().join(ACTIVE)).unwrap().unwrap())
        .unwrap();
    assert!(!temp.path().join(COMPLETED).exists());

    original.retire_at(&root).unwrap();
    assert!(read(&root, &root.path().join(ACTIVE)).unwrap().is_none());
    original
        .same(&read_completed(&root, original.operation).unwrap().unwrap())
        .unwrap();
    original.retain_at(&root).unwrap();
    original.retire_at(&root).unwrap();
}

#[test]
fn existing_completion_archive_requires_a_directory_and_valid_original_record() {
    let temp = tempfile::tempdir().unwrap();
    let root = DiskRoot::open(temp.path()).unwrap();
    let original = binding();
    let directory = root.path().join(COMPLETED);
    std::fs::write(&directory, b"not a directory").unwrap();
    assert!(read_completed(&root, original.operation).is_err());
    assert!(original.retain_at(&root).is_err());
    assert!(!root.path().join(ACTIVE).exists());
    std::fs::remove_file(&directory).unwrap();

    root.private_dir(&directory).unwrap();
    assert!(read_completed(&root, original.operation).unwrap().is_none());
    std::fs::write(original.archive(&root), b"not a completion record").unwrap();
    assert!(read_completed(&root, original.operation).is_err());
    assert!(original.retain_at(&root).is_err());
}

#[test]
fn a_completion_archive_symlink_is_not_treated_as_absent() {
    let temp = tempfile::tempdir().unwrap();
    let exterior = tempfile::tempdir().unwrap();
    let root = DiskRoot::open(temp.path()).unwrap();
    std::os::unix::fs::symlink(exterior.path(), root.path().join(COMPLETED)).unwrap();
    assert!(read_completed(&root, binding().operation).is_err());
}
