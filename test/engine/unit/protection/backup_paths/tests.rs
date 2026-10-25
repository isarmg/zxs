use super::*;
use std::{fs, os::unix::fs::MetadataExt, path::Path};
#[path = "precedence_tests.rs"]
mod precedence_tests;
#[path = "retained_tests.rs"]
mod retained_tests;
#[path = "route_changes_tests.rs"]
mod route_changes_tests;

fn initialize(root: &Path, copies: u32, capacity: Option<u64>) -> (Engine, Repository) {
    let mut config = Config::development([root.join("p"), root.join("b"), root.join("c")]);
    for disk in &mut config.disks {
        disk.capacity_bytes = capacity;
    }
    let mut engine = Engine::initialize(&root.join("state"), config).unwrap();
    let repo = engine
        .create_repository("备份规则", "P01", copies, "admin", &new_id("op"))
        .unwrap();
    (engine, repo)
}
fn save(engine: &mut Engine, repo: &Repository, path: &str, bytes: &[u8]) -> Entry {
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, path, bytes)
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: new_id("op"),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "backup fixture".into(),
            request_digest: None,
        })
        .unwrap();
    engine
        .list_tree(&repo.main_branch_id)
        .unwrap()
        .into_iter()
        .find(|entry| entry.path == path)
        .unwrap()
}
fn prefixes(values: &[(&str, &str)]) -> BTreeMap<String, String> {
    values
        .iter()
        .map(|(slot, path)| (slot.to_string(), path.to_string()))
        .collect()
}
fn container(engine: &Engine, repo: &Repository, file: &str, slot: &str) -> std::path::PathBuf {
    let disk = engine
        .config
        .disks
        .iter()
        .find(|disk| disk.slot_id == slot)
        .unwrap();
    let location = engine
        .state
        .file_backup_paths
        .get(file)
        .and_then(|paths| paths.get(slot))
        .or_else(|| engine.state.file_locations.get(file))
        .unwrap();
    disk.root
        .join("histories")
        .join(&repo.history_tree_id)
        .join("file-containers")
        .join(file)
        .join("namespace")
        .join(location)
}
fn rename(engine: &mut Engine, repo: &Repository, path: &str, destination: &str) {
    engine
        .file_namespace(
            &repo.repository_id,
            NamespaceOperation {
                kind: "rename".into(),
                path: path.into(),
                destination: Some(destination.into()),
            },
            "admin",
            &new_id("op"),
        )
        .unwrap();
}

#[test]
fn backup_paths_backfill_all_versions_and_survive_primary_moves_and_new_writes() {
    let temp = tempfile::tempdir().unwrap();
    let (mut engine, repo) = initialize(temp.path(), 1, None);
    let first = save(&mut engine, &repo, "documents/deep/report.txt", b"first");
    let second = save(&mut engine, &repo, "documents/deep/report.txt", b"second");
    let proofs = canonical_json(&engine.state.versions).unwrap();
    let primary = container(&engine, &repo, &first.file_id, "P01");
    let primary_inode = fs::metadata(&primary).unwrap().ino();
    let configured = engine
        .configure_backups(crate::BackupConfigurationRequest {
            repository_id: &repo.repository_id,
            path: "documents",
            target_copies: 3,
            backup_prefixes: &prefixes(&[("B01", "我的存档"), ("C01", "异地/材料")]),
            expected_generation: 0,
            principal: "admin",
            operation_id: &new_id("op"),
        })
        .unwrap();
    assert_eq!(configured.result["version_count"], "2");
    assert_eq!(canonical_json(&engine.state.versions).unwrap(), proofs);
    for version in [&first.version_id, &second.version_id] {
        assert_eq!(
            engine.state.placements[version].route_slot_ids,
            vec!["P01", "B01", "C01"]
        );
    }
    assert!(
        container(&engine, &repo, &first.file_id, "B01")
            .ends_with("namespace/我的存档/deep/report.txt")
    );
    assert!(
        container(&engine, &repo, &first.file_id, "C01")
            .ends_with("namespace/异地/材料/deep/report.txt")
    );
    let backup = container(&engine, &repo, &first.file_id, "B01");
    let backup_inode = fs::metadata(&backup).unwrap().ino();
    rename(&mut engine, &repo, "documents", "moved");
    rename(
        &mut engine,
        &repo,
        "moved/deep/report.txt",
        "moved/deep/renamed.txt",
    );
    assert_eq!(
        fs::metadata(container(&engine, &repo, &first.file_id, "P01"))
            .unwrap()
            .ino(),
        primary_inode
    );
    assert_eq!(container(&engine, &repo, &first.file_id, "B01"), backup);
    let third = save(&mut engine, &repo, "moved/deep/renamed.txt", b"third");
    assert_eq!(engine.state.placements[&third.version_id].target_copies, 3);
    assert_eq!(fs::metadata(&backup).unwrap().ino(), backup_inode);
    assert_eq!(
        engine.state.file_backup_paths[&first.file_id]["B01"],
        "我的存档/deep/report.txt"
    );
    let policy_id = engine.state.file_backup_policies[&first.file_id].clone();
    drop(engine);
    let mut engine = Engine::open(&temp.path().join("state")).unwrap();
    assert_eq!(engine.state.file_backup_policies[&first.file_id], policy_id);
    let read = engine
        .backup_configuration(&repo.repository_id, "moved/deep/renamed.txt")
        .unwrap();
    assert_eq!(read["inherited"], true);
    assert_eq!(read["generation"], "0");
    assert!(read.get("can_manage").is_none());
    engine
        .configure_backups(crate::BackupConfigurationRequest {
            repository_id: &repo.repository_id,
            path: "moved",
            target_copies: 3,
            backup_prefixes: &prefixes(&[("B01", "重新配置"), ("C01", "冷存档")]),
            expected_generation: 1,
            principal: "admin",
            operation_id: &new_id("op"),
        })
        .unwrap();
    assert!(
        container(&engine, &repo, &first.file_id, "B01")
            .ends_with("namespace/重新配置/deep/renamed.txt")
    );
    assert_eq!(
        fs::metadata(container(&engine, &repo, &first.file_id, "B01"))
            .unwrap()
            .ino(),
        backup_inode
    );
}

#[test]
fn parent_configuration_keeps_explicit_children_and_empty_directories_inherit() {
    let temp = tempfile::tempdir().unwrap();
    let (mut engine, repo) = initialize(temp.path(), 1, None);
    let child = save(&mut engine, &repo, "docs/special/a.txt", b"child");
    let other = save(&mut engine, &repo, "docs/b.txt", b"other");
    engine
        .configure_backups(crate::BackupConfigurationRequest {
            repository_id: &repo.repository_id,
            path: "docs/special",
            target_copies: 3,
            backup_prefixes: &prefixes(&[("B01", "child"), ("C01", "child-cold")]),
            expected_generation: 0,
            principal: "admin",
            operation_id: &new_id("op"),
        })
        .unwrap();
    let child_policy = engine.state.file_backup_policies[&child.file_id].clone();
    let configured = engine
        .configure_backups(crate::BackupConfigurationRequest {
            repository_id: &repo.repository_id,
            path: "docs",
            target_copies: 2,
            backup_prefixes: &prefixes(&[("B01", "parent")]),
            expected_generation: 0,
            principal: "admin",
            operation_id: &new_id("op"),
        })
        .unwrap();
    assert_eq!(configured.result["file_count"], "1");
    assert_eq!(
        engine.state.file_backup_policies[&child.file_id],
        child_policy
    );
    assert_eq!(engine.state.placements[&child.version_id].target_copies, 3);
    assert_eq!(engine.state.placements[&other.version_id].target_copies, 2);
    assert_eq!(
        engine.state.file_backup_paths[&child.file_id]["B01"],
        "child/a.txt"
    );
    let new = save(&mut engine, &repo, "docs/special/new.txt", b"new");
    assert_eq!(engine.state.placements[&new.version_id].target_copies, 3);
    assert_eq!(
        engine.state.file_backup_paths[&new.file_id]["B01"],
        "child/new.txt"
    );
    engine
        .file_namespace(
            &repo.repository_id,
            NamespaceOperation {
                kind: "mkdir".into(),
                path: "empty".into(),
                destination: None,
            },
            "admin",
            &new_id("op"),
        )
        .unwrap();
    engine
        .configure_backups(crate::BackupConfigurationRequest {
            repository_id: &repo.repository_id,
            path: "empty",
            target_copies: 2,
            backup_prefixes: &prefixes(&[("B01", "future")]),
            expected_generation: 0,
            principal: "admin",
            operation_id: &new_id("op"),
        })
        .unwrap();
    let future = save(&mut engine, &repo, "empty/new.txt", b"future");
    assert_eq!(
        engine.state.file_backup_paths[&future.file_id]["B01"],
        "future/new.txt"
    );
}

#[test]
fn prefix_changes_reuse_verified_payloads_at_capacity_and_failures_are_atomic() {
    let temp = tempfile::tempdir().unwrap();
    let (mut engine, repo) = initialize(temp.path(), 3, Some(4));
    let file = save(&mut engine, &repo, "f.txt", b"full");
    let old = container(&engine, &repo, &file.file_id, "B01");
    let inode = fs::metadata(&old).unwrap().ino();
    let config = prefixes(&[("B01", "logical"), ("C01", "cold")]);
    engine
        .configure_backups(crate::BackupConfigurationRequest {
            repository_id: &repo.repository_id,
            path: "f.txt",
            target_copies: 3,
            backup_prefixes: &config,
            expected_generation: 0,
            principal: "admin",
            operation_id: &new_id("op"),
        })
        .unwrap();
    assert_eq!(
        fs::metadata(container(&engine, &repo, &file.file_id, "B01"))
            .unwrap()
            .ino(),
        inode
    );
    let before = canonical_json(engine.state()).unwrap();
    for (copies, prefixes, generation, scope, code) in [
        (
            4,
            config.clone(),
            1,
            None,
            storage_contract::ErrorCode::RouteIncomplete,
        ),
        (
            3,
            BTreeMap::new(),
            1,
            None,
            storage_contract::ErrorCode::BackupPrefixRequired,
        ),
        (
            3,
            config.clone(),
            0,
            None,
            storage_contract::ErrorCode::GenerationMismatch,
        ),
        (
            3,
            config.clone(),
            1,
            Some("file_wrong"),
            storage_contract::ErrorCode::ScopeChanged,
        ),
    ] {
        assert_eq!(
            engine
                .configure_backups_scoped(crate::ScopedBackupConfigurationRequest {
                    repository_id: &repo.repository_id,
                    path: "f.txt",
                    expected_scope_id: scope,
                    target_copies: copies,
                    backup_prefixes: &prefixes,
                    expected_generation: generation,
                    principal: "admin",
                    operation_id: &new_id("op"),
                })
                .unwrap_err()
                .code(),
            code
        );
        assert_eq!(canonical_json(engine.state()).unwrap(), before);
    }
}

#[test]
fn backup_projection_recovers_without_copying_or_renaming_primary() {
    for fault in [
        "after_finalize",
        "after_container_names",
        "after_commit_proofs",
        "after_publication_1",
        "after_local_publications",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let (mut engine, repo) = initialize(temp.path(), 3, None);
        let entry = save(&mut engine, &repo, "source/f.txt", b"body");
        let primary = container(&engine, &repo, &entry.file_id, "P01");
        let inode = fs::metadata(container(&engine, &repo, &entry.file_id, "B01"))
            .unwrap()
            .ino();
        let config = prefixes(&[("B01", "recovery-one"), ("C01", "recovery-two")]);
        let operation = new_id("op");
        engine.inject_fault_once(fault);
        assert_eq!(
            engine
                .configure_backups(crate::BackupConfigurationRequest {
                    repository_id: &repo.repository_id,
                    path: "source",
                    target_copies: 3,
                    backup_prefixes: &config,
                    expected_generation: 0,
                    principal: "admin",
                    operation_id: &operation,
                })
                .unwrap_err()
                .code(),
            storage_contract::ErrorCode::InDoubt
        );
        drop(engine);
        let mut engine = Engine::open(&temp.path().join("state")).unwrap();
        engine
            .configure_backups(crate::BackupConfigurationRequest {
                repository_id: &repo.repository_id,
                path: "source",
                target_copies: 3,
                backup_prefixes: &config,
                expected_generation: 0,
                principal: "admin",
                operation_id: &operation,
            })
            .unwrap();
        assert_eq!(engine.state.versions.len(), 1);
        assert_eq!(container(&engine, &repo, &entry.file_id, "P01"), primary);
        assert_eq!(
            fs::metadata(container(&engine, &repo, &entry.file_id, "B01"))
                .unwrap()
                .ino(),
            inode
        );
        assert!(
            container(&engine, &repo, &entry.file_id, "B01")
                .ends_with("namespace/recovery-one/f.txt")
        );
    }
}

#[test]
fn arbitrary_fixed_route_copy_count_and_independent_prefixes_are_supported() {
    let temp = tempfile::tempdir().unwrap();
    let mut config = Config::development([
        temp.path().join("p"),
        temp.path().join("b"),
        temp.path().join("c"),
    ]);
    config.disks.push(DiskConfig {
        slot_id: "D01".into(),
        disk_id: new_id("disk"),
        root: temp.path().join("d"),
        level: 4,
        upstream_slot_id: Some("C01".into()),
        metadata_member: false,
        binding_generation: 1,
        capacity_bytes: None,
    });
    let mut engine = Engine::initialize(&temp.path().join("state"), config).unwrap();
    let repo = engine
        .create_repository("four", "P01", 1, "admin", &new_id("op"))
        .unwrap();
    let file = save(&mut engine, &repo, "a.txt", b"four copies");
    engine
        .configure_backups(crate::BackupConfigurationRequest {
            repository_id: &repo.repository_id,
            path: "a.txt",
            target_copies: 4,
            backup_prefixes: &prefixes(&[("B01", "one"), ("C01", "two"), ("D01", "three")]),
            expected_generation: 0,
            principal: "admin",
            operation_id: &new_id("op"),
        })
        .unwrap();
    assert_eq!(
        engine.state.placements[&file.version_id].route_slot_ids,
        vec!["P01", "B01", "C01", "D01"]
    );
    assert!(container(&engine, &repo, &file.file_id, "D01").ends_with("namespace/three/a.txt"));
}

#[test]
fn empty_extensions_use_sparse_current_state_and_policy_encoding() {
    let state = json_codec::to_value(State::default()).unwrap();
    for field in [
        "file_metadata_nodes",
        "file_branch_merges",
        "file_branch_aliases",
        "file_backup_paths",
        "file_backup_policies",
    ] {
        assert!(state.get(field).is_none());
    }
    let recovered: State = json_codec::from_value(state.clone()).unwrap();
    assert_eq!(
        canonical_json(&recovered).unwrap(),
        canonical_json(&state).unwrap()
    );
    let minimal = json!({"policy_id":new_id("policy"),"repository_id":new_id("repo"),"path_prefix":"docs","target_copies":2,"inheritable":true,"generation":"1"});
    let policy: ProtectionPolicy = json_codec::from_value(minimal.clone()).unwrap();
    assert_eq!(
        canonical_json(&policy).unwrap(),
        canonical_json(&minimal).unwrap()
    );
}

#[test]
fn configuration_exposes_all_historical_primary_chains_before_collecting_prefixes() {
    let temp = tempfile::tempdir().unwrap();
    let mut config = Config::development([
        temp.path().join("p"),
        temp.path().join("b"),
        temp.path().join("c"),
    ]);
    for (slot, level, upstream) in [("P02", 1, None), ("B02", 2, Some("P02"))] {
        config.disks.push(DiskConfig {
            slot_id: slot.into(),
            disk_id: new_id("disk"),
            root: temp.path().join(slot),
            level,
            upstream_slot_id: upstream.map(str::to_owned),
            metadata_member: false,
            binding_generation: 1,
            capacity_bytes: None,
        });
    }
    let mut engine = Engine::initialize(&temp.path().join("state"), config).unwrap();
    let repo = engine
        .create_repository("mixed routes", "P01", 1, "admin", &new_id("op"))
        .unwrap();
    let first = save(&mut engine, &repo, "a.txt", b"first route");
    engine
        .update_placement_rule(crate::PlacementRuleUpdate {
            rule_id: &new_id("rule"),
            repository_id: &repo.repository_id,
            path_prefix: "a.txt",
            primary_slot_id: "P02",
            expected_generation: 0,
            principal: "admin",
            operation_id: &new_id("op"),
        })
        .unwrap();
    let second = save(&mut engine, &repo, "a.txt", b"second route");
    let read = engine
        .backup_configuration(&repo.repository_id, "a.txt")
        .unwrap();
    assert_eq!(
        read["backup_routes"],
        json!([["P01", "B01", "C01"], ["P02", "B02"]])
    );
    assert_eq!(read["backup_slot_ids"], json!(["B01", "B02", "C01"]));
    assert_eq!(read["max_copies"], 2);
    let before = canonical_json(engine.state()).unwrap();
    assert_eq!(
        engine
            .configure_backups(crate::BackupConfigurationRequest {
                repository_id: &repo.repository_id,
                path: "a.txt",
                target_copies: 2,
                backup_prefixes: &prefixes(&[("B02", "second")]),
                expected_generation: 0,
                principal: "admin",
                operation_id: &new_id("op"),
            })
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::BackupPrefixRequired
    );
    assert_eq!(before, canonical_json(engine.state()).unwrap());
    engine
        .configure_backups(crate::BackupConfigurationRequest {
            repository_id: &repo.repository_id,
            path: "a.txt",
            target_copies: 2,
            backup_prefixes: &prefixes(&[("B01", "first"), ("B02", "second")]),
            expected_generation: 0,
            principal: "admin",
            operation_id: &new_id("op"),
        })
        .unwrap();
    assert_eq!(
        engine.state.placements[&first.version_id].route_slot_ids,
        vec!["P01", "B01"]
    );
    assert_eq!(
        engine.state.placements[&second.version_id].route_slot_ids,
        vec!["P02", "B02"]
    );
    assert_eq!(
        engine.state.file_backup_paths[&first.file_id]["B01"],
        "first/a.txt"
    );
    assert_eq!(
        engine.state.file_backup_paths[&first.file_id]["B02"],
        "second/a.txt"
    );
}

#[test]
fn top_level_backup_policy_overrides_root_regardless_of_policy_id_order() {
    let low = "policy_00000000000000000000000000000000";
    let high = "policy_ffffffffffffffffffffffffffffffff";
    for (root_policy, child_policy) in [(low, high), (high, low)] {
        let temp = tempfile::tempdir().unwrap();
        let (mut engine, repo) = initialize(temp.path(), 1, None);
        engine
            .file_namespace(
                &repo.repository_id,
                NamespaceOperation {
                    kind: "mkdir".into(),
                    path: "docs".into(),
                    destination: None,
                },
                "admin",
                &new_id("op"),
            )
            .unwrap();
        // Seed deterministic IDs through the public policy API, then upgrade
        // both scopes through the same configuration path used by the editor.
        for (policy_id, path, copies, backup_prefixes) in [
            (root_policy, "", 2, prefixes(&[("B01", "root")])),
            (
                child_policy,
                "docs",
                3,
                prefixes(&[("B01", "child"), ("C01", "cold")]),
            ),
        ] {
            engine
                .update_protection_policy(crate::ProtectionPolicyUpdate {
                    policy_id,
                    repository_id: &repo.repository_id,
                    path_prefix: path,
                    target_copies: copies,
                    expected_generation: 0,
                    principal: "admin",
                    operation_id: &new_id("op"),
                })
                .unwrap();
            engine
                .configure_backups(crate::BackupConfigurationRequest {
                    repository_id: &repo.repository_id,
                    path,
                    target_copies: copies,
                    backup_prefixes: &backup_prefixes,
                    expected_generation: 1,
                    principal: "admin",
                    operation_id: &new_id("op"),
                })
                .unwrap();
        }
        let child = save(&mut engine, &repo, "docs/new.txt", b"child");
        assert_eq!(engine.state.placements[&child.version_id].target_copies, 3);
        assert_eq!(
            engine.state.file_backup_policies[&child.file_id],
            child_policy
        );
        assert_eq!(
            engine.state.file_backup_paths[&child.file_id]["B01"],
            "child/new.txt"
        );
        let root = save(&mut engine, &repo, "other.txt", b"root");
        assert_eq!(engine.state.placements[&root.version_id].target_copies, 2);
        assert_eq!(
            engine.state.file_backup_policies[&root.file_id],
            root_policy
        );
        assert_eq!(
            engine.state.file_backup_paths[&root.file_id]["B01"],
            "root/other.txt"
        );
        let read = engine
            .backup_configuration(&repo.repository_id, "docs")
            .unwrap();
        assert_eq!(read["effective_policy_id"], child_policy);
        assert_eq!(read["target_copies"], 3);
        drop(engine);
        let mut engine = Engine::open(&temp.path().join("state")).unwrap();
        let after = save(&mut engine, &repo, "docs/after.txt", b"after reopen");
        assert_eq!(engine.state.placements[&after.version_id].target_copies, 3);
        assert_eq!(
            engine.state.file_backup_policies[&after.file_id],
            child_policy
        );
        assert_eq!(
            engine.state.file_backup_paths[&after.file_id]["C01"],
            "cold/after.txt"
        );
    }
}

#[test]
fn one_copy_scoped_policy_binds_new_files_and_survives_moves_and_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let (mut engine, repo) = initialize(temp.path(), 1, None);
    let before = save(
        &mut engine,
        &repo,
        "source/before.txt",
        b"before configuration",
    );
    engine
        .file_namespace(
            &repo.repository_id,
            NamespaceOperation {
                kind: "mkdir".into(),
                path: "destination".into(),
                destination: None,
            },
            "admin",
            &new_id("op"),
        )
        .unwrap();
    let configured = engine
        .configure_backups(crate::BackupConfigurationRequest {
            repository_id: &repo.repository_id,
            path: "source",
            target_copies: 1,
            backup_prefixes: &BTreeMap::new(),
            expected_generation: 0,
            principal: "admin",
            operation_id: &new_id("op"),
        })
        .unwrap();
    let policy_id = configured.result["policy"]["policy_id"].as_str().unwrap();
    engine
        .configure_backups(crate::BackupConfigurationRequest {
            repository_id: &repo.repository_id,
            path: "destination",
            target_copies: 2,
            backup_prefixes: &prefixes(&[("B01", "destination-backup")]),
            expected_generation: 0,
            principal: "admin",
            operation_id: &new_id("op"),
        })
        .unwrap();
    let after = save(
        &mut engine,
        &repo,
        "source/after.txt",
        b"after configuration",
    );
    let versions = canonical_json(&engine.state.versions).unwrap();
    rename(
        &mut engine,
        &repo,
        "source/before.txt",
        "destination/before.txt",
    );
    rename(
        &mut engine,
        &repo,
        "source/after.txt",
        "destination/after.txt",
    );
    assert_eq!(canonical_json(&engine.state.versions).unwrap(), versions);
    for original in [&before, &after] {
        let path = format!("destination/{}", original.name);
        let updated = save(&mut engine, &repo, &path, b"updated after move");
        assert_eq!(updated.file_id, original.file_id);
        assert_eq!(
            engine.state.placements[&updated.version_id].target_copies,
            1
        );
        assert_eq!(
            engine.state.file_backup_policies[&updated.file_id],
            policy_id
        );
        assert!(
            !engine
                .state
                .file_backup_paths
                .contains_key(&updated.file_id)
        );
    }
    drop(engine);
    let mut engine = Engine::open(&temp.path().join("state")).unwrap();
    let updated = save(&mut engine, &repo, "destination/after.txt", b"after reopen");
    assert_eq!(
        engine.state.placements[&updated.version_id].target_copies,
        1
    );
    let read = engine
        .backup_configuration(&repo.repository_id, "destination/after.txt")
        .unwrap();
    assert_eq!(read["effective_policy_id"], policy_id);
    assert_eq!(read["target_copies"], 1);
}

#[test]
fn path_only_protection_policy_follows_the_path_for_future_versions() {
    let temp = tempfile::tempdir().unwrap();
    let (mut engine, repo) = initialize(temp.path(), 2, None);
    engine
        .update_protection_policy(crate::ProtectionPolicyUpdate {
            policy_id: &new_id("policy"),
            repository_id: &repo.repository_id,
            path_prefix: "source",
            target_copies: 1,
            expected_generation: 0,
            principal: "admin",
            operation_id: &new_id("op"),
        })
        .unwrap();
    let original = save(&mut engine, &repo, "source/file.txt", b"first");
    assert_eq!(
        engine.state.placements[&original.version_id].target_copies,
        1
    );
    assert!(
        !engine
            .state
            .file_backup_policies
            .contains_key(&original.file_id)
    );
    rename(&mut engine, &repo, "source/file.txt", "moved.txt");
    let updated = save(&mut engine, &repo, "moved.txt", b"second");
    assert_eq!(updated.file_id, original.file_id);
    assert_eq!(
        engine.state.placements[&updated.version_id].target_copies,
        2
    );
    assert!(
        !engine
            .state
            .file_backup_policies
            .contains_key(&updated.file_id)
    );
}
