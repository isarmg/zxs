//! Actual native publication, physical file-container and cold-replay tests.
use super::*;

#[test]
fn recursive_follow_replay_binds_request_and_rejects_omission_or_extra_paths() {
    let mut fixture = ready();
    let unrelated = save(
        &mut fixture.engine,
        &fixture.repository,
        "other/untouched",
        b"not selected",
    );
    let capture = fixture.engine.recursive_backup_capture().unwrap();
    configure(&mut fixture, "docs", 3, 0, &capture, op()).unwrap();
    let previous = Arc::clone(&fixture.engine.state);
    let view = Arc::clone(fixture.engine.current_namespaces.as_ref().unwrap());
    let change =
        NamespaceOperation::parse(NamespaceOperationKind::Rename, "docs", Some("renamed")).unwrap();
    let receipt = fixture
        .engine
        .file_namespace(&fixture.repository.repository_id, change, "admin", op())
        .unwrap();
    let next = fixture.engine.state.as_ref();
    view.advance(&previous, next).unwrap();

    let mut omitted = next.clone();
    omitted
        .operations
        .get_mut(&receipt.operation_id)
        .unwrap()
        .result
        .as_object_mut()
        .unwrap()
        .remove("recursive_follow");
    assert!(view.advance(&previous, &omitted).is_err());

    // Recreate the old native envelope consistently, leaving only the illicit
    // new backup paths. Resetting policies isolates the path-downgrade gate.
    let mut downgraded = next.clone();
    downgraded.protection_policies = previous.protection_policies.clone();
    let result = downgraded
        .operations
        .get_mut(&receipt.operation_id)
        .unwrap();
    let request = result.result["namespace_request"].clone();
    result.kind = "file.namespace".into();
    result.request_digest = Engine::native_request_digest(&result.kind, "admin", &request).unwrap();
    let body = result.result.as_object_mut().unwrap();
    body.remove("recursive_follow");
    body.remove("recursive_directory_follow");
    body.remove("namespace_request");
    downgraded.events.last_mut().unwrap().kind = "file.namespace".into();
    assert!(
        view.advance(&previous, &downgraded).is_err(),
        "a complete old envelope cannot smuggle new recursive path writes"
    );

    let mut old_semantics = downgraded.clone();
    old_semantics.file_backup_paths = previous.file_backup_paths.clone();
    view.advance(&previous, &old_semantics).unwrap();

    let mut redirected = next.clone();
    let result = redirected
        .operations
        .get_mut(&receipt.operation_id)
        .unwrap();
    result.result["namespace_request"] = json!({
        "repository_id": fixture.repository.repository_id,
        "change": NamespaceOperation::parse(NamespaceOperationKind::Rename, "other", Some("different")).unwrap(),
    });
    result.request_digest =
        Engine::native_request_digest(&result.kind, "admin", &result.result["namespace_request"])
            .unwrap();
    assert!(
        view.advance(&previous, &redirected).is_err(),
        "a self-consistent request hash cannot authorize a different rename"
    );

    let mut extra = next.clone();
    extra.file_backup_paths.insert(
        unrelated.file_id,
        BTreeMap::from([("B01".into(), "smuggled/path".into())]),
    );
    assert!(view.advance(&previous, &extra).is_err());

    let mut wrong_source = next.clone();
    wrong_source
        .operations
        .get_mut(&receipt.operation_id)
        .unwrap()
        .result["recursive_follow"]["paths"][&fixture.first.file_id]["source_operation_id"] =
        json!(op());
    assert!(view.advance(&previous, &wrong_source).is_err());
}

fn configure(
    fixture: &mut Fixture,
    path: &str,
    copies: u32,
    generation: u64,
    capture: &RecursiveBackupCapture,
    operation_id: OperationId,
) -> Result<OperationResult> {
    let scope = fixture
        .engine
        .backup_configuration(&fixture.repository.repository_id, path)?
        .scope;
    fixture
        .engine
        .configure_recursive_backups(RecursiveBackupConfigurationRequest {
            repository_id: &fixture.repository.repository_id,
            path,
            expected_scope_id: &scope.id,
            target_copies: copies,
            expected_generation: generation,
            capture,
            principal: "admin",
            operation_id,
        })
}

fn ready() -> Fixture {
    let mut fixture = Fixture::new();
    fixture.exit(3, MembershipExitAction::Deregister);
    fixture
}

#[test]
fn recursive_backup_writes_actual_three_level_paths_and_cold_reopens() {
    let mut fixture = ready();
    let capture = fixture.engine.recursive_backup_capture().unwrap();
    let result = configure(&mut fixture, "docs", 3, 0, &capture, op()).unwrap();
    assert_eq!(result.kind, "backup.configure-recursive");
    assert_eq!(result.commit_state, CommitState::Published);
    assert_eq!(result.protection_at_publish, ProtectionAtPublish::TargetMet);
    let expected = BTreeMap::from([
        (
            "B01".to_owned(),
            "disk-0/live backup/docs/report".to_owned(),
        ),
        (
            "C01".to_owned(),
            "disk-1/disk-0/live backup/docs/report".to_owned(),
        ),
    ]);
    assert_eq!(
        fixture.engine.state().file_backup_paths[&fixture.first.file_id],
        expected
    );
    for (entry, bytes) in [
        (&fixture.first, b"first retained body".as_slice()),
        (&fixture.latest, b"latest retained body".as_slice()),
    ] {
        let version = &fixture.engine.state().versions[&entry.version_id];
        for disk in fixture.config.disks.iter().take(3).skip(1) {
            let directory =
                crate::disk::version_dir(disk, &version.history_tree_id, &version.version_id);
            let actual = crate::container_storage::content_path(&directory, version).unwrap();
            let target = disk
                .root
                .join("histories")
                .join(&version.history_tree_id)
                .join("file-containers")
                .join(&version.file_id)
                .join("namespace")
                .join(&expected[&disk.slot_id])
                .join("versions")
                .join(&version.storage_name);
            assert_eq!(actual, target);
            assert_eq!(fs::read(&actual).unwrap(), bytes);
        }
    }
    let repository_id = fixture.repository.repository_id.clone();
    let scope_id = result.result["scope"]["id"].as_str().unwrap().to_owned();
    let system = fixture.temp.path().join("system");
    drop(fixture.engine);
    let mut reopened = Engine::open(&system).unwrap();
    assert_eq!(reopened.operation(result.operation_id).unwrap(), result);
    assert_eq!(
        reopened.state().file_backup_paths[&fixture.first.file_id],
        expected
    );
    let retry = reopened
        .configure_recursive_backups(RecursiveBackupConfigurationRequest {
            repository_id: &repository_id,
            path: "docs",
            expected_scope_id: &scope_id,
            target_copies: 3,
            expected_generation: 0,
            capture: &capture,
            principal: "admin",
            operation_id: result.operation_id,
        })
        .unwrap();
    assert_eq!(retry, result);
}

#[test]
fn recursive_file_scope_keeps_all_primary_ancestors() {
    let mut fixture = ready();
    let capture = fixture.engine.recursive_backup_capture().unwrap();
    configure(&mut fixture, "docs/report", 3, 0, &capture, op()).unwrap();
    let paths = &fixture.engine.state().file_backup_paths[&fixture.first.file_id];
    assert_eq!(paths["B01"], "disk-0/live backup/docs/report");
    assert_eq!(paths["C01"], "disk-1/disk-0/live backup/docs/report");
    assert!(!paths["C01"].starts_with("disk-2/"));
}

#[test]
fn recursive_retry_reuses_capture_and_rejects_changed_idempotent_input() {
    let mut fixture = ready();
    let capture = fixture.engine.recursive_backup_capture().unwrap();
    let result = configure(&mut fixture, "docs", 2, 0, &capture, op()).unwrap();
    let fresh = fixture.engine.recursive_backup_capture().unwrap();
    assert_ne!(capture, fresh);
    assert_eq!(
        configure(&mut fixture, "docs", 2, 0, &capture, result.operation_id).unwrap(),
        result
    );
    assert_eq!(
        configure(&mut fixture, "docs", 2, 0, &fresh, result.operation_id)
            .unwrap_err()
            .code(),
        ErrorCode::IdempotencyKeyReused
    );
    assert_eq!(
        configure(&mut fixture, "docs", 2, 1, &capture, op())
            .unwrap_err()
            .code(),
        ErrorCode::ScopeChanged
    );
}

#[test]
fn stale_or_forged_recursive_capture_is_rejected_before_publication_io() {
    let mut fixture = ready();
    let capture = fixture.engine.recursive_backup_capture().unwrap();
    let original = json_codec::to_value(&capture).unwrap();
    let before_state = canonical_json(fixture.engine.state()).unwrap();
    let before_files = fixture
        .config
        .disks
        .iter()
        .map(|disk| files(&disk.root))
        .collect::<Vec<_>>();
    let changes: [fn(&mut json_codec::Value); 6] = [
        |value| value["source_sequence"] = json!("0"),
        |value| value["disks"]["P01"]["name"] = json!("renamed-primary"),
        |value| value["disks"]["B01"]["name"] = json!("renamed-backup"),
        |value| value["disks"]["P01"]["identity"]["binding_generation"] = json!("99"),
        |value| value["disks"]["C01"]["upstream_slot_id"] = json!("P01"),
        |value| {
            value["disks"].as_object_mut().unwrap().remove("B01");
        },
    ];
    for change in changes {
        let mut value = original.clone();
        change(&mut value);
        let forged = json_codec::from_value(value).unwrap();
        assert_eq!(
            configure(&mut fixture, "docs", 3, 0, &forged, op())
                .unwrap_err()
                .code(),
            ErrorCode::ScopeChanged
        );
    }
    assert_eq!(
        canonical_json(fixture.engine.state()).unwrap(),
        before_state
    );
    for (disk, expected) in fixture.config.disks.iter().zip(before_files) {
        assert_eq!(files(&disk.root), expected);
    }
}

#[test]
fn recursive_receipt_and_path_updates_are_strictly_replayed_live_and_cold() {
    let mut fixture = ready();
    let view = Arc::clone(fixture.engine.current_namespaces.as_ref().unwrap());
    let previous = Arc::clone(&fixture.engine.state);
    let capture = fixture.engine.recursive_backup_capture().unwrap();
    let result = configure(&mut fixture, "docs", 3, 0, &capture, op()).unwrap();
    view.advance(&previous, fixture.engine.state()).unwrap();
    for field in ["name", "upstream_slot_id"] {
        let mut forged = fixture.engine.state().clone();
        forged
            .operations
            .get_mut(&result.operation_id)
            .unwrap()
            .result["backup_request"]["capture"]["disks"]["B01"][field] = json!("forged");
        rejected(&view, &previous, &forged, field);
    }
    let mut flattened = fixture.engine.state().clone();
    flattened
        .file_backup_paths
        .get_mut(&fixture.first.file_id)
        .unwrap()
        .insert("C01".into(), "disk-0/live backup/docs/report".into());
    rejected(
        &view,
        &previous,
        &flattened,
        "higher tier cannot flatten its direct source",
    );
    let mut changed_kind = fixture.engine.state().clone();
    changed_kind
        .operations
        .get_mut(&result.operation_id)
        .unwrap()
        .kind = "backup.configure".into();
    rejected(
        &view,
        &previous,
        &changed_kind,
        "recursive request is not a prefix request",
    );

    let mut injected_deletion = fixture.engine.state().clone();
    injected_deletion.file_deletions.insert(
        fixture.first.file_id.clone(),
        FileDeletionNode {
            node_id: "fdel_00000000000000000000000000000000".into(),
            repository_id: fixture.repository.repository_id.clone(),
            history_tree_id: fixture.repository.history_tree_id.clone(),
            file_id: fixture.first.file_id.clone(),
            path: name_policy::PortablePath::parse("docs/report").unwrap(),
            branch_heads: BTreeMap::new(),
            parent_node_ids: Vec::new(),
            operation_id: result.operation_id,
            author: "admin".into(),
            sequence: injected_deletion.sequence,
            created_at: 0,
        },
    );
    assert!(!super::super::unchanged_state(
        &previous,
        &injected_deletion
    ));
    rejected(
        &view,
        &previous,
        &injected_deletion,
        "recursive backup cannot write the deletion table",
    );
}

#[test]
fn empty_recursive_scope_publishes_a_directory_namespace_without_fake_file_versions() {
    let mut fixture = ready();
    fixture
        .engine
        .file_namespace(
            &fixture.repository.repository_id,
            NamespaceOperation::parse(NamespaceOperationKind::Mkdir, "empty", None).unwrap(),
            "admin",
            op(),
        )
        .unwrap();
    let capture = fixture.engine.recursive_backup_capture().unwrap();
    let versions = fixture.engine.state().versions.clone();
    let result = configure(&mut fixture, "empty", 3, 0, &capture, op()).unwrap();
    assert_eq!(result.commit_state, CommitState::Published);
    assert_eq!(
        result.protection_at_publish,
        ProtectionAtPublish::NotApplicable
    );
    assert_eq!(fixture.engine.state().versions, versions);
    let directories = fixture.engine.published_backup_directories("C01").unwrap();
    assert_eq!(directories.len(), 1);
    assert_eq!(
        directories[0].path().as_str(),
        "disk-1/disk-0/live backup/empty"
    );
}

#[test]
fn recursive_backup_does_not_reactivate_a_stopped_direct_source() {
    let mut fixture = ready();
    let stopped = fixture.exit(1, MembershipExitAction::PlannedOffline);
    let capture = fixture.engine.recursive_backup_capture().unwrap();
    let before = canonical_json(fixture.engine.state()).unwrap();
    assert_eq!(
        configure(&mut fixture, "docs", 3, 0, &capture, op())
            .unwrap_err()
            .code(),
        ErrorCode::PlacementInvalid
    );
    assert_eq!(canonical_json(fixture.engine.state()).unwrap(), before);
    assert_eq!(files(&fixture.config.disks[1].root), stopped);
}

#[test]
fn recursive_directory_rows_are_part_of_exact_live_and_cold_replay() {
    let mut fixture = ready();
    fixture
        .engine
        .file_namespace(
            &fixture.repository.repository_id,
            NamespaceOperation::parse(NamespaceOperationKind::Mkdir, "empty", None).unwrap(),
            "admin",
            op(),
        )
        .unwrap();
    let view = Arc::clone(fixture.engine.current_namespaces.as_ref().unwrap());
    let previous = Arc::clone(&fixture.engine.state);
    let capture = fixture.engine.recursive_backup_capture().unwrap();
    let result = configure(&mut fixture, "empty", 3, 0, &capture, op()).unwrap();
    view.advance(&previous, fixture.engine.state()).unwrap();
    for field in ["path", "entry_id"] {
        let mut forged = fixture.engine.state().clone();
        forged
            .operations
            .get_mut(&result.operation_id)
            .unwrap()
            .result["directory_namespace"]["replicas"]["C01"]["directories"][0][field] =
            json!("forged");
        rejected(&view, &previous, &forged, field);
    }
    let mut missing = fixture.engine.state().clone();
    missing
        .operations
        .get_mut(&result.operation_id)
        .unwrap()
        .result
        .as_object_mut()
        .unwrap()
        .remove("directory_namespace");
    rejected(&view, &previous, &missing, "missing directory evidence");
    let mut rebound = fixture.engine.state().clone();
    rebound
        .operations
        .get_mut(&result.operation_id)
        .unwrap()
        .result["directory_namespace"]["replicas"]["C01"]["identity"]["binding_generation"] =
        json!("99");
    rejected(&view, &previous, &rebound, "directory target identity");
}

#[test]
fn empty_follow_replay_authenticates_exact_snapshot_and_preserves_legacy_semantics() {
    let mut fixture = ready();
    for path in ["empty", "empty/child"] {
        fixture
            .engine
            .file_namespace(
                &fixture.repository.repository_id,
                NamespaceOperation::parse(NamespaceOperationKind::Mkdir, path, None).unwrap(),
                "admin",
                op(),
            )
            .unwrap();
    }
    let capture = fixture.engine.recursive_backup_capture().unwrap();
    let configured = configure(&mut fixture, "empty", 3, 0, &capture, op()).unwrap();
    let previous = Arc::clone(&fixture.engine.state);
    let view = Arc::clone(fixture.engine.current_namespaces.as_ref().unwrap());
    let receipt = fixture
        .engine
        .file_namespace(
            &fixture.repository.repository_id,
            NamespaceOperation::parse(NamespaceOperationKind::Rename, "empty", Some("renamed"))
                .unwrap(),
            "admin",
            op(),
        )
        .unwrap();
    let next = fixture.engine.state.as_ref();
    view.advance(&previous, next).unwrap();
    let policy = configured.result["policy"]["policy_id"].as_str().unwrap();
    for attack in [
        "missing",
        "source",
        "identity",
        "path",
        "omitted_row",
        "extra_row",
        "binding",
        "old_kind",
    ] {
        let mut invalid = next.clone();
        let result = invalid.operations.get_mut(&receipt.operation_id).unwrap();
        match attack {
            "missing" => {
                result
                    .result
                    .as_object_mut()
                    .unwrap()
                    .remove("recursive_directory_follow");
            }
            "source" => {
                result.result["recursive_directory_follow"]["policies"][policy]["source_operation_id"] =
                    json!(op())
            }
            "identity" => {
                result.result["recursive_directory_follow"]["policies"][policy]["namespace"]["replicas"]
                    ["B01"]["directories"][0]["entry_id"] = json!(new_id("entry").unwrap())
            }
            "path" => {
                result.result["recursive_directory_follow"]["policies"][policy]["namespace"]["replicas"]
                    ["B01"]["directories"][0]["path"] = json!("wrong/root")
            }
            "omitted_row" => {
                result.result["recursive_directory_follow"]["policies"][policy]["namespace"]["replicas"]["B01"]["directories"].as_array_mut().unwrap().pop();
            }
            "extra_row" => {
                let rows = result.result["recursive_directory_follow"]["policies"][policy]["namespace"]["replicas"]["B01"]["directories"].as_array_mut().unwrap();
                rows.push(rows[0].clone());
            }
            "binding" => {
                result.result["recursive_directory_follow"]["policies"][policy]["namespace"]["replicas"]
                    ["B01"]["identity"]["binding_generation"] = json!("999")
            }
            "old_kind" => {
                result.kind = "file.namespace.recursive".into();
                result.request_digest = Engine::native_request_digest(
                    &result.kind,
                    "admin",
                    &result.result["namespace_request"],
                )
                .unwrap();
                invalid.events.last_mut().unwrap().kind = "file.namespace.recursive".into();
            }
            _ => unreachable!(),
        }
        assert!(
            view.advance(&previous, &invalid).is_err(),
            "attack {attack}"
        );
    }
    // The previously published file-follow protocol also ignores empty snapshots.
    let mut old_recursive = next.clone();
    old_recursive.protection_policies = previous.protection_policies.clone();
    old_recursive.file_backup_paths = previous.file_backup_paths.clone();
    let result = old_recursive
        .operations
        .get_mut(&receipt.operation_id)
        .unwrap();
    result.kind = "file.namespace.recursive".into();
    result.request_digest =
        Engine::native_request_digest(&result.kind, "admin", &result.result["namespace_request"])
            .unwrap();
    result.result["recursive_follow"] =
        json!({"format": "recursive-namespace-follow", "policies": {}, "paths": {}});
    result
        .result
        .as_object_mut()
        .unwrap()
        .remove("recursive_directory_follow");
    old_recursive.events.last_mut().unwrap().kind = "file.namespace.recursive".into();
    view.advance(&previous, &old_recursive).unwrap();

    // Old native receipts are not reinterpreted as new directory writes. Their
    // unchanged snapshot remains current; a forged new marker is never accepted.
    let mut legacy = next.clone();
    legacy.protection_policies = previous.protection_policies.clone();
    legacy.file_backup_paths = previous.file_backup_paths.clone();
    let result = legacy.operations.get_mut(&receipt.operation_id).unwrap();
    result.kind = "file.namespace".into();
    result.request_digest =
        Engine::native_request_digest(&result.kind, "admin", &result.result["namespace_request"])
            .unwrap();
    for field in [
        "recursive_follow",
        "recursive_directory_follow",
        "namespace_request",
    ] {
        result.result.as_object_mut().unwrap().remove(field);
    }
    legacy.events.last_mut().unwrap().kind = "file.namespace".into();
    view.advance(&previous, &legacy).unwrap();
    assert_eq!(legacy.operations[&configured.operation_id], configured);
}

#[test]
fn routed_follow_replay_rejects_missing_required_paths_extra_paths_and_old_kind() {
    let temp = tempfile::Builder::new()
        .permissions(std::os::unix::fs::PermissionsExt::from_mode(0o700))
        .tempdir()
        .unwrap();
    let mut config =
        Config::development((0..7).map(|i| temp.path().join(format!("disk-{i}")))).unwrap();
    for (index, level, upstream) in [
        (3, 1, None),
        (4, 2, Some("D04")),
        (5, 3, Some("D05")),
        (6, 1, None),
    ] {
        config.disks[index].level = level;
        config.disks[index].upstream_slot_id = upstream.map(str::to_owned);
        config.disks[index].metadata_member = false;
    }
    let mut engine = Engine::initialize(&temp.path().join("system"), config.clone()).unwrap();
    let repository = engine
        .create_repository(
            &RepositoryName::parse("routes").unwrap(),
            "P01",
            1,
            "admin",
            op(),
        )
        .unwrap();
    engine
        .begin_disk_exit(
            &config.disks[6].disk_id,
            MembershipExitAction::Deregister,
            "admin",
            op(),
        )
        .unwrap();
    let archive = engine.build_managed_membership_control_archive().unwrap();
    engine
        .enroll_pending_members(&archive.archive_path, &archive.root_hash, Vec::new())
        .unwrap();
    engine.resume_membership_joint_decision().unwrap();
    engine.activate_pending_disk_exit().unwrap();
    engine
        .update_placement_rule(PlacementRuleUpdate {
            rule_id: &new_id("rule").unwrap(),
            repository_id: &repository.repository_id,
            path_prefix: "root/second",
            primary_slot_id: "D04",
            expected_generation: 0,
            principal: "admin",
            operation_id: op(),
        })
        .unwrap();
    save(&mut engine, &repository, "root/seed", b"seed");
    let capture = engine.recursive_backup_capture().unwrap();
    let scope = engine
        .backup_configuration(&repository.repository_id, "root")
        .unwrap()
        .scope;
    let configured = engine
        .configure_recursive_backups(RecursiveBackupConfigurationRequest {
            repository_id: &repository.repository_id,
            path: "root",
            expected_scope_id: &scope.id,
            target_copies: 3,
            expected_generation: 0,
            capture: &capture,
            principal: "admin",
            operation_id: op(),
        })
        .unwrap();
    let file = save(&mut engine, &repository, "root/inherited", b"new");
    let previous = Arc::clone(&engine.state);
    let view = Arc::clone(engine.current_namespaces.as_ref().unwrap());
    let change = NamespaceOperation::parse(
        NamespaceOperationKind::Rename,
        "root/inherited",
        Some("root/moved"),
    )
    .unwrap();
    let receipt = engine
        .file_namespace(&repository.repository_id, change, "admin", op())
        .unwrap();
    assert_eq!(receipt.kind, "file.namespace.recursive-routes");
    let next = engine.state.as_ref();
    view.advance(&previous, next).unwrap();
    for attack in ["missing", "extra", "source", "old_kind"] {
        let mut invalid = next.clone();
        let result = invalid.operations.get_mut(&receipt.operation_id).unwrap();
        match attack {
            "missing" => {
                invalid
                    .file_backup_paths
                    .get_mut(&file.file_id)
                    .unwrap()
                    .remove("B01");
                result.result["recursive_follow"]["paths"][&file.file_id]["backup_paths"]
                    .as_object_mut()
                    .unwrap()
                    .remove("B01");
            }
            "extra" => {
                invalid
                    .file_backup_paths
                    .get_mut(&file.file_id)
                    .unwrap()
                    .insert("D05".into(), "disk-3/routes/root/moved".into());
                result.result["recursive_follow"]["paths"][&file.file_id]["backup_paths"]["D05"] =
                    json!("disk-3/routes/root/moved");
            }
            "source" => {
                result.result["recursive_follow"]["paths"][&file.file_id]["source_operation_id"] =
                    json!(op())
            }
            "old_kind" => {
                result.kind = "file.namespace.recursive".into();
                result
                    .result
                    .as_object_mut()
                    .unwrap()
                    .remove("recursive_directory_follow");
                result.request_digest = Engine::native_request_digest(
                    &result.kind,
                    "admin",
                    &result.result["namespace_request"],
                )
                .unwrap();
                invalid.events.last_mut().unwrap().kind = result.kind.clone();
            }
            _ => unreachable!(),
        }
        assert!(
            view.advance(&previous, &invalid).is_err(),
            "attack {attack}"
        );
    }
    assert_eq!(next.operations[&configured.operation_id], configured);
}
