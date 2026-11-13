use crate::business::namespace::container_name::{RenameError, plan_file_names};
use crate::{Entry, NamespaceEntryKind};
use name_policy::{Name, PortablePath};
use std::collections::BTreeMap;
use storage_contract::FileId;

fn file(path: &str, number: u8) -> Entry {
    Entry {
        entry_id: format!("entry_{number:032x}"),
        file_id: format!("file_{number:032x}"),
        version_id: format!("ver_{number:032x}"),
        path: PortablePath::parse(path).unwrap(),
        kind: NamespaceEntryKind::File,
        size: 7,
    }
}

fn name(number: u8, text: &str) -> (FileId, Name) {
    (format!("file_{number:032x}").parse().unwrap(), Name::parse(text).unwrap())
}

#[test]
fn current_names_plan_every_alias_without_changing_sources_or_content_identity() {
    let entries = vec![file("a/old.txt", 1), file("b/stale.txt", 1), file("c/other.txt", 2)];
    let original = entries.clone();
    let names = BTreeMap::from([name(1, "最新.txt")]);
    let changes = plan_file_names(&entries, &names).unwrap();
    assert_eq!(changes.len(), 2);
    for (change, entry) in changes.iter().zip(&entries) {
        assert_eq!(change.entry_id.to_string(), entry.entry_id);
        assert_eq!(change.file_id.to_string(), entry.file_id);
        assert_eq!(change.before, entry.path);
        assert_eq!(change.after.file_name(), "最新.txt");
        assert_eq!(change.after.parent(), entry.path.parent());
    }
    assert_eq!(entries, original);
}

#[test]
fn current_names_reject_alias_collisions_and_path_overflow_without_partial_changes() {
    let entries = vec![file("a/old.txt", 1), file("b/old.txt", 1), file("b/NEW.txt", 2)];
    let original = entries.clone();
    assert!(matches!(
        plan_file_names(&entries, &BTreeMap::from([name(1, "new.txt")])),
        Err(RenameError::Collision(_)),
    ));
    assert_eq!(entries, original);
    let entries = vec![file(&format!("{}/x.txt", "a".repeat(180)), 1)];
    let original = entries.clone();
    assert!(matches!(
        plan_file_names(&entries, &BTreeMap::from([name(1, &"b".repeat(60))])),
        Err(RenameError::Path(_)),
    ));
    assert_eq!(entries, original);
}

#[test]
fn current_names_admit_atomic_swaps_case_changes_and_exact_no_ops() {
    let entries = vec![file("a.txt", 1), file("b.txt", 2)];
    let changes = plan_file_names(
        &entries,
        &BTreeMap::from([name(1, "b.txt"), name(2, "a.txt")]),
    )
    .unwrap();
    assert_eq!(changes[0].after.as_str(), "b.txt");
    assert_eq!(changes[1].after.as_str(), "a.txt");
    let changes = plan_file_names(&entries, &BTreeMap::from([name(1, "A.txt")])).unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].after.as_str(), "A.txt");
    assert!(plan_file_names(&entries, &BTreeMap::from([name(1, "a.txt")])).unwrap().is_empty());
}
