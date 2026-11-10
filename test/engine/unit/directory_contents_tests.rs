use super::*;
use storage_contract::NamespaceEntryKind;

fn branch(label: &str) -> DirectoryBranch {
    DirectoryBranch {
        directory_branch_id: format!("branch-{label}"),
        repository_id: format!("repository-{label}"),
        directory_id: format!("directory-{label}"),
        path: label.into(),
        name: label.into(),
        parent_branch_id: Some(format!("parent-{label}")),
        head_tree_id: format!("head-{label}"),
        commit_id: format!("commit-{label}"),
        entries: vec![Entry {
            entry_id: format!("entry-{label}"),
            path: name_policy::PortablePath::parse(&format!("{label}/File")).unwrap(),
            kind: NamespaceEntryKind::File,
            file_id: format!("file-{label}"),
            version_id: format!("version-{label}"),
            size: 7,
        }],
        file_branches: BTreeMap::from([(format!("file-{label}"), format!("file-branch-{label}"))]),
        file_contexts: BTreeMap::from([(
            format!("{label}/file"),
            FileContext {
                node_id: Some(format!("node-{label}")),
                branch_id: format!("context-branch-{label}"),
                inherited_from: Some(format!("origin-{label}")),
                selection_generation: 11,
            },
        )]),
        inherited_files: BTreeMap::from([(format!("file-{label}"), format!("origin-{label}"))]),
        directory_refs: BTreeMap::from([(
            format!("child-{label}"),
            DirectoryReference {
                directory_branch_id: format!("child-branch-{label}"),
                tree_id: format!("child-tree-{label}"),
            },
        )]),
        materialized: false,
        sequence: u64::MAX,
    }
}

#[test]
fn branch_contents_borrow_every_payload_without_copying_or_validating_it() {
    let branch = branch("current");
    let contents = DirectoryContents::from(&branch);
    assert!(std::ptr::eq(contents.commit_id, branch.commit_id.as_str()));
    assert!(std::ptr::eq(contents.path, branch.path.as_str()));
    assert!(std::ptr::eq(contents.entries, branch.entries.as_slice()));
    assert!(std::ptr::eq(contents.file_branches, &branch.file_branches));
    assert!(std::ptr::eq(contents.file_contexts, &branch.file_contexts));
    assert!(std::ptr::eq(
        contents.inherited_files,
        &branch.inherited_files
    ));
    assert!(std::ptr::eq(
        contents.directory_refs,
        &branch.directory_refs
    ));
}

#[test]
fn tree_contents_borrow_the_frozen_payload_instead_of_the_owners_current_head() {
    let owner = branch("current");
    let frozen = branch("frozen");
    let tree = DirectoryTree {
        tree_id: "old-tree".into(),
        directory_branch_id: owner.directory_branch_id.clone(),
        parent_tree_id: Some("older-tree".into()),
        commit_id: frozen.commit_id,
        path: frozen.path,
        entries: frozen.entries,
        file_branches: frozen.file_branches,
        file_contexts: frozen.file_contexts,
        inherited_files: frozen.inherited_files,
        directory_refs: frozen.directory_refs,
        sequence: 0,
    };
    let contents = DirectoryContents::from(&tree);
    assert!(std::ptr::eq(contents.commit_id, tree.commit_id.as_str()));
    assert!(std::ptr::eq(contents.path, tree.path.as_str()));
    assert!(std::ptr::eq(contents.entries, tree.entries.as_slice()));
    assert!(std::ptr::eq(contents.file_branches, &tree.file_branches));
    assert!(std::ptr::eq(contents.file_contexts, &tree.file_contexts));
    assert!(std::ptr::eq(
        contents.inherited_files,
        &tree.inherited_files
    ));
    assert!(std::ptr::eq(contents.directory_refs, &tree.directory_refs));
    assert_ne!(contents.commit_id, owner.commit_id);
    assert_ne!(contents.path, owner.path);
    assert_ne!(contents.entries, owner.entries.as_slice());
    assert_ne!(contents.file_branches, &owner.file_branches);
    assert_ne!(contents.file_contexts, &owner.file_contexts);
    assert_ne!(contents.inherited_files, &owner.inherited_files);
    assert_ne!(contents.directory_refs, &owner.directory_refs);
}
