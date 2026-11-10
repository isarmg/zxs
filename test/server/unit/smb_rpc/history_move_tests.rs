//! Adapter/SMB admission regressions. Scripted replies are not publication proofs.
use super::*;
use storage_contract::{CommitState, CurrentHealth, ProtectionAtPublish, Repository};

struct Roots {
    _fixture: Fixture,
    shared: Arc<Shared>,
    backend: Arc<LostCommitReply>,
    source: Repository,
    target: Repository,
}

fn roots() -> Roots {
    roots_with_target(false)
}

fn roots_with_target(occupied: bool) -> Roots {
    let temp = tempfile::tempdir().unwrap();
    let mut config = Config::development([temp.path().join("p"), temp.path().join("b")]).unwrap();
    config.disks[1].level = 1;
    config.disks[1].slot_id = "P02".into();
    config.disks[1].upstream_slot_id = None;
    let mut engine = Engine::initialize(&temp.path().join("state"), config).unwrap();
    let source = engine
        .create_disk_root("P01", "alice", OperationId::generate().unwrap())
        .unwrap();
    let target = engine
        .create_disk_root("P02", "alice", OperationId::generate().unwrap())
        .unwrap();
    let workspace = engine
        .create_workspace(&source.main_branch_id, "alice")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "source.txt", b"original")
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: OperationId::generate().unwrap(),
            principal: "alice".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "source".into(),
            request_digest: None,
        })
        .unwrap();
    if occupied {
        let workspace = engine
            .create_workspace(&target.main_branch_id, "alice")
            .unwrap();
        engine
            .stage_bytes(&workspace.workspace_id, "arrived.txt", b"do not overwrite")
            .unwrap();
        engine
            .commit_workspace(CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: OperationId::generate().unwrap(),
                principal: "alice".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "existing target".into(),
                request_digest: None,
            })
            .unwrap();
    }
    let (_fixture, shared, backend) = fixture_backend(temp, engine);
    Roots {
        _fixture,
        shared,
        backend,
        source,
        target,
    }
}

fn share(roots: &Roots) -> default_share::DefaultShare {
    let generation = roots.shared.account.read().unwrap().selection().generation;
    default_share::DefaultShare::new(
        roots.shared.clone(),
        identity(&roots.shared),
        generation,
        4096,
    )
    .unwrap()
}

fn request(operation: OperationId, destination: &str, replace: bool) -> Request {
    Request::Namespace {
        kind: smb_protocol::protocol::NamespaceOperation::Rename,
        path: "p/source.txt".into(),
        destination: Some(destination.into()),
        replace,
        operation_id: operation.to_string(),
    }
}

fn scripted_result(
    operation: OperationId,
    state: CommitState,
) -> storage_contract::OperationResult {
    storage_contract::OperationResult {
        operation_id: operation,
        kind: "file.history.move".into(),
        commit_id: None,
        history_tree_id: None,
        branch_id: None,
        commit_state: state,
        protection_at_publish: ProtectionAtPublish::TargetMet,
        current_health: CurrentHealth::Healthy,
        request_digest: "scripted-reply-only".into(),
        sequence: 12,
        result: json!({}),
    }
}

fn original_entry(roots: &Roots) -> Entry {
    roots
        .shared
        .backend
        .lookup_current(
            &identity(&roots.shared),
            &roots.source.main_branch_id,
            &name_policy::LookupPath::parse("source.txt").unwrap(),
        )
        .unwrap()
        .unwrap()
}

fn command(roots: &Roots, operation: OperationId) -> crate::MoveCurrentFile {
    let entry = original_entry(roots);
    let binding = |repository: &Repository| crate::auth::SmbBinding {
        repository_id: repository.repository_id.clone(),
        branch_id: repository.main_branch_id.clone(),
        generation: repository.smb_binding_generation,
    };
    crate::MoveCurrentFile {
        source_binding: binding(&roots.source),
        target_binding: binding(&roots.target),
        source_path: name_policy::LookupPath::parse("source.txt").unwrap(),
        target_name: crate::MoveTargetName::parse("arrived.txt").unwrap(),
        expected_source_entry_id: entry.entry_id,
        expected_source_file_id: storage_contract::history_window::HistoryFileId::parse(
            &entry.file_id,
        )
        .unwrap(),
        operation_id: operation,
    }
}

#[test]
fn default_move_passes_both_pinned_bindings_and_cleans_only_confirmed_intent() {
    let roots = roots();
    let entry = original_entry(&roots);
    let operation = OperationId::generate().unwrap();
    *roots.backend.move_result.lock().unwrap() =
        Some(Ok(scripted_result(operation, CommitState::Published)));
    let (reply, descriptor) = share(&roots)
        .dispatch(request(operation, "b/arrived.txt", false))
        .unwrap();
    assert!(descriptor.is_none());
    let reply = json_reply(reply);
    assert_eq!(reply["operation_kind"], "file.history.move");
    let calls = roots.backend.move_requests.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["source_repository_id"], roots.source.repository_id);
    assert_eq!(calls[0]["target_repository_id"], roots.target.repository_id);
    assert_eq!(calls[0]["source_branch_id"], roots.source.main_branch_id);
    assert_eq!(calls[0]["target_branch_id"], roots.target.main_branch_id);
    assert_eq!(
        calls[0]["source_generation"],
        roots.source.smb_binding_generation.to_string()
    );
    assert_eq!(
        calls[0]["target_generation"],
        roots.target.smb_binding_generation.to_string()
    );
    assert_eq!(calls[0]["source_entry_id"], entry.entry_id);
    assert_eq!(calls[0]["source_file_id"], entry.file_id);
    assert_eq!(calls[0]["operation_id"], operation.to_string());
    assert!(
        !roots
            .shared
            .spool
            .join(format!("{operation}.namespace.json"))
            .exists()
    );
    assert!(roots.shared.file_writers.lock().unwrap().is_empty());
}

#[test]
fn nonpublished_move_retains_original_intent_and_blocks_namespace_replay() {
    let roots = roots();
    let entry = original_entry(&roots);
    let operation = OperationId::generate().unwrap();
    *roots.backend.move_result.lock().unwrap() =
        Some(Ok(scripted_result(operation, CommitState::Prepared)));
    let mut share = share(&roots);
    let failure = share
        .dispatch(request(operation, "b/arrived.txt", false))
        .unwrap_err();
    assert_eq!(
        failure.error.code,
        storage_contract::ErrorCode::RecoveryRequired
    );
    assert_eq!(failure.operation_id, Some(operation.to_string()));
    assert!(!failure.error.retryable);
    let record = roots
        .shared
        .spool
        .join(format!("{operation}.namespace.json"));
    let bytes = fs::read(&record).unwrap();
    let intent: Value = json_codec::from_slice(&bytes).unwrap();
    assert_eq!(intent["format"], "smb-file-history-move");
    assert_eq!(intent["operation_id"], operation.to_string());
    assert_eq!(
        intent["move_request"]["source_repository_id"],
        roots.source.repository_id
    );
    assert_eq!(intent["move_request"]["source_path"], "source.txt");
    assert_eq!(
        intent["move_request"]["target_repository_id"],
        roots.target.repository_id
    );
    assert_eq!(intent["move_request"]["target_name"], "arrived.txt");
    assert_eq!(intent["source_entry_id"], entry.entry_id);
    assert_eq!(intent["source_file_id"], entry.file_id);
    let replay = share
        .dispatch(request(operation, "b/different.txt", false))
        .unwrap_err();
    assert_eq!(
        replay.error.code,
        storage_contract::ErrorCode::RecoveryRequired
    );
    assert_eq!(roots.backend.move_requests.lock().unwrap().len(), 1);
    assert_eq!(fs::read(record).unwrap(), bytes);
    assert_eq!(original_entry(&roots), entry);
    assert!(roots.shared.file_writers.lock().unwrap().is_empty());
}

#[test]
fn failed_move_keeps_original_operation_and_does_not_fall_back_to_workspace_copy_delete() {
    let roots = roots();
    let entry = original_entry(&roots);
    let operation = OperationId::generate().unwrap();
    *roots.backend.move_result.lock().unwrap() =
        Some(Err(ApiError::io(io::Error::other("lost native result"))));
    let failure = share(&roots)
        .dispatch(request(operation, "b/arrived.txt", false))
        .unwrap_err();
    assert_eq!(failure.operation_id, Some(operation.to_string()));
    assert!(!failure.error.retryable);
    assert_eq!(roots.backend.move_requests.lock().unwrap().len(), 1);
    assert!(
        roots
            .shared
            .spool
            .join(format!("{operation}.namespace.json"))
            .exists()
    );
    assert_eq!(original_entry(&roots), entry);
    assert!(
        roots
            .shared
            .backend
            .lookup_current(
                &identity(&roots.shared),
                &roots.target.main_branch_id,
                &name_policy::LookupPath::parse("arrived.txt").unwrap(),
            )
            .unwrap()
            .is_none()
    );
    assert!(roots.shared.file_writers.lock().unwrap().is_empty());
}

#[test]
fn unsynced_move_intent_prevents_core_submission_and_keeps_available_evidence() {
    for directory in [false, true] {
        let roots = roots();
        let operation = OperationId::generate().unwrap();
        namespace::fail_next_record_sync(directory);
        let failure = share(&roots)
            .dispatch(request(operation, "b/arrived.txt", false))
            .unwrap_err();
        assert_eq!(failure.operation_id, Some(operation.to_string()));
        assert!(!failure.error.retryable);
        assert!(roots.backend.move_requests.lock().unwrap().is_empty());
        assert!(
            roots
                .shared
                .spool
                .join(format!("{operation}.namespace.json"))
                .exists()
        );
        assert!(roots.shared.file_writers.lock().unwrap().is_empty());
    }
}

#[test]
fn cross_disk_replace_or_directory_destination_never_submits_native_move() {
    let roots = roots();
    let mut share = share(&roots);
    for (target, replace) in [("b/arrived.txt", true), ("b/folder/arrived.txt", false)] {
        let operation = OperationId::generate().unwrap();
        let failure = share
            .dispatch(request(operation, target, replace))
            .unwrap_err();
        assert_eq!(
            failure.error.code,
            storage_contract::ErrorCode::UnsupportedOperation
        );
        assert!(roots.backend.move_requests.lock().unwrap().is_empty());
        assert!(
            !roots
                .shared
                .spool
                .join(format!("{operation}.namespace.json"))
                .exists()
        );
    }
}

#[test]
fn move_reserves_both_paths_atomically_and_respects_case_and_ancestor_writers() {
    let roots = roots();
    for (branch, path) in [
        (roots.source.main_branch_id.clone(), "SOURCE.TXT"),
        (roots.target.main_branch_id.clone(), "arrived.txt/child"),
    ] {
        let key = (branch, name_policy::path_key(path));
        roots
            .shared
            .file_writers
            .lock()
            .unwrap()
            .insert(key.clone());
        let operation = OperationId::generate().unwrap();
        let failure = share(&roots)
            .dispatch(request(operation, "b/arrived.txt", false))
            .unwrap_err();
        assert_eq!(
            failure.error.code,
            storage_contract::ErrorCode::FileWriteBusy
        );
        assert_eq!(
            *roots.shared.file_writers.lock().unwrap(),
            BTreeSet::from([key.clone()])
        );
        assert!(roots.backend.move_requests.lock().unwrap().is_empty());
        roots.shared.file_writers.lock().unwrap().remove(&key);
    }
    let held = path_reservation::MovePaths::acquire(
        &roots.shared,
        [
            (&roots.source.main_branch_id, "source.txt"),
            (&roots.target.main_branch_id, "arrived.txt"),
        ],
    )
    .unwrap();
    assert_eq!(roots.shared.file_writers.lock().unwrap().len(), 2);
    for (branch, path) in [
        (&roots.source.main_branch_id, "SOURCE.TXT"),
        (&roots.target.main_branch_id, "ARRIVED.TXT"),
    ] {
        let mut other = session(roots.shared.clone(), branch);
        assert_eq!(
            other.reserve_paths(&[path]).unwrap_err().code,
            storage_contract::ErrorCode::FileWriteBusy
        );
    }
    drop(held);
    assert!(roots.shared.file_writers.lock().unwrap().is_empty());
}

#[test]
fn changed_account_generation_refuses_move_before_record_or_core_admission() {
    let roots = roots();
    let mut share = share(&roots);
    let generation = roots.shared.account.read().unwrap().selection().generation;
    roots
        .shared
        .account
        .select(generation, "bob", || Ok(()))
        .unwrap();
    let operation = OperationId::generate().unwrap();
    let failure = share
        .dispatch(request(operation, "b/arrived.txt", false))
        .unwrap_err();
    assert_eq!(
        failure.error.code,
        storage_contract::ErrorCode::SmbRebindRequired
    );
    assert!(!failure.error.retryable);
    assert!(roots.backend.move_requests.lock().unwrap().is_empty());
    assert!(
        !roots
            .shared
            .spool
            .join(format!("{operation}.namespace.json"))
            .exists()
    );
}

#[test]
fn engine_move_rechecks_either_binding_under_owner_lock() {
    for target in [false, true] {
        let roots = roots();
        let command = command(&roots, OperationId::generate().unwrap());
        let repository = if target { &roots.target } else { &roots.source };
        let binding = roots
            .backend
            .inner
            .smb_binding_status(&identity(&roots.shared), &repository.repository_id)
            .unwrap();
        roots
            .backend
            .inner
            .update_smb_binding(
                &identity(&roots.shared),
                crate::UpdateSmbBinding {
                    repository: &repository.repository_id,
                    branch: &repository.main_branch_id,
                    enabled: false,
                    expected_history_generation: binding.history_generation,
                    operation: OperationId::generate().unwrap(),
                },
            )
            .unwrap();
        let error = roots
            .backend
            .inner
            .move_current_file(&identity(&roots.shared), command)
            .unwrap_err();
        assert_eq!(error.code, storage_contract::ErrorCode::SmbRebindRequired);
    }
}

#[test]
fn engine_move_refuses_changed_captured_source_identity_before_native_capture() {
    let roots = roots();
    let mut command = command(&roots, OperationId::generate().unwrap());
    command.expected_source_entry_id = "not-the-captured-entry".into();
    let error = roots
        .backend
        .inner
        .move_current_file(&identity(&roots.shared), command)
        .unwrap_err();
    assert_eq!(error.code, storage_contract::ErrorCode::WorkspaceChanged);
}

#[test]
fn occupied_move_target_is_not_overwritten_or_submitted_to_core() {
    let roots = roots_with_target(true);
    let before = roots
        .shared
        .backend
        .lookup_current(
            &identity(&roots.shared),
            &roots.target.main_branch_id,
            &name_policy::LookupPath::parse("arrived.txt").unwrap(),
        )
        .unwrap()
        .unwrap();
    let operation = OperationId::generate().unwrap();
    let failure = share(&roots)
        .dispatch(request(operation, "b/ARRIVED.TXT", false))
        .unwrap_err();
    assert_eq!(
        failure.error.code,
        storage_contract::ErrorCode::NameCollision
    );
    assert!(roots.backend.move_requests.lock().unwrap().is_empty());
    assert!(
        !roots
            .shared
            .spool
            .join(format!("{operation}.namespace.json"))
            .exists()
    );
    let after = roots
        .shared
        .backend
        .lookup_current(
            &identity(&roots.shared),
            &roots.target.main_branch_id,
            &name_policy::LookupPath::parse("arrived.txt").unwrap(),
        )
        .unwrap()
        .unwrap();
    assert_eq!(after, before);
}

#[test]
fn default_connection_cannot_adopt_reenabled_target_binding_generation() {
    let roots = roots();
    let mut share = share(&roots);
    assert!(
        share
            .dispatch(Request::Stat {
                path: "b/arrived.txt".into()
            })
            .is_err()
    );
    for enabled in [false, true] {
        let binding = roots
            .backend
            .inner
            .smb_binding_status(&identity(&roots.shared), &roots.target.repository_id)
            .unwrap();
        roots
            .backend
            .inner
            .update_smb_binding(
                &identity(&roots.shared),
                crate::UpdateSmbBinding {
                    repository: &roots.target.repository_id,
                    branch: &roots.target.main_branch_id,
                    enabled,
                    expected_history_generation: binding.history_generation,
                    operation: OperationId::generate().unwrap(),
                },
            )
            .unwrap();
    }
    let operation = OperationId::generate().unwrap();
    let failure = share
        .dispatch(request(operation, "b/arrived.txt", false))
        .unwrap_err();
    assert_eq!(
        failure.error.code,
        storage_contract::ErrorCode::SmbRebindRequired
    );
    assert!(roots.backend.move_requests.lock().unwrap().is_empty());
    assert!(
        !roots
            .shared
            .spool
            .join(format!("{operation}.namespace.json"))
            .exists()
    );
}

#[path = "history_move_identity_tests.rs"]
mod history_move_identity_tests;

