use super::*;
use json_codec::json;

#[test]
fn default_directory_wire_distinguishes_absent_selection_from_named_metadata() {
    let empty = DefaultDirectoryBranchResponse::from(DefaultDirectoryBranchSnapshot {
        repository_id: "repo".into(),
        namespace_branch_id: "namespace".into(),
        selected: None,
        directory_branches: vec![],
    });
    assert_eq!(storage_contract::canonical_json(&empty).unwrap(),br#"{"directory_branch_id":null,"directory_branches":[],"name":"main","namespace_branch_id":"namespace","repository_id":"repo"}"#);
    let named = DefaultDirectoryBranchResponse::from(DefaultDirectoryBranchSnapshot {
        repository_id: "repo".into(),
        namespace_branch_id: "namespace".into(),
        selected: Some(SelectedDirectoryBranch {
            directory_branch_id: "directory".into(),
            name: "草稿".into(),
        }),
        directory_branches: vec![DirectoryBranchSummary {
            directory_branch_id: "directory".into(),
            name: "草稿".into(),
            path: "".into(),
            current: true,
            materialized: false,
            head_tree_id: "tree".into(),
            file_count: usize::MAX,
            parent_branch_id: None,
        }],
    });
    let expected = json!({"repository_id":"repo","namespace_branch_id":"namespace","directory_branch_id":"directory","name":"草稿","directory_branches":[{"directory_branch_id":"directory","name":"草稿","path":"","current":true,"materialized":false,"head_tree_id":"tree","file_count":usize::MAX.to_string(),"parent_branch_id":null}]});
    assert_eq!(json_codec::to_value(&named).unwrap(), expected);
    assert_eq!(
        storage_contract::canonical_json(&named).unwrap(),
        storage_contract::canonical_json(&expected).unwrap()
    );
}
