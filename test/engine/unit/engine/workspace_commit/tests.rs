use super::*;

#[test]
fn detached_snapshot_keeps_only_target_and_preserves_live_retained_evidence() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("state");
    let mut config = Config::development([temp.path().join("primary")]);
    config.max_retained_workspaces = 1;
    config.max_retained_workspaces_per_principal = 1;
    let limits = RetainedWorkspaceLimits {
        max_retained_workspaces: 4,
        max_retained_workspaces_per_principal: 2,
        max_retained_workspace_metadata_bytes: config.max_retained_workspace_metadata_bytes * 2,
        max_retained_workspace_metadata_bytes_per_principal: config
            .max_retained_workspace_metadata_bytes_per_principal
            * 2,
    };
    let mut engine = Engine::initialize_with_workspace_limits(&root, config, Some(limits)).unwrap();
    let repo = engine
        .create_repository(
            "planning",
            "P01",
            1,
            "alice",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    for (principal, abort) in [("alice", true), ("bob", true), ("bob", false)] {
        let workspace = engine
            .create_workspace(&repo.main_branch_id, principal)
            .unwrap();
        engine
            .stage_bytes(&workspace.workspace_id, "unrelated", b"retained payload")
            .unwrap();
        if abort {
            engine
                .abort_workspace(&workspace.workspace_id, principal)
                .unwrap();
        }
    }
    let unrelated = engine.workspaces.clone();
    let target = engine
        .create_workspace(&repo.main_branch_id, "alice")
        .unwrap();
    engine
        .stage_bytes(&target.workspace_id, "target", b"target payload")
        .unwrap();
    let request = CommitRequest {
        workspace_id: target.workspace_id.clone(),
        operation_id: storage_contract::OperationId::generate(),
        principal: "alice".into(),
        expected_head: target.base_head,
        fencing_token: target.fencing_token,
        message: "target-only snapshot".into(),
        request_digest: None,
    };
    let before = canonical_json(&engine.workspaces).unwrap();
    let frozen_state = Arc::clone(&engine.state);
    let frozen_bytes = canonical_json(engine.state()).unwrap();
    let capacities: Vec<_> = ["alice", "bob", "absent"]
        .into_iter()
        .map(|principal| (principal, engine.workspace_capacity(principal)))
        .collect();
    let preparation = engine
        .prepare_workspace_commit(request.clone(), None)
        .unwrap();
    let planning = &preparation.snapshot;
    assert!(Arc::ptr_eq(&planning.state, &engine.state));
    assert_eq!(planning.config.effective_workspace_limits(), limits);
    assert_eq!(
        canonical_json(&planning.workspace).unwrap(),
        canonical_json(&engine.workspace(&target.workspace_id).unwrap()).unwrap()
    );
    for workspace_id in unrelated.keys() {
        assert_ne!(&planning.workspace.workspace_id, workspace_id);
    }
    for (principal, capacity) in &capacities {
        assert_eq!(&engine.workspace_capacity(principal), capacity);
    }
    assert_eq!(engine.workspaces.len(), 4);
    assert_eq!(canonical_json(&engine.workspaces).unwrap(), before);
    engine.assert_workspace_budget_consistent();
    let prepared = preparation.execute().unwrap();
    assert!(Arc::ptr_eq(&frozen_state, &engine.state));
    assert_eq!(canonical_json(engine.state()).unwrap(), frozen_bytes);
    assert_eq!(canonical_json(&engine.workspaces).unwrap(), before);
    let result = engine.finish_workspace_commit(prepared).unwrap();
    assert!(!Arc::ptr_eq(&frozen_state, &engine.state));
    assert_eq!(canonical_json(frozen_state.as_ref()).unwrap(), frozen_bytes);
    assert_eq!(result.operation_id, request.operation_id);
    assert_eq!(
        engine
            .workspace(&target.workspace_id)
            .unwrap()
            .state
            .as_str(),
        "committed"
    );
    assert_eq!(engine.workspaces.len(), 4);
    for (id, workspace) in &unrelated {
        assert_eq!(
            canonical_json(&engine.workspace(id).unwrap()).unwrap(),
            canonical_json(workspace).unwrap()
        );
    }
    engine.assert_workspace_budget_consistent();
    let committed = canonical_json(&engine.workspaces).unwrap();
    let published_state = canonical_json(engine.state()).unwrap();
    let capacities: Vec<_> = capacities
        .into_iter()
        .map(|(principal, _)| (principal, engine.workspace_capacity(principal)))
        .collect();
    drop(engine);
    let mut engine = Engine::open_with_workspace_limits(&root, Some(limits)).unwrap();
    assert_eq!(canonical_json(engine.state()).unwrap(), published_state);
    assert_eq!(canonical_json(&engine.workspaces).unwrap(), committed);
    assert_eq!(engine.operation(request.operation_id).unwrap(), result);
    let replay = engine
        .prepare_workspace_commit(request, None)
        .unwrap()
        .execute()
        .unwrap();
    assert_eq!(engine.finish_workspace_commit(replay).unwrap(), result);
    assert_eq!(canonical_json(engine.state()).unwrap(), published_state);
    assert_eq!(canonical_json(&engine.workspaces).unwrap(), committed);
    for (principal, capacity) in capacities {
        assert_eq!(engine.workspace_capacity(principal), capacity);
    }
    engine.assert_workspace_budget_consistent();
}

#[test]
fn shared_state_stays_frozen_across_cancelled_preparation_and_plan_writes() {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &temp.path().join("state"),
        Config::development([temp.path().join("primary")]),
    )
    .unwrap();
    let repo = engine
        .create_repository(
            "shared state",
            "P01",
            1,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let mut commits = Vec::new();
    for bytes in [b"first".as_slice(), b"second".as_slice()] {
        let workspace = engine
            .create_workspace(&repo.main_branch_id, "admin")
            .unwrap();
        engine
            .stage_bytes(&workspace.workspace_id, "file.txt", bytes)
            .unwrap();
        let result = engine
            .commit_workspace(CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: storage_contract::OperationId::generate(),
                principal: "admin".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "source".into(),
                request_digest: None,
            })
            .unwrap();
        commits.push(result.commit_id.unwrap());
    }
    let old_version = engine.state.commits[&commits[0]].entries[0]
        .version_id
        .clone();
    let options = HistoricalSaveOptions {
        source_commit_id: commits[0].clone(),
        name: "historical preview".into(),
        workspace_id: None,
        primary_slot_id: None,
        target_copies: None,
    };
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "file.txt", b"third")
        .unwrap();
    let request = CommitRequest {
        workspace_id: workspace.workspace_id,
        operation_id: storage_contract::OperationId::generate(),
        principal: "admin".into(),
        expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token,
        message: "after cancelled preparation".into(),
        request_digest: None,
    };
    let preparation = engine
        .prepare_workspace_commit(request.clone(), None)
        .unwrap();
    let frozen = Arc::clone(&preparation.snapshot.state);
    let frozen_bytes = canonical_json(frozen.as_ref()).unwrap();
    assert!(Arc::ptr_eq(&frozen, &engine.state));
    assert_eq!(
        engine
            .create_purge_plan(&repo.history_tree_id, vec![old_version.clone()], "admin")
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::CommitPreparationBusy
    );
    assert_eq!(
        engine
            .create_historical_save_plan(&repo.history_tree_id, options.clone(), "admin")
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::CommitPreparationBusy
    );
    assert!(Arc::ptr_eq(&frozen, &engine.state));
    assert_eq!(canonical_json(engine.state()).unwrap(), frozen_bytes);
    // Retain only a test-owned immutable view, without bypassing live admission.
    drop(preparation);
    let purge = engine
        .create_purge_plan(&repo.history_tree_id, vec![old_version], "admin")
        .unwrap();
    assert!(!Arc::ptr_eq(&frozen, &engine.state));
    assert_eq!(canonical_json(frozen.as_ref()).unwrap(), frozen_bytes);
    assert!(!frozen.plans.contains_key(&purge.plan_id));
    assert_eq!(engine.state.plans[&purge.plan_id], purge);

    let with_purge = Arc::clone(&engine.state);
    let with_purge_bytes = canonical_json(with_purge.as_ref()).unwrap();
    let historical = engine
        .create_historical_save_plan(&repo.history_tree_id, options, "admin")
        .unwrap();
    assert!(!Arc::ptr_eq(&with_purge, &engine.state));
    assert_eq!(
        canonical_json(with_purge.as_ref()).unwrap(),
        with_purge_bytes
    );
    assert!(!with_purge.plans.contains_key(&historical.plan_id));
    assert_eq!(engine.state.plans[&historical.plan_id], historical);
    assert_eq!(engine.state.plans[&purge.plan_id], purge);

    let before_publish = Arc::clone(&engine.state);
    let before_publish_bytes = canonical_json(before_publish.as_ref()).unwrap();
    let result = engine.commit_workspace(request).unwrap();
    assert!(!Arc::ptr_eq(&before_publish, &engine.state));
    assert_eq!(
        canonical_json(before_publish.as_ref()).unwrap(),
        before_publish_bytes
    );
    assert!(!before_publish.operations.contains_key(&result.operation_id));
    assert_eq!(engine.state.operations[&result.operation_id], result);
    assert_eq!(canonical_json(frozen.as_ref()).unwrap(), frozen_bytes);
    assert_eq!(
        canonical_json(with_purge.as_ref()).unwrap(),
        with_purge_bytes
    );
}

#[test]
fn plan_only_record_mutations_and_abandoned_candidates_leave_published_state_unchanged() {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &temp.path().join("state"),
        Config::development([temp.path().join("primary")]),
    )
    .unwrap();
    let repo = engine
        .create_repository(
            "record isolation",
            "P01",
            1,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "nested/file.txt", b"published")
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: storage_contract::OperationId::generate(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "published".into(),
            request_digest: None,
        })
        .unwrap();
    engine
        .initialize_directory_scope(
            &repo.repository_id,
            "nested",
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let frozen = Arc::clone(&engine.state);
    let before = canonical_json(frozen.as_ref()).unwrap();
    let commit_id = frozen.branches[&repo.main_branch_id].head_commit_id.clone();
    let tree_id = frozen.directory_trees.keys().next().unwrap().clone();
    let version_id = frozen.versions.keys().next().unwrap().clone();
    let mutate_records = |next: &mut State| {
        assert!(next.versions.shares_storage_with(&frozen.versions));
        next.commits.get_mut(&commit_id).unwrap().message = "private candidate".into();
        next.directory_trees.get_mut(&tree_id).unwrap().path = "private/candidate".into();
        next.versions.get_mut(&version_id).unwrap().creation_name = "private candidate".into();
        assert!(!next.versions.shares_storage_with(&frozen.versions));
        let mut commit = next.commits[&commit_id].clone();
        commit.commit_id = "candidate-only-commit".into();
        assert!(
            next.commits
                .insert(commit.commit_id.clone(), commit)
                .is_none()
        );
        let mut tree = next.directory_trees[&tree_id].clone();
        tree.tree_id = "candidate-only-tree".into();
        assert!(
            next.directory_trees
                .insert(tree.tree_id.clone(), tree)
                .is_none()
        );
        let mut version = next.versions[&version_id].clone();
        version.version_id = "candidate-only-version".into();
        assert!(
            next.versions
                .insert(version.version_id.clone(), version)
                .is_none()
        );
        assert!(next.commits.remove_discard("candidate-only-commit"));
        assert!(next.directory_trees.remove_discard("candidate-only-tree"));
        assert!(next.versions.remove_discard("candidate-only-version"));
    };
    let failed_operation = storage_contract::OperationId::generate();
    let failed = engine.plan_mutation_digest(
        "test.record_isolation",
        "admin",
        failed_operation,
        json_codec::json!({}),
        None,
        |_, next, _| {
            mutate_records(next);
            Err(Error::new(
                storage_contract::ErrorCode::InjectedFailure,
                "abandon mutated metadata",
            ))
        },
    );
    assert!(
        matches!(failed, Err(ref error) if error.code() == storage_contract::ErrorCode::InjectedFailure)
    );
    assert_eq!(canonical_json(engine.state()).unwrap(), before);
    assert!(Arc::ptr_eq(&frozen, &engine.state));
    assert!(!engine.state.operations.contains_key(&failed_operation));

    let operation = storage_contract::OperationId::generate();
    let planned = engine
        .plan_mutation_digest(
            "test.record_isolation",
            "admin",
            operation,
            json_codec::json!({}),
            None,
            |_, next, _| {
                mutate_records(next);
                Ok(json_codec::json!({}))
            },
        )
        .unwrap();
    let MutationPlan::Prepared { next, prepared, .. } = planned else {
        panic!("fresh operation must produce an independent candidate");
    };
    assert!(prepared.is_empty());
    assert_eq!(next.commits[&commit_id].message, "private candidate");
    assert_eq!(next.directory_trees[&tree_id].path, "private/candidate");
    assert!(!next.commits.contains_key("candidate-only-commit"));
    assert!(!next.directory_trees.contains_key("candidate-only-tree"));
    assert_eq!(
        next.versions[&version_id].creation_name,
        "private candidate"
    );
    assert!(!next.versions.contains_key("candidate-only-version"));
    assert!(next.operations.contains_key(&operation));
    assert_eq!(canonical_json(engine.state()).unwrap(), before);
    drop(next); // A successful plan is not a publication; cancellation discards it.
    assert_eq!(canonical_json(frozen.as_ref()).unwrap(), before);
    assert_eq!(canonical_json(engine.state()).unwrap(), before);
    assert!(Arc::ptr_eq(&frozen, &engine.state));
    assert!(!engine.state.operations.contains_key(&operation));
    assert!(!engine.is_read_only());
}

#[test]
fn plan_failed_change_can_abandon_a_temporarily_missing_main_head() {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &temp.path().join("state"),
        Config::development([temp.path().join("primary")]),
    )
    .unwrap();
    let repo = engine
        .create_repository(
            "abandoned head",
            "P01",
            1,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let frozen = Arc::clone(&engine.state);
    let before = canonical_json(frozen.as_ref()).unwrap();
    let head = frozen.branches[&repo.main_branch_id].head_commit_id.clone();
    let operation = storage_contract::OperationId::generate();
    let result = engine.plan_mutation_digest(
        "test.abandon_head",
        "admin",
        operation,
        json_codec::json!({}),
        None,
        |_, next, _| {
            assert!(next.commits.remove_discard(&head));
            Err(Error::new(
                storage_contract::ErrorCode::InjectedFailure,
                "abandon temporary missing head",
            ))
        },
    );
    assert!(
        matches!(result, Err(ref error) if error.code() == storage_contract::ErrorCode::InjectedFailure)
    );
    assert!(Arc::ptr_eq(&frozen, &engine.state));
    assert_eq!(canonical_json(engine.state()).unwrap(), before);
    assert!(!engine.state.operations.contains_key(&operation));
    assert!(!engine.is_read_only());
}

#[test]
#[should_panic(expected = "invariant: every branch has its head commit")]
fn plan_successful_change_must_restore_main_head_before_returning() {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &temp.path().join("state"),
        Config::development([temp.path().join("primary")]),
    )
    .unwrap();
    let repo = engine
        .create_repository(
            "broken successful head",
            "P01",
            1,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let head = engine.state.branches[&repo.main_branch_id]
        .head_commit_id
        .clone();
    engine
        .plan_mutation_digest(
            "test.broken_head",
            "admin",
            storage_contract::OperationId::generate(),
            json_codec::json!({}),
            None,
            |_, next, _| {
                assert!(next.commits.remove_discard(&head));
                Ok(json_codec::json!({}))
            },
        )
        .unwrap_or_else(|error| panic!("unexpected planning error: {error}"));
}

#[test]
fn wire_envelope_errors_precede_missing_workspace_lookup() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("system");
    let mut engine =
        Engine::initialize(&root, Config::development([temp.path().join("disk")])).unwrap();
    let mut request = CommitRequest {
        workspace_id: new_id("ws"),
        operation_id: storage_contract::OperationId::generate(),
        principal: "alice".into(),
        expected_head: new_id("commit"),
        fencing_token: 1,
        message: String::new(),
        request_digest: None,
    };
    for body in [json_codec::Value::Null, json_codec::json!({})] {
        assert_eq!(
            storage_contract::PublicCommitEnvelope::parse(&request.workspace_id, body)
                .unwrap_err()
                .code(),
            ErrorCode::InvalidRequest
        );
    }
    let body = json_codec::json!({
        "operation_id": request.operation_id,
        "workspace_id": request.workspace_id,
        "repository_id": "missing",
        "branch_id": "missing",
        "expected_head": request.expected_head,
        "fencing_token": "1",
        "message": "",
        "requested_protection": "strict",
        "file_placement": {},
        "expected_topology_generation": "1",
        "sealed_manifest_hash": "missing",
        "request_digest": "missing",
    });
    assert_eq!(
        engine
            .commit_workspace_wire(request.clone(), public_envelope(body.clone()))
            .unwrap_err()
            .code(),
        ErrorCode::WorkspaceNotFound
    );
    let operation = storage_contract::OperationId::generate();
    engine
        .create_repository("repo", "P01", 1, "alice", operation)
        .unwrap();
    request.operation_id = operation;
    assert_eq!(
        engine
            .commit_workspace_wire(request, public_envelope(body))
            .unwrap_err()
            .code(),
        ErrorCode::IdempotencyKeyReused
    );
}

fn public_envelope(body: json_codec::Value) -> storage_contract::PublicCommitEnvelope {
    let workspace = body["workspace_id"].as_str().unwrap().to_owned();
    storage_contract::PublicCommitEnvelope::parse(&workspace, body).unwrap()
}

#[test]
fn public_preparation_fences_registration_and_finish_rechecks_live_receipt() {
    for change in ["same", "digest", "actor"] {
        let root = tempfile::tempdir().unwrap();
        let mut engine = Engine::initialize(
            &root.path().join("system"),
            Config::development([root.path().join("disk")]),
        )
        .unwrap();
        let repo = engine
            .create_repository("receipt race", "P01", 1, "owner", OperationId::generate())
            .unwrap();
        let workspace = engine
            .create_workspace(&repo.main_branch_id, "owner")
            .unwrap();
        let mut body = json_codec::to_value(
            engine
                .seal_workspace(&workspace.workspace_id, "owner")
                .unwrap(),
        )
        .unwrap();
        body["request_digest"] = crate::commit_request_digest(&workspace.workspace_id, &body)
            .unwrap()
            .into();
        let envelope = public_envelope(body.clone());
        let request = CommitRequest {
            workspace_id: workspace.workspace_id.clone(),
            operation_id: envelope.operation_id(),
            principal: "owner".into(),
            expected_head: workspace.base_head.clone(),
            fencing_token: workspace.fencing_token,
            message: String::new(),
            request_digest: Some(envelope.claimed_digest().into()),
        };
        let plan = engine
            .prepare_workspace_commit(request.clone(), Some(envelope))
            .unwrap();
        let mut registered_body = body;
        let mut registered_request = request.clone();
        if change == "digest" {
            registered_body["message"] = "different registration".into();
            registered_request.message = "different registration".into();
            registered_body["request_digest"] =
                crate::commit_request_digest(&workspace.workspace_id, &registered_body)
                    .unwrap()
                    .into();
            registered_request.request_digest =
                Some(registered_body["request_digest"].as_str().unwrap().into());
        }
        assert_eq!(
            engine
                .commit_workspace_wire(
                    registered_request.clone(),
                    public_envelope(registered_body.clone())
                )
                .unwrap_err()
                .code(),
            ErrorCode::CommitPreparationBusy
        );
        let mut prepared = plan.execute().unwrap();
        // Production keeps this admission until finish. Release it explicitly in
        // this unit fixture to exercise the live-receipt branch independently of
        // today's global write fence; this is not a claim that writers bypass it.
        drop(prepared._admission.take());
        let registered = engine
            .commit_workspace_wire(registered_request, public_envelope(registered_body))
            .unwrap();
        if change == "actor" {
            // Simulate a conflicting actor receipt after capture, without adding
            // any production hook or pretending another actor owns this workspace.
            Arc::make_mut(&mut engine.state)
                .operation_principals
                .insert(request.operation_id, "different-actor".into());
        }
        let sequence = engine.state.sequence;
        let result = engine.finish_workspace_commit(prepared);
        if change == "same" {
            assert_eq!(result.unwrap(), registered);
        } else {
            assert_eq!(result.unwrap_err().code(), ErrorCode::IdempotencyKeyReused);
        }
        assert_eq!(engine.state.sequence, sequence);
    }
}
