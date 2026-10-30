use super::*;
use std::sync::Arc;

fn engine(root: &std::path::Path) -> Engine {
    let mut engine = Engine::initialize(
        &root.join("state"),
        Config::development([root.join("a"), root.join("b"), root.join("c")]),
    )
    .unwrap();
    engine
        .create_repository(
            &storage_contract::RepositoryName::parse("first").unwrap(),
            "P01",
            1,
            "alice",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    engine
}
fn config_hash(engine: &Engine) -> String {
    object_hash("publication-config", &engine.config).unwrap()
}

fn save_capture_file(engine: &mut Engine) -> String {
    let branch = engine
        .state
        .repositories
        .values()
        .next()
        .unwrap()
        .main_branch_id
        .clone();
    let workspace = engine.create_workspace(&branch, "alice").unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "folder/original.txt", b"body")
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: OperationId::generate(),
            principal: "alice".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "capture fixture".into(),
            request_digest: None,
        })
        .unwrap();
    engine.state.file_names.keys().next().unwrap().clone()
}

#[test]
fn name_capture_receipt_is_bound_only_to_its_immutable_state() {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = engine(temp.path());
    let file = save_capture_file(&mut engine);
    let base = engine.published_state_base.clone().unwrap();
    assert_eq!(engine.state.file_locations[&file], "folder/original.txt");
    assert!(base.names_captured_for(&engine.state));

    // Disk metadata authority can be revoked without changing logical coverage.
    engine.metadata_integrity.invalidate();
    assert!(!base.matches(&engine.planning_context(), &config_hash(&engine)));
    assert!(base.names_captured_for(&engine.state));
    engine.config = ValidatedConfig::try_from(engine.config().clone()).unwrap();
    engine.snapshot = None;
    assert!(base.names_captured_for(&engine.state));
    assert_eq!(
        base.history_owners,
        crate::state_projection::DerivedOwners::from_state(&engine.state)
    );
    let mut next = engine.state().clone();
    next.file_notes.insert(file.clone(), "changed note".into());
    let mut projection = crate::state_projection::ProjectionTransition::new(
        &engine.state,
        &next,
        Some(&base.history_owners),
    )
    .unwrap();
    assert!(!projection.take_history_edits().unwrap().is_empty());
    assert_eq!(
        projection.into_owners(),
        crate::state_projection::DerivedOwners::from_state(&next)
    );

    let equal_state = Arc::new(engine.state().clone());
    assert!(!base.names_captured_for(&equal_state));
    // Even a harmless private plan-map edit detaches the allocation from Weak.
    Arc::make_mut(&mut engine.state).plans.clear();
    assert!(!base.names_captured_for(&engine.state));
}

#[test]
fn cold_planning_captures_admitted_missing_fields_before_the_business_closure() {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = engine(temp.path());
    let file = save_capture_file(&mut engine);
    let published = canonical_json(engine.state()).unwrap();
    for retain_name in [false, true] {
        let mut raw = engine.state().clone();
        raw.file_locations.remove(&file);
        if !retain_name {
            raw.file_names.remove(&file);
        }
        let decoded: crate::history_package::RawState =
            json_codec::from_slice(&canonical_json(&raw).unwrap()).unwrap();
        let admitted = Arc::new(State::try_from(decoded).unwrap());
        let old_receipt = engine.published_state_base.as_ref().unwrap();
        assert!(!old_receipt.names_captured_for(&admitted));
        let mut context = engine.planning_context();
        context.state = &admitted;
        // Deliberately retain the old receipt: allocation identity, not a caller
        // clearing the Option, must select the cold pre-capture boundary.
        let plan = context
            .plan_new_mutation(
                "auth.capture-test",
                "alice",
                OperationId::generate(),
                "capture-test".into(),
                |_, next, _| {
                    assert_eq!(next.file_names[&file], "original.txt");
                    assert_eq!(next.file_locations[&file], "folder/original.txt");
                    Ok(json!({}))
                },
            )
            .unwrap();
        let crate::engine::MutationPlan::Prepared { next, .. } = plan else {
            panic!("new operation must produce a candidate");
        };
        assert_eq!(next.file_locations[&file], "folder/original.txt");
        assert!(!admitted.file_locations.contains_key(&file));
        assert_eq!(canonical_json(engine.state()).unwrap(), published);
        let storage_contract::MapChanges::Keys(locations) =
            next.file_locations.changes_since(&admitted.file_locations)
        else {
            panic!("cold capture must stay in the candidate write set");
        };
        assert_eq!(
            locations.as_ref(),
            &std::collections::BTreeSet::from([file.clone()])
        );
    }
}

#[test]
fn cold_owner_update_keeps_full_history_rejection_before_io_and_allows_retry() {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = engine(temp.path());
    save_capture_file(&mut engine);
    let root = engine.root.clone();
    drop(engine);
    let mut engine = Engine::open(&root).unwrap();
    assert!(engine.published_state_base.is_none());
    let operation = OperationId::generate();
    let plan = engine
        .planning_context()
        .plan_new_mutation(
            "auth.cold-test",
            "alice",
            operation,
            "cold-test".into(),
            |_, next, _| {
                next.control_auth = Some(json!({"cold":"valid"}));
                Ok(json!({}))
            },
        )
        .unwrap();
    let crate::engine::MutationPlan::Prepared {
        next,
        prepared,
        result,
    } = plan
    else {
        panic!("new operation must produce a candidate");
    };
    let mut damaged = next.as_ref().clone();
    // Two defects exercise the existing full-admission first-error order.
    let repository = damaged.repositories.values().next().unwrap();
    let head = damaged.branches[&repository.main_branch_id]
        .head_commit_id
        .clone();
    let entries = &mut damaged.commits.get_mut(&head).unwrap().entries;
    entries[0].entry_id = "invalid-entry".into();
    entries[1].file_id = "invalid-file".into();
    let mut projection =
        crate::state_projection::ProjectionTransition::new(&engine.state, &damaged, None).unwrap();
    assert!(projection.take_history_edits().is_none());
    let expected = match crate::history_package::prepare_history_manifests(&damaged, MAX_METADATA) {
        Ok(_) => panic!("full history admission must reject the damaged entries"),
        Err(error) => error,
    };
    let members = engine.members();
    let logs = members
        .iter()
        .map(|disk| std::fs::read(log_path(disk)).unwrap())
        .collect::<Vec<_>>();
    let error = match engine.prepare_publication(damaged, Vec::new(), result.clone(), MAX_METADATA)
    {
        Ok(_) => panic!("cold owner reuse must not authorize damaged history"),
        Err(error) => error,
    };
    assert_eq!(error.code(), expected.code());
    assert_eq!(error.message(), expected.message());
    assert_eq!(format!("{error:?}"), format!("{expected:?}"));
    for (disk, bytes) in members.iter().zip(logs) {
        assert_eq!(std::fs::read(log_path(disk)).unwrap(), bytes);
    }
    assert!(engine.published_state_base.is_none());
    let publication = engine
        .prepare_publication(*next, prepared, result, MAX_METADATA)
        .unwrap();
    engine.decide_publication(publication).unwrap();
    assert_eq!(engine.state.control_auth, Some(json!({"cold":"valid"})));
    assert!(engine.state.operations.contains_key(&operation));
}

#[test]
fn cold_owner_update_preserves_corrupt_page_failure_and_exact_retry() {
    let temp = tempfile::tempdir().unwrap();
    let engine = engine(temp.path());
    let root = engine.root.clone();
    drop(engine);
    let mut engine = Engine::open(&root).unwrap();
    assert!(engine.published_state_base.is_none());
    let member = engine.members()[0].clone();
    let snapshot: Snapshot =
        load_object(&member, "snapshot", engine.snapshot.as_deref().unwrap()).unwrap();
    let (kind, hash) = map_addresses(&member, &snapshot.maps["repositories"])[0].clone();
    let path = object_path(&member, kind, &hash);
    let original = std::fs::read(&path).unwrap();
    std::fs::write(&path, b"{").unwrap();
    let operation = OperationId::generate();
    let body = json!({"auth":"cold page retry"});
    let sequence = engine.state.sequence;
    let error = engine
        .persist_auth_state(body.clone(), "alice", operation)
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::ObjectCollision);
    assert_eq!(engine.state.sequence, sequence);
    assert!(engine.published_state_base.is_none());
    assert_eq!(std::fs::read(&path).unwrap(), b"{");
    std::fs::write(&path, original).unwrap();
    engine
        .persist_auth_state(body.clone(), "alice", operation)
        .unwrap();
    assert_eq!(engine.state.control_auth, Some(body));
    assert!(engine.state.operations.contains_key(&operation));
}

#[test]
fn cold_owner_update_retains_first_prepare_io_cause_and_recovers_for_retry() {
    let temp = tempfile::tempdir().unwrap();
    let engine = engine(temp.path());
    let root = engine.root.clone();
    drop(engine);
    let mut engine = Engine::open(&root).unwrap();
    assert!(engine.published_state_base.is_none());
    let first = engine.members()[0].disk_id.clone();
    engine.journals.get_mut(&first).unwrap().fault = Some(crate::journal::JournalFault::Sync);
    let operation = OperationId::generate();
    let body = json!({"auth":"cold io retry"});
    let sequence = engine.state.sequence;
    let error = engine
        .persist_auth_state(body.clone(), "alice", operation)
        .unwrap_err();
    let mut cause: &dyn std::error::Error = &error;
    loop {
        if let Some(io) = cause.downcast_ref::<std::io::Error>() {
            assert_eq!(io.raw_os_error(), Some(5));
            break;
        }
        cause = cause
            .source()
            .expect("the first PREPARE EIO must retain its cause");
    }
    assert_eq!(engine.state.sequence, sequence);
    assert!(!engine.state.operations.contains_key(&operation));
    assert!(engine.published_state_base.is_none());
    for member in engine.members() {
        assert!(!scan(&member).unwrap().records.iter().any(|(record, _)| {
            record.sequence > sequence && record.record_type == RecordType::Accept
        }));
    }
    drop(engine);
    let mut engine = Engine::open(&root).unwrap();
    engine
        .persist_auth_state(body.clone(), "alice", operation)
        .unwrap();
    assert_eq!(engine.state.control_auth, Some(body));
    assert!(engine.state.operations.contains_key(&operation));
}

#[test]
fn history_baseline_requires_exact_state_allocation_snapshot_sequence_and_configuration() {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = engine(temp.path());
    let base = engine.published_state_base.clone().unwrap();
    assert!(base.snapshot_basis.matches_snapshot(&base.snapshot));
    assert!(base.matches(&engine.planning_context(), &config_hash(&engine)));
    assert!(!base.matches(&engine.planning_context(), "wrong config hash"));
    let saved = engine.snapshot.take();
    assert!(!base.matches(&engine.planning_context(), &config_hash(&engine)));
    engine.snapshot = saved;
    // Same values and sequence do not authorize another allocation. A Weak
    // baseline also forces Arc::make_mut to detach even with one strong owner.
    let bytes = canonical_json(engine.state()).unwrap();
    Arc::make_mut(&mut engine.state).plans.clear();
    assert_eq!(canonical_json(engine.state()).unwrap(), bytes);
    assert!(!base.matches(&engine.planning_context(), &config_hash(&engine)));
    engine
        .persist_auth_state(
            json!({"auth":"next"}),
            "alice",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    assert!(
        engine
            .published_state_base
            .as_ref()
            .unwrap()
            .matches(&engine.planning_context(), &config_hash(&engine))
    );
    let published = engine.published_state_base.as_ref().unwrap();
    assert!(
        published
            .snapshot_basis
            .matches_snapshot(engine.snapshot.as_deref().unwrap())
    );
    let root = engine.root.clone();
    drop(engine);
    let engine = Engine::open(&root).unwrap();
    assert!(
        engine.published_state_base.is_none(),
        "restart never trusts an in-memory publication receipt"
    );
}

#[test]
fn preparation_and_indeterminate_failure_never_advance_the_baseline() {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = engine(temp.path());
    let base = engine.published_state_base.clone().unwrap();
    engine.inject_fault_once("after_payloads");
    assert!(
        engine
            .persist_auth_state(
                json!({"auth":"prepared"}),
                "alice",
                storage_contract::OperationId::generate()
            )
            .is_err()
    );
    assert!(Arc::ptr_eq(
        &base,
        engine.published_state_base.as_ref().unwrap()
    ));
    assert!(base.matches(&engine.planning_context(), &config_hash(&engine)));
    engine.inject_fault_once("after_accept_1");
    let error = engine
        .persist_auth_state(
            json!({"auth":"accepted"}),
            "alice",
            storage_contract::OperationId::generate(),
        )
        .unwrap_err();
    assert_eq!(error.code(), storage_contract::ErrorCode::InDoubt);
    assert!(error.is_retryable());
    let source = std::error::Error::source(&error).expect("publication retains its cause");
    assert!(source.to_string().contains("after_accept_1"));
    assert!(engine.blocked);
    assert!(Arc::ptr_eq(
        &base,
        engine.published_state_base.as_ref().unwrap()
    ));
}

#[test]
fn derived_index_failure_preserves_completed_member_authority_not_future_failures() {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = engine(temp.path());
    let base = engine.published_state_base.clone().unwrap();
    let epoch = engine.metadata_integrity.capture();
    let index = engine.root.join("index.sqlite");
    std::fs::rename(&index, engine.root.join("saved-index.sqlite")).unwrap();
    std::fs::create_dir(&index).unwrap();
    let operation = storage_contract::OperationId::generate();
    engine
        .persist_auth_state(json!({"auth":"published"}), "alice", operation)
        .unwrap();
    assert!(!engine.blocked);
    assert!(engine.state.operations.contains_key(&operation));
    assert!(epoch.matches(&engine.metadata_integrity));
    let current = engine.published_state_base.clone().unwrap();
    assert!(!Arc::ptr_eq(&base, &current));
    assert!(current.matches(&engine.planning_context(), &config_hash(&engine)));
    let crate::IndexHealth::Unavailable(first) = engine.index_health() else {
        panic!("expected retained index failure");
    };
    engine
        .persist_auth_state(json!({"auth":"later"}), "alice", OperationId::generate())
        .unwrap();
    let crate::IndexHealth::Unavailable(later) = engine.index_health() else {
        panic!("expected same unavailable index");
    };
    assert!(Arc::ptr_eq(&first, &later));
    assert!(epoch.matches(&engine.metadata_integrity));
    let current = engine.published_state_base.clone().unwrap();
    assert!(current.matches(&engine.planning_context(), &config_hash(&engine)));

    // Disabling the disposable accelerator does not weaken the actual decision barrier.
    engine.inject_fault_once("after_accept_1");
    let error = engine
        .persist_auth_state(
            json!({"auth":"uncertain"}),
            "alice",
            OperationId::generate(),
        )
        .unwrap_err();
    assert_eq!(error.code(), storage_contract::ErrorCode::InDoubt);
    assert!(engine.blocked);
    assert!(!epoch.matches(&engine.metadata_integrity));
    assert!(Arc::ptr_eq(
        &current,
        engine.published_state_base.as_ref().unwrap()
    ));
    assert!(!current.matches(&engine.planning_context(), &config_hash(&engine)));
}

#[test]
fn old_writer_observer_revokes_every_later_receipt_without_reviving_an_old_epoch() {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = engine(temp.path());
    let old_observer = Arc::clone(&engine.metadata_integrity);
    engine
        .persist_auth_state(json!({"auth":"second"}), "alice", OperationId::generate())
        .unwrap();
    let second = engine.published_state_base.clone().unwrap();
    assert!(second.matches(&engine.planning_context(), &config_hash(&engine)));
    old_observer.invalidate();
    assert!(!second.matches(&engine.planning_context(), &config_hash(&engine)));
    // A new complete publication can establish its current dependencies again,
    // but does not detach the shared observer held by old read plans.
    engine
        .persist_auth_state(json!({"auth":"third"}), "alice", OperationId::generate())
        .unwrap();
    let third = engine.published_state_base.clone().unwrap();
    assert!(third.matches(&engine.planning_context(), &config_hash(&engine)));
    old_observer.invalidate();
    assert!(!third.matches(&engine.planning_context(), &config_hash(&engine)));
    assert!(!second.matches(&engine.planning_context(), &config_hash(&engine)));
}

#[test]
fn healthy_history_fallback_still_revokes_the_failed_member_publication_provenance() {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = engine(temp.path());
    let base = engine.published_state_base.clone().unwrap();
    let history = engine.state.histories.keys().next().unwrap();
    let hash = &base.roots.catalog_binding(history).unwrap().hash;
    let member = engine.members()[0].clone();
    let path = object_path(&member, "history-manifest", hash);
    std::fs::remove_file(&path).unwrap();
    engine
        .persist_auth_state(json!({"auth":"fallback"}), "alice", OperationId::generate())
        .unwrap();
    assert!(
        path.is_file(),
        "the unchanged full installation still restores exact bytes"
    );
    assert!(
        engine.published_state_base.is_none(),
        "successful fallback must not hide its observed member fault"
    );
    assert!(!base.matches(&engine.planning_context(), &config_hash(&engine)));
    engine
        .persist_auth_state(
            json!({"auth":"readmitted"}),
            "alice",
            OperationId::generate(),
        )
        .unwrap();
    assert!(
        engine
            .published_state_base
            .as_ref()
            .unwrap()
            .matches(&engine.planning_context(), &config_hash(&engine))
    );
}

#[test]
fn attempted_recovery_revokes_provenance_before_replay_failure() {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = engine(temp.path());
    let base = engine.published_state_base.clone().unwrap();
    // Recovery is entered on a fresh replay owner in production. A live tail is
    // deliberately not a valid replay prefix; even this failed attempt must
    // revoke prior provenance before any disk observation or replay failure.
    let error = engine
        .recover_mode(crate::engine::recovery::RecoveryMode::PreservePending)
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::LogFork);
    assert!(engine.published_state_base.is_none());
    assert!(!base.matches(&engine.planning_context(), &config_hash(&engine)));
}

#[test]
fn exact_snapshot_reconstruction_reports_missing_sources_before_restoring_all_copies() {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = engine(temp.path());
    let members = engine.members();
    let snapshot: Snapshot =
        load_object(&members[0], "snapshot", engine.snapshot.as_deref().unwrap()).unwrap();
    let addresses = map_addresses(&members[0], &snapshot.maps["repositories"]);
    let paths: Vec<_> = members
        .iter()
        .flat_map(|member| {
            addresses
                .iter()
                .map(move |(kind, hash)| object_path(member, kind, hash))
        })
        .collect();
    assert!(!paths.is_empty());
    for path in &paths {
        std::fs::remove_file(path).unwrap();
    }
    engine
        .persist_auth_state(
            json!({"auth":"exact reconstruction"}),
            "alice",
            OperationId::generate(),
        )
        .unwrap();
    assert!(paths.iter().all(|path| path.is_file()));
    assert!(engine.published_state_base.is_none());
}

#[test]
fn detached_publication_rejects_faults_observed_after_full_preparation_before_any_decision() {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = engine(temp.path());
    let mut next = engine.state().clone();
    next.sequence = engine.next_slot;
    let result = engine.state.operations.values().next().unwrap().clone();
    let detached = engine
        .prepare_publication(next, Vec::new(), result, MAX_METADATA)
        .unwrap();
    let sequence = engine.state.sequence;
    let slot = engine.next_slot;
    let journals: Vec<_> = engine
        .members()
        .iter()
        .map(|member| std::fs::read(log_path(member)).unwrap())
        .collect();
    engine.metadata_integrity.invalidate();
    assert_eq!(
        engine.decide_publication(detached).unwrap_err().code(),
        ErrorCode::PreparationStale
    );
    assert_eq!(engine.state.sequence, sequence);
    assert_eq!(engine.next_slot, slot);
    for (member, before) in engine.members().iter().zip(journals) {
        assert_eq!(std::fs::read(log_path(member)).unwrap(), before);
    }
}

#[test]
fn a_real_source_exceeding_its_bound_revokes_provenance_without_remapping_resource_limit() {
    let temp = tempfile::tempdir().unwrap();
    let engine = engine(temp.path());
    let base = engine.published_state_base.clone().unwrap();
    let history = engine.state.histories.keys().next().unwrap();
    let hash = &base.roots.catalog_binding(history).unwrap().hash;
    let member = &engine.members()[0];
    let path = object_path(member, "history-manifest", hash);
    let mut bytes = std::fs::read(&path).unwrap();
    let admitted_length = bytes.len();
    bytes.push(b' ');
    std::fs::write(&path, bytes).unwrap();
    let error = crate::disk::read_bytes(&path, admitted_length).unwrap_err();
    assert_eq!(error.code(), ErrorCode::ResourceLimit);
    engine.metadata_integrity.fault(&error);
    assert!(!base.matches(&engine.planning_context(), &config_hash(&engine)));
    assert_eq!(
        error.code(),
        ErrorCode::ResourceLimit,
        "fault notification preserves the concrete source error"
    );
}

#[test]
fn workspace_identity_observation_revokes_a_current_publication_receipt() {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = engine(temp.path());
    let branch = engine.state.branches.keys().next().unwrap().clone();
    let workspace = engine.create_workspace(&branch, "alice").unwrap();
    let base = engine.published_state_base.clone().unwrap();
    let plan = engine.planning_context();
    let repository = plan.repository_for_branch(&workspace.branch_id).unwrap();
    let disk = plan.disk(&repository.primary_slot_id).unwrap();
    assert!(disk.metadata_member);
    std::fs::remove_file(disk.root.join("disk.json")).unwrap();
    assert!(plan.workspace_data_directory(&workspace).is_err());
    assert!(!base.matches(&engine.planning_context(), &config_hash(&engine)));
    assert!(!engine.blocked);
}

#[test]
fn index_read_failure_does_not_revoke_member_metadata_epoch() {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = engine(temp.path());
    let base = engine.published_state_base.clone().unwrap();
    let epoch = engine.metadata_integrity.capture();
    let index = engine.root.join("index.sqlite");
    std::fs::rename(&index, engine.root.join("saved-index.sqlite")).unwrap();
    std::fs::create_dir(&index).unwrap();
    assert!(engine.indexed_file_history("absent").is_none());
    let crate::IndexHealth::Unavailable(failure) = engine.index_health() else {
        panic!("expected index read failure");
    };
    assert_eq!(failure.stage(), crate::IndexFailureStage::Read);
    assert!(epoch.matches(&engine.metadata_integrity));
    assert!(base.matches(&engine.planning_context(), &config_hash(&engine)));
    engine
        .persist_auth_state(
            json!({"auth":"published"}),
            "alice",
            OperationId::generate(),
        )
        .unwrap();
    assert!(epoch.matches(&engine.metadata_integrity));
    assert!(
        engine
            .published_state_base
            .as_ref()
            .unwrap()
            .matches(&engine.planning_context(), &config_hash(&engine))
    );
}
