use super::*;

fn fixture() -> (tempfile::TempDir, Engine, Repository) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let mut config =
        Config::development([root.join("primary"), root.join("backup"), root.join("cold")])
            .unwrap();
    for (slot, level, upstream) in [("P02", 1, None), ("B02", 2, Some("P02"))] {
        config.disks.push(DiskConfig {
            slot_id: slot.into(),
            disk_id: new_id("disk").unwrap(),
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
            &RepositoryName::parse("Documents").unwrap(),
            "P01",
            1,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    save(&mut engine, &repo, "docs/a.txt", b"old");
    (temp, engine, repo)
}
fn save(engine: &mut Engine, repo: &Repository, path: &str, body: &[u8]) {
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, path, body)
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: OperationId::generate().unwrap(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "fixture".into(),
            request_digest: None,
        })
        .unwrap();
}
fn request<'a>(
    snapshot: &'a PlacementConfigurationSnapshot,
    primary: &'a str,
    operation: OperationId,
) -> PlacementConfigurationRequest<'a> {
    PlacementConfigurationRequest {
        repository_id: &snapshot.repository_id,
        path: &snapshot.scope.path,
        primary_slot_id: primary,
        expected_scope_id: &snapshot.scope.id,
        expected_rule_id: snapshot.rule_id.as_deref(),
        expected_generation: snapshot.generation,
        principal: "admin",
        operation_id: operation,
    }
}

#[test]
fn placement_configuration_reads_real_directory_and_repository_without_creating_rules() {
    let (_temp, engine, repo) = fixture();
    let before = canonical_json(engine.state()).unwrap();
    let snapshot = engine
        .placement_configuration(&repo.repository_id, "DOCS")
        .unwrap();
    assert_eq!(snapshot.scope.path, "docs");
    assert!(snapshot.scope.id.starts_with("entry_"));
    assert_eq!(snapshot.repository_name, "Documents");
    assert!(snapshot.inherited);
    assert_eq!(snapshot.rule_id, None);
    assert_eq!(snapshot.generation, 0);
    assert_eq!(snapshot.primary_slot_id, "P01");
    assert_eq!(
        snapshot
            .targets
            .iter()
            .map(|target| target.slot_id.as_str())
            .collect::<Vec<_>>(),
        ["P01", "P02"]
    );
    assert!(
        snapshot
            .targets
            .iter()
            .all(|target| target.unavailable_reason.is_none() && !target.root.is_empty())
    );
    let root = engine
        .placement_configuration(&repo.repository_id, "")
        .unwrap();
    assert_eq!(root.scope.id, repo.repository_id);
    assert!(root.scope.path.is_empty());
    assert_eq!(canonical_json(engine.state()).unwrap(), before);
    assert_eq!(
        engine
            .placement_configuration(&repo.repository_id, "docs/a.txt")
            .unwrap_err()
            .code(),
        ErrorCode::InvalidRequest
    );
}

#[test]
fn placement_configuration_changes_only_future_versions_and_replays_original_operation() {
    let (_temp, mut engine, repo) = fixture();
    let snapshot = engine
        .placement_configuration(&repo.repository_id, "docs")
        .unwrap();
    let old_placements = canonical_json(&engine.state().placements).unwrap();
    let old_versions = canonical_json(&engine.state().versions).unwrap();
    let old_policies = canonical_json(&engine.state().protection_policies).unwrap();
    let operation = OperationId::generate().unwrap();
    let result = engine
        .configure_placement(request(&snapshot, "P02", operation))
        .unwrap();
    let replay = engine
        .configure_placement(request(&snapshot, "P02", operation))
        .unwrap();
    assert_eq!(result.operation_id, replay.operation_id);
    assert_eq!(result.sequence, replay.sequence);
    assert_eq!(
        canonical_json(&engine.state().placements).unwrap(),
        old_placements
    );
    assert_eq!(
        canonical_json(&engine.state().versions).unwrap(),
        old_versions
    );
    assert_eq!(
        canonical_json(&engine.state().protection_policies).unwrap(),
        old_policies
    );
    let current = engine
        .placement_configuration(&repo.repository_id, "docs")
        .unwrap();
    assert!(current.rule_id.as_deref().unwrap().starts_with("rule_"));
    assert_eq!(current.generation, 1);
    assert!(!current.inherited);
    assert_eq!(current.primary_slot_id, "P02");
    assert_eq!(
        engine
            .configure_placement(request(&snapshot, "P01", OperationId::generate().unwrap()))
            .unwrap_err()
            .code(),
        ErrorCode::GenerationMismatch
    );
    save(&mut engine, &repo, "docs/a.txt", b"new");
    assert!(
        engine
            .state()
            .placements
            .values()
            .any(|placement| placement.primary_slot_id == "P01")
    );
    assert!(
        engine
            .state()
            .placements
            .values()
            .any(|placement| placement.primary_slot_id == "P02")
    );
}

#[test]
fn placement_configuration_reuses_legacy_exact_rule_and_fences_replaced_directory() {
    let (_temp, mut engine, repo) = fixture();
    let id = new_id("rule").unwrap();
    engine
        .update_placement_rule(PlacementRuleUpdate {
            rule_id: &id,
            repository_id: &repo.repository_id,
            path_prefix: "docs",
            primary_slot_id: "P01",
            expected_generation: 0,
            principal: "admin",
            operation_id: OperationId::generate().unwrap(),
        })
        .unwrap();
    let snapshot = engine
        .placement_configuration(&repo.repository_id, "docs")
        .unwrap();
    assert_eq!(snapshot.rule_id.as_deref(), Some(id.as_str()));
    engine
        .configure_placement(request(&snapshot, "P02", OperationId::generate().unwrap()))
        .unwrap();
    assert_eq!(engine.state().placement_rules.len(), 1);
    assert_eq!(engine.state().placement_rules[&id].generation, 2);
    let stale = engine
        .placement_configuration(&repo.repository_id, "docs")
        .unwrap();
    engine
        .file_namespace(
            &repo.repository_id,
            NamespaceOperation::parse(NamespaceOperationKind::Rename, "docs", Some("moved"))
                .unwrap(),
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    save(&mut engine, &repo, "docs/replacement.txt", b"replacement");
    assert_eq!(
        engine
            .configure_placement(request(&stale, "P01", OperationId::generate().unwrap()))
            .unwrap_err()
            .code(),
        ErrorCode::ScopeChanged
    );
}

#[test]
fn placement_configuration_checks_descendant_copies_and_preserves_independent_child_rules() {
    let (_temp, mut engine, repo) = fixture();
    save(&mut engine, &repo, "docs/child/a.txt", b"child");
    let prefixes = std::collections::BTreeMap::from([
        ("B01".into(), "backup".into()),
        ("C01".into(), "cold".into()),
    ]);
    engine
        .configure_backups(BackupConfigurationRequest {
            repository_id: &repo.repository_id,
            path: "docs/child",
            target_copies: 3,
            backup_prefixes: &prefixes,
            expected_generation: 0,
            principal: "admin",
            operation_id: OperationId::generate().unwrap(),
        })
        .unwrap();
    let blocked = engine
        .placement_configuration(&repo.repository_id, "docs")
        .unwrap();
    assert_eq!(blocked.required_copies, 3);
    assert_eq!(
        blocked
            .targets
            .iter()
            .find(|target| target.slot_id == "P02")
            .unwrap()
            .unavailable_reason,
        Some(PrimaryPlacementBlock::InsufficientCopies)
    );
    assert_eq!(
        engine
            .configure_placement(request(&blocked, "P02", OperationId::generate().unwrap()))
            .unwrap_err()
            .code(),
        ErrorCode::RouteIncomplete
    );
    let child = engine
        .placement_configuration(&repo.repository_id, "docs/child")
        .unwrap();
    engine
        .configure_placement(request(&child, "P01", OperationId::generate().unwrap()))
        .unwrap();
    let parent = engine
        .placement_configuration(&repo.repository_id, "docs")
        .unwrap();
    assert_eq!(parent.required_copies, 1);
    engine
        .configure_placement(request(&parent, "P02", OperationId::generate().unwrap()))
        .unwrap();
    assert_eq!(
        engine
            .new_file_placement(&repo.repository_id, "docs/new.txt")
            .unwrap()
            .0,
        "P02"
    );
    assert_eq!(
        engine
            .new_file_placement(&repo.repository_id, "docs/child/new.txt")
            .unwrap(),
        ("P01".into(), 3)
    );
}

#[test]
fn placement_configuration_requires_destination_backup_prefixes_before_switching_chains() {
    let (_temp, mut engine, repo) = fixture();
    let prefixes = std::collections::BTreeMap::from([("B01".into(), "documents".into())]);
    engine
        .configure_backups(BackupConfigurationRequest {
            repository_id: &repo.repository_id,
            path: "docs",
            target_copies: 2,
            backup_prefixes: &prefixes,
            expected_generation: 0,
            principal: "admin",
            operation_id: OperationId::generate().unwrap(),
        })
        .unwrap();
    let blocked = engine
        .placement_configuration(&repo.repository_id, "docs")
        .unwrap();
    assert_eq!(blocked.required_copies, 2);
    assert_eq!(
        blocked
            .targets
            .iter()
            .find(|target| target.slot_id == "P02")
            .unwrap()
            .unavailable_reason,
        Some(PrimaryPlacementBlock::MissingBackupPrefixes)
    );
    let before = canonical_json(engine.state()).unwrap();
    assert_eq!(
        engine
            .configure_placement(request(&blocked, "P02", OperationId::generate().unwrap()))
            .unwrap_err()
            .code(),
        ErrorCode::BackupPrefixRequired
    );
    assert_eq!(canonical_json(engine.state()).unwrap(), before);
    // An explicitly prepared destination prefix is accepted. Placement itself
    // never invents it or mutates the backup policy and old version placements.
    let prefixes = std::collections::BTreeMap::from([
        ("B01".into(), "documents".into()),
        ("B02".into(), "future-documents".into()),
    ]);
    engine
        .configure_backups(BackupConfigurationRequest {
            repository_id: &repo.repository_id,
            path: "docs",
            target_copies: 2,
            backup_prefixes: &prefixes,
            expected_generation: 1,
            principal: "admin",
            operation_id: OperationId::generate().unwrap(),
        })
        .unwrap();
    let ready = engine
        .placement_configuration(&repo.repository_id, "docs")
        .unwrap();
    assert_eq!(
        ready
            .targets
            .iter()
            .find(|target| target.slot_id == "P02")
            .unwrap()
            .unavailable_reason,
        None
    );
    let old_placements = canonical_json(&engine.state().placements).unwrap();
    let old_policies = canonical_json(&engine.state().protection_policies).unwrap();
    engine
        .configure_placement(request(&ready, "P02", OperationId::generate().unwrap()))
        .unwrap();
    assert_eq!(
        canonical_json(&engine.state().placements).unwrap(),
        old_placements
    );
    assert_eq!(
        canonical_json(&engine.state().protection_policies).unwrap(),
        old_policies
    );
    save(&mut engine, &repo, "docs/a.txt", b"on the new chain");
    assert!(
        engine
            .state()
            .placements
            .values()
            .any(|placement| placement.route_slot_ids == ["P02", "B02"])
    );
}
