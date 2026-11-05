//! Directory selection and ordinary file merge consume the same current proof.
use super::*;

fn select_projected_directory(fixture: &mut Fixture) -> (String, String) {
    crate::directory_git_fixture::checkpoint(&mut fixture.engine, &fixture.repository, "dir");
    let branch = fixture
        .engine
        .create_directory_branches(
            &fixture.repository.repository_id,
            Some(&name_policy::LookupPath::parse("dir").unwrap()),
            &name("selected merge directory"),
            "admin",
            op(),
        )
        .unwrap()
        .result["directory_branch_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let frozen = fixture.engine.state.directory_branches[&branch]
        .head_tree_id
        .clone();
    fixture.exit(0, MembershipExitAction::Deregister);
    crate::directory_git_fixture::checkpoint(&mut fixture.engine, &fixture.repository, "dir");
    fixture
        .engine
        .select_directory_branches(
            &fixture.repository.repository_id,
            Some(&name_policy::LookupPath::parse("dir").unwrap()),
            &name("selected merge directory"),
            "admin",
            op(),
        )
        .unwrap();
    let current = &fixture.engine.state.directory_branches[&branch];
    assert_eq!(
        fixture.engine.state.directory_tree_selections[&current.directory_id],
        current.head_tree_id
    );
    assert_ne!(current.head_tree_id, frozen);
    (branch, frozen)
}

#[test]
fn disk_exit_selected_projection_then_current_file_merge_and_restart() {
    let mut fixture = Fixture::new(2);
    let (selected, _) = select_projected_directory(&mut fixture);
    let old_commits = fixture.engine.state.commits.clone();
    let old_trees = fixture.engine.state.directory_trees.clone();
    let old_head = fixture.engine.state.directory_branches[&selected]
        .head_tree_id
        .clone();
    let inputs = fixture.inputs();
    let operation = op();
    let merge = |engine: &mut Engine| {
        engine.merge_file_branches(FileBranchMergeRequest {
            file_id: &fixture.current.file_id,
            inputs: &inputs,
            chosen_node_id: &fixture.first.version_id,
            surviving_branch_id: &fixture.file_main,
            location: None,
            principal: "admin",
            operation_id: operation,
        })
    };
    let receipt = merge(&mut fixture.engine).unwrap();
    let changed = &fixture.engine.state.directory_branches[&selected];
    let tree = &fixture.engine.state.directory_trees[&changed.head_tree_id];
    assert_eq!(tree.parent_tree_id.as_ref(), Some(&old_head));
    assert_eq!(
        fixture.engine.state.directory_tree_selections[&changed.directory_id],
        changed.head_tree_id
    );
    assert!(
        changed
            .entries
            .iter()
            .all(|entry| entry.file_id != fixture.hidden.file_id)
    );
    let source = changed
        .entries
        .iter()
        .find(|entry| entry.file_id == fixture.current.file_id)
        .unwrap();
    assert_eq!(source.version_id, fixture.first.version_id);
    assert_eq!(
        changed.file_contexts[source.lookup_key()].branch_id,
        fixture.file_main
    );
    assert!(
        changed.file_contexts[source.lookup_key()]
            .inherited_from
            .is_none()
    );
    fixture.assert_hidden();
    for (id, old) in old_commits.iter() {
        assert_eq!(fixture.engine.state.commits.get(id), Some(old));
    }
    for (id, old) in old_trees.iter() {
        assert_eq!(fixture.engine.state.directory_trees.get(id), Some(old));
    }
    assert_eq!(merge(&mut fixture.engine).unwrap(), receipt);
    let system = fixture.temp.path().join("system");
    drop(fixture.engine);
    let mut engine = Engine::open(&system).unwrap();
    assert_eq!(merge(&mut engine).unwrap(), receipt);
    assert_eq!(
        entry(&engine, &fixture.repository, "dir/live").version_id,
        fixture.first.version_id
    );
}

#[test]
fn disk_exit_independent_child_merge_does_not_require_a_parked_parent_to_follow() {
    let mut fixture = Fixture::new(2);
    crate::directory_git_fixture::checkpoint(&mut fixture.engine, &fixture.repository, "dir");
    crate::directory_git_fixture::checkpoint(&mut fixture.engine, &fixture.repository, "");
    let parent = fixture
        .engine
        .create_directory_branches(
            &fixture.repository.repository_id,
            None,
            &name("parked independent parent"),
            "admin",
            op(),
        )
        .unwrap()
        .result["directory_branch_id"]
        .as_str()
        .unwrap()
        .to_owned();
    save(
        &mut fixture.engine,
        &fixture.repository.main_branch_id,
        "dir/live",
        b"new independent child work",
    );
    crate::directory_git_fixture::checkpoint(&mut fixture.engine, &fixture.repository, "dir");
    fixture.exit(0, MembershipExitAction::Deregister);
    crate::directory_git_fixture::checkpoint(&mut fixture.engine, &fixture.repository, "dir");
    crate::directory_git_fixture::checkpoint(&mut fixture.engine, &fixture.repository, "");
    let parked = fixture.engine.state.directory_branches[&parent].clone();
    let parent_git =
        fixture.engine.state.directory_git_repositories[&fixture.repository.repository_id].clone();
    let inputs = fixture.inputs();
    let operation = op();
    let merge = |engine: &mut Engine| {
        engine.merge_file_branches(FileBranchMergeRequest {
            file_id: &fixture.current.file_id,
            inputs: &inputs,
            chosen_node_id: &fixture.first.version_id,
            surviving_branch_id: &fixture.file_main,
            location: None,
            principal: "admin",
            operation_id: operation,
        })
    };
    let receipt = merge(&mut fixture.engine).unwrap();
    assert_eq!(fixture.engine.state.directory_branches[&parent], parked);
    assert_eq!(
        fixture.engine.state.directory_git_repositories[&fixture.repository.repository_id],
        parent_git
    );
    assert_eq!(
        entry(&fixture.engine, &fixture.repository, "dir/live").version_id,
        fixture.first.version_id
    );
    assert_eq!(merge(&mut fixture.engine).unwrap(), receipt);
    drop(fixture.engine);
    let mut engine = Engine::open(&fixture.temp.path().join("system")).unwrap();
    assert_eq!(merge(&mut engine).unwrap(), receipt);
    assert_eq!(engine.state.directory_branches[&parent], parked);
    assert_eq!(
        engine.state.directory_git_repositories[&fixture.repository.repository_id],
        parent_git
    );
}

#[test]
fn disk_exit_file_merge_rejects_tampered_selected_occurrence_context_and_ref() {
    let mut fixture = Fixture::new(2);
    let (selected, unselected_tree) = select_projected_directory(&mut fixture);
    let state = fixture.engine.state.clone();
    let view = fixture.engine.current_namespaces.as_ref().unwrap();
    let inputs = fixture
        .inputs()
        .into_iter()
        .map(|input| input.branch_id)
        .collect();
    view.file_merge_directories(
        &state,
        &fixture.repository.repository_id,
        &fixture.current.file_id,
        &inputs,
    )
    .unwrap();
    let directory = &state.directory_branches[&selected].directory_id;
    let tree = &state.directory_tree_selections[directory];
    for attack in 0..3 {
        let mut forged = state.as_ref().clone();
        match attack {
            0 => {
                forged
                    .directory_trees
                    .get_mut(tree)
                    .unwrap()
                    .entries
                    .push(fixture.hidden.clone());
            }
            1 => {
                forged
                    .directory_trees
                    .get_mut(tree)
                    .unwrap()
                    .file_contexts
                    .remove("dir/live");
            }
            2 => {
                forged
                    .directory_tree_selections
                    .insert(directory.clone(), unselected_tree.clone());
            }
            _ => unreachable!(),
        }
        assert!(
            view.file_merge_directories(
                &forged,
                &fixture.repository.repository_id,
                &fixture.current.file_id,
                &inputs
            )
            .is_err(),
            "admitted tampering case {attack}"
        );
    }
    assert_eq!(
        canonical_json(fixture.engine.state()).unwrap(),
        canonical_json(state.as_ref()).unwrap()
    );
}

fn parked_parent_with_owned_merge_alias() -> (Fixture, String) {
    let mut fixture = Fixture::new(2);
    crate::directory_git_fixture::checkpoint(&mut fixture.engine, &fixture.repository, "dir");
    crate::directory_git_fixture::checkpoint(&mut fixture.engine, &fixture.repository, "");
    let source = fixture
        .engine
        .create_directory_branches(
            &fixture.repository.repository_id,
            Some(&name_policy::LookupPath::parse("dir").unwrap()),
            &name("owned alias source"),
            "admin",
            op(),
        )
        .unwrap()
        .result["directory_branch_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let root = fixture.engine.state.directory_selections[&fixture.repository.repository_id].clone();
    fixture
        .engine
        .directory_namespace_batch(
            &fixture.repository.repository_id,
            &DirectoryNamespaceBatch {
                kind: DirectoryBatchKind::Move,
                items: vec![DirectoryNamespaceItem {
                    source: DirectoryNamespaceSource {
                        directory_branch_id: source.clone(),
                        expected_tree_id: fixture.engine.state.directory_branches[&source]
                            .head_tree_id
                            .clone(),
                        path: "dir/live".into(),
                        entry_id: fixture.current.entry_id.clone(),
                    },
                    target: Some(DirectoryNamespaceTarget {
                        directory_branch_id: root.clone(),
                        expected_tree_id: fixture.engine.state.directory_branches[&root]
                            .head_tree_id
                            .clone(),
                        path: "dir-owned-alias".into(),
                    }),
                }],
            },
            "admin",
            op(),
        )
        .unwrap();
    fixture
        .engine
        .select_file_branch_at(
            &fixture.repository.repository_id,
            &name_policy::PortablePath::parse("dir-owned-alias").unwrap(),
            &fixture.current.file_id,
            &fixture.file_main,
            "admin",
            op(),
        )
        .unwrap();
    let owned = entry(&fixture.engine, &fixture.repository, "dir-owned-alias");
    let child = entry(&fixture.engine, &fixture.repository, "dir/live");
    assert_eq!(owned.entry_id, child.entry_id);
    assert_eq!(owned.file_id, child.file_id);
    crate::directory_git_fixture::checkpoint(&mut fixture.engine, &fixture.repository, "");
    let parent = fixture
        .engine
        .create_directory_branches(
            &fixture.repository.repository_id,
            None,
            &name("parked parent owning alias"),
            "admin",
            op(),
        )
        .unwrap()
        .result["directory_branch_id"]
        .as_str()
        .unwrap()
        .to_owned();
    save(
        &mut fixture.engine,
        &fixture.repository.main_branch_id,
        "dir/live",
        b"independent child advances after parent parked",
    );
    crate::directory_git_fixture::checkpoint(&mut fixture.engine, &fixture.repository, "dir");
    fixture.exit(0, MembershipExitAction::Deregister);
    crate::directory_git_fixture::checkpoint(&mut fixture.engine, &fixture.repository, "dir");
    crate::directory_git_fixture::checkpoint(&mut fixture.engine, &fixture.repository, "");
    (fixture, parent)
}

#[test]
fn disk_exit_owned_alias_keeps_parked_parent_merge_closure_fenced_after_restart() {
    let (mut fixture, parent) = parked_parent_with_owned_merge_alias();
    let branch = &fixture.engine.state.directory_branches[&parent];
    let inputs = fixture.inputs();
    let input_ids = inputs.iter().map(|input| input.branch_id.clone()).collect();
    assert!(
        owns_merge_input(
            &fixture.engine.state,
            branch,
            &fixture.current.file_id,
            &input_ids,
        )
        .unwrap(),
        "an owned alias outside dir is not the independent child's occurrence"
    );
    let unrelated_inputs = input_ids
        .iter()
        .filter(|id| **id != branch.file_contexts["dir-owned-alias"].branch_id)
        .cloned()
        .collect::<BTreeSet<_>>();
    assert!(!unrelated_inputs.is_empty());
    assert!(
        !owns_merge_input(
            &fixture.engine.state,
            branch,
            &fixture.current.file_id,
            &unrelated_inputs,
        )
        .unwrap(),
        "same file identity alone does not make another path's branch an input"
    );
    let mut separate_intent = branch.clone();
    separate_intent.file_contexts.remove("dir-owned-alias");
    assert!(
        owns_merge_input(
            &fixture.engine.state,
            &separate_intent,
            &fixture.current.file_id,
            &input_ids,
        )
        .is_err(),
        "a missing owned path context cannot turn a required import into a skip"
    );
    let view = fixture.engine.current_namespaces.as_ref().unwrap();
    let mut unselected_inputs = BTreeSet::new();
    for candidate in fixture.engine.state.directory_branches.values() {
        if candidate.repository_id != fixture.repository.repository_id {
            continue;
        }
        if fixture.engine.state.directory_selections[&candidate.directory_id]
            == candidate.directory_branch_id
        {
            view.batch_scope(&fixture.engine.state, &candidate.directory_branch_id)
                .unwrap();
        } else if owns_merge_input(
            &fixture.engine.state,
            candidate,
            &fixture.current.file_id,
            &input_ids,
        )
        .unwrap()
        {
            unselected_inputs.insert(candidate.directory_branch_id.clone());
        }
    }
    assert_eq!(unselected_inputs, BTreeSet::from([parent.clone()]));
    let child = entry(&fixture.engine, &fixture.repository, "dir");
    assert_ne!(
        branch.directory_refs[&child.entry_id].tree_id,
        fixture.engine.state.directory_tree_selections[&child.entry_id],
        "only the parked participating parent has the stale child proof"
    );
    assert_eq!(
        view.file_merge_directories(
            &fixture.engine.state,
            &fixture.repository.repository_id,
            &fixture.current.file_id,
            &input_ids,
        )
        .unwrap_err()
        .code(),
        ErrorCode::RecoveryRequired
    );
    let before = canonical_json(fixture.engine.state()).unwrap();
    let before_disks = fixture
        .config
        .disks
        .iter()
        .map(|disk| files(&disk.root))
        .collect::<Vec<_>>();
    let operation = op();
    let merge = |engine: &mut Engine| {
        engine.merge_file_branches(FileBranchMergeRequest {
            file_id: &fixture.current.file_id,
            inputs: &inputs,
            chosen_node_id: &fixture.first.version_id,
            surviving_branch_id: &fixture.file_main,
            location: None,
            principal: "admin",
            operation_id: operation,
        })
    };
    assert_eq!(
        merge(&mut fixture.engine).unwrap_err().code(),
        ErrorCode::RecoveryRequired,
        "an owned input cannot borrow its parked parent's stale child closure"
    );
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
    for (disk, expected) in fixture.config.disks.iter().zip(&before_disks) {
        assert_eq!(
            files(&disk.root),
            *expected,
            "refusal must preserve payloads and every active or historical journal"
        );
    }
    drop(fixture.engine);
    let mut engine = Engine::open(&fixture.temp.path().join("system")).unwrap();
    assert_eq!(canonical_json(engine.state()).unwrap(), before);
    assert_eq!(
        merge(&mut engine).unwrap_err().code(),
        ErrorCode::RecoveryRequired
    );
    assert_eq!(canonical_json(engine.state()).unwrap(), before);
}
