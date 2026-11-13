use super::*;
use storage_contract::Generation;

fn target() -> Target {
    Target {
        epoch: 2, navigation: 3, entry: "entry_00000000000000000000000000000001".into(),
        request: ReadRequest {
            file_id: storage_contract::FileId::from_random([1; 16]),
            context: Context {
                repository_id: storage_contract::RepositoryId::from_random([2; 16]),
                path: name_policy::PortablePath::parse("file.txt").unwrap(),
            },
        },
    }
}

#[test]
fn policy_reads_reject_previous_session_navigation_occurrence_and_request() {
    let target = target();
    let panel = Panel { target: Some(target.clone()), generation: 9, ..Panel::default() };
    assert!(panel.accepts(&target, 9));
    assert!(!panel.accepts(&target, 8));
    for change in 0..4 {
        let mut stale = target.clone();
        match change {
            0 => stale.epoch += 1,
            1 => stale.navigation += 1,
            2 => stale.entry = "other".into(),
            _ => stale.request.context.path = name_policy::PortablePath::parse("other.txt").unwrap(),
        }
        assert!(!panel.accepts(&stale, 9));
    }
}

#[test]
fn unknown_state_never_enables_automatic_and_dirty_cannot_switch() {
    assert!(decode(json_codec::json!({})).is_err());
    assert!(decode(json_codec::json!({"state":"manual_dirty","generation":"2"})).is_err());
    assert!(can_switch(&ContainerEdit::initial()));
    assert!(can_switch(&ContainerEdit::ManualClean { generation: Generation::new(1) }));
    let dirty = decode(json_codec::json!({
        "state":"manual_dirty","generation":"2","draft":{
            "operation":"op_00000000000000000000000000000001",
            "expected_generation":"1","principal":"alice","size":"5",
            "content_hash":"0000000000000000000000000000000000000000000000000000000000000000",
            "base":{
                "entry":"entry_00000000000000000000000000000001",
                "repository":"repo_00000000000000000000000000000001",
                "branch":"br_00000000000000000000000000000001",
                "node":"ver_00000000000000000000000000000001",
                "version":"ver_00000000000000000000000000000001",
                "selection_generation":"0"
            }
        }
    })).unwrap();
    assert!(!can_switch(&dirty));
    assert!(dirty.set_mode(Generation::new(2), CommitMode::Automatic).is_err());
}

pub(super) fn preparation() -> CommitPreparation {
    CommitPreparation {
        principal: storage_contract::Principal::new("alice".into()).unwrap(),
        target: target(), generation: Generation::new(2),
        draft: storage_contract::OperationId::from_random([3; 16]),
        operation: storage_contract::OperationId::from_random([4; 16]),
        envelope: None, in_flight: false, commit_started: false, settled: None, error: String::new(),
    }
}

pub(super) fn prepared() -> CommitPreparation {
    let intent = preparation();
    let binding = json_codec::json!({
        "file":intent.target.request.file_id,"generation":"2","operation":intent.operation,
        "revision":{
            "operation":intent.draft,"expected_generation":"1","principal":"alice","size":"5",
            "content_hash":"0000000000000000000000000000000000000000000000000000000000000000",
            "base":{
                "entry":"entry_00000000000000000000000000000001",
                "repository":intent.target.request.context.repository_id,
                "branch":"br_00000000000000000000000000000001",
                "node":"ver_00000000000000000000000000000001",
                "version":"ver_00000000000000000000000000000001","selection_generation":"0"
            }
        }
    });
    let seal = json_codec::json!({
        "workspace_id":"workspace","operation_id":intent.operation,"repository_id":"repo",
        "branch_id":"branch","expected_head":"head","fencing_token":"7","message":"",
        "requested_protection":"strict","file_placement":{},"expected_topology_generation":"1",
        "sealed_manifest_hash":"manifest","container_draft_commit":binding,
    });
    let mut intent = intent;
    intent.envelope = Some(frozen_envelope(seal, &intent).unwrap());
    intent
}

#[test]
fn frozen_preparation_validates_binding_and_preserves_original_commit_envelope() {
    let intent = preparation();
    let binding = json_codec::json!({
        "file":intent.target.request.file_id,"generation":"2","operation":intent.operation,
        "revision":{
            "operation":intent.draft,"expected_generation":"1","principal":"alice","size":"5",
            "content_hash":"0000000000000000000000000000000000000000000000000000000000000000",
            "base":{
                "entry":"entry_00000000000000000000000000000001",
                "repository":intent.target.request.context.repository_id,
                "branch":"br_00000000000000000000000000000001",
                "node":"ver_00000000000000000000000000000001",
                "version":"ver_00000000000000000000000000000001","selection_generation":"0"
            }
        }
    });
    let seal = json_codec::json!({
        "workspace_id":"workspace","operation_id":intent.operation,"repository_id":"repo",
        "branch_id":"branch","expected_head":"head","fencing_token":"7","message":"",
        "requested_protection":"strict","file_placement":{},"expected_topology_generation":"1",
        "sealed_manifest_hash":"manifest","container_draft_commit":binding,
    });
    let frozen = frozen_envelope(seal.clone(), &intent).unwrap();
    assert_eq!(frozen["container_draft_commit"], binding);
    let envelope = storage_contract::PublicCommitEnvelope::parse("workspace", frozen).unwrap();
    assert_eq!(envelope.claimed_digest(), envelope.digest());
    let mut changed = seal;
    changed["container_draft_commit"]["generation"] = json_codec::json!("3");
    assert!(frozen_envelope(changed, &intent).is_err());
}

#[test]
fn navigation_keeps_original_intent_unknown_receipts_do_not_clear_it_and_new_session_cannot_retry() {
    let mut intent = preparation();
    intent.envelope = Some(json_codec::json!({"request_digest":"original-frozen-digest"}));
    intent.commit_started = true;
    let mut state = State { authenticated: true, session_epoch: intent.target.epoch,
        ..Default::default() };
    state.principal = Some(intent.principal.clone());
    state.management.storage = crate::management::Storage::Ready;
    state.files.container_draft_commit = Some(intent.clone());
    state.files.clear_selection();
    assert!(owns_intent(&state, &intent));
    assert!(can_retry_commit(&state));
    let receipt = json_codec::json!({"operation_kind":"content_save",
        "request_digest":"original-frozen-digest"});
    assert!(receipt_matches(&state, &intent.operation.to_string(), &receipt));
    let changed = json_codec::json!({"operation_kind":"content_save","request_digest":"other"});
    assert!(!receipt_matches(&state, &intent.operation.to_string(), &changed));
    settle(&mut state.files, &intent.operation.to_string(), model::SaveStatus::Unknown);
    assert_eq!(state.files.container_draft_commit.as_ref().unwrap().envelope, intent.envelope);
    settle(&mut state.files, "unrelated", model::SaveStatus::Published);
    assert!(state.files.container_draft_commit.is_some());
    state.session_epoch += 1;
    assert!(!owns_intent(&state, &intent));
    assert!(!can_retry_commit(&state));
    state.session_epoch = intent.target.epoch;
    state.files.mutation_pending = true;
    settle(&mut state.files, &intent.operation.to_string(), model::SaveStatus::Published);
    assert_eq!(state.files.container_draft_commit.as_ref().unwrap().settled,
        Some(model::SaveStatus::Published));
    assert!(!state.files.mutation_pending);
    assert!(!owns_active_intent(&state, &intent));
}

#[test]
fn malformed_recovery_blocks_writes_without_erasing_the_original_intent() {
    let intent = preparation();
    let mut state = State {
        authenticated: true,
        session_epoch: intent.target.epoch,
        ..Default::default()
    };
    state.principal = Some(intent.principal.clone());
    state.management.storage = crate::management::Storage::Ready;
    state.files.container_draft_commit = Some(intent.clone());
    state.files.container_draft_recovery_error = Some(CommitPreparationError::RecoveryBinding.into());
    assert!(actions::pending(&state));
    assert!(!can_retry_commit(&state));
    assert_eq!(state.files.container_draft_commit.as_ref().unwrap().operation, intent.operation);
}
