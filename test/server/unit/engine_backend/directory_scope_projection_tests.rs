//! Wire projection keeps only the sources belonging to admitted current entries.
use super::*;
use std::collections::BTreeMap;
use storage_contract::{
    DirectoryBranch, DirectoryReference, Entry, FileContext, NamespaceEntryKind,
};

fn file(id: &str, path: &str, file_id: &str) -> Entry {
    Entry {
        entry_id: id.into(),
        path: name_policy::PortablePath::parse(path).unwrap(),
        kind: NamespaceEntryKind::File,
        file_id: file_id.into(),
        version_id: format!("version-{id}"),
        size: 7,
    }
}
fn directory(id: &str, path: &str) -> Entry {
    Entry {
        entry_id: id.into(),
        path: name_policy::PortablePath::parse(path).unwrap(),
        kind: NamespaceEntryKind::Directory,
        file_id: String::new(),
        version_id: String::new(),
        size: 0,
    }
}
fn fixture() -> DirectoryBranch {
    let entries = vec![
        file("gone", "gone.txt", "gone-file"),
        directory("hidden-directory", "hidden"),
        file("alias", "hidden/alias.txt", "shared-file"),
        directory("visible-directory", "Visible"),
        file("alias", "Visible/Alias.txt", "shared-file"),
    ];
    let file_contexts = entries
        .iter()
        .filter(|entry| entry.kind == NamespaceEntryKind::File)
        .map(|entry| {
            (
                entry.lookup_key().to_owned(),
                FileContext {
                    node_id: Some(format!("node-{}", entry.lookup_key())),
                    selection_generation: 2,
                    branch_id: format!("branch-{}", entry.file_id),
                    inherited_from: Some(format!("origin-{}", entry.file_id)),
                },
            )
        })
        .collect();
    DirectoryBranch {
        directory_branch_id: "scope".into(),
        repository_id: "repo".into(),
        directory_id: "root".into(),
        path: String::new(),
        name: "frozen".into(),
        parent_branch_id: Some("parent".into()),
        head_tree_id: "historical-tree".into(),
        commit_id: "historical-commit".into(),
        entries,
        file_branches: BTreeMap::from([
            ("gone-file".into(), "branch-gone-file".into()),
            ("shared-file".into(), "branch-shared-file".into()),
        ]),
        file_contexts,
        inherited_files: BTreeMap::from([
            ("gone-file".into(), "origin-gone-file".into()),
            ("shared-file".into(), "origin-shared-file".into()),
        ]),
        directory_refs: BTreeMap::from([
            (
                "hidden-directory".into(),
                DirectoryReference {
                    directory_branch_id: "hidden-scope".into(),
                    tree_id: "hidden-tree".into(),
                },
            ),
            (
                "visible-directory".into(),
                DirectoryReference {
                    directory_branch_id: "visible-scope".into(),
                    tree_id: "visible-tree".into(),
                },
            ),
        ]),
        materialized: false,
        sequence: 8,
    }
}

#[test]
fn current_directory_payload_filters_sources_by_visible_file_key_and_directory_identity() {
    let branch = fixture();
    let historical = branch.clone();
    let entries = &branch.entries[3..];
    let current = current_branch(&branch, entries);
    assert_eq!(current.entries, entries);
    assert_eq!(
        current.file_branches,
        BTreeMap::from([("shared-file".into(), "branch-shared-file".into())])
    );
    assert_eq!(
        current.inherited_files,
        BTreeMap::from([("shared-file".into(), "origin-shared-file".into())])
    );
    assert_eq!(
        current.file_contexts,
        BTreeMap::from([(
            "visible/alias.txt".into(),
            branch.file_contexts["visible/alias.txt"].clone(),
        )])
    );
    assert_eq!(
        current.directory_refs,
        BTreeMap::from([(
            "visible-directory".into(),
            branch.directory_refs["visible-directory"].clone(),
        )])
    );
    assert_eq!(current.head_tree_id, branch.head_tree_id);
    assert_eq!(current.commit_id, branch.commit_id);
    assert_eq!(current.parent_branch_id, branch.parent_branch_id);
    assert_eq!(current.materialized, branch.materialized);
    assert_eq!(current.sequence, branch.sequence);
    assert_eq!(branch, historical);
    assert_eq!(
        DirectoryBranchChoice::capture_entries(&branch, true, entries).file_count,
        1
    );
    assert_eq!(DirectoryBranchChoice::capture(&branch, true).file_count, 3);
}

#[test]
fn empty_current_directory_payload_has_no_hidden_sources_or_child_references() {
    let branch = fixture();
    let current = current_branch(&branch, &[]);
    assert!(current.entries.is_empty());
    assert!(current.file_branches.is_empty());
    assert!(current.file_contexts.is_empty());
    assert!(current.inherited_files.is_empty());
    assert!(current.directory_refs.is_empty());
    assert_eq!(current.directory_branch_id, branch.directory_branch_id);
    assert_eq!(current.directory_id, branch.directory_id);
    assert_eq!(current.head_tree_id, branch.head_tree_id);
    assert_eq!(branch.entries.len(), 5);
}
