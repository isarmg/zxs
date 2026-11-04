//! Placement consumers use the verified-view fixture, not simulated lifecycle I/O.
use super::*;
use std::os::unix::fs::MetadataExt;

fn placement_request<'a>(
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

fn project_slot(engine: &mut Engine, slot: &str, disable: bool) {
    let base = verified(&engine.prepare_disk_registry_base().unwrap());
    let target = base
        .historical_topology()
        .find(|disk| disk.identity.slot_id == slot)
        .unwrap()
        .identity
        .clone();
    let source = FrozenDetachSource::capture(engine, &base, &target).unwrap();
    let mask = source.prepare_mask().unwrap();
    let mask_hash = mask.hash().to_owned();
    let objects = mask
        .objects()
        .map(|(kind, hash, bytes)| ((kind.into(), hash.into()), bytes.to_vec()))
        .collect();
    let mask = source
        .verify_mask(mask.hash(), |kind, hash| read(&objects, kind, hash))
        .unwrap();
    let successor = verified(
        &base
            .prepare_exit(RegistryExitRequest {
                target,
                operation_id: op(),
                principal: "admin".into(),
                request_digest: "3".repeat(64),
                intent_hash: "4".repeat(64),
                effect: if disable {
                    RegistryExitEffect::Disable
                } else {
                    RegistryExitEffect::Detach {
                        occurrence_mask_hash: mask_hash,
                    }
                },
            })
            .unwrap(),
    );
    let prepared = if disable {
        source.prepare_disabled_namespaces(&successor).unwrap()
    } else {
        mask.prepare_current_namespaces(&successor).unwrap()
    };
    engine.current_namespaces = Some(Arc::new(prepared.view));
    engine.workspace_namespace_baselines.clear();
}

fn path_rule(engine: &mut Engine, repository: &str, path: &str, primary: &str) {
    engine
        .update_placement_rule(PlacementRuleUpdate {
            rule_id: &new_id("rule").unwrap(),
            repository_id: repository,
            path_prefix: path,
            primary_slot_id: primary,
            expected_generation: 0,
            principal: "admin",
            operation_id: op(),
        })
        .unwrap();
}

fn future_policy(engine: &mut Engine, repository: &str, path: &str, copies: u32) {
    engine
        .update_protection_policy(ProtectionPolicyUpdate {
            policy_id: &new_id("policy").unwrap(),
            repository_id: repository,
            path_prefix: path,
            target_copies: copies,
            expected_generation: 0,
            principal: "admin",
            operation_id: op(),
        })
        .unwrap();
}

#[test]
fn current_placement_detached_targets_disappear_and_disabled_targets_are_explicitly_blocked() {
    for disable in [false, true] {
        let mut fixture = Fixture::new(true);
        let primary = fixture
            .engine
            .config
            .disks
            .iter()
            .find(|disk| disk.slot_id == "P01")
            .unwrap();
        let retained = fixture
            .engine
            .config
            .disks
            .iter()
            .find(|disk| disk.slot_id == "D04")
            .unwrap();
        assert_eq!(
            std::fs::metadata(&primary.root).unwrap().dev(),
            std::fs::metadata(&retained.root).unwrap().dev()
        );
        assert_ne!(primary.root, retained.root);
        let snapshot = fixture
            .engine
            .placement_configuration(&fixture.repo.repository_id, "dir")
            .unwrap();
        let original_operation = op();
        let original_result = fixture
            .engine
            .configure_placement(placement_request(&snapshot, "P01", original_operation))
            .unwrap();
        let policies = canonical_json(&fixture.engine.state.protection_policies).unwrap();
        let placements = canonical_json(&fixture.engine.state.placements).unwrap();
        fixture.project(disable);

        let replay = fixture
            .engine
            .configure_placement(placement_request(&snapshot, "P01", original_operation))
            .unwrap();
        assert_eq!(replay.operation_id, original_result.operation_id);
        assert_eq!(replay.sequence, original_result.sequence);
        assert_eq!(replay.result, original_result.result);
        let current = fixture
            .engine
            .placement_configuration(&fixture.repo.repository_id, "dir")
            .unwrap();
        assert_eq!(current.primary_slot_id, "P01");
        let removed = current
            .targets
            .iter()
            .find(|target| target.slot_id == "P01");
        if disable {
            assert_eq!(
                removed.unwrap().unavailable_reason,
                Some(PrimaryPlacementBlock::RouteUnavailable)
            );
        } else {
            assert!(removed.is_none());
        }
        assert_eq!(
            current
                .targets
                .iter()
                .find(|target| target.slot_id == "D04")
                .unwrap()
                .unavailable_reason,
            None
        );
        let before = canonical_json(fixture.engine.state()).unwrap();
        assert_eq!(
            fixture
                .engine
                .configure_placement(placement_request(&current, "P01", op()))
                .unwrap_err()
                .code(),
            if disable {
                ErrorCode::PlacementInvalid
            } else {
                ErrorCode::SlotNotFound
            }
        );
        assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);

        let mut wrong_rule = current.clone();
        wrong_rule.rule_id = Some(new_id("rule").unwrap());
        assert_eq!(
            fixture
                .engine
                .configure_placement(placement_request(&wrong_rule, "D04", op()))
                .unwrap_err()
                .code(),
            ErrorCode::GenerationMismatch
        );
        assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
        let operation = op();
        let result = fixture
            .engine
            .configure_placement(placement_request(&current, "D04", operation))
            .unwrap();
        assert_eq!(
            fixture
                .engine
                .configure_placement(placement_request(&current, "D04", operation))
                .unwrap()
                .result,
            result.result
        );
        assert_eq!(
            fixture
                .engine
                .configure_placement(placement_request(&current, "D04", op()))
                .unwrap_err()
                .code(),
            ErrorCode::GenerationMismatch
        );
        let changed = fixture
            .engine
            .placement_configuration(&fixture.repo.repository_id, "dir")
            .unwrap();
        assert_eq!(changed.rule_id, current.rule_id);
        assert_eq!(changed.generation, current.generation + 1);
        assert_eq!(changed.primary_slot_id, "D04");
        assert_eq!(
            canonical_json(&fixture.engine.state.protection_policies).unwrap(),
            policies
        );
        assert_eq!(
            canonical_json(&fixture.engine.state.placements).unwrap(),
            placements
        );
    }
}

#[test]
fn current_placement_hidden_scope_and_replaced_directory_cannot_reuse_old_scope_identity() {
    let mut fixture = Fixture::new(true);
    let removed_repo = fixture
        .engine
        .create_repository(
            &RepositoryName::parse("removed owner").unwrap(),
            "P01",
            1,
            "admin",
            op(),
        )
        .unwrap();
    save(
        &mut fixture.engine,
        &removed_repo.main_branch_id,
        "hidden/file",
        b"history only",
    );
    let hidden = fixture
        .engine
        .placement_configuration(&removed_repo.repository_id, "hidden")
        .unwrap();
    let old_head = fixture.engine.state.branches[&removed_repo.main_branch_id]
        .head_commit_id
        .clone();
    let retained = fixture
        .engine
        .placement_configuration(&fixture.repo.repository_id, "dir")
        .unwrap();
    fixture.project(false);
    let before = canonical_json(fixture.engine.state()).unwrap();
    for path in ["", "hidden"] {
        assert_eq!(
            fixture
                .engine
                .placement_configuration(&removed_repo.repository_id, path)
                .unwrap_err()
                .code(),
            ErrorCode::BranchNotActive
        );
    }
    assert_eq!(
        fixture
            .engine
            .configure_placement(placement_request(&hidden, "D04", op()))
            .unwrap_err()
            .code(),
        ErrorCode::BranchNotActive
    );
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
    assert!(
        fixture
            .engine
            .namespace_tree_at(&removed_repo.main_branch_id, &old_head)
            .unwrap()
            .get("hidden")
            .is_some()
    );

    fixture
        .engine
        .file_namespace(
            &fixture.repo.repository_id,
            NamespaceOperation::parse(NamespaceOperationKind::DeleteTree, "dir", None).unwrap(),
            "admin",
            op(),
        )
        .unwrap();
    save(
        &mut fixture.engine,
        &fixture.repo.main_branch_id,
        "dir/new",
        b"fresh directory",
    );
    let current = fixture
        .engine
        .placement_configuration(&fixture.repo.repository_id, "dir")
        .unwrap();
    assert_ne!(current.scope.id, retained.scope.id);
    let before = canonical_json(fixture.engine.state()).unwrap();
    assert_eq!(
        fixture
            .engine
            .configure_placement(placement_request(&retained, "D04", op()))
            .unwrap_err()
            .code(),
        ErrorCode::ScopeChanged
    );
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
    fixture
        .engine
        .configure_placement(placement_request(&current, "D04", op()))
        .unwrap();
}

#[test]
fn current_placement_hidden_file_bindings_do_not_constrain_or_attach_to_fresh_same_name_files() {
    let mut fixture = Fixture::new(true);
    let rule = fixture
        .engine
        .state
        .placement_rules
        .values()
        .next()
        .unwrap()
        .clone();
    // Preserve the old version's P01 placement but remove its independent future
    // path route, so the parent directory genuinely controls the replacement.
    fixture
        .engine
        .update_placement_rule(PlacementRuleUpdate {
            rule_id: &rule.rule_id,
            repository_id: &fixture.repo.repository_id,
            path_prefix: "independent",
            primary_slot_id: "D04",
            expected_generation: rule.generation,
            principal: "admin",
            operation_id: op(),
        })
        .unwrap();
    let policy_id = new_id("policy").unwrap();
    let policy = ProtectionPolicy {
        policy_id: policy_id.clone(),
        repository_id: fixture.repo.repository_id.clone(),
        path_prefix: fixture.removed.path().into(),
        target_copies: 3,
        inheritable: true,
        scope_directory_id: None,
        scope_file_id: Some(fixture.removed.file_id.clone()),
        backup_prefixes: BTreeMap::from([
            ("B01".into(), "backup".into()),
            ("C01".into(), "cold".into()),
        ]),
        generation: 1,
    };
    // A semantic future-binding fixture: content placement remains one copy.
    // No lifecycle activation, replica I/O or public backup-config flow is claimed.
    fixture
        .engine
        .mutate(
            "protection-policy.update",
            "admin",
            op(),
            json_codec::json!({"fixture":"future file binding"}),
            |_, state, _| {
                state
                    .file_backup_policies
                    .insert(fixture.removed.file_id.clone(), policy_id.clone());
                state.protection_policies.insert(policy_id.clone(), policy);
                Ok(json_codec::json!({}))
            },
        )
        .unwrap();
    assert_eq!(
        fixture
            .engine
            .placement_configuration(&fixture.repo.repository_id, "dir")
            .unwrap()
            .required_copies,
        3
    );
    let policies = canonical_json(&fixture.engine.state.protection_policies).unwrap();
    fixture.project(false);
    assert!(
        fixture
            .engine
            .state
            .file_locations
            .contains_key(&fixture.removed.file_id)
    );
    let current = fixture
        .engine
        .placement_configuration(&fixture.repo.repository_id, "dir")
        .unwrap();
    assert_eq!(current.required_copies, 1);
    assert_eq!(
        fixture
            .engine
            .new_file_placement(&fixture.repo.repository_id, "dir/a")
            .unwrap(),
        ("D04".into(), 1)
    );
    fixture
        .engine
        .configure_placement(placement_request(&current, "D04", op()))
        .unwrap();
    save(
        &mut fixture.engine,
        &fixture.repo.main_branch_id,
        "dir/a",
        b"new identity",
    );
    let entry = fixture
        .engine
        .namespace_tree(&fixture.repo.main_branch_id)
        .unwrap()
        .get("dir/a")
        .unwrap();
    assert_ne!(entry.file_id, fixture.removed.file_id);
    assert_eq!(
        fixture.engine.state.placements[&entry.version_id].target_copies,
        1
    );
    assert!(
        !fixture
            .engine
            .state
            .file_backup_policies
            .contains_key(&entry.file_id)
    );
    assert_eq!(
        fixture.engine.state.file_backup_policies[&fixture.removed.file_id],
        policy_id
    );
    assert_eq!(
        canonical_json(&fixture.engine.state.protection_policies).unwrap(),
        policies
    );
}

#[test]
fn current_placement_future_path_filters_and_unavailable_backup_routes_remain_effective() {
    for disable in [false, true] {
        let mut fixture = Fixture::new(true);
        path_rule(
            &mut fixture.engine,
            &fixture.repo.repository_id,
            "dir",
            "P01",
        );
        future_policy(
            &mut fixture.engine,
            &fixture.repo.repository_id,
            "dir/future",
            2,
        );
        project_slot(&mut fixture.engine, "B01", disable);
        let directory = fixture
            .engine
            .placement_configuration(&fixture.repo.repository_id, "dir")
            .unwrap();
        assert_eq!(directory.required_copies, 2);
        assert_eq!(
            directory
                .targets
                .iter()
                .find(|target| target.slot_id == "P01")
                .unwrap()
                .unavailable_reason,
            Some(PrimaryPlacementBlock::RouteUnavailable)
        );
        let before = canonical_json(fixture.engine.state()).unwrap();
        assert_eq!(
            fixture
                .engine
                .configure_placement(placement_request(&directory, "P01", op()))
                .unwrap_err()
                .code(),
            ErrorCode::PlacementInvalid
        );
        assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
        // The independent dir rule keeps its policy from blocking unrelated
        // one-copy scope edits. The removed backup is not in this selected route.
        let root = fixture
            .engine
            .placement_configuration(&fixture.repo.repository_id, "")
            .unwrap();
        assert_eq!(root.required_copies, 1);
        assert_eq!(
            root.targets
                .iter()
                .find(|target| target.slot_id == "P01")
                .unwrap()
                .unavailable_reason,
            None
        );
        fixture
            .engine
            .configure_placement(placement_request(&root, "P01", op()))
            .unwrap();
    }
}

#[test]
fn current_placement_disabled_namespace_owner_keeps_registration_without_edit_authority() {
    let mut fixture = Fixture::new(true);
    let snapshot = fixture
        .engine
        .placement_configuration(&fixture.repo.repository_id, "dir")
        .unwrap();
    project_slot(&mut fixture.engine, "D04", true);
    assert!(
        fixture
            .engine
            .registered_disks()
            .any(|disk| disk.slot_id == "D04"
                && disk.registration_mode == RegistrationMode::Disabled)
    );
    assert!(
        fixture
            .engine
            .namespace_tree(&fixture.repo.main_branch_id)
            .unwrap()
            .get("dir")
            .is_some()
    );
    let before = canonical_json(fixture.engine.state()).unwrap();
    for path in ["", "dir"] {
        assert_eq!(
            fixture
                .engine
                .placement_configuration(&fixture.repo.repository_id, path)
                .unwrap_err()
                .code(),
            ErrorCode::BranchNotActive
        );
    }
    assert_eq!(
        fixture
            .engine
            .configure_placement(placement_request(&snapshot, "P01", op()))
            .unwrap_err()
            .code(),
        ErrorCode::BranchNotActive
    );
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
}
