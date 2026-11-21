use super::*;
#[test]
fn kernel_subtree_and_missing_mounts_are_not_whole_ancestor_evidence(){
    let table=Mounts::parse(b"14 1 1:2 / / rw - tmpfs tmpfs rw\n21 14 1:2 /private/subdir /an-alias rw - tmpfs tmpfs rw\n").unwrap();
    assert_eq!(table.0.get(&14).map(|m|m.whole),Some(true));
    assert_eq!(table.0.get(&21).map(|m|m.whole),Some(false));
    assert!(table.0.get(&99).is_none());
    assert!(Mounts::parse(b"14 1 1:2 / / rw\n14 1 1:2 / / rw\n").is_err());
    assert!(Mounts::parse(b"14 1 1:2 / / rw").is_err());
}

#[test]
fn distinct_mounts_may_still_be_views_of_the_same_superblock(){
    let table=Mounts::parse(b"14 1 1:2 / /a rw - ext4 /dev/a rw\n21 14 1:2 /project /alias rw - ext4 /dev/a rw\n42 14 1:3 / /b rw - ext4 /dev/b rw\n").unwrap();
    assert!(!table.separate(14,14).unwrap());assert!(!table.separate(14,21).unwrap());
    assert!(table.separate(14,42).unwrap());assert!(table.separate(42,99).is_err());
    assert!(Mounts::parse(b"14 1 invalid / /a rw - ext4 /dev/a rw\n").is_err());
}

#[test]
fn same_inode_across_mounts_is_not_an_end_of_ancestry(){
    use crate::fs::{Entity,Location,lineage_end};
    let e=Entity{device:17,inode:2};
    let root=Location{entity:e,mount:5};
    let same_inode_other_mount=Location{entity:e,mount:19};
    let nested=Location{entity:Entity{device:21,inode:2},mount:31};
    assert!(lineage_end(same_inode_other_mount,root,root).is_err());
    assert!(lineage_end(nested,nested,root).is_err());
    assert!(!lineage_end(nested,root,root).unwrap());
    assert!(lineage_end(root,root,root).unwrap());
}
