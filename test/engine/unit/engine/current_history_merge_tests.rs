//! Explicit history consumers share the actual D3 fixture from their parent.
use super::*;
use storage_contract::{FileHistoryMergeInput, FileLocation};

fn history_merge(
    engine: &mut Engine,
    file: &str,
    inputs: &[FileHistoryMergeInput],
    chosen: &str,
    label: &str,
    operation: OperationId,
    location: Option<&FileLocation>,
) -> Result<OperationResult> {
    engine.merge_file_history(FileHistoryMergeRequest {
        file_id: file,
        inputs,
        chosen_node_id: chosen,
        name: &name(label),
        location,
        principal: "admin",
        operation_id: operation,
    })
}

fn history_inputs(fixture: &Fixture) -> Vec<FileHistoryMergeInput> {
    let latest =
        crate::business::file_nodes::branch_head(fixture.engine.state(), &fixture.file_main)
            .unwrap();
    [fixture.first.version_id.clone(), latest.clone()]
        .into_iter()
        .map(|source_node_id| FileHistoryMergeInput {
            branch_id: fixture.file_main.clone(),
            source_node_id,
            expected_node_id: latest.clone(),
        })
        .collect()
}

fn route_live(fixture: &mut Fixture, slot: &str) {
    let rule = fixture
        .engine
        .state
        .placement_rules
        .values()
        .find(|rule| {
            rule.repository_id == fixture.repository.repository_id && rule.path_prefix == "dir/live"
        })
        .unwrap()
        .clone();
    fixture
        .engine
        .update_placement_rule(PlacementRuleUpdate {
            rule_id: &rule.rule_id,
            repository_id: &fixture.repository.repository_id,
            path_prefix: "dir/live",
            primary_slot_id: slot,
            expected_generation: rule.generation,
            principal: "admin",
            operation_id: op(),
        })
        .unwrap();
}

/// Its newest body will be removed from current projection, but the reviewed
/// old source nodes still have independently readable retained copies.
fn hide_source_head(fixture: &mut Fixture) -> Vec<FileHistoryMergeInput> {
    let historical =
        crate::business::file_nodes::branch_head(fixture.engine.state(), &fixture.alternate)
            .unwrap();
    route_live(fixture, "P01");
    fixture
        .engine
        .select_file_branch(&fixture.current.file_id, &fixture.alternate, "admin", op())
        .unwrap();
    save(
        &mut fixture.engine,
        &fixture.alternate,
        "dir/live",
        b"latest source only on departed target",
    );
    let head = crate::business::file_nodes::branch_head(fixture.engine.state(), &fixture.alternate)
        .unwrap();
    fixture
        .engine
        .select_file_branch(&fixture.current.file_id, &fixture.file_main, "admin", op())
        .unwrap();
    route_live(fixture, "D04");
    [fixture.first.version_id.clone(), historical]
        .into_iter()
        .map(|source_node_id| FileHistoryMergeInput {
            branch_id: fixture.alternate.clone(),
            source_node_id,
            expected_node_id: head.clone(),
        })
        .collect()
}

#[test]
fn disk_exit_history_merge_uses_readable_old_nodes_of_hidden_source_head_and_restarts() {
    let mut fixture = Fixture::new(2);
    let inputs = hide_source_head(&mut fixture);
    let target = fixture.exit(0, MembershipExitAction::Deregister);
    assert!(
        fixture
            .engine
            .namespace_snapshot(&fixture.alternate)
            .unwrap()
            .entries()
            .iter()
            .all(|entry| entry.kind != NamespaceEntryKind::File)
    );
    let previous = fixture.engine.state.clone();
    let operation = op();
    let location = FileLocation {
        repository_id: fixture.repository.repository_id.clone(),
        path: "dir/LIVE".into(),
    };
    let result = history_merge(
        &mut fixture.engine,
        &fixture.current.file_id,
        &inputs,
        &inputs[1].source_node_id,
        "historical result",
        operation,
        Some(&location),
    )
    .unwrap();
    let result_branch = result.branch_id.clone().unwrap();
    assert_eq!(result.result["reused_node"], false);
    assert_eq!(result.result["directory_branch_ids"], json_codec::json!([]));
    assert_ne!(result.result["node_id"], inputs[1].source_node_id);
    assert_eq!(result.result["version_id"], inputs[1].source_node_id);
    for (id, branch) in &previous.branches {
        assert_eq!(fixture.engine.state.branches.get(id), Some(branch));
    }
    for (id, commit) in previous.commits.iter() {
        assert_eq!(fixture.engine.state.commits.get(id), Some(commit));
    }
    for (id, receipt) in &previous.operations {
        assert_eq!(fixture.engine.state.operations.get(id), Some(receipt));
    }
    assert_eq!(
        canonical_json(&fixture.engine.state.versions).unwrap(),
        canonical_json(&previous.versions).unwrap()
    );
    assert_eq!(fixture.engine.state.file_contexts, previous.file_contexts);
    assert_eq!(
        entry(&fixture.engine, &fixture.repository, "dir/live"),
        fixture.current
    );
    fixture.assert_hidden();
    let mut bytes = Vec::new();
    fixture
        .engine
        .open_content(&inputs[1].source_node_id)
        .unwrap()
        .file
        .read_to_end(&mut bytes)
        .unwrap();
    assert_eq!(bytes, b"alternate source");
    assert_eq!(
        history_merge(
            &mut fixture.engine,
            &fixture.current.file_id,
            &inputs,
            &inputs[1].source_node_id,
            "historical result",
            operation,
            Some(&location)
        )
        .unwrap(),
        result
    );
    assert_eq!(files(&fixture.config.disks[0].root), target);
    let system = fixture.temp.path().join("system");
    drop(fixture.engine);
    let mut engine = Engine::open(&system).unwrap();
    assert_eq!(
        history_merge(
            &mut engine,
            &fixture.current.file_id,
            &inputs,
            &inputs[1].source_node_id,
            "historical result",
            operation,
            Some(&location)
        )
        .unwrap(),
        result
    );
    engine
        .select_file_branch_at(
            &fixture.repository.repository_id,
            fixture.current.location(),
            &fixture.current.file_id,
            &result_branch,
            "admin",
            op(),
        )
        .unwrap();
    save(
        &mut engine,
        &result_branch,
        "dir/live",
        b"edited historical result",
    );
    assert_eq!(
        entry(&engine, &fixture.repository, "dir/live").file_id,
        fixture.current.file_id
    );
    assert!(
        engine
            .namespace_tree(&fixture.repository.main_branch_id)
            .unwrap()
            .get("dir/gone")
            .is_none()
    );
    assert_eq!(files(&fixture.config.disks[0].root), target);
}

#[test]
fn disk_exit_history_merge_never_revives_deleted_identity_or_same_path_replacement() {
    let mut fixture = Fixture::new(2);
    let inputs = history_inputs(&fixture);
    let target = fixture.exit(0, MembershipExitAction::Deregister);
    fixture
        .engine
        .file_namespace(
            &fixture.repository.repository_id,
            NamespaceOperation::parse(NamespaceOperationKind::DeleteTree, "dir/live", None)
                .unwrap(),
            "admin",
            op(),
        )
        .unwrap();
    let before = canonical_json(fixture.engine.state()).unwrap();
    assert_eq!(
        history_merge(
            &mut fixture.engine,
            &fixture.current.file_id,
            &inputs,
            &fixture.first.version_id,
            "cannot resurrect",
            op(),
            None
        )
        .unwrap_err()
        .code(),
        ErrorCode::FileNotFound
    );
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
    save(
        &mut fixture.engine,
        &fixture.repository.main_branch_id,
        "dir/live",
        b"unrelated replacement",
    );
    let replacement = entry(&fixture.engine, &fixture.repository, "dir/live");
    assert_ne!(replacement.file_id, fixture.current.file_id);
    assert_ne!(replacement.entry_id, fixture.current.entry_id);
    let before = canonical_json(fixture.engine.state()).unwrap();
    let location = FileLocation {
        repository_id: fixture.repository.repository_id.clone(),
        path: "dir/live".into(),
    };
    for location in [None, Some(&location)] {
        assert_eq!(
            history_merge(
                &mut fixture.engine,
                &fixture.current.file_id,
                &inputs,
                &fixture.first.version_id,
                "not the new identity",
                op(),
                location
            )
            .unwrap_err()
            .code(),
            ErrorCode::FileNotFound
        );
    }
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
    assert_eq!(files(&fixture.config.disks[0].root), target);
}

#[test]
fn disk_exit_history_merge_rejects_unavailable_chosen_body_without_partial_helpers() {
    let mut fixture = Fixture::new(1);
    let retained = fixture.current.clone();
    save(
        &mut fixture.engine,
        &fixture.repository.main_branch_id,
        "dir/live",
        b"another retained node",
    );
    let latest =
        crate::business::file_nodes::branch_head(fixture.engine.state(), &fixture.file_main)
            .unwrap();
    let inputs =
        [retained.version_id, latest.clone()].map(|source_node_id| FileHistoryMergeInput {
            branch_id: fixture.file_main.clone(),
            source_node_id,
            expected_node_id: latest.clone(),
        });
    let target = fixture.exit(0, MembershipExitAction::Deregister);
    let before = canonical_json(fixture.engine.state()).unwrap();
    let operation = op();
    assert_eq!(
        history_merge(
            &mut fixture.engine,
            &fixture.current.file_id,
            &inputs,
            &fixture.first.version_id,
            "missing body",
            operation,
            None
        )
        .unwrap_err()
        .code(),
        ErrorCode::ContentUnavailable
    );
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
    assert!(!fixture.engine.state.operations.contains_key(&operation));
    assert_eq!(files(&fixture.config.disks[0].root), target);
}

#[test]
fn disabled_history_owner_rejects_new_derivation_but_replays_old_real_receipt() {
    let mut fixture = Fixture::new(2);
    let inputs = history_inputs(&fixture);
    let operation = op();
    let result = history_merge(
        &mut fixture.engine,
        &fixture.current.file_id,
        &inputs,
        &fixture.first.version_id,
        "before disabled",
        operation,
        None,
    )
    .unwrap();
    let target = fixture.exit(3, MembershipExitAction::PlannedOffline);
    let before = canonical_json(fixture.engine.state()).unwrap();
    assert_eq!(
        history_merge(
            &mut fixture.engine,
            &fixture.current.file_id,
            &inputs,
            &fixture.first.version_id,
            "disabled must not derive",
            op(),
            None
        )
        .unwrap_err()
        .code(),
        ErrorCode::BranchNotActive
    );
    assert_eq!(
        history_merge(
            &mut fixture.engine,
            &fixture.current.file_id,
            &inputs,
            &fixture.first.version_id,
            "before disabled",
            operation,
            None
        )
        .unwrap(),
        result
    );
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
    assert_eq!(files(&fixture.config.disks[3].root), target);
}

#[test]
fn disk_exit_history_merge_recovery_rejects_request_actor_graph_and_occurrence_tampering() {
    let mut fixture = Fixture::new(2);
    let inputs = history_inputs(&fixture);
    fixture.exit(0, MembershipExitAction::Deregister);
    let previous = fixture.engine.state.clone();
    let view = fixture.engine.current_namespaces.clone().unwrap();
    let operation = op();
    let location = FileLocation {
        repository_id: fixture.repository.repository_id.clone(),
        path: "dir/live".into(),
    };
    let result = history_merge(
        &mut fixture.engine,
        &fixture.current.file_id,
        &inputs,
        &fixture.first.version_id,
        "recovery proof",
        operation,
        Some(&location),
    )
    .unwrap();
    let published = fixture.engine.state.clone();
    view.advance(&previous, &published).unwrap();
    let event = &published.file_branch_merges[result.result["merge_id"].as_str().unwrap()];
    let branch = &published.branches[&event.surviving_branch_id];
    let head = &published.commits[&branch.head_commit_id];
    let pending_id = &head.parent_commit_ids[0];
    let unchanged = canonical_json(fixture.engine.state()).unwrap();
    for attack in 0..9 {
        let mut forged = published.as_ref().clone();
        match attack {
            0 => {
                forged
                    .operation_principals
                    .insert(operation, "another actor".into());
            }
            1 => {
                forged.operations.get_mut(&operation).unwrap().result["source_inputs"][0]["expected_node_id"] =
                    json_codec::json!(fixture.first.version_id);
            }
            2 => {
                forged.operations.get_mut(&operation).unwrap().result["location"] =
                    json_codec::Value::Null;
            }
            3 => {
                forged
                    .file_branch_aliases
                    .insert(event.input_branch_ids[1].clone(), fixture.file_main.clone());
            }
            4 => {
                forged
                    .file_metadata_nodes
                    .get_mut(&event.result_node_id)
                    .unwrap()
                    .parent_node_ids
                    .clear();
            }
            5 => {
                forged
                    .commits
                    .get_mut(&head.commit_id)
                    .unwrap()
                    .parent_commit_ids = vec![branch.fork_commit_id.clone()];
            }
            6 => {
                forged
                    .commits
                    .get_mut(pending_id)
                    .unwrap()
                    .entries
                    .push(fixture.hidden.clone());
            }
            7 => {
                let entries = &mut forged.commits.get_mut(pending_id).unwrap().entries;
                let entry = entries
                    .iter_mut()
                    .find(|entry| entry.kind == NamespaceEntryKind::File)
                    .unwrap();
                entry.entry_id = fixture.hidden.entry_id.clone();
            }
            8 => {
                forged
                    .operations
                    .get_mut(&operation)
                    .unwrap()
                    .result
                    .as_object_mut()
                    .unwrap()
                    .remove("location");
            }
            _ => unreachable!(),
        }
        assert!(
            view.advance(&previous, &forged).is_err(),
            "admitted tampering case {attack}"
        );
        assert_eq!(canonical_json(fixture.engine.state()).unwrap(), unchanged);
    }
}
