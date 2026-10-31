use super::published_workspace_retirement::{evidence_entries, manifests};
use super::*;

fn historical(engine: &mut Engine, repo: &Repository, name: &str) -> Workspace {
    let head = engine.state().branches[&repo.main_branch_id]
        .head_commit_id
        .clone();
    let workspace = engine
        .create_historical_workspace(&repo.main_branch_id, &head, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, name, b"historical FULL")
        .unwrap();
    workspace
}

fn plan(engine: &mut Engine, repo: &Repository, workspace: &Workspace, name: &str) -> Plan {
    engine
        .create_historical_save_plan(
            &repo.history_tree_id,
            crate::HistoricalSaveOptions {
                source_commit_id: workspace.base_head.clone(),
                name: name.into(),
                workspace_id: Some(workspace.workspace_id.clone()),
                primary_slot_id: None,
                target_copies: None,
            },
            "admin",
        )
        .unwrap()
}

#[test]
fn empty_historical_abort_retires_but_unpublished_payload_and_plan_stay_retained() {
    let mut f = Fixture::new();
    f.config.max_retained_workspaces = 1;
    f.config.max_retained_workspaces_per_principal = 1;
    let mut engine = f.initialize();
    let repo = repository(&mut engine, 1);
    let head = engine.state().branches[&repo.main_branch_id]
        .head_commit_id
        .clone();
    let empty = engine
        .create_historical_workspace(&repo.main_branch_id, &head, "admin")
        .unwrap();
    let empty_plan = plan(&mut engine, &repo, &empty, "empty");
    let sequence = engine.state().sequence;
    engine
        .abort_workspace(&empty.workspace_id, "admin")
        .unwrap();
    assert_eq!(engine.workspace_capacity("admin").principal.records.used, 0);
    assert_eq!(engine.workspace_capacity("admin").principal.cold_records, 1);
    let operation = OperationId::generate();
    assert_eq!(
        engine
            .commit_historical_save_plan(&empty_plan.plan_id, "admin", operation)
            .unwrap_err()
            .code(),
        ErrorCode::WorkspaceClosed
    );
    assert!(!engine.state().operations.contains_key(&operation));
    let pending = historical(&mut engine, &repo, "pending");
    let pending_plan = plan(&mut engine, &repo, &pending, "pending");
    engine
        .abort_workspace(&pending.workspace_id, "admin")
        .unwrap();
    let capacity = engine.workspace_capacity("admin");
    assert_eq!(capacity.principal.records.used, 1);
    assert_eq!(capacity.principal.cold_records, 1);
    assert!(capacity.principal.metadata_bytes.used > 0);
    assert_eq!(engine.state().sequence, sequence);
    engine.assert_workspace_budget_consistent();
    drop(engine);
    let mut engine = Engine::open(&f.root).unwrap();
    assert_eq!(engine.workspace_capacity("admin"), capacity);
    assert_eq!(
        engine.workspace(&empty.workspace_id).unwrap().fencing_token,
        empty.fencing_token + 1
    );
    let retained = engine.workspace(&pending.workspace_id).unwrap();
    assert_eq!(retained.state, WorkspaceState::Aborted);
    for file in retained.files.values() {
        assert_eq!(fs::read(&file.source_path).unwrap(), b"historical FULL");
    }
    assert_eq!(
        engine.plan(&pending_plan.plan_id).unwrap().state,
        PlanState::Ready
    );
    assert_eq!(
        engine
            .commit_historical_save_plan(&pending_plan.plan_id, "admin", operation)
            .unwrap_err()
            .code(),
        ErrorCode::WorkspaceClosed
    );
    assert_eq!(
        engine
            .create_workspace(&repo.main_branch_id, "admin")
            .unwrap_err()
            .code(),
        ErrorCode::ResourceLimit
    );
    assert_eq!(engine.state().sequence, sequence);
}

#[test]
fn historical_retirement_binds_original_operation_actor_plan_and_frozen_manifest() {
    let f = Fixture::new();
    let mut engine = f.initialize();
    let repo = repository(&mut engine, 1);
    let workspace = historical(&mut engine, &repo, "bound");
    let plan = plan(&mut engine, &repo, &workspace, "bound");
    let operation = OperationId::generate();
    let published = engine
        .commit_historical_save_plan(&plan.plan_id, "admin", operation)
        .unwrap();
    let committed = engine.workspace(&workspace.workspace_id).unwrap();
    assert_eq!(
        engine
            .historical_workspace_completion_fence(&committed)
            .unwrap(),
        Some(committed.fencing_token)
    );
    for field in ["principal", "source", "branch", "manifest", "workspace"] {
        let mut forged = committed.clone();
        match field {
            "principal" => forged.principal = "foreign".into(),
            "source" => forged.base_head = published.commit_id.clone().unwrap(),
            "branch" => forged.branch_id = published.branch_id.clone().unwrap(),
            "manifest" => forged.files.clear(),
            "workspace" => forged.workspace_id = new_id("workspace"),
            _ => unreachable!(),
        }
        assert_eq!(
            engine
                .historical_workspace_completion_fence(&forged)
                .unwrap(),
            None,
            "{field}"
        );
    }
    assert_eq!(
        engine
            .commit_historical_save_plan(&plan.plan_id, "foreign", operation)
            .unwrap_err()
            .code(),
        ErrorCode::IdempotencyKeyReused
    );
    // A changed cold mirror cannot silently reactivate or resubmit the save.
    let path = manifests(&f, &workspace.workspace_id)[0].clone();
    let mut forged = committed.clone();
    forged.principal = "foreign".into();
    crate::disk::write_json(&path, &forged, true).unwrap();
    assert_eq!(
        engine
            .workspace(&workspace.workspace_id)
            .unwrap_err()
            .code(),
        ErrorCode::RecoveryRequired
    );
    let sequence = engine.state().sequence;
    assert_eq!(
        engine
            .commit_historical_save_plan(&plan.plan_id, "admin", operation)
            .unwrap(),
        published
    );
    assert_eq!(engine.state().sequence, sequence);
    assert_eq!(fs::read(path).unwrap(), canonical_json(&forged).unwrap());
}

#[test]
fn incomplete_historical_publication_and_local_completed_plan_cannot_bypass_quota_or_write() {
    for point in ["local_plan", "after_publication_1", "after_publication_2"] {
        let mut f = Fixture::new();
        f.config.max_retained_workspaces = 1;
        f.config.max_retained_workspaces_per_principal = 1;
        let raised = crate::RetainedWorkspaceLimits {
            max_retained_workspaces: 2,
            max_retained_workspaces_per_principal: 2,
            max_retained_workspace_metadata_bytes: f.config.max_retained_workspace_metadata_bytes,
            max_retained_workspace_metadata_bytes_per_principal: f
                .config
                .max_retained_workspace_metadata_bytes_per_principal,
        };
        let mut engine =
            Engine::initialize_with_workspace_limits(&f.root, f.config.clone(), Some(raised))
                .unwrap();
        let repo = repository(&mut engine, 3);
        let anchor = engine
            .create_workspace(&repo.main_branch_id, "admin")
            .unwrap();
        let anchor_full = engine
            .stage_bytes(&anchor.workspace_id, "anchor", b"unpublished anchor")
            .unwrap();
        engine
            .abort_workspace(&anchor.workspace_id, "admin")
            .unwrap();
        let workspace = historical(&mut engine, &repo, "pending");
        let mut plan = plan(&mut engine, &repo, &workspace, "unproven");
        let prefix = engine.state().sequence;
        let operation = OperationId::generate();
        if point != "local_plan" {
            engine.inject_fault_once(point);
            let error = engine
                .commit_historical_save_plan(&plan.plan_id, "admin", operation)
                .unwrap_err();
            assert_eq!(error.code(), ErrorCode::InDoubt);
            let Error::Context { source, .. } = error else {
                panic!("missing publication fault source")
            };
            assert_eq!(source.code(), ErrorCode::InjectedFailure);
            assert_eq!(source.message(), format!("interrupted at {point}"));
        }
        assert_eq!(engine.state().sequence, prefix);
        assert!(!engine.state().operations.contains_key(&operation));
        // Only local, non-authoritative evidence claims Completed. No fake
        // admitted State or operation is manufactured by this fixture.
        plan.state = PlanState::Completed;
        crate::disk::write_json(
            &f.root.join("plans").join(format!("{}.json", plan.plan_id)),
            &plan,
            true,
        )
        .unwrap();
        let mut claimed = engine.workspace(&workspace.workspace_id).unwrap();
        claimed.state = WorkspaceState::Committed;
        claimed.fencing_token += 1;
        for path in manifests(&f, &workspace.workspace_id) {
            crate::disk::write_json(&path, &claimed, true).unwrap();
        }
        let members = engine.members();
        let count = members
            .iter()
            .flat_map(|disk| crate::journal::scan(disk).unwrap().records)
            .filter(|(record, _)| {
                record.sequence > prefix && record.record_type == RecordType::Publication
            })
            .count();
        assert_eq!(
            count,
            match point {
                "after_publication_1" => 1,
                "after_publication_2" => 2,
                _ => 0,
            }
        );
        drop(engine);
        // The final sorted member has no publication in any tested case.
        let missing = crate::journal::log_path(&members[2]);
        fs::rename(&missing, missing.with_extension("retained")).unwrap();
        for disk in &members[..2] {
            fs::OpenOptions::new()
                .append(true)
                .open(crate::journal::log_path(disk))
                .unwrap()
                .write_all(b"ZXS")
                .unwrap();
        }
        let roots: Vec<_> = std::iter::once(f.root.clone())
            .chain(f.config.disks.iter().map(|disk| disk.root.clone()))
            .collect();
        let before: Vec<_> = roots.iter().map(|root| evidence_entries(root)).collect();
        let restored = f.temp.path().join("rejected");
        let disks: Vec<_> = f
            .config
            .disks
            .iter()
            .map(|disk| disk.root.clone())
            .collect();
        for mode in ["open", "preserve", "restore"] {
            let result = match mode {
                "open" => Engine::open(&f.root),
                "preserve" => Engine::open_for_recovery(&f.root),
                _ => Engine::restore_from_disks(&restored, &disks),
            };
            assert_eq!(
                result.err().unwrap().code(),
                ErrorCode::ResourceLimit,
                "{point}: {mode}"
            );
            assert!(!missing.exists());
            assert_eq!(
                roots
                    .iter()
                    .map(|root| evidence_entries(root))
                    .collect::<Vec<_>>(),
                before
            );
            assert!(!restored.join("config.json").exists());
            assert!(!restored.join("index.sqlite").exists());
            assert_eq!(
                fs::read(&anchor_full.source_path).unwrap(),
                b"unpublished anchor"
            );
            for file in claimed.files.values() {
                assert_eq!(fs::read(&file.source_path).unwrap(), b"historical FULL");
            }
        }
    }
}

#[test]
fn recovery_reads_old_full_generations_from_cold_historical_workspaces() {
    let f = Fixture::new();
    let mut engine = f.initialize();
    let repo = repository(&mut engine, 3);
    let cold = historical(&mut engine, &repo, "file");
    let old = engine
        .stage_bytes(&cold.workspace_id, "file", b"old historical recovery bytes")
        .unwrap();
    let current = engine
        .stage_bytes(&cold.workspace_id, "file", b"current historical bytes")
        .unwrap();
    assert_ne!(old.source_path, current.source_path);
    let saved_plan = plan(&mut engine, &repo, &cold, "cold");
    let saved_id = OperationId::generate();
    let saved = engine
        .commit_historical_save_plan(&saved_plan.plan_id, "admin", saved_id)
        .unwrap();
    assert!(!engine.workspaces.contains_key(&cold.workspace_id));
    let pending = historical(&mut engine, &repo, "file");
    let staged = engine
        .stage_bytes(
            &pending.workspace_id,
            "file",
            b"old historical recovery bytes",
        )
        .unwrap();
    let pending_plan = plan(&mut engine, &repo, &pending, "recovered");
    let operation = OperationId::generate();
    engine.inject_fault_once("after_accept_2");
    assert_eq!(
        engine
            .commit_historical_save_plan(&pending_plan.plan_id, "admin", operation)
            .unwrap_err()
            .code(),
        ErrorCode::InDoubt
    );
    let member = &engine.members()[0];
    let held = &engine.disk_resources.get(member).unwrap().root;
    let scan = crate::journal::scan_at(held, member).unwrap();
    let hash = &scan.records.last().unwrap().0.candidate_hash;
    let candidate: crate::journal::Candidate =
        crate::journal::load_object_at(held, member, "candidate", hash).unwrap();
    assert_eq!(candidate.operation_id, operation);
    drop(engine);
    fs::remove_file(&staged.source_path).unwrap();
    for evidence in &candidate.versions {
        for disk in &f.config.disks {
            fs::remove_dir_all(crate::disk::version_dir(
                disk,
                &evidence.version.history_tree_id,
                &evidence.version.version_id,
            ))
            .unwrap();
        }
    }
    let mut engine = Engine::open(&f.root).unwrap();
    assert!(!engine.workspaces.contains_key(&cold.workspace_id));
    assert_eq!(
        engine.operation(operation).unwrap().commit_state,
        CommitState::Published
    );
    assert_eq!(
        bytes(&engine, &candidate.versions[0].version.version_id),
        b"old historical recovery bytes"
    );
    assert_eq!(
        fs::read(&old.source_path).unwrap(),
        b"old historical recovery bytes"
    );
    assert_eq!(
        fs::read(&current.source_path).unwrap(),
        b"current historical bytes"
    );
    let sequence = engine.state().sequence;
    assert_eq!(
        engine
            .commit_historical_save_plan(&saved_plan.plan_id, "admin", saved_id)
            .unwrap(),
        saved
    );
    assert_eq!(
        engine
            .commit_historical_save_plan(&pending_plan.plan_id, "admin", operation)
            .unwrap()
            .operation_id,
        operation
    );
    assert_eq!(engine.state().sequence, sequence);
    engine.assert_workspace_budget_consistent();
}

#[test]
fn historical_completion_mirror_failure_preserves_published_result_and_resident_evidence() {
    let f = Fixture::new();
    let mut engine = f.initialize();
    let repo = repository(&mut engine, 1);
    let workspace = historical(&mut engine, &repo, "file");
    let plan = plan(&mut engine, &repo, &workspace, "mirror failure");
    let path = manifests(&f, &workspace.workspace_id)[0].clone();
    let sealed = fs::read(&path).unwrap();
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    let operation = OperationId::generate();
    let error = engine
        .commit_historical_save_plan(&plan.plan_id, "admin", operation)
        .unwrap_err();
    let Error::PublishedWorkspaceReconciliation(error) = error else {
        panic!("lost already-published outcome")
    };
    let published = error.result().clone();
    let source = std::error::Error::source(error.as_ref())
        .unwrap()
        .downcast_ref::<Error>()
        .unwrap();
    assert_eq!(source.code(), ErrorCode::UnsafePath);
    assert_eq!(error.workspace_id(), workspace.workspace_id);
    assert_eq!(error.principal(), "admin");
    assert_eq!(published.operation_id, operation);
    assert_eq!(published.commit_state, CommitState::Published);
    assert_eq!(engine.workspace_capacity("admin").principal.records.used, 1);
    assert!(engine.blocked);
    assert_eq!(
        engine
            .commit_historical_save_plan(&plan.plan_id, "admin", operation)
            .unwrap(),
        published
    );
    fs::remove_dir(&path).unwrap();
    fs::write(&path, sealed).unwrap();
    drop(engine);
    let mut engine = Engine::open(&f.root).unwrap();
    assert_eq!(
        engine.workspace(&workspace.workspace_id).unwrap().state,
        WorkspaceState::Committed
    );
    assert_eq!(
        engine
            .commit_historical_save_plan(&plan.plan_id, "admin", operation)
            .unwrap(),
        published
    );
    // Sealed/committed mirrors are conservatively resident until both agree.
    assert_eq!(engine.workspace_capacity("admin").principal.records.used, 1);
    for file in engine
        .workspace(&workspace.workspace_id)
        .unwrap()
        .files
        .values()
    {
        assert_eq!(fs::read(&file.source_path).unwrap(), b"historical FULL");
    }
}

#[test]
fn upload_owner_keeps_historical_completion_resident_until_terminal_reconciliation() {
    let f = Fixture::new();
    let mut engine = f.initialize();
    let repo = repository(&mut engine, 1);
    let workspace = historical(&mut engine, &repo, "file");
    let source = f.temp.path().join("unconfirmed");
    fs::write(&source, b"unconfirmed owner bytes").unwrap();
    let owner = engine
        .prepare_upload(
            &workspace.workspace_id,
            &new_id("entry"),
            "pending",
            &source,
            1,
        )
        .unwrap();
    let plan = plan(&mut engine, &repo, &workspace, "owner held");
    let operation = OperationId::generate();
    let published = engine
        .commit_historical_save_plan(&plan.plan_id, "admin", operation)
        .unwrap();
    assert!(engine.workspaces.contains_key(&workspace.workspace_id));
    assert_eq!(engine.workspace_capacity("admin").principal.records.used, 1);
    assert_eq!(fs::read(&source).unwrap(), b"unconfirmed owner bytes");
    drop(owner);
    engine
        .abort_workspace(&workspace.workspace_id, "admin")
        .unwrap();
    assert!(!engine.workspaces.contains_key(&workspace.workspace_id));
    assert_eq!(engine.workspace_capacity("admin").principal.records.used, 0);
    assert_eq!(engine.workspace_capacity("admin").principal.cold_records, 1);
    assert_eq!(
        engine
            .commit_historical_save_plan(&plan.plan_id, "admin", operation)
            .unwrap(),
        published
    );
    engine.assert_workspace_budget_consistent();
}

#[test]
fn historical_retirement_keeps_unknown_and_unconfirmed_directory_material_resident() {
    for name in [
        "commit-request.json",
        "wire-request.json",
        "scratch.prepared",
        ".zxs-full-pending",
        "unknown",
    ] {
        let f = Fixture::new();
        let mut engine = f.initialize();
        let repo = repository(&mut engine, 1);
        let workspace = historical(&mut engine, &repo, "file");
        let plan = plan(&mut engine, &repo, &workspace, "retained material");
        let primary = manifests(&f, &workspace.workspace_id)[1]
            .parent()
            .unwrap()
            .join(name);
        fs::write(&primary, b"unconfirmed evidence").unwrap();
        let operation = OperationId::generate();
        let result = engine
            .commit_historical_save_plan(&plan.plan_id, "admin", operation)
            .unwrap();
        assert_eq!(
            engine.workspace_capacity("admin").principal.records.used,
            1,
            "{name}"
        );
        assert_eq!(engine.workspace_capacity("admin").principal.cold_records, 0);
        let capacity = engine.workspace_capacity("admin");
        drop(engine);
        let mut engine = Engine::open(&f.root).unwrap();
        assert_eq!(engine.workspace_capacity("admin"), capacity);
        assert_eq!(fs::read(primary).unwrap(), b"unconfirmed evidence");
        assert_eq!(
            engine
                .commit_historical_save_plan(&plan.plan_id, "admin", operation)
                .unwrap(),
            result
        );
        engine.assert_workspace_budget_consistent();
    }
}
