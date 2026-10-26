use super::*;
use storage_engine::FileMergeInput;

fn fork(pool: &mut Pool, file: &str, node: &str, name: &str) -> String {
    let version = pool.engine.file_node_version(file, node).unwrap();
    pool.engine
        .create_file_branch_from_node(file, &version, node, name, "admin", &new_id("op"))
        .unwrap()
        .result["branch_id"]
        .as_str()
        .unwrap()
        .into()
}
fn choose(pool: &mut Pool, file: &str, branch: &str, path: &str) {
    pool.engine
        .select_file_branch_at(
            &pool.repo.repository_id,
            path,
            file,
            branch,
            "admin",
            &new_id("op"),
        )
        .unwrap();
}
fn inputs(pool: &Pool, branches: &[&str]) -> Vec<FileMergeInput> {
    branches
        .iter()
        .map(|branch| FileMergeInput {
            branch_id: (*branch).into(),
            expected_node_id: pool.engine.file_branch_head(branch).unwrap(),
        })
        .collect()
}

#[test]
fn adopting_an_arbitrary_third_node_creates_one_metadata_node_and_only_participating_views_advance()
{
    let mut p = Pool::new();
    p.save("docs/report.txt", b"base");
    let base = p.current_entry("docs/report.txt");
    let left = fork(&mut p, &base.file_id, &base.version_id, "left");
    choose(&mut p, &base.file_id, &left, "docs/report.txt");
    p.save("docs/report.txt", b"left");
    let left_node = p.engine.file_branch_head(&left).unwrap();
    let right = fork(&mut p, &base.file_id, &base.version_id, "right");
    choose(&mut p, &base.file_id, &right, "docs/report.txt");
    p.save("docs/report.txt", b"right");
    let right_node = p.engine.file_branch_head(&right).unwrap();
    let other = fork(&mut p, &base.file_id, &base.version_id, "other");
    choose(&mut p, &base.file_id, &other, "docs/report.txt");
    p.save("docs/report.txt", b"chosen third body");
    let chosen = p.engine.file_branch_head(&other).unwrap();
    let frozen = p.register("docs", "unrelated");
    let frozen_tree = p.branch(&frozen).head_tree_id;
    choose(&mut p, &base.file_id, &left, "docs/report.txt");
    let involved = p.register("docs", "involved");
    let old_involved = p.branch(&involved).head_tree_id;
    let versions = p.engine.state().versions.len();
    let request = inputs(&p, &[&left, &right]);
    let operation = new_id("op");
    let receipt = p
        .engine
        .merge_file_branches(storage_engine::FileBranchMergeRequest {
            file_id: &base.file_id,
            inputs: &request,
            chosen_node_id: &chosen,
            surviving_branch_id: &left,
            location: None,
            principal: "admin",
            operation_id: &operation,
        })
        .unwrap();
    let node = receipt.result["node_id"].as_str().unwrap().to_owned();
    assert!(node.starts_with("fmeta_"));
    assert_eq!(p.engine.state().versions.len(), versions);
    let metadata = &p.engine.state().file_metadata_nodes[&node];
    assert_eq!(metadata.kind, storage_contract::FileMetadataKind::Merge);
    assert_eq!(metadata.version_id, chosen);
    assert_eq!(
        metadata
            .parent_node_ids
            .iter()
            .collect::<std::collections::BTreeSet<_>>(),
        [&left_node, &right_node, &chosen].into_iter().collect()
    );
    assert_ne!(p.branch(&involved).head_tree_id, old_involved);
    assert_eq!(p.entry(&involved, "docs/report.txt").version_id, chosen);
    assert_eq!(p.branch(&frozen).head_tree_id, frozen_tree);
    assert_eq!(
        p.engine.state().branches[&right].state,
        storage_contract::BranchState::Merged
    );
    assert_eq!(p.engine.state().file_branch_aliases[&right], left);
    assert_eq!(p.engine.file_branch_head(&right).unwrap(), right_node);
    assert_eq!(p.bytes(&chosen), b"chosen third body");
    assert_eq!(
        p.engine
            .merge_file_branches(storage_engine::FileBranchMergeRequest {
                file_id: &base.file_id,
                inputs: &request,
                chosen_node_id: &chosen,
                surviving_branch_id: &left,
                location: None,
                principal: "admin",
                operation_id: &operation
            })
            .unwrap()
            .result,
        receipt.result
    );
    let location = storage_engine::FileLocation {
        repository_id: p.repo.repository_id.clone(),
        path: "docs/report.txt".into(),
    };
    let state = storage_engine::canonical_json(p.engine.state()).unwrap();
    for (principal, location) in [("other-actor", None), ("admin", Some(&location))] {
        assert_eq!(
            p.engine
                .merge_file_branches(storage_engine::FileBranchMergeRequest {
                    file_id: &base.file_id,
                    inputs: &request,
                    chosen_node_id: &chosen,
                    surviving_branch_id: &left,
                    location,
                    principal,
                    operation_id: &operation,
                })
                .unwrap_err()
                .code(),
            storage_contract::ErrorCode::IdempotencyKeyReused
        );
        assert_eq!(
            storage_engine::canonical_json(p.engine.state()).unwrap(),
            state
        );
    }
    p.reopen(|engine| assert_eq!(engine.file_branch_head(&left).unwrap(), node));
}

#[test]
fn reused_ancestor_head_omits_cycle_edges_and_stale_merges_publish_nothing() {
    let mut p = Pool::new();
    p.save("report.txt", b"base");
    let base = p.current_entry("report.txt");
    let left = fork(&mut p, &base.file_id, &base.version_id, "left");
    choose(&mut p, &base.file_id, &left, "report.txt");
    p.save("report.txt", b"ancestor");
    let ancestor = p.engine.file_branch_head(&left).unwrap();
    let right = fork(&mut p, &base.file_id, &ancestor, "right");
    choose(&mut p, &base.file_id, &right, "report.txt");
    p.save("report.txt", b"descendant");
    let descendant = p.engine.file_branch_head(&right).unwrap();
    let request = inputs(&p, &[&left, &right]);
    let before = p.engine.state().clone();
    let mut stale = request.clone();
    stale[1].expected_node_id = base.version_id.clone();
    assert_eq!(
        p.engine
            .merge_file_branches(storage_engine::FileBranchMergeRequest {
                file_id: &base.file_id,
                inputs: &stale,
                chosen_node_id: &ancestor,
                surviving_branch_id: &left,
                location: None,
                principal: "admin",
                operation_id: &new_id("op")
            })
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::FileBranchConflict
    );
    assert_eq!(
        storage_engine::canonical_json(p.engine.state()).unwrap(),
        storage_engine::canonical_json(&before).unwrap()
    );
    let result = p
        .engine
        .merge_file_branches(storage_engine::FileBranchMergeRequest {
            file_id: &base.file_id,
            inputs: &request,
            chosen_node_id: &ancestor,
            surviving_branch_id: &left,
            location: None,
            principal: "admin",
            operation_id: &new_id("op"),
        })
        .unwrap();
    assert_eq!(result.result["node_id"], ancestor);
    assert_eq!(result.result["reused_node"], true);
    assert_eq!(
        p.engine.state().file_metadata_nodes.len(),
        before.file_metadata_nodes.len()
    );
    let merge = p.engine.state().file_branch_merges.values().next().unwrap();
    assert!(merge.parent_node_ids.is_empty());
    assert!(merge.input_node_ids.contains(&descendant));
    assert_eq!(
        p.engine
            .file_node_parents(&base.file_id, &ancestor)
            .unwrap(),
        vec![base.version_id]
    );
    p.reopen(|engine| assert_eq!(engine.file_branch_head(&left).unwrap(), ancestor));
}

#[test]
fn merge_preserves_an_unselected_alias_context_of_the_same_stable_file() {
    let mut p = Pool::new();
    let (n, l, base) = selected_aliases(&mut p);
    let unselected = p
        .engine
        .file_context(&p.repo.repository_id, "a/report.txt")
        .unwrap();
    let selected = p
        .engine
        .file_context(&p.repo.repository_id, "b/report.txt")
        .unwrap()
        .branch_id;
    let second = fork(
        &mut p,
        &base.file_id,
        &base.version_id,
        "second merge input",
    );
    choose(&mut p, &base.file_id, &second, "b/report.txt");
    p.save("b/report.txt", b"second");
    let chosen = p.engine.file_branch_head(&second).unwrap();
    choose(&mut p, &base.file_id, &selected, "b/report.txt");
    let before_a = p.branch(&n).head_tree_id;
    let request = inputs(&p, &[&selected, &second]);
    p.engine
        .merge_file_branches(storage_engine::FileBranchMergeRequest {
            file_id: &base.file_id,
            inputs: &request,
            chosen_node_id: &chosen,
            surviving_branch_id: &selected,
            location: None,
            principal: "admin",
            operation_id: &new_id("op"),
        })
        .unwrap();
    assert_eq!(
        p.engine
            .file_context(&p.repo.repository_id, "a/report.txt")
            .unwrap(),
        unselected
    );
    assert_eq!(p.branch(&n).head_tree_id, before_a);
    assert_eq!(p.entry(&n, "a/report.txt").version_id, base.version_id);
    assert_eq!(p.entry(&l, "b/report.txt").version_id, chosen);
    assert_eq!(p.current_entry("b/report.txt").version_id, chosen);
    assert_eq!(p.current_entry("a/report.txt").version_id, base.version_id);
    let repo = p.repo.repository_id.clone();
    p.reopen(|engine| {
        assert_eq!(
            engine.file_context(&repo, "a/report.txt").unwrap(),
            unselected
        )
    });
}

#[test]
fn historical_alias_ownership_follows_a_later_owner_merge_without_moving_its_frozen_head() {
    let mut p = Pool::new();
    p.save("report.txt", b"base");
    let base = p.current_entry("report.txt");
    let a = fork(&mut p, &base.file_id, &base.version_id, "a");
    let b = fork(&mut p, &base.file_id, &base.version_id, "b");
    let c = fork(&mut p, &base.file_id, &base.version_id, "c");
    let frozen_b = p.engine.file_branch_head(&b).unwrap();
    p.engine
        .merge_file_branches(storage_engine::FileBranchMergeRequest {
            file_id: &base.file_id,
            inputs: &inputs(&p, &[&a, &b]),
            chosen_node_id: &base.version_id,
            surviving_branch_id: &a,
            location: None,
            principal: "admin",
            operation_id: &new_id("op"),
        })
        .unwrap();
    p.engine
        .merge_file_branches(storage_engine::FileBranchMergeRequest {
            file_id: &base.file_id,
            inputs: &inputs(&p, &[&a, &c]),
            chosen_node_id: &base.version_id,
            surviving_branch_id: &c,
            location: None,
            principal: "admin",
            operation_id: &new_id("op"),
        })
        .unwrap();
    assert_eq!(p.engine.state().file_branch_aliases[&a], c);
    assert_eq!(p.engine.state().file_branch_aliases[&b], c);
    assert_eq!(p.engine.file_branch_head(&b).unwrap(), frozen_b);
    p.reopen(|engine| assert_eq!(engine.state().file_branch_aliases[&b], c));
}

#[test]
fn a_frozen_parent_derives_its_own_changed_child_without_following_the_childs_new_members() {
    let mut p = Pool::new();
    p.save("dir/child/report.txt", b"base");
    p.save("dir/child/stay.txt", b"old sibling");
    let file = p.current_entry("dir/child/report.txt");
    let left = fork(&mut p, &file.file_id, &file.version_id, "left");
    let right = fork(&mut p, &file.file_id, &file.version_id, "right");
    choose(&mut p, &file.file_id, &left, "dir/child/report.txt");
    p.save("dir/child/report.txt", b"left");
    let child = p.register("dir/child", "child-view");
    p.select("dir/child", "child-view");
    choose(&mut p, &file.file_id, &left, "dir/child/report.txt");
    let parent = p.register("dir", "frozen-parent");
    let old_parent = p.branch(&parent);
    let child_entry = p.current_entry("dir/child");
    let frozen_child = old_parent.directory_refs[&child_entry.entry_id]
        .tree_id
        .clone();
    let sibling = p.entry(&parent, "dir/child/stay.txt");
    p.save("dir/child/stay.txt", b"new sibling outside frozen parent");
    assert_ne!(p.branch(&child).head_tree_id, frozen_child);
    let old_child_record = p.engine.state().directory_trees[&frozen_child].clone();
    let request = inputs(&p, &[&left, &right]);
    p.engine
        .merge_file_branches(storage_engine::FileBranchMergeRequest {
            file_id: &file.file_id,
            inputs: &request,
            chosen_node_id: &file.version_id,
            surviving_branch_id: &left,
            location: None,
            principal: "admin",
            operation_id: &new_id("op"),
        })
        .unwrap();
    let next = p.branch(&parent);
    let reference = &next.directory_refs[&child_entry.entry_id];
    assert_ne!(reference.tree_id, frozen_child);
    assert_ne!(
        reference.tree_id,
        p.branch(&child).head_tree_id,
        "frozen child gets a separate derived composition"
    );
    let derived = &p.engine.state().directory_trees[&reference.tree_id];
    assert_eq!(
        derived.parent_tree_id.as_deref(),
        Some(frozen_child.as_str())
    );
    assert_eq!(
        derived
            .entries
            .iter()
            .find(|entry| entry.path == "dir/child/stay.txt")
            .unwrap()
            .version_id,
        sibling.version_id
    );
    assert_eq!(
        p.entry(&parent, "dir/child/stay.txt").version_id,
        sibling.version_id
    );
    assert_ne!(
        p.current_entry("dir/child/stay.txt").version_id,
        sibling.version_id
    );
    assert_eq!(
        storage_engine::canonical_json(&p.engine.state().directory_trees[&frozen_child]).unwrap(),
        storage_engine::canonical_json(&old_child_record).unwrap()
    );
    p.reopen(|engine| {
        assert!(
            engine
                .state()
                .directory_trees
                .contains_key(&reference.tree_id)
        )
    });
}
