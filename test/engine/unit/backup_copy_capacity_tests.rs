use super::*;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};

fn root() -> (tempfile::TempDir, DiskRoot) {
    let temp = tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let held = DiskRoot::open(temp.path()).unwrap();
    (temp, held)
}

#[test]
fn copy_census_counts_implicit_shared_parents_zero_files_and_block_rounding() {
    let (_temp, held) = root();
    let mut census = Census::new(&held).unwrap();
    let unit = census.unit.get();
    census
        .file("histories/a/versions/v/checksums.bin".into(), 1)
        .unwrap();
    census
        .file("histories/a/versions/v/content/empty".into(), 0)
        .unwrap();
    census
        .file("histories/a/versions/v/version.json".into(), unit + 1)
        .unwrap();
    census
        .file("histories/b/publication.json".into(), unit)
        .unwrap();
    census
        .file("pool-snapshot/objects/state-page/a.json".into(), 1)
        .unwrap();
    census
        .file("pool-snapshot/objects/state-page/b.json".into(), 1)
        .unwrap();

    assert_eq!(census.needed.files, 6);
    assert_eq!(census.needed.directories, 9);
    // Six file-data units, six new file names, nine directory blocks and nine directory names.
    assert_eq!(census.needed.bytes, 30 * unit);
    assert_eq!(fs::read_dir(held.path()).unwrap().count(), 0);
}

#[test]
fn partial_target_charges_only_missing_entries_and_keeps_retained_material() {
    let (_temp, held) = root();
    let retained = held.path().join("pool-snapshot/objects/state-page/a.json");
    held.private_dir(retained.parent().unwrap()).unwrap();
    held.write_atomic(&retained, b"unchanged", false).unwrap();
    let mut census = Census::new(&held).unwrap();
    let unit = census.unit.get();
    // Existing is an allocation observation only, not evidence for the declared bytes.
    census
        .file("pool-snapshot/objects/state-page/a.json".into(), 99)
        .unwrap();
    census
        .file("pool-snapshot/objects/state-page/b.json".into(), 0)
        .unwrap();
    census
        .file("pool-snapshot/objects/state-page/c.json".into(), unit + 1)
        .unwrap();
    assert_eq!((census.needed.directories, census.needed.files), (0, 2));
    assert_eq!(census.needed.bytes, 4 * unit);
    assert_eq!(fs::read(retained).unwrap(), b"unchanged");
}

#[test]
fn held_census_never_treats_links_as_stored_entries_or_follows_a_replaced_root() {
    let (temp, parent) = root();
    let named = temp.path().join("named");
    parent.private_dir(&named).unwrap();
    let held = DiskRoot::open(&named).unwrap();
    let inside = named.join("before");
    fs::create_dir(&inside).unwrap();
    fs::write(inside.join("a"), b"held").unwrap();
    symlink("before", named.join("link")).unwrap();
    let mut census = Census::new(&held).unwrap();
    assert!(census.file("link/a".into(), 4).is_err());
    fs::hard_link(inside.join("a"), inside.join("b")).unwrap();
    let mut census = Census::new(&held).unwrap();
    assert!(census.file("before/a".into(), 4).is_err());
    assert_eq!(fs::read(inside.join("b")).unwrap(), b"held");

    let moved = temp.path().join("retained");
    fs::rename(&named, &moved).unwrap();
    fs::create_dir(&named).unwrap();
    fs::write(named.join("elsewhere"), b"outside").unwrap();
    let mut census = Census::new(&held).unwrap();
    census.file("elsewhere".into(), 7).unwrap();
    assert_eq!((census.needed.directories, census.needed.files), (0, 1));
    assert_eq!(fs::read(named.join("elsewhere")).unwrap(), b"outside");
}

#[test]
fn bytes_inodes_and_existing_reserve_are_independent() {
    let unit = NonZeroU64::new(4096).unwrap();
    let mut work = Needed::default();
    let mut space = Space {
        unit,
        bytes: 64 * 1024,
        inodes: Some(0),
    };
    work.require(&space).unwrap();
    space.bytes -= 1;
    assert_eq!(work.require(&space).unwrap_err().code(), ErrorCode::NoSpace);

    work.charge(EntryCost::File(0), unit).unwrap();
    space.bytes = RESERVE + 4096;
    assert_eq!(work.require(&space).unwrap_err().code(), ErrorCode::NoSpace);
    space.inodes = Some(1);
    work.require(&space).unwrap();
    space.bytes -= 1;
    assert_eq!(work.require(&space).unwrap_err().code(), ErrorCode::NoSpace);
    // No fixed inode count is reported on every supported filesystem.
    space.bytes += 1;
    space.inodes = None;
    work.require(&space).unwrap();
    assert_eq!(
        work.charge(EntryCost::File(u64::MAX), unit)
            .unwrap_err()
            .code(),
        ErrorCode::ResourceLimit,
    );
}

#[test]
fn duplicate_out_of_order_and_file_parent_addresses_do_not_undercount() {
    let (_temp, held) = root();
    for (first, next) in [("a", "a"), ("b", "a"), ("a", "a/b")] {
        let mut census = Census::new(&held).unwrap();
        census.file(first.into(), 0).unwrap();
        assert_eq!(
            census.file(next.into(), 0).unwrap_err().code(),
            ErrorCode::HistoryPackageInvalid
        );
    }
    let mut census = Census::new(&held).unwrap();
    assert!(census.file("../escape".into(), 0).is_err());
}
