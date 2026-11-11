use super::*;
use json_codec::json;

#[test]
fn complete_batch_undo_has_two_typed_identifiers_and_no_member_override() {
    let original = OperationId::generate().unwrap();
    let operation = OperationId::generate().unwrap();
    let body = UndoFileDeletionBatchBody {
        deletion_operation_id: original,
        operation_id: operation,
    };
    let value = json!({"deletion_operation_id": original, "operation_id": operation});
    assert_eq!(json_codec::to_value(body).unwrap(), value);
    assert_eq!(
        json_codec::from_value::<UndoFileDeletionBatchBody>(value.clone()).unwrap(),
        body
    );
    assert_eq!(UNDO_KIND, "file.delete.batch.undo");
    assert_eq!(MAX_RESPONSE_BYTES, 512 * 1024);
    for field in ["file_ids", "deletion_nodes", "force", "principal"] {
        let mut invalid = value.clone();
        invalid[field] = json!([]);
        assert!(json_codec::from_value::<UndoFileDeletionBatchBody>(invalid).is_err());
    }
    for field in ["deletion_operation_id", "operation_id"] {
        let mut missing = value.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert!(json_codec::from_value::<UndoFileDeletionBatchBody>(missing).is_err());
        for bad in [
            json!(null),
            json!(17),
            json!(""),
            json!("op_not-an-id"),
            json!("file_00000000000000000000000000000001"),
        ] {
            let mut invalid = value.clone();
            invalid[field] = bad;
            assert!(json_codec::from_value::<UndoFileDeletionBatchBody>(invalid).is_err());
        }
        let duplicate = format!(
            "{{\"deletion_operation_id\":\"{original}\",\"operation_id\":\"{operation}\",\"{field}\":\"{original}\"}}"
        );
        assert!(json_codec::from_str::<UndoFileDeletionBatchBody>(&duplicate).is_err());
    }
}

#[test]
fn batch_undo_preview_uses_only_original_operation_and_full_node_list() {
    let original = OperationId::generate().unwrap();
    let preview = FileDeletionBatchUndoPreview {
        deletion_operation_id: original,
        deletion_nodes: Vec::new(),
    };
    let value = json!({"deletion_operation_id": original, "deletion_nodes": []});
    assert_eq!(json_codec::to_value(&preview).unwrap(), value);
    assert_eq!(
        json_codec::from_value::<FileDeletionBatchUndoPreview>(value.clone()).unwrap(),
        preview
    );
    // Empty lists are a wire value only; the Engine proves a nonempty original
    // batch and complete current eligibility before producing this response.
    for field in ["deletion_operation_id", "deletion_nodes"] {
        let mut invalid = value.clone();
        invalid.as_object_mut().unwrap().remove(field);
        assert!(json_codec::from_value::<FileDeletionBatchUndoPreview>(invalid).is_err());
        let duplicate = format!(
            "{{\"deletion_operation_id\":\"{original}\",\"deletion_nodes\":[],\"{field}\":[]}}"
        );
        assert!(json_codec::from_str::<FileDeletionBatchUndoPreview>(&duplicate).is_err());
    }
    let mut invalid = value;
    invalid["may_restore"] = json!(true);
    assert!(json_codec::from_value::<FileDeletionBatchUndoPreview>(invalid).is_err());
}
