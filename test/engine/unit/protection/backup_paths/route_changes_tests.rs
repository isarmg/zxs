use super::*;

#[path = "descendant_routes_tests.rs"]
mod descendant_routes_tests;

fn initialize_routes(root: &Path) -> (Engine, Repository) {
    let mut config = Config::development([root.join("p"), root.join("b"), root.join("c")]);
    for (slot, level, upstream) in [("P02", 1, None), ("B02", 2, Some("P02"))] {
        config.disks.push(DiskConfig {
            slot_id: slot.into(),
            disk_id: new_id("disk"),
            root: root.join(slot),
            level,
            upstream_slot_id: upstream.map(str::to_owned),
            metadata_member: false,
            binding_generation: 1,
            capacity_bytes: None,
        });
    }
    let mut engine = Engine::initialize(&root.join("state"), config).unwrap();
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("shorter route").unwrap(),
            "P01",
            1,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    (engine, repo)
}

fn moved_to_shorter_route(root: &Path) -> (Engine, Repository, Entry) {
    let (mut engine, repo) = initialize_routes(root);
    let file = save(&mut engine, &repo, "source.txt", b"original");
    engine
        .configure_backups(BackupConfigurationRequest {
            repository_id: &repo.repository_id,
            path: "source.txt",
            target_copies: 3,
            backup_prefixes: &prefixes(&[("B01", "first"), ("C01", "cold")]),
            expected_generation: 0,
            principal: "admin",
            operation_id: storage_contract::OperationId::generate(),
        })
        .unwrap();
    engine
        .update_placement_rule(PlacementRuleUpdate {
            rule_id: &new_id("rule"),
            repository_id: &repo.repository_id,
            path_prefix: "moved.txt",
            primary_slot_id: "P02",
            expected_generation: 0,
            principal: "admin",
            operation_id: storage_contract::OperationId::generate(),
        })
        .unwrap();
    rename(&mut engine, &repo, "source.txt", "moved.txt");
    (engine, repo, file)
}

#[test]
fn moved_file_configuration_remains_readable_when_current_chain_is_shorter() {
    let temp = tempfile::tempdir().unwrap();
    let (engine, repo, _) = moved_to_shorter_route(temp.path());
    let before = canonical_json(engine.state()).unwrap();
    let read = engine
        .backup_configuration(&repo.repository_id, "moved.txt")
        .unwrap();
    assert_eq!(read.target_copies, 3);
    assert_eq!(read.max_copies, 2);
    assert_eq!(json_codec::json!(read.route_slot_ids), json!([]));
    assert_eq!(
        json_codec::json!(read.available_route_slot_ids),
        json!(["P02", "B02"])
    );
    assert_eq!(
        json_codec::json!(read.backup_routes),
        json!([["P01", "B01", "C01"], ["P02", "B02"]])
    );
    assert_eq!(canonical_json(engine.state()).unwrap(), before);
}

fn child_with_changed_primary(root: &Path) -> (Engine, Repository, Entry) {
    let (mut engine, repo) = initialize_routes(root);
    let file = save(&mut engine, &repo, "docs/a.txt", b"original");
    engine
        .update_placement_rule(PlacementRuleUpdate {
            rule_id: &new_id("rule"),
            repository_id: &repo.repository_id,
            path_prefix: "docs/a.txt",
            primary_slot_id: "P02",
            expected_generation: 0,
            principal: "admin",
            operation_id: storage_contract::OperationId::generate(),
        })
        .unwrap();
    (engine, repo, file)
}

#[test]
fn directory_configuration_includes_child_future_placement_chain() {
    let temp = tempfile::tempdir().unwrap();
    let (engine, repo, _) = child_with_changed_primary(temp.path());
    let read = engine
        .backup_configuration(&repo.repository_id, "docs")
        .unwrap();
    assert_eq!(
        json_codec::json!(read.backup_routes),
        json!([["P01", "B01", "C01"], ["P02", "B02"]])
    );
    assert_eq!(read.max_copies, 2);
    assert_eq!(
        json_codec::json!(read.backup_slot_ids),
        json!(["B01", "B02", "C01"])
    );
}

#[test]
fn directory_configuration_requires_child_future_placement_prefixes() {
    let temp = tempfile::tempdir().unwrap();
    let (mut engine, repo, file) = child_with_changed_primary(temp.path());
    let before = canonical_json(engine.state()).unwrap();
    let failure = engine
        .configure_backups(BackupConfigurationRequest {
            repository_id: &repo.repository_id,
            path: "docs",
            target_copies: 2,
            backup_prefixes: &prefixes(&[("B01", "first")]),
            expected_generation: 0,
            principal: "admin",
            operation_id: storage_contract::OperationId::generate(),
        })
        .unwrap_err();
    assert_eq!(
        failure.code(),
        storage_contract::ErrorCode::BackupPrefixRequired
    );
    assert_eq!(canonical_json(engine.state()).unwrap(), before);
    engine
        .configure_backups(BackupConfigurationRequest {
            repository_id: &repo.repository_id,
            path: "docs",
            target_copies: 2,
            backup_prefixes: &prefixes(&[("B01", "first"), ("B02", "second")]),
            expected_generation: 0,
            principal: "admin",
            operation_id: storage_contract::OperationId::generate(),
        })
        .unwrap();
    assert_eq!(
        engine.state.placements[&file.version_id].route_slot_ids,
        vec!["P01", "B01"]
    );
    let written = save(&mut engine, &repo, "docs/a.txt", b"new primary");
    assert_eq!(
        engine.state.placements[&written.version_id].route_slot_ids,
        vec!["P02", "B02"]
    );
    assert_eq!(
        engine.state.file_backup_paths[&file.file_id]["B02"],
        "second/a.txt"
    );
}

#[test]
fn directory_routes_exclude_files_with_independent_backup_settings() {
    let temp = tempfile::tempdir().unwrap();
    let (mut engine, repo, file) = child_with_changed_primary(temp.path());
    engine
        .configure_backups(BackupConfigurationRequest {
            repository_id: &repo.repository_id,
            path: "docs/a.txt",
            target_copies: 1,
            backup_prefixes: &BTreeMap::new(),
            expected_generation: 0,
            principal: "admin",
            operation_id: storage_contract::OperationId::generate(),
        })
        .unwrap();
    let read = engine
        .backup_configuration(&repo.repository_id, "docs")
        .unwrap();
    assert_eq!(
        json_codec::json!(read.backup_routes),
        json!([["P01", "B01", "C01"]])
    );
    assert_eq!(read.max_copies, 3);
    assert_eq!(read.file_count, 0);
    engine
        .configure_backups(BackupConfigurationRequest {
            repository_id: &repo.repository_id,
            path: "docs",
            target_copies: 3,
            backup_prefixes: &prefixes(&[("B01", "first"), ("C01", "cold")]),
            expected_generation: 0,
            principal: "admin",
            operation_id: storage_contract::OperationId::generate(),
        })
        .unwrap();
    assert_eq!(engine.state.placements[&file.version_id].target_copies, 1);
    let written = save(&mut engine, &repo, "docs/a.txt", b"independent");
    assert_eq!(
        engine.state.placements[&written.version_id].route_slot_ids,
        vec!["P02"]
    );
}

#[test]
fn moved_file_can_reduce_copy_count_to_fit_its_current_chain() {
    let temp = tempfile::tempdir().unwrap();
    let (mut engine, repo, file) = moved_to_shorter_route(temp.path());
    let before = canonical_json(engine.state()).unwrap();
    for (copies, values, code) in [
        (
            3,
            prefixes(&[("B01", "first"), ("B02", "second"), ("C01", "cold")]),
            storage_contract::ErrorCode::RouteIncomplete,
        ),
        (
            2,
            prefixes(&[("B02", "second")]),
            storage_contract::ErrorCode::BackupPrefixRequired,
        ),
        (
            2,
            prefixes(&[("B01", "first")]),
            storage_contract::ErrorCode::BackupPrefixRequired,
        ),
    ] {
        assert_eq!(
            engine
                .configure_backups(BackupConfigurationRequest {
                    repository_id: &repo.repository_id,
                    path: "moved.txt",
                    target_copies: copies,
                    backup_prefixes: &values,
                    expected_generation: 1,
                    principal: "admin",
                    operation_id: storage_contract::OperationId::generate(),
                })
                .unwrap_err()
                .code(),
            code
        );
        assert_eq!(canonical_json(engine.state()).unwrap(), before);
    }
    let values = prefixes(&[("B01", "first"), ("B02", "second")]);
    let operation = storage_contract::OperationId::generate();
    let request = BackupConfigurationRequest {
        repository_id: &repo.repository_id,
        path: "moved.txt",
        target_copies: 2,
        backup_prefixes: &values,
        expected_generation: 1,
        principal: "admin",
        operation_id: operation,
    };
    let configured = engine.configure_backups(request).unwrap();
    let before = canonical_json(engine.state()).unwrap();
    assert_eq!(
        canonical_json(&engine.configure_backups(request).unwrap()).unwrap(),
        canonical_json(&configured).unwrap()
    );
    assert_eq!(canonical_json(engine.state()).unwrap(), before);
    assert_eq!(engine.state.placements[&file.version_id].target_copies, 2);
    let written = save(&mut engine, &repo, "moved.txt", b"after move");
    assert_eq!(written.file_id, file.file_id);
    assert_eq!(
        engine.state.placements[&written.version_id].route_slot_ids,
        vec!["P02", "B02"]
    );
    assert_eq!(
        engine.state.file_backup_paths[&file.file_id]["B02"],
        "second/moved.txt"
    );
    drop(engine);
    let mut engine = Engine::open(&temp.path().join("state")).unwrap();
    let written = save(&mut engine, &repo, "moved.txt", b"after reopen");
    assert_eq!(
        engine.state.placements[&written.version_id].route_slot_ids,
        vec!["P02", "B02"]
    );
}

#[test]
fn placement_rule_can_repair_a_moved_files_incomplete_current_route() {
    let temp = tempfile::tempdir().unwrap();
    let (mut engine, repo, _) = moved_to_shorter_route(temp.path());
    let rule = engine
        .state
        .placement_rules
        .values()
        .next()
        .unwrap()
        .clone();
    engine
        .update_placement_rule(PlacementRuleUpdate {
            rule_id: &rule.rule_id,
            repository_id: &repo.repository_id,
            path_prefix: "moved.txt",
            primary_slot_id: "P01",
            expected_generation: 1,
            principal: "admin",
            operation_id: storage_contract::OperationId::generate(),
        })
        .unwrap();
    let written = save(&mut engine, &repo, "moved.txt", b"repaired primary");
    assert_eq!(
        engine.state.placements[&written.version_id].route_slot_ids,
        vec!["P01", "B01", "C01"]
    );
}

#[test]
fn path_policy_can_reduce_unsatisfiable_count_after_primary_rule_change() {
    let temp = tempfile::tempdir().unwrap();
    let (mut engine, repo) = initialize_routes(temp.path());
    let policy = new_id("policy");
    engine
        .update_protection_policy(ProtectionPolicyUpdate {
            policy_id: &policy,
            repository_id: &repo.repository_id,
            path_prefix: "docs",
            target_copies: 3,
            expected_generation: 0,
            principal: "admin",
            operation_id: storage_contract::OperationId::generate(),
        })
        .unwrap();
    engine
        .update_placement_rule(PlacementRuleUpdate {
            rule_id: &new_id("rule"),
            repository_id: &repo.repository_id,
            path_prefix: "",
            primary_slot_id: "P02",
            expected_generation: 0,
            principal: "admin",
            operation_id: storage_contract::OperationId::generate(),
        })
        .unwrap();
    engine
        .update_protection_policy(ProtectionPolicyUpdate {
            policy_id: &policy,
            repository_id: &repo.repository_id,
            path_prefix: "docs",
            target_copies: 2,
            expected_generation: 1,
            principal: "admin",
            operation_id: storage_contract::OperationId::generate(),
        })
        .unwrap();
    let written = save(&mut engine, &repo, "docs/new.txt", b"repaired count");
    assert_eq!(
        engine.state.placements[&written.version_id].route_slot_ids,
        vec!["P02", "B02"]
    );
}
