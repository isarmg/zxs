use super::*;
use std::{fs, os::unix::fs::PermissionsExt};

#[test]
fn a_pending_vfs_receipt_is_not_completed_and_requires_a_published_original_intent() {
    let temp = tempfile::tempdir().unwrap();
    let host = BundleDestination::open(temp.path()).unwrap();
    let inspect = || inspect_records(&host, "original", b"plan", "completed", b"receipt");
    assert_eq!(
        inspect().unwrap(),
        (BundleState::Absent, BundleState::Absent)
    );
    let pending = temp.path().join("completed.pending");
    fs::create_dir(&pending).unwrap();
    fs::set_permissions(&pending, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(inspect().is_err());
    host.publish("original", &[("plan.json", b"plan")]).unwrap();
    assert_eq!(
        inspect().unwrap(),
        (BundleState::Published, BundleState::Pending)
    );
    fs::write(pending.join("receipt.json"), b"receipt").unwrap();
    fs::set_permissions(
        pending.join("receipt.json"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    assert_eq!(
        inspect().unwrap(),
        (BundleState::Published, BundleState::Pending)
    );
    host.publish("completed", &[("receipt.json", b"receipt")])
        .unwrap();
    assert_eq!(
        inspect().unwrap(),
        (BundleState::Published, BundleState::Published)
    );
    fs::rename(
        temp.path().join("original"),
        temp.path().join("original.pending"),
    )
    .unwrap();
    assert!(inspect().is_err());
    assert_eq!(
        fs::read(temp.path().join("completed/receipt.json")).unwrap(),
        b"receipt"
    );
}
