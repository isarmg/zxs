use super::*;
use json_codec::json;
#[test]
fn archive_wire_is_the_raw_receipt_without_health_observation_fields() {
    let receipt = OperationResult {
        operation_id: "op_00000000000000000000000000000000".parse().unwrap(),
        kind: "branch.archive".into(),
        commit_id: None,
        history_tree_id: Some("history".into()),
        branch_id: Some("branch".into()),
        commit_state: CommitState::Published,
        protection_at_publish: ProtectionAtPublish::NotApplicable,
        current_health: CurrentHealth::Recovering,
        request_digest: "digest".into(),
        sequence: u64::MAX,
        result: json!({"branch_id":"branch","history_tree_id":"history"}),
    };
    let legacy = json_codec::to_value(&receipt).unwrap();
    let response = ArchiveResponse::from(receipt);
    assert_eq!(json_codec::to_value(&response).unwrap(), legacy);
    assert_eq!(legacy.as_object().unwrap().len(), 11);
    assert_eq!(legacy["current_health"], "RECOVERING");
    assert!(legacy.get("operation_kind").is_none());
    assert!(legacy.get("publication_sequence").is_none());
    assert_eq!(storage_contract::canonical_json(&response).unwrap(),br#"{"branch_id":"branch","commit_id":null,"commit_state":"PUBLISHED","current_health":"RECOVERING","history_tree_id":"history","kind":"branch.archive","operation_id":"op_00000000000000000000000000000000","protection_at_publish":"NOT_APPLICABLE","request_digest":"digest","result":{"branch_id":"branch","history_tree_id":"history"},"sequence":"18446744073709551615"}"#);
}
