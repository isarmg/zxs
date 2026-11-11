use super::*;

#[test]
fn whole_file_body_rejects_duplicate_unknown_and_missing_identity_fields() {
    let source = r#"{"repository_id":"repo_11111111111111111111111111111111","path":"A.txt","operation_id":"op_11111111111111111111111111111111"}"#;
    let body: Delete = storage_contract::decode_strict(source.as_bytes()).unwrap();
    assert_eq!(
        body.repository_id.as_str(),
        "repo_11111111111111111111111111111111"
    );
    assert_eq!(body.path, "A.txt");
    for changed in [
        source.replace(
            "\"path\":\"A.txt\"",
            "\"path\":\"A.txt\",\"path\":\"B.txt\"",
        ),
        source.replace("\"path\":\"A.txt\"", "\"path\":\"A.txt\",\"branch_ids\":[]"),
        source.replace("\"path\":\"A.txt\"", "\"path\":\"A.txt\",\"force\":true"),
        source.replace("\"path\":\"A.txt\",", ""),
        source.replace(
            "repo_11111111111111111111111111111111",
            "file_11111111111111111111111111111111",
        ),
    ] {
        assert!(storage_contract::decode_strict::<Delete>(changed.as_bytes()).is_err());
    }
}
#[test]
fn undo_body_uses_the_same_strict_json_boundary() {
    let source = r#"{"deletion_node_id":"fdel_11111111111111111111111111111111","operation_id":"op_11111111111111111111111111111111"}"#;
    assert!(storage_contract::decode_strict::<Undo>(source.as_bytes()).is_ok());
    let duplicate = source.replace(
        "\"deletion_node_id\":",
        "\"operation_id\":\"op_22222222222222222222222222222222\",\"deletion_node_id\":",
    );
    assert!(storage_contract::decode_strict::<Undo>(duplicate.as_bytes()).is_err());
}

#[test]
fn batch_undo_accepts_only_original_batch_and_new_operation() {
    use storage_contract::file_deletion_batch::UndoFileDeletionBatchBody;
    let source = r#"{"deletion_operation_id":"op_11111111111111111111111111111111","operation_id":"op_22222222222222222222222222222222"}"#;
    assert!(
        storage_contract::decode_strict::<UndoFileDeletionBatchBody>(source.as_bytes()).is_ok()
    );
    for changed in [
        source.replace("\"operation_id\":", "\"file_ids\":[],\"operation_id\":"),
        source.replace("\"operation_id\":", "\"force\":true,\"operation_id\":"),
        source.replace(
            "op_11111111111111111111111111111111",
            "file_11111111111111111111111111111111",
        ),
        source.replace(
            "\"operation_id\":",
            "\"operation_id\":\"op_33333333333333333333333333333333\",\"operation_id\":",
        ),
    ] {
        assert!(
            storage_contract::decode_strict::<UndoFileDeletionBatchBody>(changed.as_bytes())
                .is_err()
        );
    }
}
