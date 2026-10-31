use std::fs;
use storage_contract::{CommitState, ErrorCode, OperationId, RepositoryName, WorkspaceState};
use storage_engine::{Config, Engine, HistoricalSaveOptions};

#[test]
fn historical_publications_release_resident_quota_keep_original_ids_and_restore_without_system() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("system");
    let mut config = Config::development([
        temp.path().join("d1"),
        temp.path().join("d2"),
        temp.path().join("d3"),
    ]);
    config.max_retained_workspaces = 1;
    config.max_retained_workspaces_per_principal = 1;
    let disks: Vec<_> = config.disks.iter().map(|disk| disk.root.clone()).collect();
    let mut engine = Engine::initialize(&root, config).unwrap();
    let repo = engine
        .create_repository(
            &RepositoryName::parse("historical").unwrap(),
            "P01",
            3,
            "admin",
            OperationId::generate(),
        )
        .unwrap();
    let source = engine.state().branches[&repo.main_branch_id]
        .head_commit_id
        .clone();
    let mut saves = Vec::new();
    let mut charged = 0;
    for index in 0..3 {
        let workspace = engine
            .create_historical_workspace(&repo.main_branch_id, &source, "admin")
            .unwrap();
        let payload = format!("historical FULL {index}");
        let staged = engine
            .stage_bytes(&workspace.workspace_id, "file", payload.as_bytes())
            .unwrap();
        let plan = engine
            .create_historical_save_plan(
                &repo.history_tree_id,
                HistoricalSaveOptions {
                    source_commit_id: source.clone(),
                    name: format!("save {index}"),
                    workspace_id: Some(workspace.workspace_id.clone()),
                    primary_slot_id: None,
                    target_copies: None,
                },
                "admin",
            )
            .unwrap();
        assert_eq!(engine.workspace_capacity("admin").principal.records.used, 1);
        let operation = OperationId::generate();
        let result = engine
            .commit_historical_save_plan(&plan.plan_id, "admin", operation)
            .unwrap();
        assert_eq!(result.commit_state, CommitState::Published);
        assert_eq!(
            engine.workspace(&workspace.workspace_id).unwrap().state,
            WorkspaceState::Committed
        );
        let capacity = engine.workspace_capacity("admin");
        assert_eq!(capacity.principal.records.used, 0);
        assert_eq!(capacity.principal.cold_records, index + 1);
        assert!(capacity.principal.metadata_bytes.used > charged);
        charged = capacity.principal.metadata_bytes.used;
        saves.push((
            workspace.workspace_id,
            plan.plan_id,
            operation,
            result,
            staged.source_path,
            payload,
        ));
    }
    let active = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(
            &active.workspace_id,
            "pending",
            b"unpublished recovery evidence",
        )
        .unwrap();
    let capacity = engine.workspace_capacity("admin");
    let sequence = engine.state().sequence;
    let branches = engine.state().branches.len();
    let commits = engine.state().commits.len();
    for (_, plan, operation, result, _, _) in &saves {
        assert_eq!(
            engine
                .commit_historical_save_plan(plan, "admin", *operation)
                .unwrap(),
            *result
        );
    }
    assert_eq!(
        engine
            .create_historical_workspace(&repo.main_branch_id, &source, "admin")
            .unwrap_err()
            .code(),
        ErrorCode::ResourceLimit
    );
    drop(engine);
    let mut engine = Engine::open(&root).unwrap();
    assert_eq!(engine.workspace_capacity("admin"), capacity);
    for (workspace, plan, operation, result, path, payload) in &saves {
        engine.abort_workspace(workspace, "admin").unwrap();
        assert_eq!(
            engine
                .commit_historical_save_plan(plan, "admin", *operation)
                .unwrap(),
            *result
        );
        assert_eq!(fs::read(path).unwrap(), payload.as_bytes());
    }
    assert_eq!(engine.workspace_capacity("admin"), capacity);
    assert_eq!(
        engine.state().branches[&repo.main_branch_id].head_commit_id,
        source
    );
    drop(engine);
    // Retain the simulated lost system as evidence; restore relies on disks.
    fs::rename(&root, temp.path().join("lost-system")).unwrap();
    let restored = temp.path().join("restored");
    let mut engine = Engine::restore_from_disks(&restored, &disks).unwrap();
    assert_eq!(engine.workspace_capacity("admin"), capacity);
    assert_eq!(
        engine.workspace(&active.workspace_id).unwrap().state,
        WorkspaceState::Open
    );
    for (workspace, plan, operation, result, path, payload) in saves {
        assert_eq!(
            engine.workspace(&workspace).unwrap().state,
            WorkspaceState::Aborted
        );
        assert_eq!(
            engine
                .commit_historical_save_plan(&plan, "admin", operation)
                .unwrap(),
            result
        );
        assert_eq!(
            engine
                .commit_historical_save_plan(&plan, "foreign", operation)
                .unwrap_err()
                .code(),
            ErrorCode::IdempotencyKeyReused
        );
        assert_eq!(fs::read(path).unwrap(), payload.as_bytes());
        let new_id = OperationId::generate();
        assert_eq!(
            engine
                .commit_historical_save_plan(&plan, "admin", new_id)
                .unwrap_err()
                .code(),
            ErrorCode::PlanClosed
        );
        assert!(engine.operation_owner(new_id).unwrap().is_none());
    }
    assert_eq!(engine.state().sequence, sequence);
    assert_eq!(engine.state().branches.len(), branches);
    assert_eq!(engine.state().commits.len(), commits);
    assert_eq!(
        engine.state().branches[&repo.main_branch_id].head_commit_id,
        source
    );
}

#[test]
fn cold_historical_manifests_still_exhaust_metadata_bytes_without_blocking_original_replay() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("system");
    let mut config = Config::development([
        temp.path().join("d1"),
        temp.path().join("d2"),
        temp.path().join("d3"),
    ]);
    config.max_retained_workspace_metadata_bytes = 4096;
    config.max_retained_workspace_metadata_bytes_per_principal = 4096;
    let mut engine = Engine::initialize(&root, config).unwrap();
    let repo = engine
        .create_repository(
            &RepositoryName::parse("historical").unwrap(),
            "P01",
            1,
            "admin",
            OperationId::generate(),
        )
        .unwrap();
    let source = engine.state().branches[&repo.main_branch_id]
        .head_commit_id
        .clone();
    let workspace = engine
        .create_historical_workspace(&repo.main_branch_id, &source, "admin")
        .unwrap();
    let staged = engine
        .stage_bytes(&workspace.workspace_id, "file", b"retained bytes")
        .unwrap();
    let plan = engine
        .create_historical_save_plan(
            &repo.history_tree_id,
            HistoricalSaveOptions {
                source_commit_id: source.clone(),
                name: "byte-bound save".into(),
                workspace_id: Some(workspace.workspace_id.clone()),
                primary_slot_id: None,
                target_copies: None,
            },
            "admin",
        )
        .unwrap();
    let operation = OperationId::generate();
    let result = engine
        .commit_historical_save_plan(&plan.plan_id, "admin", operation)
        .unwrap();
    // Fill the actual persisted byte allowance with cold empty historical
    // records. Resident records stay at zero throughout this exhaustion.
    loop {
        match engine.create_historical_workspace(&repo.main_branch_id, &source, "admin") {
            Ok(empty) => {
                engine
                    .abort_workspace(&empty.workspace_id, "admin")
                    .unwrap();
                assert_eq!(engine.workspace_capacity("admin").principal.records.used, 0);
                assert!(engine.workspace_capacity("admin").principal.cold_records < 16);
            }
            Err(error) => {
                assert_eq!(error.code(), ErrorCode::ResourceLimit);
                assert!(error.message().contains("metadata bytes"));
                break;
            }
        }
    }
    let capacity = engine.workspace_capacity("admin");
    assert!(capacity.principal.cold_records > 1);
    assert!(capacity.principal.metadata_bytes.used <= 4096);
    let sequence = engine.state().sequence;
    drop(engine);
    let mut engine = Engine::open(&root).unwrap();
    assert_eq!(engine.workspace_capacity("admin"), capacity);
    assert_eq!(
        engine
            .create_historical_workspace(&repo.main_branch_id, &source, "admin")
            .unwrap_err()
            .code(),
        ErrorCode::ResourceLimit
    );
    assert_eq!(
        engine
            .commit_historical_save_plan(&plan.plan_id, "admin", operation)
            .unwrap(),
        result
    );
    engine
        .abort_workspace(&workspace.workspace_id, "admin")
        .unwrap();
    assert_eq!(
        engine
            .workspace_capacity("admin")
            .principal
            .metadata_bytes
            .used,
        capacity.principal.metadata_bytes.used
    );
    assert_eq!(
        engine.workspace_capacity("admin").principal.cold_records,
        capacity.principal.cold_records
    );
    assert_eq!(fs::read(staged.source_path).unwrap(), b"retained bytes");
    assert_eq!(engine.state().sequence, sequence);
}
