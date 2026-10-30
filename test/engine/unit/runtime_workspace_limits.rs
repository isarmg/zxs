use super::*;

fn limits(config: &Config) -> RetainedWorkspaceLimits {
    RetainedWorkspaceLimits {
        max_retained_workspaces: config.max_retained_workspaces,
        max_retained_workspaces_per_principal: config.max_retained_workspaces_per_principal,
        max_retained_workspace_metadata_bytes: config.max_retained_workspace_metadata_bytes,
        max_retained_workspace_metadata_bytes_per_principal: config
            .max_retained_workspace_metadata_bytes_per_principal,
    }
}

fn files(root: &std::path::Path) -> std::collections::BTreeMap<std::path::PathBuf, blake3::Hash> {
    let mut result = std::collections::BTreeMap::new();
    for entry in fs::read_dir(root).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            result.extend(files(&entry.path()));
        } else {
            result.insert(entry.path(), blake3::hash(&fs::read(entry.path()).unwrap()));
        }
    }
    result
}

#[test]
fn runtime_limits_validate_each_dimension_before_initialization_or_restore_writes() {
    for dimension in 0..4 {
        for zero in [false, true] {
            let mut f = Fixture::new();
            f.config.max_retained_workspaces = 3;
            f.config.max_retained_workspaces_per_principal = 4;
            f.config.max_retained_workspace_metadata_bytes = 1024;
            f.config.max_retained_workspace_metadata_bytes_per_principal = 2048;
            // Principal limits above global limits are valid, like Config itself.
            let baseline = limits(&f.config);
            let admitted = ValidatedConfig::try_from(f.config.clone()).unwrap();
            baseline.validate_for(&admitted).unwrap();
            let mut invalid = baseline;
            match dimension {
                0 => invalid.max_retained_workspaces = if zero { 0 } else { 2 },
                1 => invalid.max_retained_workspaces_per_principal = if zero { 0 } else { 3 },
                2 => invalid.max_retained_workspace_metadata_bytes = if zero { 0 } else { 1023 },
                _ => {
                    invalid.max_retained_workspace_metadata_bytes_per_principal =
                        if zero { 0 } else { 2047 }
                }
            }
            assert_eq!(invalid.validate().is_err(), zero);
            assert_eq!(
                invalid.validate_for(&admitted).unwrap_err().code(),
                storage_contract::ErrorCode::ConfigInvalid
            );
            assert_eq!(
                Engine::initialize_with_workspace_limits(&f.root, f.config.clone(), Some(invalid))
                    .err()
                    .unwrap()
                    .code(),
                storage_contract::ErrorCode::ConfigInvalid
            );
            assert!(!f.root.exists());
            assert!(f.config.disks.iter().all(|disk| !disk.root.exists()));
            drop(f.initialize());
            let before = files(f.temp.path());
            for recovery in [false, true] {
                let result = if recovery {
                    Engine::open_for_recovery_with_workspace_limits(&f.root, Some(invalid))
                } else {
                    Engine::open_with_workspace_limits(&f.root, Some(invalid))
                };
                assert_eq!(
                    result.err().unwrap().code(),
                    storage_contract::ErrorCode::ConfigInvalid
                );
                assert_eq!(files(f.temp.path()), before);
            }
            let restored = f.temp.path().join("restored");
            let disks: Vec<_> = f
                .config
                .disks
                .iter()
                .map(|disk| disk.root.clone())
                .collect();
            assert_eq!(
                Engine::restore_from_disks_with_workspace_limits(&restored, &disks, Some(invalid))
                    .err()
                    .unwrap()
                    .code(),
                storage_contract::ErrorCode::ConfigInvalid
            );
            assert!(!restored.exists());
            assert_eq!(files(f.temp.path()), before);
        }
    }
}

#[test]
fn absent_runtime_override_uses_persisted_configuration_limits() {
    for custom in [false, true] {
        let mut f = Fixture::new();
        assert_eq!(f.config.max_retained_workspaces, 4096);
        assert_eq!(f.config.max_retained_workspaces_per_principal, 1024);
        assert_eq!(
            f.config.max_retained_workspace_metadata_bytes,
            128 * 1024 * 1024
        );
        assert_eq!(
            f.config.max_retained_workspace_metadata_bytes_per_principal,
            64 * 1024 * 1024
        );
        if custom {
            f.config.max_retained_workspaces = 7;
            f.config.max_retained_workspaces_per_principal = 5;
            f.config.max_retained_workspace_metadata_bytes = 9000;
            f.config.max_retained_workspace_metadata_bytes_per_principal = 8000;
        }
        let engine =
            Engine::initialize_with_workspace_limits(&f.root, f.config.clone(), None).unwrap();
        let expected = engine.workspace_capacity("admin");
        assert_eq!(
            expected.global.records.limit,
            f.config.max_retained_workspaces as u64
        );
        assert_eq!(
            expected.principal.records.limit,
            f.config.max_retained_workspaces_per_principal as u64
        );
        assert_eq!(
            expected.global.metadata_bytes.limit,
            f.config.max_retained_workspace_metadata_bytes
        );
        assert_eq!(
            expected.principal.metadata_bytes.limit,
            f.config.max_retained_workspace_metadata_bytes_per_principal
        );
        drop(engine);
        let engine = Engine::open_with_workspace_limits(&f.root, None).unwrap();
        assert_eq!(engine.workspace_capacity("admin"), expected);
    }
}

#[test]
fn runtime_record_limits_preserve_terminal_evidence_anchors_and_restore_retry() {
    let mut f = Fixture::new();
    f.config.max_retained_workspaces = 1;
    f.config.max_retained_workspaces_per_principal = 1;
    let mut engine = f.initialize();
    let repo = repository(&mut engine, 1);
    let first = save(&mut engine, &repo.main_branch_id, "saved", b"saved payload");
    assert_eq!(engine.workspace_capacity("admin").principal.records.used, 0);
    let retained = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&retained.workspace_id, "unpublished", b"resident evidence")
        .unwrap();
    engine
        .abort_workspace(&retained.workspace_id, "admin")
        .unwrap();
    assert_eq!(
        engine
            .create_workspace(&repo.main_branch_id, "admin")
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::ResourceLimit
    );
    let config_bytes = fs::read(f.root.join("config.json")).unwrap();
    let anchors: Vec<_> = f
        .config
        .disks
        .iter()
        .flat_map(|disk| {
            [
                disk.root.join("disk.json"),
                disk.root.join("metadata/pool-membership/config.json"),
            ]
        })
        .map(|path| {
            let bytes = fs::read(&path).unwrap();
            (path, bytes)
        })
        .collect();
    let mut raised = limits(&f.config);
    raised.max_retained_workspaces = 3;
    raised.max_retained_workspaces_per_principal = 2;
    drop(engine);
    let mut engine = Engine::open_with_workspace_limits(&f.root, Some(raised)).unwrap();
    assert_eq!(fs::read(f.root.join("config.json")).unwrap(), config_bytes);
    let aborted = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&aborted.workspace_id, "retained", b"evidence")
        .unwrap();
    engine
        .abort_workspace(&aborted.workspace_id, "admin")
        .unwrap();
    let error = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap_err();
    assert!(error.to_string().contains("principal record count"));
    let third = engine
        .create_workspace(&repo.main_branch_id, "writer")
        .unwrap();
    engine
        .stage_bytes(&third.workspace_id, "another", b"another payload")
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: third.workspace_id.clone(),
            operation_id: storage_contract::OperationId::generate(),
            principal: "writer".into(),
            expected_head: third.base_head,
            fencing_token: third.fencing_token,
            message: "raised budget".into(),
            request_digest: None,
        })
        .unwrap();
    assert!(!engine.workspaces.contains_key(&third.workspace_id));
    let retained = engine
        .create_workspace(&repo.main_branch_id, "writer")
        .unwrap();
    engine
        .stage_bytes(&retained.workspace_id, "unpublished", b"writer evidence")
        .unwrap();
    engine
        .abort_workspace(&retained.workspace_id, "writer")
        .unwrap();
    let error = engine
        .create_workspace(&repo.main_branch_id, "writer")
        .unwrap_err();
    assert!(error.to_string().contains("global record count"));
    let report = engine.workspace_capacity("admin");
    assert_eq!(report.global.records.used, 3);
    assert_eq!(report.global.records.limit, 3);
    assert_eq!(report.principal.records.used, 2);
    assert!(report.automatic_retirement);
    engine.assert_workspace_budget_consistent();
    drop(engine);
    let before = files(f.temp.path());
    for recovery in [false, true] {
        let result = if recovery {
            Engine::open_for_recovery(&f.root)
        } else {
            Engine::open(&f.root)
        };
        assert_eq!(
            result.err().unwrap().code(),
            storage_contract::ErrorCode::ResourceLimit
        );
        assert_eq!(
            files(f.temp.path()),
            before,
            "omitted override must not delete or repair evidence"
        );
    }
    let engine = Engine::open_for_recovery_with_workspace_limits(&f.root, Some(raised)).unwrap();
    assert_eq!(engine.workspace_capacity("admin"), report);
    assert_eq!(engine.operation(first.operation_id).unwrap(), first);
    drop(engine);
    let before = files(f.temp.path());
    let restored = f.temp.path().join("restored");
    let disks: Vec<_> = f
        .config
        .disks
        .iter()
        .map(|disk| disk.root.clone())
        .collect();
    assert_eq!(
        Engine::restore_from_disks(&restored, &disks)
            .err()
            .unwrap()
            .code(),
        storage_contract::ErrorCode::ResourceLimit
    );
    assert!(!restored.join("config.json").exists());
    let mut insufficient = raised;
    insufficient.max_retained_workspaces_per_principal = 1;
    assert_eq!(
        Engine::restore_from_disks_with_workspace_limits(&restored, &disks, Some(insufficient))
            .err()
            .unwrap()
            .code(),
        storage_contract::ErrorCode::ResourceLimit
    );
    assert!(!restored.join("config.json").exists());
    assert!(
        fs::read_dir(&restored)
            .unwrap()
            .all(|entry| entry.unwrap().file_name() == "writer.lock")
    );
    for (path, bytes) in &before {
        assert_eq!(
            blake3::hash(&fs::read(path).unwrap()),
            *bytes,
            "failed preflight must not repair or checkpoint"
        );
    }
    let engine =
        Engine::restore_from_disks_with_workspace_limits(&restored, &disks, Some(raised)).unwrap();
    assert_eq!(engine.workspace_capacity("admin"), report);
    assert_eq!(
        engine
            .workspace(&aborted.workspace_id)
            .unwrap()
            .state
            .as_str(),
        "aborted"
    );
    assert_eq!(
        engine
            .workspace(&third.workspace_id)
            .unwrap()
            .state
            .as_str(),
        "committed"
    );
    assert_eq!(engine.operation(first.operation_id).unwrap(), first);
    assert_eq!(
        fs::read(restored.join("config.json")).unwrap(),
        config_bytes
    );
    assert_eq!(fs::read(f.root.join("config.json")).unwrap(), config_bytes);
    for (path, bytes) in anchors {
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
    engine.assert_workspace_budget_consistent();
    drop(engine);
    let engine = Engine::open_with_workspace_limits(&restored, Some(raised)).unwrap();
    assert_eq!(engine.workspace_capacity("admin"), report);
}

#[test]
fn raised_runtime_metadata_limits_still_enforce_each_finite_dimension() {
    for principal in [false, true] {
        let mut f = Fixture::new();
        if principal {
            f.config.max_retained_workspace_metadata_bytes_per_principal = 1;
        } else {
            f.config.max_retained_workspace_metadata_bytes = 1;
        }
        let mut raised = limits(&f.config);
        if principal {
            raised.max_retained_workspace_metadata_bytes_per_principal = 2048;
        } else {
            raised.max_retained_workspace_metadata_bytes = 2048;
        }
        let mut engine =
            Engine::initialize_with_workspace_limits(&f.root, f.config.clone(), Some(raised))
                .unwrap();
        let repo = repository(&mut engine, 1);
        let workspace = engine
            .create_workspace(&repo.main_branch_id, "admin")
            .unwrap();
        let mut accepted = 0;
        loop {
            let result = engine.namespace_operation(
                &workspace.workspace_id,
                NamespaceOperation::parse(
                    storage_contract::NamespaceOperationKind::Mkdir,
                    &(format!("directory-{accepted:04}")),
                    None,
                )
                .unwrap(),
            );
            match result {
                Ok(_) => {
                    accepted += 1;
                    assert!(accepted < 100);
                }
                Err(error) => {
                    assert_eq!(error.code(), storage_contract::ErrorCode::ResourceLimit);
                    assert!(error.to_string().contains(if principal {
                        "principal metadata bytes"
                    } else {
                        "global metadata bytes"
                    }));
                    break;
                }
            }
        }
        assert!(accepted > 0);
        engine
            .abort_workspace(&workspace.workspace_id, "admin")
            .unwrap();
        let report = engine.workspace_capacity("admin");
        assert!(report.global.metadata_bytes.used <= 2048);
        engine.assert_workspace_budget_consistent();
        drop(engine);
        let before = files(f.temp.path());
        assert_eq!(
            Engine::open(&f.root).err().unwrap().code(),
            storage_contract::ErrorCode::ResourceLimit
        );
        assert_eq!(files(f.temp.path()), before);
        let engine = Engine::open_with_workspace_limits(&f.root, Some(raised)).unwrap();
        assert_eq!(engine.workspace_capacity("admin"), report);
        assert_eq!(
            engine
                .workspace(&workspace.workspace_id)
                .unwrap()
                .state
                .as_str(),
            "aborted"
        );
        drop(engine);
        let before = files(f.temp.path());
        let restored = f.temp.path().join("restored");
        let disks: Vec<_> = f
            .config
            .disks
            .iter()
            .map(|disk| disk.root.clone())
            .collect();
        assert_eq!(
            Engine::restore_from_disks(&restored, &disks)
                .err()
                .unwrap()
                .code(),
            storage_contract::ErrorCode::ResourceLimit
        );
        assert!(!restored.join("config.json").exists());
        assert!(
            fs::read_dir(&restored)
                .unwrap()
                .all(|entry| entry.unwrap().file_name() == "writer.lock")
        );
        for (path, bytes) in &before {
            assert_eq!(blake3::hash(&fs::read(path).unwrap()), *bytes);
        }
        let engine =
            Engine::restore_from_disks_with_workspace_limits(&restored, &disks, Some(raised))
                .unwrap();
        assert_eq!(engine.workspace_capacity("admin"), report);
    }
}
