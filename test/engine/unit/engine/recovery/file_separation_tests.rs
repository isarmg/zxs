use super::*;

#[test]
fn retained_occurrence_guard_rejects_source_mutation_but_allows_child_cache_advance() {
    let entry = Entry {
        entry_id: "retained-entry".into(),
        path: name_policy::PortablePath::parse("left/report.txt").unwrap(),
        kind: NamespaceEntryKind::File,
        file_id: "original-file".into(),
        version_id: "original-version".into(),
        size: 7,
    };
    let context = FileContext {
        node_id: Some("original-node".into()),
        branch_id: "original-file-branch".into(),
        selection_generation: 2,
        inherited_from: None,
    };
    let branch = DirectoryBranch {
        directory_branch_id: "left-main".into(),
        repository_id: "repository".into(),
        directory_id: "left-directory".into(),
        path: "left".into(),
        name: "main".into(),
        parent_branch_id: None,
        head_tree_id: "tree-before".into(),
        commit_id: "commit-before".into(),
        entries: vec![entry.clone()],
        file_branches: BTreeMap::from([(entry.file_id.clone(), context.branch_id.clone())]),
        file_contexts: BTreeMap::from([(entry.lookup_key().to_owned(), context.clone())]),
        inherited_files: BTreeMap::new(),
        directory_refs: BTreeMap::new(),
        materialized: true,
        sequence: 3,
    };
    let retained = RetainedFile {
        directory_branch_id: branch.directory_branch_id.clone(),
        entry,
        context,
    };
    verify_retained_file(&branch, &branch.entries, &retained).unwrap();
    for change in 0..6 {
        let mut corrupted = branch.clone();
        match change {
            0 => corrupted.entries.clear(),
            1 => corrupted.entries[0].file_id = "different-file".into(),
            2 => corrupted.entries[0].version_id = "different-version".into(),
            3 => corrupted.file_contexts.clear(),
            4 => {
                corrupted
                    .file_contexts
                    .get_mut("left/report.txt")
                    .unwrap()
                    .node_id = None
            }
            5 => {
                corrupted
                    .file_contexts
                    .get_mut("left/report.txt")
                    .unwrap()
                    .branch_id = "other-branch".into()
            }
            _ => unreachable!(),
        }
        assert_eq!(
            verify_retained_file(&corrupted, &corrupted.entries, &retained)
                .unwrap_err()
                .code(),
            ErrorCode::PublicationInvalid,
            "accepted retained-source corruption {change}"
        );
    }
    let mut parent_advanced = branch;
    parent_advanced.head_tree_id = "new-child-cache-tree".into();
    parent_advanced.commit_id = "new-working-commit".into();
    parent_advanced.sequence += 1;
    parent_advanced.directory_refs.insert(
        "nested-child".into(),
        storage_contract::DirectoryReference {
            directory_branch_id: "nested-main".into(),
            tree_id: "new-child-tree".into(),
        },
    );
    verify_retained_file(&parent_advanced, &parent_advanced.entries, &retained).unwrap();
}
