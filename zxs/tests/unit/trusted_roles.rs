//! The startup decision uses only evidence observed in the selected path walks.
use super::*;
use crate::test_support::PrivateTemp;
use std::{fs::DirBuilder, os::unix::fs::DirBuilderExt};

#[test]
fn selected_root_in_either_visible_chain_is_overlap_but_a_shared_parent_is_not() {
    let temp = PrivateTemp::under(&std::env::temp_dir());
    let a = temp.0.join("left");
    let b = temp.0.join("right");
    let child = a.join("deeper");
    for path in [&a, &b, &child] {
        DirBuilder::new().mode(0o700).create(path).unwrap();
    }
    let a = TrustedRoot::open(&a).unwrap();
    let mut b = TrustedRoot::open(&b).unwrap();
    let child = TrustedRoot::open(&child).unwrap();
    assert!(!a.overlaps(&b));
    assert!(a.overlaps(&child));
    assert!(child.ancestors.contains(&a.held.entity));
    assert!(!b.ancestors.contains(&a.held.entity));
    // Simulate the evidence captured beneath an alias; no mount is created.
    b.ancestors.push(a.held.entity);
    assert!(a.overlaps(&b));
    assert!(b.overlaps(&a));
}
