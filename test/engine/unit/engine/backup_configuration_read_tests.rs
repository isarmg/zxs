use super::*;
use crate::{BackupConfigurationRequest, CommitRequest, Config, Error, ErrorCode};
use std::{cell::Cell, collections::BTreeMap};

fn pool() -> (tempfile::TempDir, Engine, storage_contract::Repository) {
    let root = tempfile::tempdir().unwrap();
    let config = Config::development([
        root.path().join("p"),
        root.path().join("b"),
        root.path().join("c"),
    ]);
    let mut engine = Engine::initialize(&root.path().join("system"), config).unwrap();
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("captured backup").unwrap(),
            "P01",
            3,
            "owner",
            OperationId::generate(),
        )
        .unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "owner")
        .unwrap();
    for path in ["a.txt", "b.txt", "c.txt"] {
        engine
            .stage_bytes(&workspace.workspace_id, path, b"independent full body")
            .unwrap();
    }
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: OperationId::generate(),
            principal: "owner".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "capture sources".into(),
            request_digest: None,
        })
        .unwrap();
    (root, engine, repo)
}

fn configure(engine: &mut Engine, repository: &str, generation: u64, prefix: &str) {
    engine
        .configure_backups(BackupConfigurationRequest {
            repository_id: repository,
            path: "",
            target_copies: 3,
            backup_prefixes: &BTreeMap::from([
                ("B01".into(), format!("{prefix}/b")),
                ("C01".into(), format!("{prefix}/c")),
            ]),
            expected_generation: generation,
            principal: "owner",
            operation_id: OperationId::generate(),
        })
        .unwrap();
}

#[test]
fn backup_view_retains_exact_state_config_resources_and_old_policy_after_replacement() {
    let (_root, mut engine, repo) = pool();
    configure(&mut engine, &repo.repository_id, 0, "old");
    let expected_root = engine
        .backup_configuration(&repo.repository_id, "")
        .unwrap();
    let expected_file = engine
        .backup_configuration(&repo.repository_id, "a.txt")
        .unwrap();
    let view = engine.backup_configuration_read_view();
    assert!(Arc::ptr_eq(&view.view.state, &engine.state));
    assert!(Arc::ptr_eq(&view.view.resources, &engine.disk_resources));
    assert!(Arc::ptr_eq(&view.view.caches, &engine.segment_caches));
    assert!(view.config.same_admission(&engine.config));
    assert_eq!(
        crate::canonical_json(&view.view.members).unwrap(),
        crate::canonical_json(&engine.members()).unwrap(),
    );
    configure(&mut engine, &repo.repository_id, 1, "new");
    assert_ne!(
        engine
            .backup_configuration(&repo.repository_id, "")
            .unwrap(),
        expected_root
    );
    assert_ne!(
        engine
            .backup_configuration(&repo.repository_id, "a.txt")
            .unwrap(),
        expected_file
    );
    let (_replacement_root, replacement, _) = pool();
    engine = replacement;
    assert!(!view.config.same_admission(&engine.config));
    assert!(!Arc::ptr_eq(&view.view.resources, &engine.disk_resources));
    let observed = Cell::new(0);
    let observe = || {
        observed.set(observed.get() + 1);
        Ok(())
    };
    assert_eq!(
        view.read(&repo.repository_id, "", &observe).unwrap(),
        expected_root
    );
    assert_eq!(
        view.read(&repo.repository_id, "a.txt", &observe).unwrap(),
        expected_file
    );
    assert!(observed.get() > 1);
}

#[test]
fn warm_backup_reads_can_cancel_at_every_observed_scan_boundary_and_retry_cleanly() {
    let (_root, mut engine, repo) = pool();
    configure(&mut engine, &repo.repository_id, 0, "backup");
    let view = engine.backup_configuration_read_view();
    let visited = Cell::new(0);
    let observe = || {
        visited.set(visited.get() + 1);
        Ok(())
    };
    // Warm the same captured namespace cache before measuring checkpoint positions.
    let expected = view.read(&repo.repository_id, "", &observe).unwrap();
    visited.set(0);
    assert_eq!(
        view.read(&repo.repository_id, "", &observe).unwrap(),
        expected
    );
    let total = visited.get();
    assert!(total > 3);
    for stop_at in 1..=total {
        let step = Cell::new(0);
        let stop = || {
            step.set(step.get() + 1);
            if step.get() == stop_at {
                Err(Error::new(
                    ErrorCode::RequestTimeout,
                    "cancel captured backup",
                ))
            } else {
                Ok(())
            }
        };
        let error = view.read(&repo.repository_id, "", &stop).unwrap_err();
        assert_eq!(error.code(), ErrorCode::RequestTimeout);
        assert_eq!(step.get(), stop_at);
    }
    assert_eq!(
        view.read(&repo.repository_id, "", &observe).unwrap(),
        expected
    );
}

#[test]
fn backup_projection_keeps_path_repository_and_namespace_failure_order() {
    let (_root, engine, repo) = pool();
    let reads = Cell::new(0);
    let source = crate::namespace::LoaderSource(|_: &storage_contract::NamespaceReference| {
        reads.set(reads.get() + 1);
        Err(Error::new(
            ErrorCode::ObjectCorrupt,
            "captured namespace failure",
        ))
    });
    let read = |repository: &str, path: &str| {
        crate::protection::backup_configuration(
            &engine.state,
            &engine.config,
            repository,
            path,
            &source,
            None,
        )
    };
    let invalid = Error::from(name_policy::PortablePath::parse("../escape").unwrap_err());
    assert_eq!(
        read("missing", "../escape").unwrap_err().code(),
        invalid.code()
    );
    assert_eq!(reads.get(), 0);
    assert_eq!(
        read("missing", "").unwrap_err().code(),
        ErrorCode::RepositoryNotFound
    );
    assert_eq!(reads.get(), 0);
    for path in ["", "missing.txt", "a.txt"] {
        let error = read(&repo.repository_id, path).unwrap_err();
        assert_eq!(error.code(), ErrorCode::ObjectCorrupt);
        assert_eq!(error.message(), "captured namespace failure");
    }
    assert_eq!(reads.get(), 3);
}
