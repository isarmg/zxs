//! Compact-summary projection only. Synthetic in-memory metadata is not a real
//! file import, and branch count is fixed at one: unlimited branches remain a
//! separate, unpaginated response dimension.
use super::*;
use storage_contract::{DirectoryBranch, Entry};

#[test]
fn one_branch_summary_does_not_serialize_its_growing_entry_graph() {
    let mut baseline_bytes = None;
    for size in [1_000, 10_000, 100_000] {
        let branch = DirectoryBranch {
            directory_branch_id: "branch-fixture".into(),
            repository_id: "repo-fixture".into(),
            directory_id: "directory-fixture".into(),
            path: "docs".into(),
            name: "main".into(),
            parent_branch_id: None,
            head_tree_id: "tree-fixture".into(),
            commit_id: "commit-fixture".into(),
            entries: (0..size)
                .map(|index| {
                    let name = format!("file-{index:06}.txt");
                    let path = format!("docs/{name}");
                    Entry {
                        entry_id: format!("entry-{index:06}"),
                        path: path.clone(),
                        name,
                        lookup_key: path,
                        kind: "file".into(),
                        file_id: format!("file-{index:06}"),
                        version_id: "version-fixture".into(),
                        size: 1024,
                    }
                })
                .collect(),
            file_branches: [("file-fixture".into(), "file-branch-fixture".into())].into(),
            file_contexts: Default::default(),
            inherited_files: [("file-fixture".into(), "source-fixture".into())].into(),
            directory_refs: Default::default(),
            materialized: true,
            sequence: 42,
        };
        let summary = branch_summary(&branch, true);
        assert_eq!(summary["directory_branch_id"], "branch-fixture");
        assert_eq!(summary["file_count"], size.to_string());
        assert_eq!(summary["current"], true);
        assert_eq!(summary["sequence"], "42");
        for omitted in [
            "entries",
            "file_contexts",
            "file_branches",
            "inherited_files",
            "directory_refs",
        ] {
            assert!(summary.get(omitted).is_none(), "summary leaked {omitted}");
        }
        let bytes = json_codec::to_vec(&json!({"directory_branches":[summary]}))
            .unwrap()
            .len();
        let adjusted = bytes - size.to_string().len();
        assert_eq!(
            *baseline_bytes.get_or_insert(adjusted),
            adjusted,
            "fixed-branch summary size may grow only by file-count digits"
        );
        println!(
            "synthetic_branch_entries={size}; branch_count=1; summary_response_bytes={bytes}; physical_files=0"
        );
    }
}
