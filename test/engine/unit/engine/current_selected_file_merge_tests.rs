//! Directory selection and ordinary file merge consume the same current proof.
use super::*;

fn select_projected_directory(fixture: &mut Fixture) -> String {
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
    fixture.exit(0, MembershipExitAction::Deregister);
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
    assert_ne!(
        fixture.engine.state.directory_tree_selections[&current.directory_id],
        current.head_tree_id
    );
    branch
}

#[test]
fn disk_exit_selected_projection_then_current_file_merge_and_restart() {
    let mut fixture = Fixture::new(2);
    let selected = select_projected_directory(&mut fixture);
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
    for (id, old) in &old_commits {
        assert_eq!(fixture.engine.state.commits.get(id), Some(old));
    }
    for (id, old) in &old_trees {
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
fn disk_exit_file_merge_rejects_tampered_selected_occurrence_context_and_ref() {
    let mut fixture = Fixture::new(2);
    let selected = select_projected_directory(&mut fixture);
    let state = fixture.engine.state.clone();
    let view = fixture.engine.current_namespaces.as_ref().unwrap();
    view.file_merge_directories(&state, &fixture.repository.repository_id)
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
                forged.directory_tree_selections.insert(
                    directory.clone(),
                    state.directory_branches[&selected].head_tree_id.clone(),
                );
            }
            _ => unreachable!(),
        }
        assert!(
            view.file_merge_directories(&forged, &fixture.repository.repository_id)
                .is_err(),
            "admitted tampering case {attack}"
        );
    }
    assert_eq!(
        canonical_json(fixture.engine.state()).unwrap(),
        canonical_json(&state).unwrap()
    );
}
