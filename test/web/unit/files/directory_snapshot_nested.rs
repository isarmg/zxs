use super::tests::{head, state, unborn_row};
use super::{ReadItem, Review, Target, can_initialize, can_review};
use crate::files::test_entries::parse_entry;
use std::collections::BTreeSet;
use storage_contract::snapshot_worktree::discovery::SnapshotHeadDiscovery;

#[test]
fn nested_ordinary_review_binds_actual_page_parent_and_cannot_initialize_or_manage_worktrees() {
    let mut state = state();
    state.files.repository_path = "first/second".into();
    let owner = Target::capture(&state).unwrap();
    let mut row = unborn_row();
    row["path"] = "first/second/first.txt".into();
    row["repository_path"] = row["path"].clone();
    let actual = parse_entry(row).unwrap();
    let item = ReadItem::from_page(&actual, &owner).unwrap().unwrap();
    assert_eq!(item.target.path.as_str(), "first/second/first.txt");
    assert!(item.endpoint().contains("path=first%2Fsecond%2Ffirst.txt"));
    let root_row = parse_entry(unborn_row()).unwrap();
    assert!(ReadItem::from_page(&root_row, &owner).is_err());
    let mut root_head = head();
    root_head.head = SnapshotHeadDiscovery::NotInitialized;
    state.files.directory_snapshot.review = Some(Review {
        target: owner.clone(),
        head: root_head,
        candidates: Vec::new(),
        selected: BTreeSet::new(),
    });
    assert!(can_review(&state));
    assert!(
        !can_initialize(&state),
        "an empty child cannot initialize the real host"
    );
    state.files.directory_snapshot.review.as_mut().unwrap().head = head();
    assert!(
        !super::worktree::available(&state),
        "no new worktree parent is inferred"
    );
    state.files.repository_path = "other".into();
    assert!(!owner.current(&state));
    assert!(ReadItem::from_page(&actual, &Target::capture(&state).unwrap()).is_err());
    state.files.listing_failed = true;
    assert!(Target::capture(&state).is_none());
}
