use super::*;
use json_codec::{Value, json};

fn snapshot() -> BranchSnapshot {
    BranchSnapshot {
        branch_id: "branch".into(),
        history_tree_id: "history".into(),
        name: String::new(),
        head_commit_id: "head".into(),
        fork_commit_id: "fork".into(),
        branch_kind: BranchKind::Full,
        file_id: None,
        file_node_id: None,
        pending_file_versions: Default::default(),
        generation: u64::MAX,
        state: BranchState::Active,
        file: None,
    }
}
#[test]
fn borrowed_pending_union_is_sorted_unique_and_branch_overrides_even_empty() {
    let head: BTreeMap<String, String> = [
        ("a".into(), "head-a".into()),
        ("c".into(), "head-c".into()),
        ("z".into(), "head-z".into()),
    ]
    .into();
    let branch = [
        ("b".into(), "branch-b".into()),
        ("c".into(), "".into()),
        ("z".into(), "branch-z".into()),
    ]
    .into();
    let pending = PendingUnion::new(&head, &branch);
    assert_eq!(pending.get("a"), Some("head-a"));
    assert_eq!(pending.get("c"), Some(""));
    assert_eq!(pending.get("missing"), None);
    assert_eq!(pending.get("z").unwrap().as_ptr(), branch["z"].as_ptr());
    assert_eq!(
        json_codec::to_vec(&pending).unwrap(),
        br#"{"a":"head-a","b":"branch-b","c":"","z":"branch-z"}"#
    );
    assert!(!pending.is_empty());
    assert_eq!(
        json_codec::to_value(PendingUnion::new(&BTreeMap::new(), &BTreeMap::new())).unwrap(),
        json!({})
    );
}
#[test]
fn branch_metadata_single_and_list_accept_exact_cap_and_reject_one_more_byte() {
    for list in [false, true] {
        let encode = |snapshot| {
            if list {
                BranchMetadataSnapshot::encode_list([snapshot])
            } else {
                BranchMetadataSnapshot::encode(snapshot)
            }
        };
        let base = encode(snapshot()).unwrap().body().len();
        let mut exact = snapshot();
        exact.name = "x".repeat(storage_contract::client_protocol::MAX_RESPONSE_BYTES - base);
        let mut oversized = exact.clone();
        oversized.name.push('x');
        let encoded = encode(exact).unwrap();
        assert_eq!(
            encoded.body().len(),
            storage_contract::client_protocol::MAX_RESPONSE_BYTES
        );
        assert!(json_codec::from_slice::<Value>(encoded.body()).is_ok());
        assert_eq!(
            encode(oversized).unwrap_err().code,
            storage_contract::ErrorCode::ResourceLimitExceeded
        );
    }
}
#[test]
fn branch_metadata_wire_keeps_decimal_generation_and_moves_encoded_bytes() {
    let encoded = BranchMetadataSnapshot::encode(snapshot()).unwrap();
    let value: Value = json_codec::from_slice(encoded.body()).unwrap();
    assert_eq!(value["generation"], u64::MAX.to_string());
    assert_eq!(value["pending_file_versions"], json!({}));
    assert!(value.get("file_id").is_none());
    let pointer = encoded.body().as_ptr();
    assert_eq!(encoded.into_encoded().body().as_ptr(), pointer);
    assert_eq!(
        json_codec::from_slice::<Value>(BranchMetadataSnapshot::encode_list([]).unwrap().body())
            .unwrap(),
        json!({"branches":[]})
    );
}

#[test]
fn borrowed_single_and_list_share_owned_wire_with_escaped_union_fields() {
    let BranchSnapshot {
        branch_id,
        history_tree_id,
        name: _,
        head_commit_id,
        fork_commit_id,
        branch_kind,
        file_id,
        file_node_id,
        pending_file_versions: _,
        generation,
        state,
        file,
    } = snapshot();
    let branch = storage_contract::Branch {
        branch_id,
        history_tree_id,
        name: "quoted\"\\\n草稿".into(),
        head_commit_id,
        fork_commit_id,
        branch_kind,
        file_id,
        file_node_id,
        pending_file_versions: [("c".into(), "".into()), ("b\"".into(), "branch\\".into())].into(),
        generation,
        state,
    };
    let head: BTreeMap<String, String> = [
        ("a".into(), "head\n".into()),
        ("c".into(), "overridden".into()),
    ]
    .into();
    let mut owned = snapshot();
    owned.name = branch.name.clone();
    owned.pending_file_versions = head.clone();
    owned
        .pending_file_versions
        .extend(branch.pending_file_versions.clone());
    let expected: Value =
        json_codec::from_slice(BranchMetadataSnapshot::encode(owned).unwrap().body()).unwrap();
    let row = || BorrowedBranch {
        branch: &branch,
        pending: PendingUnion::new(&head, &branch.pending_file_versions),
        file: file.clone(),
    };
    let actual = BranchMetadataSnapshot::encode_borrowed(row()).unwrap();
    assert_eq!(
        json_codec::from_slice::<Value>(actual.body()).unwrap(),
        expected
    );
    let list = BranchMetadataSnapshot::encode_borrowed_list(vec![row()]).unwrap();
    assert_eq!(
        json_codec::from_slice::<Value>(list.body()).unwrap(),
        json!({"branches":[expected]})
    );
}
