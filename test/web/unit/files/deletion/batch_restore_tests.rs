use crate::files::deletion::{batch_restore::Request, request::Intent};
use json_codec::{Value, json};
use storage_contract::{FileDeletionNode, OperationId};
use std::sync::Arc;

fn operation(n: u32) -> OperationId {
    format!("op_{n:032x}").parse().unwrap()
}
fn node(n: u32) -> FileDeletionNode {
    json_codec::from_value(json!({
        "node_id":format!("fdel_{n:032x}"),
        "history_tree_id":format!("history_{:032x}", 1),
        "file_id":format!("file_{n:032x}"),
        "repository_id":format!("repo_{:032x}", 1),
        "path":format!("{n}.txt"),
        "branch_heads":{format!("br_{n:032x}"):format!("ver_{n:032x}")},
        "parent_node_ids":[format!("ver_{n:032x}")],
        "operation_id":operation(1), "author":"alice", "sequence":"1", "created_at":"123"
    })).unwrap()
}
fn request() -> Request {
    Request::admit(operation(1), vec![node(1), node(2)]).unwrap()
}
fn receipt() -> Value {
    json!({
        "operation_kind":"file.delete.batch.undo",
        "protection_at_publish":"NOT_APPLICABLE",
        "publication_sequence":"2",
        "result":{
            "deletion_operation_id":operation(1),
            "removed_deletion_nodes":[node(1),node(2)],
            "timestamp":"124", "membership_epoch":"1", "version_protection":[]
        }
    })
}
#[test]
fn preview_requires_selected_exact_node_and_original_batch() {
    let preview = json!({"deletion_operation_id":operation(1),"deletion_nodes":[node(1), node(2)]});
    assert!(Request::decode(preview.clone(), &node(2)).is_ok());
    assert!(Request::decode(preview.clone(), &node(3)).is_err());
    let mut changed = node(1);
    changed.operation_id = operation(3);
    assert!(Request::decode(preview, &changed).is_err());
    assert!(Request::admit(operation(1), vec![]).is_err());
    assert!(Request::admit(operation(1), vec![node(1);101]).is_err());
}
#[test]
fn receipt_requires_complete_exact_original_nodes() {
    let request = request();
    assert!(request.receipt_matches(&receipt()));
    let mut partial = receipt();
    partial["result"]["removed_deletion_nodes"] = json!([node(1)]);
    assert!(!request.receipt_matches(&partial));
    let mut changed = receipt();
    changed["result"]["removed_deletion_nodes"][1]["path"] = json!("changed.txt");
    assert!(!request.receipt_matches(&changed));
    let mut wrong = receipt();
    wrong["result"]["deletion_operation_id"] = json!(operation(4));
    assert!(!request.receipt_matches(&wrong));
    let mut stale = receipt();
    stale["publication_sequence"] = json!("1");
    assert!(!request.receipt_matches(&stale));
}
#[test]
fn undo_uses_one_original_operation_request_and_native_digest() {
    let request = Arc::new(request());
    let intent = Intent::UndoBatch(request);
    let (endpoint, body) = intent.request(operation(2));
    assert_eq!(endpoint, "/api/file-deletion-batches/undo");
    let expected = storage_contract::file_deletion_batch::UndoFileDeletionBatchBody {
        deletion_operation_id: operation(1),
        operation_id: operation(2),
    };
    assert!(matches!(&body, crate::files::deletion::request::Body::BatchUndo(_)));
    assert_eq!(json_codec::to_string(&body).unwrap(), json_codec::to_string(&expected).unwrap());
    assert_eq!(json_codec::to_value(body).unwrap(), json!({"deletion_operation_id":operation(1),"operation_id":operation(2)}));
    let digest = storage_contract::object_hash("request", &json!({
        "kind":"file.delete.batch.undo", "principal":"alice",
        "body":{"deletion_operation_id":operation(1)}
    })).unwrap();
    assert_eq!(intent.digest("alice").unwrap(), digest);
}

#[test]
fn batch_restore_owns_the_single_restore_review_slot_and_cancel_releases_it() {
    use crate::files::deletion::{Model, batch_restore::Review, restore};
    let request = Arc::new(request());
    let review = restore::Review::Batch(Review {
        target: restore::Target {
            epoch: 1,
            generation: 2,
            node: Arc::new(node(1)),
        },
        generation: 3,
        request,
    });
    assert!(review.single().is_none());
    assert!(review.batch().is_some());
    let mut model = Model::default();
    model.restore_review = Some(review);
    model.cancel_review();
    assert!(model.restore_review.is_none());
}
