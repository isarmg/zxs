use super::*;
use storage_contract::{BranchKind, BranchState, HistoryState, OperationId};

#[test]
fn graph_wire_retains_raw_branch_and_boundary_fields_and_canonical_bytes() {
    for populated in [false, true] {
        let operation: OperationId = "op_00000000000000000000000000000000".parse().unwrap();
        let commit = Arc::new(Commit {
            commit_id: "root".into(),
            history_tree_id: "history".into(),
            kind: "open-extension".into(),
            parent_commit_ids: vec![],
            tree_id: "tree".into(),
            sequence: u64::MAX,
            operation_id: operation,
            author: "owner".into(),
            message: "root".into(),
            namespace: crate::fixture_commit_namespace(0),
            namespace_deletions: None,
            pending_file_versions: BTreeMap::new(),
            provenance: None,
        });
        let history = HistoryTree {
            history_tree_id: "history".into(),
            repository_id: "repository".into(),
            root_commit_id: "root".into(),
            main_branch_id: populated.then(|| "branch".into()),
            generation: u64::MAX,
            state: HistoryState::Archived,
            provenance: populated.then(|| "origin".into()),
        };
        let branch = Branch {
            branch_id: "branch".into(),
            history_tree_id: "history".into(),
            name: "branch".into(),
            head_commit_id: "root".into(),
            fork_commit_id: "root".into(),
            branch_kind: BranchKind::File,
            file_id: populated.then(|| "file".into()),
            file_node_id: populated.then(|| "node".into()),
            pending_file_versions: if populated {
                BTreeMap::from([("file".into(), "source".into())])
            } else {
                BTreeMap::new()
            },
            generation: u64::MAX,
            state: BranchState::Migrated,
        };
        let link = MergeLink {
            merge_link_id: "link".into(),
            history_tree_id: "history".into(),
            source_branch_id: "source".into(),
            source_commit_id: "source-commit".into(),
            target_branch_id: "branch".into(),
            target_commit_id: "root".into(),
            author: "owner".into(),
            operation_id: operation,
        };
        let boundary = HistoryBoundary {
            boundary_id: "boundary".into(),
            history_tree_id: "history".into(),
            local_commit_id: "root".into(),
            local_branch_id: populated.then(|| "branch".into()),
            relation: "external-provenance".into(),
            external_history_tree_id: "external-history".into(),
            external_commit_id: "external-commit".into(),
            source_merge_link_id: populated.then(|| "link".into()),
        };
        let expected = storage_contract::HistoryGraph {
            history: history.clone(),
            commits: vec![commit.as_ref().clone()],
            branches: vec![branch.clone()],
            merge_links: vec![link.clone()],
            boundary_marks: vec![boundary.clone()],
        };
        let response = HistoryGraphResponse::from(HistoryGraphSnapshot {
            history,
            commits: vec![commit],
            branches: vec![branch],
            merge_links: vec![link],
            boundary_marks: vec![boundary],
        });
        assert_eq!(
            storage_contract::canonical_json(&response).unwrap(),
            storage_contract::canonical_json(&expected).unwrap()
        );
        let value = json_codec::to_value(response).unwrap();
        assert_eq!(value.as_object().unwrap().len(), 5);
        assert_eq!(value["history"]["generation"], u64::MAX.to_string());
        assert_eq!(value["branches"][0]["generation"], u64::MAX.to_string());
        assert_eq!(value["commits"][0]["kind"], "open-extension");
        assert_eq!(
            value["branches"][0].get("pending_file_versions").is_some(),
            populated
        );
        for field in [
            "materialized",
            "inherited",
            "read_only",
            "alias_of",
            "content_state",
        ] {
            assert!(value["branches"][0].get(field).is_none());
        }
        assert_eq!(
            value["boundary_marks"][0]["external_history_tree_id"],
            "external-history"
        );
        assert_eq!(
            value["boundary_marks"][0]["local_branch_id"].is_null(),
            !populated
        );
    }
}
