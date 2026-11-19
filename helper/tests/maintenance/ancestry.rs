//! Static controls and future separately provisioned native evidence; no mount operations.
use super::*;
use crate::{path_rules::Kind, test_area::subtree_bind_directory};
use std::fs;

#[test]
fn actual_filesystem_root_is_field_four_not_the_visible_mountpoint_or_source() {
    let table = Mounts::parse(b"41 1 8:2 / /home\\040area rw shared:9 - ext4 /dev/test rw\n42 41 8:2 /hidden / rw - ext4 /dev/test rw\n43 41 8:3 / /disk rw - xfs /dev/other rw\n").unwrap();
    assert_eq!(table.whole(41, makedev(8, 2)).unwrap().parent, 1);
    assert_eq!(table.whole(43, makedev(8, 3)).unwrap().parent, 41);
    for (id,dev) in [(42, makedev(8, 2)), (41, makedev(8, 3)), (9999, makedev(8, 2))] {
        assert_eq!(table.whole(id, dev).err().unwrap().kind(), io::ErrorKind::Unsupported);
    }
}

#[test]
fn a_self_parent_whole_mount_at_the_selected_root_positive_boundary_is_required() {
    let table = Mounts::parse(b"41 41 8:2 / / rw - ext4 /dev/test rw\n").unwrap();
    let root = Mount { id: 41, at_root: Some(true) };
    assert!(table.require_root_boundary(root, makedev(8, 2)).is_ok());
    for at_root in [None, Some(false)] {
        assert_eq!(table.require_root_boundary(Mount { at_root, ..root }, makedev(8, 2)).unwrap_err().kind(),
                   io::ErrorKind::Unsupported);
    }
}

#[test]
fn missing_parent_cannot_distinguish_a_chroot_at_a_full_mount_from_an_initramfs_host() {
    let table = Mounts::parse(b"41 87 8:2 / / rw - ext4 /dev/test rw\n").unwrap();
    let root = Mount { id: 41, at_root: Some(true) };
    assert!(!table.0.contains_key(&87));
    assert!(table.whole(41, makedev(8, 2)).is_ok());
    assert_eq!(table.require_root_boundary(root, makedev(8, 2)).unwrap_err().kind(), io::ErrorKind::Unsupported);
}

#[test]
fn partial_oversized_and_ambiguous_mount_evidence_is_never_a_fallback() {
    let row = b"41 1 8:2 / / rw - ext4 /dev/test rw\n";
    for bytes in [&b""[..], &row[..row.len()-1], b"1 1 8:2 / / rw - ext4\n",
                  b"+1 2 8:2 / / rw - ext4 /dev/test rw\n",
                  b"1 2 0:4294967296 / / rw - ext4 /dev/test rw\n",
                  b"1 2 8:2 / / rw ext4 /dev/test rw\n"] {
        assert!(Mounts::parse(bytes).is_err());
    }
    assert!(Mounts::parse(&row.repeat(2)).is_err(), "duplicate even when otherwise identical");
    assert!(Mounts::parse(&vec![b'x'; TABLE_BYTES+1]).is_err());
    let mut lines = Vec::new();
    for id in 1..=TABLE_ROWS {
        use std::io::Write;
        writeln!(&mut lines, "{id} 1 8:2 / / rw - ext4 /dev/test rw").unwrap();
    }
    assert!(Mounts::parse(&lines).is_ok());
    lines.extend_from_slice(b"999999 1 8:2 / / rw - ext4 /dev/test rw\n");
    assert!(Mounts::parse(&lines).is_err());
}

#[test]
fn independently_provisioned_subtree_bind_is_unsupported_not_an_empty_or_damaged_volume() {
    let private = subtree_bind_directory();
    let name = Pinned::open(private.path(), Kind::StorageRoot).unwrap();
    let held = name.storage_directory().unwrap();
    let table = Mounts::current().unwrap();
    let mount = Mount::on(&held).unwrap().unwrap();
    assert!(!table.0[&mount.id].whole,
        "test precondition: choose an actual subtree bind, not a directory on a full-root mount");
    assert_eq!(table.unclaimed_chain(&name, &held).unwrap_err().kind(), io::ErrorKind::Unsupported);
    assert_eq!(fs::read_dir(private.path()).unwrap().count(), 0);
}
