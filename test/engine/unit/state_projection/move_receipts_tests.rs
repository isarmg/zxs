//! Receipt-reader fixtures, not fabricated publication or namespace authority.
use super::*;
use crate::{
    ErrorCode, OperationId, OperationResult,
    file_deletion::retained::{self, DeletionAction},
};
use json_codec::json;
use std::sync::Arc;
use storage_contract::file_relocation::{OBJECT_MOVE_KIND, ObjectMoveRequest};

fn id(prefix: &str, number: u64) -> String {
    format!("{prefix}_{number:032x}")
}

struct Fixture {
    state: State,
    operation: OperationId,
    source: String,
    displaced: String,
    imported: String,
}

fn fixture() -> Fixture {
    let operation = OperationId::generate().unwrap();
    let prior = OperationId::generate().unwrap();
    let source = id("file", 1);
    let displaced = id("file", 2);
    let identity =
        storage_contract::object_hash("file-history-copy-identity", &json!([operation, source]))
            .unwrap();
    let imported = format!("file_{}", &identity[..32]);
    let history = id("history", 1);
    let repository = id("repo", 1);
    let request: ObjectMoveRequest = json_codec::from_value(json!({
        "source_repository_id":repository, "source_entry_id":id("entry", 1),
        "source_path":"source", "target_slot_id":"P02",
        "target_repository_id":id("repo", 2), "target_path":"destination",
        "replace":true, "target_entry_id":id("entry", 2)
    }))
    .unwrap();
    let node = |file: &str, number| {
        json!({
            "node_id":id("fdel", number), "repository_id":repository,
            "history_tree_id":history, "file_id":file, "path":"source",
            "branch_heads":{id("br", number):id("ver", number)},
            "parent_node_ids":[id("ver", number)], "operation_id":operation,
            "author":"alice", "sequence":"2", "created_at":"20"
        })
    };
    let body = json!({
        "movement":{
            "request":request, "principal":"alice",
            "target_repository_id":id("repo", 2), "target_entry_id":id("entry", 2),
            "source_file_id":source, "displaced_target_file_id":displaced
        },
        "route":{"created_at":"20", "slots":["P02"], "bindings":{"P02":"1"}},
        "namespace_basis":{
            "source_head":id("commit", 1), "source_entries":[],
            "target_head":id("commit", 2), "target_entries":[]
        },
        "version_mapping":{}, "retained_version_ids":[],
        "source_deletions":[node(&source, 1)],
        "displaced_deletions":[node(&displaced, 2)],
        "imported_deletions":[node(&imported, 3)],
        "imported_directory_deletions":[],
        "retained_lifecycle":[{
            "source_file_id":source, "source_history_tree_id":id("history", 2),
            "source_sequence":"1", "source_records_hash":"a".repeat(64),
            "file_id":imported, "history_tree_id":history,
            "events":[{
                "operation_id":prior, "action":"delete", "sequence":"1", "author":"alice",
                "node":{
                    "node_id":id("fdel", 4), "history_tree_id":history, "file_id":imported,
                    "branch_heads":{id("br", 3):id("ver", 3)},
                    "parent_node_ids":[id("ver", 3)], "operation_id":prior,
                    "author":"alice", "sequence":"1", "created_at":"10"
                }
            }]
        }],
        "branch_id":id("br", 1), "history_tree_id":history, "commit_id":id("commit", 3),
        "branch_generation":"2", "timestamp":"20"
    });
    let receipt = OperationResult {
        operation_id: operation,
        kind: OBJECT_MOVE_KIND.into(),
        commit_id: Some(id("commit", 3)),
        history_tree_id: Some(history),
        branch_id: Some(id("br", 1)),
        commit_state: storage_contract::CommitState::Published,
        protection_at_publish: storage_contract::ProtectionAtPublish::NotApplicable,
        current_health: storage_contract::CurrentHealth::Healthy,
        request_digest: request.digest("alice").unwrap(),
        sequence: 2,
        result: body,
    };
    let mut state = State::default();
    state.operations.insert(operation, receipt);
    state.operation_principals.insert(operation, "alice".into());
    Fixture {
        state,
        operation,
        source,
        displaced,
        imported,
    }
}

#[test]
fn one_projection_reuses_one_decode_for_every_file_and_lifecycle_kind() {
    let fixture = fixture();
    let transition = ProjectionTransition::new(&fixture.state, &fixture.state, None).unwrap();
    let view = transition.next();
    let decoded = view.object_move(fixture.operation).unwrap();
    for file in [&fixture.source, &fixture.displaced] {
        let rows = retained::events(view, file).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].action, DeletionAction::Delete);
        assert_eq!(rows[0].node.file_id, *file);
    }
    let rows = retained::events(view, &fixture.imported).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].sequence, 1);
    assert_eq!(rows[1].action, DeletionAction::Import);
    assert_eq!(rows[1].operation_id, fixture.operation);
    assert!(Arc::ptr_eq(
        &decoded,
        &transition.next().object_move(fixture.operation).unwrap()
    ));
    assert!(decoded.deletion(&id("file", 99)).is_err());
    assert!(decoded.retained(&fixture.source).is_err());
}

#[test]
fn one_source_snapshot_shares_membership_and_move_receipts_across_file_captures() {
    let fixture = fixture();
    let projection = ProjectionSnapshot::new(&fixture.state);
    let first = projection.view();
    let decoded = first.object_move(fixture.operation).unwrap();
    for file in [&fixture.source, &fixture.displaced, &fixture.imported] {
        let view = projection.view();
        assert!(std::ptr::eq(first.owners(), view.owners()));
        assert!(!retained::events(view, file).unwrap().is_empty());
        assert!(Arc::ptr_eq(
            &decoded,
            &view.object_move(fixture.operation).unwrap()
        ));
    }
}

#[test]
fn receipt_reuse_never_crosses_previous_next_or_new_transition_boundaries() {
    let fixture = fixture();
    let mut changed = fixture.state.clone();
    changed
        .operations
        .get_mut(&fixture.operation)
        .unwrap()
        .request_digest = "damaged".into();
    let transition = ProjectionTransition::new(&fixture.state, &changed, None).unwrap();
    let admitted = transition
        .previous()
        .object_move(fixture.operation)
        .unwrap();
    assert!(transition.next().object_move(fixture.operation).is_err());
    assert!(Arc::ptr_eq(
        &admitted,
        &transition
            .previous()
            .object_move(fixture.operation)
            .unwrap()
    ));

    let independent = ProjectionTransition::new(&fixture.state, &fixture.state, None).unwrap();
    assert!(!Arc::ptr_eq(
        &admitted,
        &independent.next().object_move(fixture.operation).unwrap()
    ));
    assert!(!Arc::ptr_eq(
        &independent
            .previous()
            .object_move(fixture.operation)
            .unwrap(),
        &independent.next().object_move(fixture.operation).unwrap(),
    ));
}

#[test]
fn file_principal_and_duplicate_members_are_still_rejected() {
    let mut fixture = fixture();
    fixture
        .state
        .operation_principals
        .insert(fixture.operation, "mallory".into());
    let transition = ProjectionTransition::new(&fixture.state, &fixture.state, None).unwrap();
    assert_eq!(
        retained::events(transition.next(), &fixture.source)
            .unwrap_err()
            .code(),
        ErrorCode::PublicationInvalid,
    );
    drop(transition);
    fixture
        .state
        .operation_principals
        .insert(fixture.operation, "alice".into());
    let receipt = fixture
        .state
        .operations
        .get_mut(&fixture.operation)
        .unwrap();
    receipt.result["displaced_deletions"][0]["file_id"] = json!(fixture.source);
    let transition = ProjectionTransition::new(&fixture.state, &fixture.state, None).unwrap();
    assert!(transition.next().object_move(fixture.operation).is_err());
    assert!(retained::events(transition.next(), &fixture.source).is_err());
}

#[test]
fn decoding_is_lazy_and_preserves_strict_payload_and_byte_limit() {
    let mut fixture = fixture();
    fixture
        .state
        .operations
        .get_mut(&fixture.operation)
        .unwrap()
        .result["unexpected"] = json!("not accepted by the strict result reader");
    let transition = ProjectionTransition::new(&fixture.state, &fixture.state, None).unwrap();
    assert!(
        retained::events(transition.next(), &id("file", 99))
            .unwrap()
            .is_empty()
    );
    assert!(transition.next().object_move(fixture.operation).is_err());
    drop(transition);
    let receipt = fixture
        .state
        .operations
        .get_mut(&fixture.operation)
        .unwrap();
    receipt.result.as_object_mut().unwrap().remove("unexpected");
    receipt.result["namespace_basis"]["source_head"] = json!("x".repeat(12 * 1024 * 1024));
    let transition = ProjectionTransition::new(&fixture.state, &fixture.state, None).unwrap();
    let error = transition
        .next()
        .object_move(fixture.operation)
        .err()
        .unwrap();
    assert_eq!(error.code(), ErrorCode::ResourceLimit);
}
