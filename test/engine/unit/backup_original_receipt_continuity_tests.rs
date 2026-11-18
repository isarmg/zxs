use super::*;
use std::{fs, os::unix::fs::PermissionsExt};

#[test]
fn seen_original_completion_cannot_be_lost_replaced_or_borrowed_from_another_root() {
    let temporary = tempfile::Builder::new().permissions(fs::Permissions::from_mode(0o700))
        .tempdir().unwrap();
    let system = DiskRoot::open(temporary.path()).unwrap();
    let roots = ["one", "two", "outside"].map(|name| {
        let path = temporary.path().join(name);
        system.private_dir(&path).unwrap();
        system.private_child(&path).unwrap()
    });
    let path = roots[0].path().join(COMPLETE);
    let parked = temporary.path().join("exact-owned-marker");
    roots[0].write_atomic(&path, b"selected local FILE material", false).unwrap();
    let mut seen = Seen::default();
    let record = Receipt::read(&roots[0]).unwrap().unwrap();
    seen.remember(&roots[0], Some(&record)).unwrap();
    seen.remember(&roots[1], None).unwrap();
    assert!(seen.read(&roots[2]).is_err());
    assert!(seen.read(&roots[1]).unwrap().is_none());
    record.require(&roots[0], b"selected local FILE material").unwrap();
    assert!(record.require(&roots[0], b"wrong current FILEs").is_err());
    fs::rename(&path, &parked).unwrap();
    assert!(seen.read(&roots[0]).is_err());
    assert!(!path.exists());
    roots[0].write_atomic(&path, b"selected local FILE material", false).unwrap();
    assert!(seen.read(&roots[0]).is_err());
    assert!(record.require(&roots[0], b"selected local FILE material").is_err());
    assert_eq!(fs::read(path).unwrap(), fs::read(parked).unwrap());
    // A genuinely unseen root can acquire a receipt, but never from a peer
    // and never without comparing its own freshly verified physical material.
    roots[1].write_atomic(&roots[1].path().join(COMPLETE), b"other proof", false).unwrap();
    let later = seen.read(&roots[1]).unwrap().unwrap();
    assert!(later.require(&roots[1], b"selected local FILE material").is_err());
    later.require(&roots[1], b"other proof").unwrap();
}
