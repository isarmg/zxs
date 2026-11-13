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
fn equivalent_lookup_rename_preserves_canonical_owner_and_cross_parent_destination() {
    use crate::business::namespace::container_name::renamed_file_location;
    let owner = PortablePath::parse("physical/Å.txt").unwrap();
    let source = name_policy::LookupPath::parse("Cafe\u{301}/A\u{30a}.txt").unwrap();
    let renamed = renamed_file_location(
        &owner, &source, &PortablePath::parse("Café/가.txt").unwrap(),
    ).unwrap();
    assert_eq!(renamed.as_str(), "physical/가.txt");
    assert_eq!(owner.as_str(), "physical/Å.txt");
    let moved = renamed_file_location(
        &owner, &source, &PortablePath::parse("elsewhere/deeper/가.txt").unwrap(),
    ).unwrap();
    assert_eq!(moved.as_str(), "elsewhere/deeper/가.txt");
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

#[test]
fn namespace_file_rename_updates_aliases_and_chained_source_identity() {
    let mut entries = vec![file("a/old.txt", 1), file("b/old.txt", 1)];
    crate::business::ensure_parents(
        &mut entries, &PortablePath::parse("a/old.txt").unwrap(),
    ).unwrap();
    crate::business::ensure_parents(
        &mut entries, &PortablePath::parse("b/old.txt").unwrap(),
    ).unwrap();
    let before = entries.clone();
    let operations = [
        crate::NamespaceOperation::parse(
            crate::NamespaceOperationKind::Rename, "a/old.txt", Some("a/new.txt"),
        ).unwrap(),
        crate::NamespaceOperation::parse(
            crate::NamespaceOperationKind::Rename, "b/new.txt", Some("b/final.txt"),
        ).unwrap(),
    ];
    let files = crate::business::namespace::container_name::operation_files(
        &before, &operations,
    ).unwrap();
    assert_eq!(files, vec![Some(name(1, "unused").0); 2]);
    for operation in &operations {
        crate::business::apply_namespace(&mut entries, operation).unwrap();
    }
    let files = entries.iter().filter(|entry| entry.kind == NamespaceEntryKind::File)
        .collect::<Vec<_>>();
    assert_eq!(files.len(), 2);
    assert_eq!(files[0].path.as_str(), "a/final.txt");
    assert_eq!(files[1].path.as_str(), "b/final.txt");
    for file in files {
        assert_eq!(file.version_id, before[0].version_id);
        assert_eq!(file.file_id, before[0].file_id);
    }
}

#[test]
fn namespace_rename_rejects_a_collision_at_another_occurrence() {
    let mut entries = vec![file("a/old.txt", 1), file("b/old.txt", 1), file("b/new.txt", 2)];
    let operation = crate::NamespaceOperation::parse(
        crate::NamespaceOperationKind::Rename, "a/old.txt", Some("a/new.txt"),
    ).unwrap();
    assert_eq!(
        crate::business::apply_namespace(&mut entries, &operation).unwrap_err().code(),
        storage_contract::ErrorCode::NameCollision,
    );
}

#[test]
fn directory_source_names_move_each_alias_context_without_rewriting_frozen_source() {
    let entries = vec![file("a/old", 1), file("b/stale", 1)];
    let context = |node: &str| crate::FileContext {
        branch_id: "br_00000000000000000000000000000001".into(),
        node_id: Some(node.into()),
        inherited_from: None,
        selection_generation: 0,
    };
    let mut branch = crate::DirectoryBranch {
        directory_branch_id: "dbr_00000000000000000000000000000001".into(),
        repository_id: "repo_00000000000000000000000000000001".into(),
        directory_id: "entry_00000000000000000000000000000002".into(),
        path: String::new(), name: "main".into(), parent_branch_id: None,
        head_tree_id: "dtree_00000000000000000000000000000001".into(),
        commit_id: "commit_00000000000000000000000000000001".into(),
        entries,
        file_contexts: BTreeMap::from([
            ("a/old".into(), context("first")), ("b/stale".into(), context("second")),
        ]),
        file_branches: BTreeMap::new(), inherited_files: BTreeMap::new(),
        directory_refs: BTreeMap::new(), materialized: true, sequence: 1,
    };
    let original = branch.clone();
    let mut state = crate::State::default();
    let id = branch.entries[0].file_id.clone();
    state.file_locations.insert(id.clone(), "latest".into());
    state.file_names.insert(id, "latest".into());
    crate::business::namespace::container_name::normalize_directory_branch(&state, &mut branch)
        .unwrap();
    assert_eq!(branch.entries[0].path(), "a/latest");
    assert_eq!(branch.entries[1].path(), "b/latest");
    assert_eq!(branch.file_contexts["a/latest"], original.file_contexts["a/old"]);
    assert_eq!(branch.file_contexts["b/latest"], original.file_contexts["b/stale"]);
    assert_eq!(original.entries[0].path(), "a/old");
    assert_eq!(branch.head_tree_id, original.head_tree_id);
}
