use super::*;
use crate::DirectoryBranch;
use std::collections::BTreeMap;

fn branch(id: &str, repository: &str, directory: &str, sequence: u64) -> DirectoryBranch {
    DirectoryBranch {
        directory_branch_id: id.into(),
        repository_id: repository.into(),
        directory_id: directory.into(),
        path: "stored spelling".into(),
        name: format!("name-{id}"),
        parent_branch_id: Some("parent".into()),
        head_tree_id: format!("tree-{id}"),
        commit_id: "commit".into(),
        entries: vec![],
        file_branches: BTreeMap::from([("file".into(), "branch".into())]),
        file_contexts: BTreeMap::new(),
        inherited_files: BTreeMap::new(),
        directory_refs: BTreeMap::new(),
        materialized: false,
        sequence,
    }
}

#[test]
fn summaries_select_only_scope_metadata_in_sequence_then_id_order() {
    // This pure projection fixture tests the algorithm, not Engine admission.
    let mut state = State::default();
    for item in [
        branch("old", "repo", "directory", 1),
        branch("b", "repo", "directory", u64::MAX),
        branch("a", "repo", "directory", u64::MAX),
        branch("foreign-repository", "other", "directory", u64::MAX),
        branch("foreign-directory", "repo", "other", u64::MAX),
    ] {
        state
            .directory_branches
            .insert(item.directory_branch_id.clone(), item);
    }
    state
        .directory_selections
        .insert("directory".into(), "b".into());
    let summaries = directory_branch_summaries(
        &state,
        DirectoryBranchScope {
            repository_id: "repo",
            directory_id: "directory",
            path: "Requested spelling",
        },
    );
    assert_eq!(
        summaries
            .iter()
            .map(|branch| branch.directory_branch_id.as_str())
            .collect::<Vec<_>>(),
        ["a", "b", "old"]
    );
    assert_eq!(
        summaries
            .iter()
            .map(|branch| branch.current)
            .collect::<Vec<_>>(),
        [false, true, false]
    );
    for summary in &summaries {
        assert_eq!(summary.path, "Requested spelling");
        assert_eq!(
            summary.file_count, 1,
            "count remains file_branches, not entries"
        );
        assert_eq!(summary.parent_branch_id.as_deref(), Some("parent"));
        assert!(!summary.materialized);
    }
    drop(state);
    assert_eq!(summaries[0].name, "name-a");
    assert_eq!(summaries[0].head_tree_id, "tree-a");
}
