use super::*;
use std::{fs, os::unix::fs::PermissionsExt};

fn payload() -> payload::Payload {
    payload::Payload {
        bytes: b"exact build fixture",
        version: "fixture",
        commit: "a",
        tree: "b",
        target: "x86_64-unknown-linux-gnu",
        sha256: "fixture-digest",
    }
}

#[test]
fn baseline_requires_exact_payload_and_parent_and_never_adopts_another_version() {
    let host_root = tempfile::tempdir().unwrap();
    let program_root = tempfile::tempdir().unwrap();
    let host = BundleDestination::open(host_root.path()).unwrap();
    let program = ProgramDestination::open(&program_root.path().join("zxs")).unwrap();
    let original = expected(&host, &program, payload()).unwrap();
    assert!(!existing(&host, &original).unwrap());
    assert!(require(&host, &original).is_err());
    host.publish(NAME, &[("receipt.json", &original)]).unwrap();
    assert!(existing(&host, &original).unwrap());
    let mut other = payload();
    other.commit = "another build";
    assert!(existing(&host, &expected(&host, &program, other).unwrap()).is_err());
    assert_eq!(
        fs::read(host_root.path().join(NAME).join("receipt.json")).unwrap(),
        original
    );
}

#[test]
fn baseline_staging_without_a_receipt_cannot_become_a_new_software_generation() {
    let host_root = tempfile::tempdir().unwrap();
    let program_root = tempfile::tempdir().unwrap();
    let pending = host_root.path().join(format!("{NAME}.pending"));
    fs::create_dir(&pending).unwrap();
    fs::set_permissions(&pending, fs::Permissions::from_mode(0o700)).unwrap();
    let host = BundleDestination::open(host_root.path()).unwrap();
    let program = ProgramDestination::open(&program_root.path().join("zxs")).unwrap();
    assert!(existing(&host, &expected(&host, &program, payload()).unwrap()).is_err());
    assert!(fs::read_dir(&pending).unwrap().next().is_none());
}
