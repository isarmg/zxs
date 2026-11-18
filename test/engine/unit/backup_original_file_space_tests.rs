use super::*;
use crate::ErrorCode;
use platform_io::fs_safety::RelativePath;
use std::{fs, num::NonZeroU64, os::unix::fs::PermissionsExt, path::Path};

fn stage(binding: &str, payload: u64) -> OriginalFileStage {
    OriginalFileStage {
        generation: RelativePath::new(Path::new(".zxs-layout/database-installs").join(binding))
            .unwrap(),
        payload,
    }
}

#[test]
fn original_exchange_space_counts_missing_parents_per_history_and_never_adopts_orphans() {
    let temporary = tempfile::Builder::new().permissions(fs::Permissions::from_mode(0o700))
        .tempdir().unwrap();
    let root = DiskRoot::open(temporary.path()).unwrap();
    let one = root.path().join("first");
    let two = root.path().join("second");
    let existing = one.join(".zxs-layout/database-installs/present");
    let unknown = existing.join("orphan");
    root.private_dir(&unknown).unwrap();
    root.write_atomic(&unknown.join("payload"), b"unowned partial", false).unwrap();
    root.private_dir(&two).unwrap();
    let first = root.private_child(&one).unwrap();
    let second = root.private_child(&two).unwrap();
    let mut space = BackupOriginalFileSpace::capture(&root).unwrap();
    let unit = space.volume.unit.get();
    let stop = AtomicBool::new(false);
    // An existing generation with an orphan still needs a new attempt,
    // payload and intent. Two FILEs share one pair of fixed ancestors.
    space.add_history(&first, vec![stage("present", 1), stage("absent", unit + 1)], &stop)
        .unwrap();
    // The same binding name on a different held history borrows no parents.
    space.add_history(&second, vec![stage("present", 0)], &stop).unwrap();
    let demand = &space.volume.needed;
    assert_eq!((demand.directories, demand.files), (7, 6));
    // Seven directories at two units; payloads cost two, three and one
    // units; every intent gets its own rounded filespace and parent allowance.
    let one_intent = OriginalFileStage::INTENT_LIMIT.div_ceil(unit) + 1;
    assert_eq!(demand.bytes, (14 + 6 + 3 * one_intent) * unit);
    assert!(!existing.parent().unwrap().join("absent").exists());
    assert!(!two.join(".zxs-layout").exists());
    assert_eq!(fs::read(unknown.join("payload")).unwrap(), b"unowned partial");

    let bad = existing.parent().unwrap().join("file-not-generation");
    root.write_atomic(&bad, b"not a directory", false).unwrap();
    assert!(space.add_history(&first, vec![stage("file-not-generation", 7)], &stop).is_err());
    assert_eq!(fs::read(&bad).unwrap(), b"not a directory");
}

#[test]
fn original_exchange_allocation_preserves_floors_and_requires_a_separate_receipt_inode() {
    let unit = NonZeroU64::new(65_536).unwrap();
    let check = |space: &Space, retained, complete, reserve| {
        let material = Needed { files: 4, directories: 3, bytes: 13 * unit.get() };
        let mut marker = Needed::default();
        marker.charge(EntryCost::File(unit.get() + 1), unit)?;
        require_counts(material, retained, marker, complete, reserve, space)
    };
    // Measured units are larger than either old floor. Missing local FILE
    // completion consumes a separate inode, not a retained entry or an FD.
    let exact = 16 * unit.get() + 17;
    let mut available = Space { unit, bytes: exact, inodes: Some(8) };
    check(&available, 1, 1, 17).unwrap();
    available.inodes = Some(7);
    assert_eq!(check(&available, 1, 1, 17).unwrap_err().code(), ErrorCode::NoSpace);
    available.inodes = None;
    available.bytes -= 1;
    assert_eq!(check(&available, 1, 1, 17).unwrap_err().code(), ErrorCode::NoSpace);
    // An older, larger retained predecessor and existing completion reserve
    // remain minima even when actual new material uses less.
    available.bytes = 60 * unit.get() + 19;
    check(&available, 20 * unit.get(), 40 * unit.get(), 19).unwrap();
    available.bytes -= 1;
    assert_eq!(check(&available, 20 * unit.get(), 40 * unit.get(), 19).unwrap_err().code(),
        ErrorCode::NoSpace);
    assert_eq!(check(&available, u64::MAX, 0, 0).unwrap_err().code(), ErrorCode::ResourceLimit);
}
