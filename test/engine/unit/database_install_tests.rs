use super::*;
use std::os::unix::fs::{MetadataExt, PermissionsExt};

#[test]
fn empty_or_missing_retention_cannot_admit_an_oversized_batch() {
    let temporary = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let root = DiskRoot::open(temporary.path()).unwrap();
    let history = root.path().join("history");
    root.private_dir(&history).unwrap();
    for populated in [false, true] {
        if populated {
            root.private_dir(&history.join(".zxs-layout/database-installs"))
                .unwrap();
        }
        for (bytes, entries) in [(MAX_RETAINED_BYTES + 1, 0), (0, MAX_RETAINED_ENTRIES + 1)] {
            assert_eq!(
                check_budget(&root, &history, bytes, entries)
                    .unwrap_err()
                    .code(),
                ErrorCode::ResourceLimit
            );
        }
    }
}

fn install_fixture() -> (tempfile::TempDir, DiskRoot, String, String) {
    let temp = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let root = DiskRoot::open(temp.path()).unwrap();
    let history = crate::new_id("history").unwrap();
    let file = crate::new_id("file").unwrap();
    (temp, root, history, file)
}
fn pending(root: &DiskRoot, history: &str, file: &str) -> PreparedInstall {
    PreparedInstall::capture(
        root,
        root.path(),
        (root.path(), root.directory_identity(root.path()).unwrap()),
        (
            Scope::file(history, file).unwrap(),
            InstallAdmission::NewOperation,
        ),
        b"new exact database".to_vec(),
        1024,
        |_| Ok(()),
    )
    .unwrap()
}

#[test]
fn capture_and_missing_unpublished_read_never_create_retention_directories() {
    let (_temp, root, history, file) = install_fixture();
    let prepared = pending(&root, &history, &file);
    assert!(prepared.unpublished_identity(&root).is_err());
    assert!(!root.path().join(".zxs-layout").exists());
}

#[test]
fn unpublished_intent_preserves_the_old_alias_and_resumes_the_same_original_pair() {
    let (_temp, root, history, file) = install_fixture();
    let target = root
        .path()
        .join(crate::container_storage::file_database::NAME);
    root.write_atomic(&target, b"old database", false).unwrap();
    let old_inode = root
        .open_read_regular(&target)
        .unwrap()
        .metadata()
        .unwrap()
        .ino();
    let mut first = pending(&root, &history, &file);
    first.stage_unpublished(&root, &mut |_| Ok(())).unwrap();
    let (stage, envelope) = first.resume.as_ref().unwrap();
    let retained = stage.path().to_path_buf();
    let new_inode = envelope.new.inode;
    assert_eq!(root.read_bytes(&target, 1024).unwrap(), b"old database");
    assert_eq!(
        root.open_read_regular(&target)
            .unwrap()
            .metadata()
            .unwrap()
            .ino(),
        old_inode
    );
    drop(first);
    let mut resumed = pending(&root, &history, &file);
    resumed.stage_unpublished(&root, &mut |_| Ok(())).unwrap();
    assert_eq!(resumed.resume.as_ref().unwrap().0.path(), retained);
    assert_eq!(resumed.resume.as_ref().unwrap().1.new.inode, new_inode);
    resumed.apply(&root, &mut |_| Ok(())).unwrap();
    assert_eq!(
        root.read_bytes(&target, 1024).unwrap(),
        b"new exact database"
    );
    assert_eq!(
        root.read_bytes(&retained.join("payload"), 1024).unwrap(),
        b"old database"
    );
    assert!(
        pending(&root, &history, &file)
            .stage_unpublished(&root, &mut |_| Ok(()))
            .is_err()
    );
}

#[test]
fn missing_original_pending_payload_cannot_be_recreated_as_an_unpublished_intent() {
    let (_temp, root, history, file) = install_fixture();
    let mut first = pending(&root, &history, &file);
    first.stage_unpublished(&root, &mut |_| Ok(())).unwrap();
    let payload = first.resume.as_ref().unwrap().0.path().join("payload");
    std::fs::remove_file(&payload).unwrap();
    drop(first);
    let mut recovered = pending(&root, &history, &file);
    assert!(recovered.stage_unpublished(&root, &mut |_| Ok(())).is_err());
    assert!(!payload.exists());
    assert!(
        !root
            .path()
            .join(crate::container_storage::file_database::NAME)
            .exists()
    );
}

#[test]
fn interrupted_unpublished_intent_is_not_exchanged_and_can_only_reuse_its_original_pair() {
    let (_temp, root, history, file) = install_fixture();
    let mut first = pending(&root, &history, &file);
    assert!(
        first
            .stage_unpublished(&root, &mut |point| {
                if point == "after_database_intent" {
                    Err(invalid("interrupted before any alias exchange"))
                } else {
                    Ok(())
                }
            })
            .is_err()
    );
    assert!(
        !root
            .path()
            .join(crate::container_storage::file_database::NAME)
            .exists()
    );
    drop(first);
    let mut recovered = pending(&root, &history, &file);
    assert!(recovered.resume.is_some());
    recovered.stage_unpublished(&root, &mut |_| Ok(())).unwrap();
    assert!(
        !root
            .path()
            .join(crate::container_storage::file_database::NAME)
            .exists()
    );
}
