use super::*;
use crate::backend_branch::BranchResponse;
use json_codec::{Value, json};

// Frozen pre-migration wire oracle; independent from the new typed projection.
fn old_content_wire(state: &State, visible: Option<&str>, source: Option<&str>) -> Value {
    let visible = source.or(visible).filter(|version| !version.is_empty());
    let inherited = source.is_some();
    let state = visible
        .and_then(|version| state.content_states.get(version))
        .map(storage_contract::ContentState::as_str)
        .unwrap_or("LIVE");
    json!({"version_id":visible,"materialized":!inherited && visible.is_some(),
        "inherited":inherited,"own_version_id":(!inherited).then_some(visible).flatten(),
        "source_version_id":source,"content_state":if inherited { "PENDING" } else { state },
        "visible_content_state":state})
}
fn old_branch_wire(
    state: &State,
    branch: &storage_contract::Branch,
) -> Result<Value, json_codec::Error> {
    let mut result = json_codec::to_value(branch)?;
    let snapshot = state
        .commits
        .get(&branch.head_commit_id)
        .expect("invariant: admitted branch head references an existing commit");
    let mut pending = snapshot.pending_file_versions.clone();
    pending.extend(branch.pending_file_versions.clone());
    result["pending_file_versions"] = json_codec::to_value(&pending)?;
    result["materialized"] = pending.is_empty().into();
    result["inherited"] = (!pending.is_empty()).into();
    result["content_state"] = if pending.is_empty() {
        "LIVE"
    } else {
        "PENDING"
    }
    .into();
    if let Some(file) = branch.file_id.as_deref() {
        result["read_only"] = (branch.state != storage_contract::BranchState::Active
            || state.file_branch_aliases.contains_key(&branch.branch_id))
        .into();
        result["alias_of"] = state
            .file_branch_aliases
            .get(&branch.branch_id)
            .cloned()
            .into();
        let visible = snapshot
            .entries
            .iter()
            .find(|entry| entry.file_id == file)
            .map(|entry| entry.version_id.as_str());
        result["node_id"] = branch
            .file_node_id
            .clone()
            .or_else(|| visible.map(str::to_owned))
            .into();
        result.as_object_mut().unwrap().extend(
            old_content_wire(state, visible, pending.get(file).map(String::as_str))
                .as_object()
                .unwrap()
                .clone(),
        );
    }
    Ok(result)
}

#[test]
fn branch_projection_matches_fixed_legacy_shapes_and_pending_precedence() {
    let root = crate::tests::private_tempdir();
    let mut engine = storage_engine::Engine::initialize(
        &root.path().join("system"),
        storage_contract::Config::development([root.path().join("disk")]),
    )
    .unwrap();
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("Repo").unwrap(),
            "P01",
            1,
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    for file in [false, true] {
        for source in [None, Some("source"), Some("")] {
            for visible in [None, Some("visible"), Some("")] {
                for disposition in [
                    None,
                    Some(storage_contract::ContentState::Available),
                    Some(storage_contract::ContentState::Purged),
                    Some(storage_contract::ContentState::Migrated),
                ] {
                    for (status, alias, node) in [
                        (BranchState::Active, false, None),
                        (BranchState::Active, true, None),
                        (BranchState::Archived, false, Some("explicit-node")),
                        (BranchState::Migrated, false, None),
                        (BranchState::Merged, true, Some("merged-node")),
                    ] {
                        let mut state = engine.state().clone();
                        let mut branch = state.branches[&repo.main_branch_id].clone();
                        branch.generation = u64::MAX;
                        branch.state = status;
                        branch.file_node_id = node.map(str::to_owned);
                        if file {
                            branch.file_id = Some("file".into());
                            branch.branch_kind = storage_contract::BranchKind::File;
                        }
                        if alias {
                            state
                                .file_branch_aliases
                                .insert(branch.branch_id.clone(), "original".into());
                        }
                        if let Some(source) = source {
                            branch
                                .pending_file_versions
                                .insert("file".into(), source.into());
                        }
                        let head = state.commits.get_mut(&branch.head_commit_id).unwrap();
                        // Branch-local pending overrides a colliding head source.
                        if source.is_some() {
                            head.pending_file_versions
                                .insert("file".into(), "head-source".into());
                        }
                        if let Some(visible) = visible {
                            head.entries.push(storage_contract::Entry {
                                entry_id: "entry".into(),
                                path: name_policy::PortablePath::parse("file.txt").unwrap(),
                                kind: storage_contract::NamespaceEntryKind::File,
                                file_id: "file".into(),
                                version_id: visible.into(),
                                size: 7,
                            });
                        }
                        if let Some(disposition) = disposition {
                            state.content_states.insert("source".into(), disposition);
                            state.content_states.insert("visible".into(), disposition);
                        }
                        let expected = old_branch_wire(&state, &branch).unwrap();
                        let snapshot = capture(&state, &branch);
                        let actual = json_codec::to_value(BranchResponse::from(snapshot)).unwrap();
                        assert_eq!(
                            actual, expected,
                            "file={file} source={source:?} visible={visible:?} state={disposition:?} alias={alias}"
                        );
                        assert_eq!(actual["generation"], u64::MAX.to_string());
                        assert_eq!(actual.get("alias_of").is_some(), file);
                        assert_eq!(actual.get("file_id").is_some(), file);
                    }
                }
            }
        }
    }
}

fn shared_branch_inputs() -> (State, Branch) {
    use storage_contract::{BranchKind, Commit, ContentState, Entry, NamespaceEntryKind};
    let mut state = State::default();
    state.commits.insert(
        "head".into(),
        Commit {
            commit_id: "head".into(),
            history_tree_id: "history".into(),
            kind: "file_branch_root".into(),
            parent_commit_ids: vec!["fork".into()],
            tree_id: "tree".into(),
            sequence: 1,
            operation_id: "op_00000000000000000000000000000001".parse().unwrap(),
            author: "owner".into(),
            message: String::new(),
            entries: vec![Entry {
                entry_id: "entry".into(),
                path: name_policy::PortablePath::parse("file.txt").unwrap(),
                kind: NamespaceEntryKind::File,
                file_id: "file".into(),
                version_id: "body".into(),
                size: 7,
            }],
            namespace_deletions: None,
            pending_file_versions: Default::default(),
            provenance: None,
        },
    );
    for version in ["body", "branch-source"] {
        state
            .content_states
            .insert(version.into(), ContentState::Available);
    }
    let branch = Branch {
        branch_id: "branch".into(),
        history_tree_id: "history".into(),
        name: "main".into(),
        head_commit_id: "head".into(),
        fork_commit_id: "fork".into(),
        branch_kind: BranchKind::File,
        file_id: Some("file".into()),
        file_node_id: None,
        pending_file_versions: Default::default(),
        generation: u64::MAX,
        state: BranchState::Active,
    };
    (state, branch)
}

fn shared_history_wire(case: &str) -> Value {
    use crate::backend_file_history::{FileHistoryResponse, FileHistorySnapshot, HistoryBranch};
    let (mut state, mut branch) = shared_branch_inputs();
    if matches!(case, "merged-pending" | "empty-source") {
        state.commits.get_mut("head").unwrap().pending_file_versions = [
            ("file".into(), "commit-source".into()),
            ("commit-only".into(), "commit-reservation".into()),
        ]
        .into();
        branch.pending_file_versions = [
            (
                "file".into(),
                if case == "empty-source" {
                    ""
                } else {
                    "branch-source"
                }
                .into(),
            ),
            ("branch-only".into(), "branch-reservation".into()),
        ]
        .into();
    }
    let selected = match case {
        "explicit-node" => {
            branch.file_node_id = Some("file-node".into());
            Some("branch")
        }
        "visible-node" | "no-selection" => None,
        "merged-pending" => Some("branch"),
        "empty-source" => {
            branch.state = BranchState::Merged;
            state
                .file_branch_aliases
                .insert("branch".into(), "survivor".into());
            Some("full-branch")
        }
        "full-selection" => Some("full-branch"),
        _ => panic!("unexpected shared branch fixture {case}"),
    };
    let branches = if matches!(case, "no-selection" | "full-selection") {
        vec![]
    } else {
        vec![HistoryBranch {
            current: selected == Some(branch.branch_id.as_str()),
            branch: capture(&state, &branch),
        }]
    };
    json_codec::to_value(FileHistoryResponse::from(FileHistorySnapshot {
        file_id: "file".into(),
        repository_id: "repository".into(),
        path: None,
        note: String::new(),
        current_node_id: None,
        selected_branch_id: selected.map(str::to_owned),
        versions: vec![],
        nodes: vec![],
        branches,
        merges: vec![],
        content: None,
    }))
    .unwrap()
}

#[test]
fn actual_branch_capture_and_history_serializer_match_shared_web_fixtures() {
    // This crosses the branch producer/consumer contract, not a complete HTTP
    // history graph: unrelated node/version payloads deliberately stay empty.
    let fixtures = storage_contract::strict_json(include_bytes!(
        "../../../fixtures/file-history-branches.json"
    ))
    .unwrap();
    let fixtures = fixtures.as_object().unwrap();
    assert_eq!(fixtures.len(), 6);
    for (case, expected) in fixtures {
        assert_eq!(&shared_history_wire(case), expected, "{case}");
    }
}
