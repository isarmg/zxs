use super::*;
use json_codec::{Value, json};

fn branch() -> DirectoryBranch {
    DirectoryBranch {
        directory_branch_id: "directory".into(),
        repository_id: "repo".into(),
        directory_id: "repo".into(),
        path: "stored/path".into(),
        name: "草稿".into(),
        parent_branch_id: None,
        head_tree_id: "tree".into(),
        commit_id: "commit".into(),
        entries: vec![],
        file_branches: [("file".into(), "branch".into())].into(),
        file_contexts: Default::default(),
        inherited_files: Default::default(),
        directory_refs: Default::default(),
        materialized: false,
        sequence: u64::MAX,
    }
}
#[test]
fn default_directory_wire_distinguishes_absent_selection_from_named_metadata() {
    let empty = DefaultDirectoryBranchSnapshot::encode("repo", "namespace", None, &[]).unwrap();
    assert_eq!(
        json_codec::from_slice::<Value>(empty.body()).unwrap(),
        json!({
            "repository_id":"repo", "namespace_branch_id":"namespace", "directory_branch_id":null,
            "name":"main", "directory_branches":[],
        })
    );
    let branch = branch();
    let row = DefaultDirectoryBranchRow {
        branch: &branch,
        current: true,
    };
    assert!(std::ptr::eq(row.branch, &branch));
    let named = DefaultDirectoryBranchSnapshot::encode(
        "repo",
        "namespace",
        Some(SelectedDirectoryBranch {
            directory_branch_id: "directory",
            name: "草稿",
        }),
        &[row],
    )
    .unwrap();
    let expected = json!({"repository_id":"repo","namespace_branch_id":"namespace","directory_branch_id":"directory","name":"草稿","directory_branches":[{"directory_branch_id":"directory","name":"草稿","path":"","current":true,"materialized":false,"head_tree_id":"tree","file_count":"1","parent_branch_id":null}]});
    assert_eq!(
        json_codec::from_slice::<Value>(named.body()).unwrap(),
        expected
    );
    let allocation = named.body().as_ptr();
    assert_eq!(named.into_encoded().body().as_ptr(), allocation);
}
#[test]
fn default_directory_complete_response_accepts_exact_cap_and_rejects_one_more_byte() {
    let base = DefaultDirectoryBranchSnapshot::encode("", "namespace", None, &[])
        .unwrap()
        .body()
        .len();
    let id = "x".repeat(storage_contract::client_protocol::MAX_RESPONSE_BYTES - base);
    let exact = DefaultDirectoryBranchSnapshot::encode(&id, "namespace", None, &[]).unwrap();
    assert_eq!(
        exact.body().len(),
        storage_contract::client_protocol::MAX_RESPONSE_BYTES
    );
    assert!(json_codec::from_slice::<Value>(exact.body()).is_ok());
    let error =
        DefaultDirectoryBranchSnapshot::encode(&(id + "x"), "namespace", None, &[]).unwrap_err();
    assert_eq!(
        error.code,
        storage_contract::ErrorCode::ResourceLimitExceeded
    );
}
