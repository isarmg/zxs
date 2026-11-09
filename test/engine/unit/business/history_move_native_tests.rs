//! The internal publication API remains fenced even for forged template callers.
use super::*;

fn receipt(kind: &str) -> OperationResult {
    OperationResult {
        operation_id: OperationId::generate().unwrap(),
        kind: kind.into(),
        commit_id: None,
        history_tree_id: None,
        branch_id: None,
        commit_state: CommitState::Published,
        protection_at_publish: ProtectionAtPublish::TargetMet,
        current_health: CurrentHealth::Healthy,
        request_digest: "0".repeat(64),
        sequence: 1,
        result: json_codec::json!({}),
    }
}

#[test]
fn no_result_or_state_template_can_enter_the_general_publication_pipeline() {
    let root = tempfile::tempdir().unwrap();
    let config =
        Config::development((0..3).map(|n| root.path().join(format!("disk-{n}")))).unwrap();
    let engine = Engine::initialize(&root.path().join("system"), config).unwrap();
    let before = engine.state().clone();
    let operation = receipt(KIND);
    let mut next = before.clone();
    next.sequence = 1;
    next.operations
        .insert(operation.operation_id, operation.clone());
    let result = engine.prepare_publication(next, Vec::new(), operation, crate::disk::MAX_METADATA);
    assert_eq!(
        result.map(|_| ()).unwrap_err().code(),
        ErrorCode::UnsupportedOperation
    );
    assert_eq!(engine.state(), &before);
}

#[test]
fn operation_result_kind_alone_cannot_bypass_the_gate_by_omitting_its_state_row() {
    let state = State::default();
    assert_eq!(
        reject_unclosed_publication(&state, &state, &receipt(KIND))
            .unwrap_err()
            .code(),
        ErrorCode::UnsupportedOperation
    );
}

#[test]
fn renaming_or_removing_a_native_state_row_does_not_evade_the_gate() {
    let mut previous = State::default();
    let operation = receipt(KIND);
    previous
        .operations
        .insert(operation.operation_id, operation.clone());
    let unrelated = receipt("unrelated.operation");
    let mut changed = previous.clone();
    changed.begin_tracking();
    changed
        .operations
        .get_mut(&operation.operation_id)
        .unwrap()
        .kind = "file.delete".into();
    assert_eq!(
        reject_unclosed_publication(&previous, &changed, &unrelated)
            .unwrap_err()
            .code(),
        ErrorCode::UnsupportedOperation
    );
    let mut removed = previous.clone();
    removed.begin_tracking();
    removed.operations.remove_discard(&operation.operation_id);
    assert_eq!(
        reject_unclosed_publication(&previous, &removed, &unrelated)
            .unwrap_err()
            .code(),
        ErrorCode::UnsupportedOperation
    );
}
