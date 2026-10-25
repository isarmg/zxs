use super::*;

fn commit(id: &str, parent: Option<&str>) -> Commit {
    Commit {
        commit_id: id.into(),
        history_tree_id: "history".into(),
        kind: "content".into(),
        parent_commit_ids: parent.into_iter().map(str::to_owned).collect(),
        tree_id: "tree".into(),
        sequence: 0,
        operation_id: "operation".into(),
        author: "author".into(),
        message: String::new(),
        entries: Vec::new(),
        pending_file_versions: Default::default(),
        namespace_deletions: None,
        provenance: None,
    }
}

#[test]
fn reachability_is_independent_of_order_and_excludes_detached_cycles() {
    let commits = [
        commit("child", Some("root")),
        commit("root", None),
        commit("grandchild", Some("child")),
        commit("sibling", Some("root")),
        commit("cycle1", Some("cycle2")),
        commit("cycle2", Some("cycle1")),
    ];
    assert_eq!(
        descendant_commits(commits.iter().rev(), "root"),
        BTreeSet::from(["root", "child", "grandchild", "sibling"].map(str::to_owned))
    );
    assert_eq!(
        descendant_commits(commits.iter(), "child"),
        BTreeSet::from(["child", "grandchild"].map(str::to_owned))
    );
    assert_eq!(descendant_commits(commits.iter(), "cycle1").len(), 2);
}

#[test]
fn long_reverse_order_chain_needs_no_recursive_stack_or_full_table_rescan() {
    let mut commits = vec![commit("0", None)];
    for id in 1..12_000 {
        commits.push(commit(&id.to_string(), Some(&(id - 1).to_string())));
    }
    assert_eq!(
        descendant_commits(commits.iter().rev(), "0").len(),
        commits.len()
    );
}
