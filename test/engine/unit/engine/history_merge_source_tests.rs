use super::super::oracle_tests::{context, fixture, request};
use super::*;
#[test]
fn on_demand_source_lookup_preserves_cancellation() {
    let view = fixture();
    let context = context();
    let request = request(HistoryCollection::Nodes, 1);
    let cancellation = HistoryWindowCancellation::new();
    let query = query::Query::new(&view, &request, &context, &cancellation).unwrap();
    cancellation.cancel();
    assert_eq!(
        resolve(&query, "ver_00000000000000000000000000000000")
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::OperationAborted
    );
}
#[test]
fn direct_head_source_succeeds_before_unneeded_large_graph_admission() {
    let mut view = fixture();
    let state = std::sync::Arc::make_mut(&mut view.state.0);
    let template = state.versions["ver_00000000000000000000000000000010"].clone();
    for n in 0..=MAX_ANCESTRY_NODES {
        let mut node = template.clone();
        node.version_id = format!("ver_{:032x}", n + 0x100000);
        node.source_version_ids.clear();
        state.versions.insert(node.version_id.clone(), node);
    }
    let context = context();
    view.history_window_roots = Some(HistoryRootBindings::recovered(
        &view.state.0,
        BTreeMap::from([(context.history_tree_id.as_str().into(), "a".repeat(64))]),
    ));
    let request = request(HistoryCollection::Nodes, 1);
    let cancellation = HistoryWindowCancellation::new();
    let query = query::Query::new(&view, &request, &context, &cancellation).unwrap();
    let direct = resolve(&query, "ver_00000000000000000000000000000010").unwrap();
    assert_eq!(
        direct.node_id.unwrap().as_str(),
        "ver_00000000000000000000000000000010"
    );
    assert_eq!(
        resolve(&query, "ver_00000000000000000000000000000001")
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::ResourceLimit
    );
}

#[test]
fn source_summary_charges_commit_entries_even_with_explicit_head() {
    let view = fixture();
    let context = context();
    let request = request(HistoryCollection::Nodes, 1);
    let cancellation = HistoryWindowCancellation::new();
    let query = query::Query::new(&view, &request, &context, &cancellation).unwrap();
    let branch = query
        .state
        .branches
        .values()
        .find(|branch| query.allowed_branch(&branch.branch_id) && branch.file_node_id.is_some())
        .unwrap();
    let mut exhausted = MAX_ANCESTRY_HEADERS;
    assert_eq!(
        summary(&query, branch, &mut exhausted).unwrap_err().code(),
        storage_contract::ErrorCode::ResourceLimit
    );
    let mut available = 0;
    assert_eq!(
        summary(&query, branch, &mut available).unwrap(),
        query.branch_summary(branch).unwrap()
    );
    assert!(available > 0);
}
