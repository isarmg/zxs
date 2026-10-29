use super::*;
use crate::business::file_nodes::{FileAncestry, MERGE_EVENT_VISITS};

#[test]
fn merge_reachability_is_lazy_and_reuses_one_parent_projection() {
    let root = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &root.path().join("system"),
        Config::development([root.path().join("disk")]),
    )
    .unwrap();
    let state = std::sync::Arc::make_mut(&mut engine.state);
    for (id, node, parents) in [
        ("merge-a", "a", vec!["a", "b", "b"]),
        ("merge-b", "b", vec!["a", "target"]),
    ] {
        state.file_branch_merges.insert(
            id.into(),
            FileBranchMerge {
                merge_id: id.into(),
                history_tree_id: "history".into(),
                file_id: "file".into(),
                input_branch_ids: vec![],
                input_node_ids: vec![],
                chosen_node_id: node.into(),
                result_node_id: node.into(),
                surviving_branch_id: "branch".into(),
                reused_node: true,
                parent_node_ids: parents.into_iter().map(str::to_owned).collect(),
                operation_id: OperationId::generate(),
                sequence: 1,
            },
        );
    }
    let mut ancestry = FileAncestry::new(&engine, "file");
    MERGE_EVENT_VISITS.set(0);
    assert!(reaches(&mut ancestry, "a", "a"));
    assert_eq!(MERGE_EVENT_VISITS.get(), 0);
    assert!(reaches(&mut ancestry, "a", "target"));
    assert_eq!(MERGE_EVENT_VISITS.get(), 2);
    assert!(!reaches(&mut ancestry, "a", "absent"));
    assert_eq!(MERGE_EVENT_VISITS.get(), 2);
}
