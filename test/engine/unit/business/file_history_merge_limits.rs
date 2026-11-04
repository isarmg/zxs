use super::*;
use std::sync::Arc;

fn fixture() -> (tempfile::TempDir, Engine, Vec<Entry>, String) {
    let temporary = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &temporary.path().join("system"),
        Config::development([temporary.path().join("p"), temporary.path().join("b")]).unwrap(),
    )
    .unwrap();
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("Budget fixture").unwrap(),
            "P01",
            2,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    let mut nodes = Vec::new();
    for body in [b"base".as_slice(), b"later".as_slice()] {
        let workspace = engine
            .create_workspace(&repo.main_branch_id, "admin")
            .unwrap();
        engine
            .stage_bytes(&workspace.workspace_id, "report.txt", body)
            .unwrap();
        engine
            .commit_workspace(CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: OperationId::generate().unwrap(),
                principal: "admin".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "Budget fixture".into(),
                request_digest: None,
            })
            .unwrap();
        let head =
            &engine.state().commits[&engine.state().branches[&repo.main_branch_id].head_commit_id];
        nodes.push(
            head.entries
                .iter()
                .find(|entry| entry.kind == NamespaceEntryKind::File)
                .unwrap()
                .clone(),
        );
    }
    let file = nodes[0].file_id.clone();
    let branch = engine
        .create_file_branch_from_node(
            &file,
            &nodes[1].version_id,
            &nodes[1].version_id,
            &storage_contract::BranchName::parse("source").unwrap(),
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap()
        .result["branch_id"]
        .as_str()
        .unwrap()
        .to_owned();
    (temporary, engine, nodes, branch)
}

#[test]
fn historical_merge_index_budget_failure_never_publishes_or_derives_branches() {
    let (_temporary, mut engine, nodes, branch) = fixture();
    let file = nodes[0].file_id.clone();
    // Only unneeded scoped header count is enlarged. The request is rejected
    // before replica checks or persistence, so no fabricated body is published.
    let state = Arc::make_mut(&mut engine.state);
    let template = state.versions[&nodes[1].version_id].clone();
    for n in 0..=MAX_ANCESTRY_NODES {
        let mut node = template.clone();
        node.version_id = format!("ver_{:032x}", n + 0x200000);
        node.source_version_ids.clear();
        state.versions.insert(node.version_id.clone(), node);
    }
    let before = Arc::clone(&engine.state);
    let sequence = engine.state.sequence;
    let inputs = nodes
        .iter()
        .map(|node| storage_contract::FileHistoryMergeInput {
            branch_id: branch.clone(),
            source_node_id: node.version_id.clone(),
            expected_node_id: nodes[1].version_id.clone(),
        })
        .collect::<Vec<_>>();
    let operation = OperationId::generate().unwrap();
    let error = engine
        .merge_file_history(FileHistoryMergeRequest {
            file_id: &file,
            inputs: &inputs,
            chosen_node_id: &nodes[0].version_id,
            name: &storage_contract::BranchName::parse("must not exist").unwrap(),
            location: None,
            principal: "admin",
            operation_id: operation,
        })
        .unwrap_err();
    assert_eq!(error.code(), storage_contract::ErrorCode::ResourceLimit);
    assert!(Arc::ptr_eq(&engine.state, &before));
    assert_eq!(engine.state.sequence, sequence);
    assert!(!engine.state.operations.contains_key(&operation));
    assert!(
        engine
            .state
            .branches
            .values()
            .all(|branch| branch.name != "must not exist")
    );
}

#[test]
fn historical_merge_rejects_mixed_history_inputs_before_deriving() {
    let (_temporary, mut engine, nodes, branch) = fixture();
    let file = nodes[0].file_id.clone();
    let foreign_version = new_id("ver").unwrap();
    let foreign_branch = new_id("br").unwrap();
    let foreign_history = new_id("history").unwrap();
    let state = Arc::make_mut(&mut engine.state);
    let mut body = state.versions[&nodes[1].version_id].clone();
    body.version_id = foreign_version.clone();
    body.history_tree_id = foreign_history.clone();
    state.versions.insert(foreign_version.clone(), body);
    let mut foreign = state.branches[&branch].clone();
    foreign.branch_id = foreign_branch.clone();
    foreign.history_tree_id = foreign_history;
    foreign.file_node_id = Some(foreign_version.clone());
    state.branches.insert(foreign_branch.clone(), foreign);
    let before = Arc::clone(&engine.state);
    let operation = OperationId::generate().unwrap();
    let inputs = [
        storage_contract::FileHistoryMergeInput {
            branch_id: branch,
            source_node_id: nodes[0].version_id.clone(),
            expected_node_id: nodes[1].version_id.clone(),
        },
        storage_contract::FileHistoryMergeInput {
            branch_id: foreign_branch,
            source_node_id: foreign_version.clone(),
            expected_node_id: foreign_version,
        },
    ];
    let error = engine
        .merge_file_history(FileHistoryMergeRequest {
            file_id: &file,
            inputs: &inputs,
            chosen_node_id: &nodes[0].version_id,
            name: &storage_contract::BranchName::parse("must not derive").unwrap(),
            location: None,
            principal: "admin",
            operation_id: operation,
        })
        .unwrap_err();
    assert_eq!(error.code(), storage_contract::ErrorCode::FileNodeNotFound);
    assert!(Arc::ptr_eq(&engine.state, &before));
    assert!(!engine.state.operations.contains_key(&operation));
}
