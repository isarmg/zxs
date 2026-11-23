use super::*;
#[test]
fn subtree_mounts_share_their_superblock_and_ambiguous_mountinfo_is_rejected() {
    let table=Mounts::parse(b"14 1 1:2 / / rw - tmpfs tmpfs rw\n21 14 1:2 /private/subdir /an-alias rw - tmpfs tmpfs rw\n").unwrap();
    assert!(!table.separate(14, 21).unwrap());
    assert!(table.separate(14, 99).is_err());
    assert!(Mounts::parse(b"14 1 1:2 relative / rw\n").is_err());
    assert!(Mounts::parse(b"14 1 1:2 / / rw\n14 1 1:2 / / rw\n").is_err());
    assert!(Mounts::parse(b"14 1 1:2 / / rw").is_err());
}

#[test]
fn distinct_mounts_may_still_be_views_of_the_same_superblock() {
    let table=Mounts::parse(b"14 1 1:2 / /a rw - ext4 /dev/a rw\n21 14 1:2 /project /alias rw - ext4 /dev/a rw\n42 14 1:3 / /b rw - ext4 /dev/b rw\n").unwrap();
    assert!(!table.separate(14, 14).unwrap());
    assert!(!table.separate(14, 21).unwrap());
    assert!(table.separate(14, 42).unwrap());
    assert!(table.separate(42, 99).is_err());
    assert!(Mounts::parse(b"14 1 invalid / /a rw - ext4 /dev/a rw\n").is_err());
}
