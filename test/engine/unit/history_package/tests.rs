use super::*;
use std::{
    io::Read,
    os::unix::fs::{MetadataExt, PermissionsExt},
};

#[test]
fn retained_policy_manifests_require_bindings_and_unique_current_scopes() {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = initialize(temp.path());
    let repo = engine
        .create_repository("retained policies", "P01", 2, "alice", &new_id("op"))
        .unwrap();
    save(&mut engine, &repo.main_branch_id, b"body");
    let file = engine
        .list_tree(&repo.main_branch_id)
        .unwrap()
        .into_iter()
        .find(|entry| entry.kind == "file")
        .unwrap();
    let configured = engine
        .configure_backups(BackupConfigurationRequest {
            repository_id: &repo.repository_id,
            path: "file.txt",
            target_copies: 2,
            backup_prefixes: &BTreeMap::from([("B01".into(), "backup".into())]),
            expected_generation: 0,
            principal: "alice",
            operation_id: &new_id("op"),
        })
        .unwrap();
    let current = configured.result["policy"]["policy_id"].as_str().unwrap();
    let mut manifest = HistoryManifest::from_state(engine.state(), &repo.history_tree_id).unwrap();
    let mut retained = manifest.protection_policies[current].clone();
    retained.policy_id = new_id("policy");
    retained.inheritable = false;
    let id = retained.policy_id.clone();
    manifest.protection_policies.insert(id.clone(), retained);
    manifest
        .file_backup_policies
        .insert(file.file_id.clone(), id.clone());
    manifest.validate().unwrap();
    let mut unbound = manifest.clone();
    unbound.file_backup_policies.remove(&file.file_id);
    assert!(unbound.validate().is_err());
    let mut ambiguous = manifest.clone();
    ambiguous
        .protection_policies
        .get_mut(&id)
        .unwrap()
        .inheritable = true;
    assert!(ambiguous.validate().is_err());
    let mut dangling = manifest.clone();
    dangling
        .protection_policies
        .get_mut(&id)
        .unwrap()
        .scope_file_id = Some(new_id("file"));
    assert!(dangling.validate().is_err());
    manifest
        .protection_policies
        .get_mut(&id)
        .unwrap()
        .scope_file_id = None;
    manifest.validate().unwrap();
}

fn initialize(root: &Path) -> Engine {
    Engine::initialize(
        &root.join("state"),
        Config::development([root.join("p"), root.join("b"), root.join("c")]),
    )
    .unwrap()
}
fn save(engine: &mut Engine, branch: &str, bytes: &[u8]) -> String {
    save_path(engine, branch, "file.txt", bytes)
}
fn save_path(engine: &mut Engine, branch: &str, path: &str, bytes: &[u8]) -> String {
    let workspace = engine.create_workspace(branch, "alice").unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, path, bytes)
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: new_id("op"),
            principal: "alice".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "save".into(),
            request_digest: None,
        })
        .unwrap()
        .commit_id
        .unwrap()
}

#[test]
fn empty_configured_backup_scopes_export_and_restore_without_version_routes() {
    for path in ["", "empty"] {
        let temp = tempfile::tempdir().unwrap();
        fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let mut engine = initialize(&temp.path().join("source"));
        let repo = engine
            .create_repository("empty backup scope", "P01", 1, "alice", &new_id("op"))
            .unwrap();
        if !path.is_empty() {
            engine
                .file_namespace(
                    &repo.repository_id,
                    NamespaceOperation {
                        kind: "mkdir".into(),
                        path: path.into(),
                        destination: None,
                    },
                    "alice",
                    &new_id("op"),
                )
                .unwrap();
        }
        engine
            .configure_backups(BackupConfigurationRequest {
                repository_id: &repo.repository_id,
                path,
                target_copies: 2,
                backup_prefixes: &BTreeMap::from([("B01".into(), "chosen-backup".into())]),
                expected_generation: 0,
                principal: "alice",
                operation_id: &new_id("op"),
            })
            .unwrap();
        assert!(engine.state.versions.is_empty());
        let package = temp.path().join("package");
        engine
            .export_history_package(&repo.history_tree_id, &package)
            .unwrap();
        let mut restored = initialize(&temp.path().join("restored"));
        let operation = new_id("op");
        let receipt = restored
            .restore_history_package(&package, "P01", 2, "alice", &operation)
            .unwrap();
        let configuration = restored
            .backup_configuration(&repo.repository_id, path)
            .unwrap();
        assert_eq!(configuration["target_copies"], 2);
        assert_eq!(configuration["backup_prefixes"]["B01"], "chosen-backup");
        let file_path = if path.is_empty() {
            "file.txt"
        } else {
            "empty/file.txt"
        };
        save_path(
            &mut restored,
            &repo.main_branch_id,
            file_path,
            b"later content",
        );
        let file = restored
            .list_tree(&repo.main_branch_id)
            .unwrap()
            .into_iter()
            .find(|entry| entry.kind == "file")
            .unwrap();
        assert_eq!(
            restored.state.file_backup_paths[&file.file_id]["B01"],
            "chosen-backup/file.txt"
        );
        let sequence = restored.state.sequence;
        let replay = restored
            .restore_history_package(&package, "P01", 2, "alice", &operation)
            .unwrap();
        assert_eq!(replay.result, receipt.result);
        assert_eq!(replay.sequence, receipt.sequence);
        assert_eq!(restored.state.sequence, sequence);
        drop(restored);
        let restored = Engine::open(&temp.path().join("restored/state")).unwrap();
        assert_eq!(
            restored
                .backup_configuration(&repo.repository_id, path)
                .unwrap()["backup_prefixes"]["B01"],
            "chosen-backup"
        );

        let mut reduced = initialize(&temp.path().join("reduced"));
        reduced
            .restore_history_package(&package, "P01", 1, "alice", &new_id("op"))
            .unwrap();
        let configuration = reduced
            .backup_configuration(&repo.repository_id, path)
            .unwrap();
        assert_eq!(configuration["target_copies"], 1);
        assert_eq!(configuration["backup_prefixes"], json!({}));

        for (name, copies, renamed) in [("expanded", 3, false), ("renamed", 2, true)] {
            let root = temp.path().join(name);
            let mut config = Config::development([root.join("p"), root.join("b"), root.join("c")]);
            if renamed {
                config.disks[1].slot_id = "OTHER".into();
                config.disks[2].upstream_slot_id = Some("OTHER".into());
            }
            let mut destination = Engine::initialize(&root.join("state"), config).unwrap();
            let before = canonical_json(destination.state()).unwrap();
            assert_eq!(
                destination
                    .restore_history_package(&package, "P01", copies, "alice", &new_id("op"))
                    .unwrap_err()
                    .code(),
                "BACKUP_PREFIX_REQUIRED"
            );
            assert_eq!(canonical_json(destination.state()).unwrap(), before);
        }
    }
}

#[test]
fn restore_backup_prefixes_preserve_evidenced_positions_and_reject_identity_collisions() {
    for (source_copies, target_copies) in [(2, 2), (3, 2), (3, 3)] {
        let temp = tempfile::tempdir().unwrap();
        fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let mut engine = initialize(&temp.path().join("source"));
        let repo = engine
            .create_repository(
                "prefix positions",
                "P01",
                source_copies,
                "alice",
                &new_id("op"),
            )
            .unwrap();
        save(&mut engine, &repo.main_branch_id, b"observed route");
        engine
            .file_namespace(
                &repo.repository_id,
                NamespaceOperation {
                    kind: "mkdir".into(),
                    path: "empty".into(),
                    destination: None,
                },
                "alice",
                &new_id("op"),
            )
            .unwrap();
        engine
            .configure_backups(BackupConfigurationRequest {
                repository_id: &repo.repository_id,
                path: "empty",
                target_copies: source_copies,
                backup_prefixes: &BTreeMap::from([
                    ("B01".into(), "first-tier".into()),
                    ("C01".into(), "second-tier".into()),
                ]),
                expected_generation: 0,
                principal: "alice",
                operation_id: &new_id("op"),
            })
            .unwrap();
        let package = temp.path().join("package");
        engine
            .export_history_package(&repo.history_tree_id, &package)
            .unwrap();
        let root = temp.path().join("restored");
        let mut config = Config::development([root.join("p"), root.join("b"), root.join("c")]);
        config.disks[1].slot_id = "C01".into();
        config.disks[2].slot_id = "B01".into();
        config.disks[2].upstream_slot_id = Some("C01".into());
        let mut restored = Engine::initialize(&root.join("state"), config).unwrap();
        let before = canonical_json(restored.state()).unwrap();
        let result = restored.restore_history_package(
            &package,
            "P01",
            target_copies,
            "alice",
            &new_id("op"),
        );
        if source_copies == 2 {
            assert_eq!(result.unwrap_err().code(), "RESTORE_BACKUP_CONFLICT");
            assert_eq!(canonical_json(restored.state()).unwrap(), before);
        } else {
            result.unwrap();
            let configuration = restored
                .backup_configuration(&repo.repository_id, "empty")
                .unwrap();
            let expected = if target_copies == 2 {
                json!({"C01":"first-tier"})
            } else {
                json!({"C01":"first-tier", "B01":"second-tier"})
            };
            assert_eq!(configuration["backup_prefixes"], expected);
        }
    }
}

fn latest_candidate(engine: &Engine) -> Candidate {
    let member = &engine.members()[0];
    let record = scan(member)
        .unwrap()
        .records
        .into_iter()
        .rev()
        .find(|(record, _)| record.record_type == "PUBLICATION")
        .unwrap()
        .0;
    load_object(member, "candidate", &record.candidate_hash).unwrap()
}

#[test]
fn paged_pending_recovery_repairs_leaf_index_and_root_and_checkpoint_checks_them() {
    for kind in ["state-page", "history-index", "history-manifest"] {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let mut engine = initialize(&source);
        let repo = engine
            .create_repository("history", "P01", 3, "alice", &new_id("op"))
            .unwrap();
        let mut next = engine.state().clone();
        next.sequence = engine.next_slot;
        let original = next.commits[&next.histories[&repo.history_tree_id].root_commit_id].clone();
        let mut parent = original.commit_id.clone();
        for _ in 0..160 {
            let mut commit = original.clone();
            commit.commit_id = new_id("commit");
            commit.parent_commit_ids = vec![parent];
            commit.sequence = next.sequence;
            parent = commit.commit_id.clone();
            next.commits.insert(commit.commit_id.clone(), commit);
        }
        let mut result = next.operations.values().next().unwrap().clone();
        result.operation_id = new_id("op");
        result.sequence = next.sequence;
        next.operations
            .insert(result.operation_id.clone(), result.clone());
        engine.inject_fault_once("after_accept_2");
        assert!(
            engine
                .persist(next.clone(), Vec::new(), result.clone())
                .is_err()
        );
        let members = engine.members();
        let record = scan(&members[0])
            .unwrap()
            .records
            .into_iter()
            .rev()
            .find(|(record, _)| record.record_type == "ACCEPT")
            .unwrap()
            .0;
        let candidate: Candidate =
            load_object(&members[0], "candidate", &record.candidate_hash).unwrap();
        let objects = bound_history_objects(&members, &candidate, Some(&next)).unwrap();
        let (_, hash, value) = objects
            .iter()
            .find(|(domain, _, _)| domain == kind)
            .unwrap();
        let expected = canonical_json(value).unwrap();
        let damaged = object_path(&members[0], kind, hash);
        fs::write(&damaged, b"damaged").unwrap();
        drop(engine);
        let engine = Engine::open(&source.join("state")).unwrap();
        assert_eq!(
            engine.operation(&result.operation_id).unwrap().sequence,
            result.sequence
        );
        assert_eq!(fs::read(&damaged).unwrap(), expected);
        drop(engine);
        // A checkpoint must not turn a now-missing graph dependency into
        // apparent publication validity. No graph re-rooting is permitted.
        for member in &members {
            for path in existing_object_paths(member, kind, hash).unwrap() {
                fs::remove_file(path).unwrap();
            }
        }
        assert!(Engine::open(&source.join("state")).is_err());
    }
}

#[test]
fn paged_history_resolves_disjoint_replicas_without_read_repair() {
    let temp = tempfile::tempdir().unwrap();
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut engine = initialize(&temp.path().join("source"));
    let repo = engine
        .create_repository("history", "P01", 3, "alice", &new_id("op"))
        .unwrap();
    save(&mut engine, &repo.main_branch_id, b"full retained bytes");
    let candidate = latest_candidate(&engine);
    let members = engine.members();
    let objects = bound_history_objects(&members, &candidate, Some(&engine.state)).unwrap();
    assert!(objects.len() > 3);
    for (i, (kind, hash, _)) in objects.iter().enumerate() {
        for (j, member) in members.iter().enumerate() {
            if j != i % 3 {
                for path in existing_object_paths(member, kind, hash).unwrap() {
                    fs::remove_file(path).unwrap();
                }
            }
        }
    }
    let selected = bound_history_objects(&members, &candidate, Some(&engine.state)).unwrap();
    assert_eq!(selected.len(), objects.len());
    for (i, (kind, hash, _)) in objects.iter().enumerate() {
        for (j, member) in members.iter().enumerate() {
            assert_eq!(
                existing_object_paths(member, kind, hash)
                    .unwrap()
                    .is_empty(),
                j != i % 3
            );
        }
    }
    repair_history_objects(&members, &selected, &mut BTreeSet::new()).unwrap();
    for member in &members {
        bound_history_objects(
            std::slice::from_ref(member),
            &candidate,
            Some(&engine.state),
        )
        .unwrap();
    }
    let (_, hash, _) = objects
        .iter()
        .find(|(kind, _, _)| kind == "history-manifest")
        .unwrap();
    assert_ne!(
        object_hash(
            "history-manifest",
            &HistoryManifest::from_state(&engine.state, &repo.history_tree_id).unwrap()
        )
        .unwrap(),
        *hash
    );
}

#[test]
fn paged_package_missing_leaf_and_flat_proof_substitution_fail_closed() {
    let temp = tempfile::tempdir().unwrap();
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut engine = initialize(&temp.path().join("source"));
    let repo = engine
        .create_repository("history", "P01", 3, "alice", &new_id("op"))
        .unwrap();
    save(&mut engine, &repo.main_branch_id, b"retained bytes");
    let package = temp.path().join("package");
    let report = engine
        .export_history_package(&repo.history_tree_id, &package)
        .unwrap();
    let (manifest, proof, _) = load_package(&package).unwrap();
    assert_eq!(proof.manifest_hash, report.manifest_hash);
    let proof_bytes = fs::read(package.join("publication.json")).unwrap();
    let mut unsupported = json_codec::to_value(&proof).unwrap();
    unsupported["unexpected"] = json!(true);
    write_json(&package.join("publication.json"), &unsupported, true).unwrap();
    assert!(offline_check_history_package(&package).is_err());
    write_atomic(&package.join("publication.json"), &proof_bytes, true).unwrap();
    let root = fs::read(package.join("manifest.json")).unwrap();
    write_json(&package.join("manifest.json"), &manifest, true).unwrap();
    assert!(offline_check_history_package(&package).is_err());
    write_atomic(&package.join("manifest.json"), &root, true).unwrap();
    let objects =
        bound_history_objects(&engine.members(), &proof.candidate, Some(&engine.state)).unwrap();
    let (kind, hash, _) = objects
        .iter()
        .find(|(kind, _, _)| kind == "state-page")
        .unwrap();
    let path = package_object(&package, kind, hash).unwrap();
    let bytes = fs::read(&path).unwrap();
    fs::remove_file(&path).unwrap();
    assert!(offline_check_history_package(&package).is_err());
    write_atomic(&path, &bytes, false).unwrap();
    offline_check_history_package(&package).unwrap();
    // Candidate-bound root loss may not be replaced with a newly encoded root.
    for member in engine.members() {
        for path in
            existing_object_paths(&member, "history-manifest", &report.manifest_hash).unwrap()
        {
            fs::remove_file(path).unwrap();
        }
    }
    assert!(
        bound_history_objects(&engine.members(), &proof.candidate, Some(&engine.state)).is_err()
    );
}

fn legacy_placeholder(
    disk: &DiskConfig,
    history: &str,
    file: &str,
    name: &str,
    branch: &str,
) -> PathBuf {
    disk.root
        .join("histories")
        .join(history)
        .join("file-containers")
        .join(file)
        .join("namespace")
        .join(name)
        .join("pending-branches")
        .join(format!("{branch}.pending"))
}

#[test]
fn nested_directory_compositions_keep_frozen_paths_and_restore_after_rename() {
    let temp = tempfile::tempdir().unwrap();
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut engine = initialize(&temp.path().join("source"));
    let repo = engine
        .create_repository("嵌套恢复", "P01", 3, "alice", &new_id("op"))
        .unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "alice")
        .unwrap();
    for (path, bytes) in [
        ("folder/a.txt", b"parent A".as_slice()),
        ("folder/nested/b.txt", b"parent B".as_slice()),
    ] {
        engine
            .stage_bytes(&workspace.workspace_id, path, bytes)
            .unwrap();
    }
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: new_id("op"),
            principal: "alice".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "nested parent files".into(),
            request_digest: None,
        })
        .unwrap();
    engine
        .initialize_directory_scope(&repo.repository_id, "folder/nested", "alice", &new_id("op"))
        .unwrap();
    let created = engine
        .create_directory_branches(
            &repo.repository_id,
            "folder",
            "draft",
            "alice",
            &new_id("op"),
        )
        .unwrap();
    let id = created.result["directory_branch_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let directory = engine.state.directory_branches[&id].directory_id.clone();
    let inherited_tree = engine.state.directory_branches[&id].head_tree_id.clone();
    let frozen = engine.state.directory_branches[&id]
        .entries
        .iter()
        .find(|entry| entry.path == "folder/nested/b.txt")
        .unwrap()
        .clone();
    engine
        .select_directory_branches(
            &repo.repository_id,
            "folder",
            "draft",
            "alice",
            &new_id("op"),
        )
        .unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "alice")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "folder/a.txt", b"owned A")
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: new_id("op"),
            principal: "alice".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "complete one file".into(),
            request_digest: None,
        })
        .unwrap();
    assert!(engine.state.directory_branches[&id].materialized);
    assert!(
        engine.state.directory_branches[&id]
            .entries
            .iter()
            .any(|entry| entry.file_id == frozen.file_id && entry.version_id == frozen.version_id)
    );
    let body_proofs = engine
        .state
        .versions
        .iter()
        .map(|(id, version)| (id.clone(), canonical_json(version).unwrap()))
        .collect::<BTreeMap<_, _>>();
    engine
        .file_namespace(
            &repo.repository_id,
            NamespaceOperation {
                kind: "rename".into(),
                path: "folder".into(),
                destination: Some("renamed".into()),
            },
            "alice",
            &new_id("op"),
        )
        .unwrap();
    assert_eq!(engine.state.file_path_nodes.len(), 2);
    for node in engine.state.file_path_nodes.values() {
        assert!(node.previous_path.starts_with("folder/"));
        assert!(node.path.starts_with("renamed/"));
        assert_eq!(
            canonical_json(&engine.state.versions[&node.version_id]).unwrap(),
            body_proofs[&node.version_id]
        );
    }
    let manifest = HistoryManifest::from_state(&engine.state, &repo.history_tree_id).unwrap();
    assert_eq!(manifest.directory_branches[&id].directory_id, directory);
    assert!(!manifest.directory_branches[&id].directory_refs.is_empty());
    assert_eq!(manifest.file_write_origins.get(&frozen.file_id), Some(&id));
    assert_eq!(manifest.directory_trees[&inherited_tree].path, "folder");
    assert_eq!(
        manifest.directory_branches[&id].path,
        manifest.directory_trees[&manifest.directory_branches[&id].head_tree_id].path
    );
    let visible = engine
        .directory_branches(&repo.repository_id, "renamed")
        .unwrap();
    assert!(
        visible
            .iter()
            .any(|branch| branch["directory_branch_id"] == id)
    );
    let package = temp.path().join("package");
    engine
        .export_history_package(&repo.history_tree_id, &package)
        .unwrap();
    let mut restored = initialize(&temp.path().join("restored"));
    restored
        .restore_history_package(&package, "P01", 3, "alice", &new_id("op"))
        .unwrap();
    assert_eq!(
        canonical_json(&restored.state.directory_branches).unwrap(),
        canonical_json(&manifest.directory_branches).unwrap()
    );
    assert_eq!(
        canonical_json(&restored.state.directory_trees).unwrap(),
        canonical_json(&manifest.directory_trees).unwrap()
    );
    assert_eq!(
        restored.state.directory_selection_generations,
        manifest.directory_selection_generations
    );
    assert_eq!(
        restored.state.file_write_origins,
        manifest.file_write_origins
    );
    assert_eq!(
        restored.state.directory_tree_selections,
        manifest.directory_tree_selections
    );
    assert_eq!(restored.state.file_contexts, manifest.file_contexts);
    assert_eq!(
        canonical_json(&restored.state.file_path_nodes).unwrap(),
        canonical_json(&manifest.file_path_nodes).unwrap()
    );
    assert_eq!(restored.state.file_node_parents, manifest.file_node_parents);
    assert_eq!(restored.state.file_path_notes, manifest.file_path_notes);
    assert_eq!(restored.state.file_locations, manifest.file_locations);
    assert_eq!(
        restored.state.file_selection_generations,
        manifest.file_selection_generations
    );
    assert!(
        restored
            .directory_branches(&repo.repository_id, "renamed")
            .unwrap()
            .iter()
            .any(|branch| branch["directory_branch_id"] == id)
    );
    assert!(restored.open_content(&frozen.version_id).is_ok());
}

#[test]
fn moved_file_aliases_restore_independent_path_contexts_and_one_physical_owner() {
    let temporary = tempfile::tempdir().unwrap();
    fs::set_permissions(temporary.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut engine = initialize(&temporary.path().join("source"));
    let repo = engine
        .create_disk_root("P01", "alice", &new_id("op"))
        .unwrap();
    let save_path = |engine: &mut Engine, path: &str, bytes: &[u8]| {
        let workspace = engine
            .create_workspace(&repo.main_branch_id, "alice")
            .unwrap();
        engine
            .stage_bytes(&workspace.workspace_id, path, bytes)
            .unwrap();
        engine
            .commit_workspace(CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: new_id("op"),
                principal: "alice".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "alias write".into(),
                request_digest: None,
            })
            .unwrap();
    };
    save_path(&mut engine, "a/f.txt", b"F1");
    save_path(&mut engine, "b/keep.txt", b"keep");
    let original = engine
        .list_tree(&repo.main_branch_id)
        .unwrap()
        .into_iter()
        .find(|entry| entry.path == "a/f.txt")
        .unwrap();
    let create = |engine: &mut Engine, path: &str, name: &str| {
        engine
            .create_directory_branches(&repo.repository_id, path, name, "alice", &new_id("op"))
            .unwrap()
            .result["directory_branch_id"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let m = create(&mut engine, "a", "m");
    let n = create(&mut engine, "a", "n");
    let l = create(&mut engine, "b", "l");
    engine
        .select_directory_branches(&repo.repository_id, "a", "m", "alice", &new_id("op"))
        .unwrap();
    engine
        .select_directory_branches(&repo.repository_id, "b", "l", "alice", &new_id("op"))
        .unwrap();
    let request = DirectoryNamespaceBatch {
        kind: "move".into(),
        items: vec![DirectoryNamespaceItem {
            source: DirectoryNamespaceSource {
                directory_branch_id: m.clone(),
                expected_tree_id: engine.state.directory_branches[&m].head_tree_id.clone(),
                path: "a/f.txt".into(),
                entry_id: original.entry_id.clone(),
            },
            target: Some(DirectoryNamespaceTarget {
                directory_branch_id: l.clone(),
                expected_tree_id: engine.state.directory_branches[&l].head_tree_id.clone(),
                path: "b/f.txt".into(),
            }),
        }],
    };
    engine
        .directory_namespace_batch(&repo.repository_id, &request, "alice", &new_id("op"))
        .unwrap();
    save_path(&mut engine, "b/f.txt", b"F2");
    let moved = engine
        .list_tree(&repo.main_branch_id)
        .unwrap()
        .into_iter()
        .find(|entry| entry.path == "b/f.txt")
        .unwrap();
    engine
        .select_directory_branches(&repo.repository_id, "a", "n", "alice", &new_id("op"))
        .unwrap();
    let saved_root = engine.state.directory_selections[&repo.repository_id].clone();
    let saved_tree = engine.state.directory_branches[&saved_root]
        .head_tree_id
        .clone();
    let saved_contexts = engine.state.file_contexts.clone();
    let alternate = create(&mut engine, "", "whole-view-draft");
    engine
        .select_default_directory_branch(&repo.repository_id, &alternate, "alice", &new_id("op"))
        .unwrap();
    engine
        .select_default_directory_branch(&repo.repository_id, &saved_root, "alice", &new_id("op"))
        .unwrap();
    assert_eq!(
        engine.state.directory_branches[&saved_root].head_tree_id,
        saved_tree
    );
    assert_eq!(engine.state.file_contexts, saved_contexts);
    let manifest = HistoryManifest::from_state(&engine.state, &repo.history_tree_id).unwrap();
    let a_key = format!("{}/a/f.txt", repo.repository_id);
    let b_key = format!("{}/b/f.txt", repo.repository_id);
    assert_eq!(
        manifest.file_contexts[&a_key].inherited_from.as_deref(),
        Some(n.as_str())
    );
    assert!(manifest.file_contexts[&b_key].inherited_from.is_none());
    assert_ne!(
        manifest.file_contexts[&a_key].branch_id,
        manifest.file_contexts[&b_key].branch_id
    );
    assert_eq!(manifest.file_locations[&original.file_id], "b/f.txt");
    let package = temporary.path().join("package");
    engine
        .export_history_package(&repo.history_tree_id, &package)
        .unwrap();
    let restored_root = temporary.path().join("restored");
    let mut restored = initialize(&restored_root);
    restored
        .restore_history_package(&package, "P01", 3, "alice", &new_id("op"))
        .unwrap();
    assert_eq!(restored.state.file_contexts, manifest.file_contexts);
    assert_eq!(restored.state.file_locations, manifest.file_locations);
    assert_eq!(
        fs::read(restored.valid_replica(&original.version_id).unwrap().0).unwrap(),
        b"F1"
    );
    assert_eq!(
        fs::read(restored.valid_replica(&moved.version_id).unwrap().0).unwrap(),
        b"F2"
    );
    save_path(&mut restored, "a/f.txt", b"F3 from old alias");
    let live = restored.list_tree(&repo.main_branch_id).unwrap();
    let a = live.iter().find(|entry| entry.path == "a/f.txt").unwrap();
    let b = live.iter().find(|entry| entry.path == "b/f.txt").unwrap();
    assert_eq!(a.file_id, b.file_id);
    assert_eq!(b.version_id, moved.version_id);
    assert_ne!(a.version_id, original.version_id);
    assert_eq!(restored.state.file_locations[&original.file_id], "b/f.txt");
    assert_eq!(
        restored.state.file_contexts[&b_key],
        manifest.file_contexts[&b_key]
    );
    let latest = a.version_id.clone();
    drop(restored);
    let restored = Engine::open(&restored_root.join("state")).unwrap();
    assert_eq!(
        fs::read(restored.valid_replica(&latest).unwrap().0).unwrap(),
        b"F3 from old alias"
    );
    assert_eq!(
        fs::read(restored.valid_replica(&moved.version_id).unwrap().0).unwrap(),
        b"F2"
    );
}

#[test]
fn rename_nodes_preserve_body_proofs_and_mixed_lineage_through_purge_export_and_restore() {
    let temporary = tempfile::tempdir().unwrap();
    fs::set_permissions(temporary.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut engine = initialize(&temporary.path().join("source"));
    let repo = engine
        .create_disk_root("P01", "alice", &new_id("op"))
        .unwrap();
    let save_path = |engine: &mut Engine, path: &str, bytes: &[u8]| {
        let workspace = engine
            .create_workspace(&repo.main_branch_id, "alice")
            .unwrap();
        engine
            .stage_bytes(&workspace.workspace_id, path, bytes)
            .unwrap();
        engine
            .commit_workspace(CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: new_id("op"),
                principal: "alice".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "node lifecycle".into(),
                request_digest: None,
            })
            .unwrap();
    };
    save_path(&mut engine, "a/f.txt", b"original body");
    let old = engine
        .list_tree(&repo.main_branch_id)
        .unwrap()
        .into_iter()
        .find(|entry| entry.kind == "file")
        .unwrap();
    let proof = canonical_json(&engine.state.versions[&old.version_id]).unwrap();
    let inode = fs::metadata(engine.valid_replica(&old.version_id).unwrap().0)
        .unwrap()
        .ino();
    for (source, destination) in [("a/f.txt", "a/g.txt"), ("a", "b")] {
        let versions = engine.state.versions.len();
        engine
            .file_namespace(
                &repo.repository_id,
                NamespaceOperation {
                    kind: "rename".into(),
                    path: source.into(),
                    destination: Some(destination.into()),
                },
                "alice",
                &new_id("op"),
            )
            .unwrap();
        assert_eq!(engine.state.versions.len(), versions);
        assert_eq!(
            canonical_json(&engine.state.versions[&old.version_id]).unwrap(),
            proof
        );
        assert_eq!(
            fs::metadata(engine.valid_replica(&old.version_id).unwrap().0)
                .unwrap()
                .ino(),
            inode
        );
    }
    assert_eq!(engine.state.file_path_nodes.len(), 2);
    let context = &engine.state.file_contexts[&format!("{}/b/g.txt", repo.repository_id)];
    let node = context.node_id.clone().unwrap();
    let parent = engine.state.file_path_nodes[&node].parent_node_ids[0].clone();
    assert_eq!(
        engine.state.file_path_nodes[&parent].parent_node_ids,
        vec![old.version_id.clone()]
    );
    assert_eq!(engine.state.file_path_nodes[&node].previous_path, "a/g.txt");
    assert_eq!(engine.state.file_path_nodes[&node].path, "b/g.txt");
    engine
        .set_file_path_note(
            &old.file_id,
            &node,
            "目录改名，正文未变",
            "alice",
            &new_id("op"),
        )
        .unwrap();
    let body = engine.valid_replica(&old.version_id).unwrap().0;
    let cache: json_codec::Value = read_json(
        &body
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("container.json"),
    )
    .unwrap();
    assert_eq!(cache["versions"].as_array().unwrap().len(), 1);
    assert_eq!(cache["path_nodes"].as_array().unwrap().len(), 2);
    assert!(
        cache["path_nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|record| record["node"]["node_id"] == node
                && record["note"] == "目录改名，正文未变")
    );
    save_path(&mut engine, "b/g.txt", b"new content after both renames");
    let latest = engine
        .list_tree(&repo.main_branch_id)
        .unwrap()
        .into_iter()
        .find(|entry| entry.kind == "file")
        .unwrap();
    assert_eq!(
        engine.state.file_node_parents[&latest.version_id],
        vec![node.clone()]
    );
    assert_eq!(
        engine.state.versions[&latest.version_id].source_version_ids,
        vec![old.version_id.clone()]
    );
    let plan = engine
        .create_purge_plan(&repo.history_tree_id, vec![old.version_id.clone()], "alice")
        .unwrap();
    engine
        .commit_plan(&plan.plan_id, "alice", &new_id("op"))
        .unwrap();
    let manifest = HistoryManifest::from_state(&engine.state, &repo.history_tree_id).unwrap();
    assert_eq!(manifest.content_states[&old.version_id], "PURGED");
    let mut cyclic = manifest.clone();
    cyclic
        .file_path_nodes
        .get_mut(&parent)
        .unwrap()
        .parent_node_ids = vec![node.clone()];
    assert_eq!(
        cyclic.validate().unwrap_err().code(),
        "HISTORY_PACKAGE_INVALID"
    );
    let mut wrong_body = manifest.clone();
    wrong_body
        .file_path_nodes
        .get_mut(&node)
        .unwrap()
        .version_id = latest.version_id.clone();
    assert_eq!(
        wrong_body.validate().unwrap_err().code(),
        "HISTORY_PACKAGE_INVALID"
    );
    let mut wrong_parent = manifest.clone();
    wrong_parent
        .file_node_parents
        .insert(latest.version_id.clone(), vec![latest.version_id.clone()]);
    assert_eq!(
        wrong_parent.validate().unwrap_err().code(),
        "HISTORY_PACKAGE_INVALID"
    );
    let package = temporary.path().join("package");
    let report = engine
        .export_history_package(&repo.history_tree_id, &package)
        .unwrap();
    assert_eq!(report.full_files, 1);
    assert_eq!(report.tombstones, 1);
    let mut restored = initialize(&temporary.path().join("restored"));
    restored
        .restore_history_package(&package, "P01", 3, "alice", &new_id("op"))
        .unwrap();
    assert_eq!(
        canonical_json(&restored.state.file_path_nodes).unwrap(),
        canonical_json(&manifest.file_path_nodes).unwrap()
    );
    assert_eq!(restored.state.file_node_parents, manifest.file_node_parents);
    assert_eq!(restored.state.file_path_notes, manifest.file_path_notes);
    assert_eq!(
        canonical_json(&restored.state.versions[&old.version_id]).unwrap(),
        proof
    );
    assert_eq!(
        restored.open_content(&old.version_id).err().unwrap().code(),
        "CONTENT_PURGED"
    );
    assert_eq!(
        fs::read(restored.valid_replica(&latest.version_id).unwrap().0).unwrap(),
        b"new content after both renames"
    );
}

#[test]
fn composed_directory_records_validate_and_restore_frozen_shared_file_references() {
    let temp = tempfile::tempdir().unwrap();
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut engine = initialize(&temp.path().join("source"));
    let repo = engine
        .create_repository("组合恢复", "P01", 3, "alice", &new_id("op"))
        .unwrap();
    let original_commit = save(&mut engine, &repo.main_branch_id, b"frozen parent");
    let original = engine.list_tree(&repo.main_branch_id).unwrap()[0].clone();
    let shared = engine
        .create_file_branch(
            &original.file_id,
            &original.version_id,
            "shared",
            "alice",
            &new_id("op"),
        )
        .unwrap()
        .branch_id
        .unwrap();
    save(&mut engine, &shared, b"shared branch advanced");
    let advanced = engine.list_tree(&shared).unwrap()[0].clone();
    let first = new_id("dbr");
    let second = new_id("dbr");
    let first_tree = new_id("dtree");
    engine
        .mutate(
            "fixture.directory_compositions",
            "alice",
            &new_id("op"),
            json!({}),
            |_, state, _| {
                let parent = DirectoryBranch {
                    directory_branch_id: first.clone(),
                    repository_id: repo.repository_id.clone(),
                    directory_id: repo.repository_id.clone(),
                    path: String::new(),
                    name: "frozen".into(),
                    parent_branch_id: None,
                    head_tree_id: first_tree.clone(),
                    commit_id: original_commit.clone(),
                    entries: vec![original.clone()],
                    file_branches: BTreeMap::from([(original.file_id.clone(), shared.clone())]),
                    file_contexts: BTreeMap::from([(
                        original.lookup_key.clone(),
                        FileContext {
                            node_id: None,
                            selection_generation: 0,
                            branch_id: shared.clone(),
                            inherited_from: None,
                        },
                    )]),
                    inherited_files: BTreeMap::new(),
                    directory_refs: BTreeMap::new(),
                    materialized: true,
                    sequence: state.sequence,
                };
                let mut child = parent.clone();
                child.directory_branch_id = second.clone();
                child.name = "composed".into();
                child.parent_branch_id = Some(first.clone());
                child.materialized = false;
                state.directory_trees.insert(
                    first_tree.clone(),
                    DirectoryTree {
                        tree_id: first_tree.clone(),
                        directory_branch_id: first.clone(),
                        parent_tree_id: None,
                        commit_id: parent.commit_id.clone(),
                        path: parent.path.clone(),
                        entries: parent.entries.clone(),
                        file_branches: parent.file_branches.clone(),
                        file_contexts: parent.file_contexts.clone(),
                        inherited_files: parent.inherited_files.clone(),
                        directory_refs: parent.directory_refs.clone(),
                        sequence: parent.sequence,
                    },
                );
                state.directory_branches.insert(first.clone(), parent);
                state.directory_branches.insert(second.clone(), child);
                state
                    .directory_selections
                    .insert(repo.repository_id.clone(), second.clone());
                state
                    .directory_selection_generations
                    .insert(repo.repository_id.clone(), 42);
                Ok(json!({}))
            },
        )
        .unwrap();
    let manifest = HistoryManifest::from_state(&engine.state, &repo.history_tree_id).unwrap();
    // Shared branches can advance without rewriting an older composition.
    assert_ne!(
        manifest.directory_branches[&first].entries[0].version_id,
        advanced.version_id
    );
    assert_eq!(
        manifest.directory_branches[&first].file_branches[&original.file_id],
        shared
    );
    let bad = |manifest: &HistoryManifest| {
        assert_eq!(
            manifest.validate().unwrap_err().code(),
            "HISTORY_PACKAGE_INVALID"
        )
    };
    for target in ["branch", "tree"] {
        let mut invalid = manifest.clone();
        if target == "branch" {
            invalid
                .directory_branches
                .get_mut(&first)
                .unwrap()
                .file_contexts
                .clear();
        } else {
            invalid
                .directory_trees
                .get_mut(&first_tree)
                .unwrap()
                .file_contexts
                .clear();
        }
        bad(&invalid);
    }
    let mut invalid = manifest.clone();
    invalid
        .directory_branches
        .get_mut(&first)
        .unwrap()
        .repository_id = new_id("repo");
    bad(&invalid);
    let mut invalid = manifest.clone();
    invalid
        .directory_branches
        .get_mut(&first)
        .unwrap()
        .commit_id = new_id("commit");
    bad(&invalid);
    let mut invalid = manifest.clone();
    invalid.directory_branches.get_mut(&first).unwrap().entries[0].version_id = new_id("ver");
    bad(&invalid);
    let mut invalid = manifest.clone();
    invalid
        .directory_branches
        .get_mut(&first)
        .unwrap()
        .file_branches
        .clear();
    bad(&invalid);
    let mut invalid = manifest.clone();
    invalid
        .directory_branches
        .get_mut(&first)
        .unwrap()
        .file_branches
        .insert(original.file_id.clone(), repo.main_branch_id.clone());
    bad(&invalid);
    let mut invalid = manifest.clone();
    invalid
        .directory_branches
        .get_mut(&first)
        .unwrap()
        .parent_branch_id = Some(second.clone());
    bad(&invalid);
    let mut invalid = manifest.clone();
    invalid
        .directory_selections
        .insert(new_id("entry"), second.clone());
    bad(&invalid);
    let mut invalid = manifest.clone();
    invalid
        .directory_trees
        .get_mut(&first_tree)
        .unwrap()
        .parent_tree_id = Some(first_tree.clone());
    bad(&invalid);
    let mut invalid = manifest.clone();
    invalid
        .directory_branches
        .get_mut(&second)
        .unwrap()
        .head_tree_id = new_id("dtree");
    bad(&invalid);
    let mut invalid = manifest.clone();
    invalid
        .directory_trees
        .get_mut(&first_tree)
        .unwrap()
        .entries[0]
        .size += 1;
    bad(&invalid);
    let mut invalid = manifest.clone();
    invalid
        .directory_selection_generations
        .insert(new_id("entry"), 1);
    bad(&invalid);
    let mut invalid_json = json_codec::to_value(&manifest).unwrap();
    assert_eq!(
        invalid_json["directory_selection_generations"][&repo.repository_id],
        "42"
    );
    invalid_json["directory_selection_generations"][&repo.repository_id] = json!(42);
    assert!(json_codec::from_value::<HistoryManifest>(invalid_json).is_err());
    let mut invalid = manifest.clone();
    invalid
        .file_locations
        .insert(new_id("file"), "other.txt".into());
    bad(&invalid);
    let mut invalid = manifest.clone();
    invalid
        .file_locations
        .insert(original.file_id.clone(), "wrong-name.txt".into());
    bad(&invalid);
    let mut invalid = manifest.clone();
    invalid
        .file_write_origins
        .insert(original.file_id.clone(), new_id("dbr"));
    bad(&invalid);
    let mut invalid = manifest.clone();
    invalid.file_selection_generations.insert(new_id("file"), 1);
    bad(&invalid);
    let mut invalid = manifest.clone();
    invalid.file_contexts.insert(
        format!("{}/missing.txt", repo.repository_id),
        FileContext {
            node_id: None,
            branch_id: shared.clone(),
            inherited_from: None,
            selection_generation: 0,
        },
    );
    bad(&invalid);
    let mut invalid = manifest.clone();
    invalid
        .directory_trees
        .get_mut(&first_tree)
        .unwrap()
        .file_contexts
        .insert(
            "missing.txt".into(),
            FileContext {
                node_id: None,
                branch_id: shared.clone(),
                inherited_from: None,
                selection_generation: 0,
            },
        );
    bad(&invalid);
    let mut invalid = manifest.clone();
    invalid
        .directory_tree_selections
        .insert(repo.repository_id.clone(), new_id("dtree"));
    bad(&invalid);
    let mut invalid = manifest.clone();
    invalid
        .directory_trees
        .get_mut(&first_tree)
        .unwrap()
        .inherited_files
        .insert(new_id("file"), first.clone());
    bad(&invalid);
    let mut invalid = manifest.clone();
    invalid
        .directory_trees
        .get_mut(&first_tree)
        .unwrap()
        .directory_refs
        .insert(
            new_id("entry"),
            DirectoryReference {
                directory_branch_id: first.clone(),
                tree_id: first_tree.clone(),
            },
        );
    bad(&invalid);

    let package = temp.path().join("package");
    let report = engine
        .export_history_package(&repo.history_tree_id, &package)
        .unwrap();
    assert_eq!(report.versions, 2);
    assert_eq!(report.full_files, 2);
    let restored_root = temp.path().join("restored");
    let mut restored = initialize(&restored_root);
    restored
        .restore_history_package(&package, "P01", 3, "alice", &new_id("op"))
        .unwrap();
    assert_eq!(
        canonical_json(&restored.state.directory_branches).unwrap(),
        canonical_json(&manifest.directory_branches).unwrap()
    );
    assert_eq!(
        canonical_json(&restored.state.directory_trees).unwrap(),
        canonical_json(&manifest.directory_trees).unwrap()
    );
    assert_eq!(
        restored.state.directory_selections,
        manifest.directory_selections
    );
    assert_eq!(
        restored.state.directory_selection_generations,
        manifest.directory_selection_generations
    );
    drop(restored);
    let restored = Engine::open(&restored_root.join("state")).unwrap();
    assert_eq!(
        restored.state.directory_branches[&first].entries[0].version_id,
        original.version_id
    );
    assert_eq!(
        restored.state.directory_selections[&repo.repository_id],
        second
    );
    assert_eq!(
        restored.state.directory_selection_generations[&repo.repository_id],
        42
    );
    let mut content = Vec::new();
    restored
        .open_content(&original.version_id)
        .unwrap()
        .file
        .read_to_end(&mut content)
        .unwrap();
    assert_eq!(content, b"frozen parent");
}

#[test]
fn package_preserves_inherited_references_without_inventing_files_or_versions() {
    let temp = tempfile::tempdir().unwrap();
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut engine = initialize(&temp.path().join("source"));
    let repo = engine
        .create_repository("引用恢复", "P01", 3, "alice", &new_id("op"))
        .unwrap();
    save(&mut engine, &repo.main_branch_id, b"source bytes");
    let file = engine.list_tree(&repo.main_branch_id).unwrap()[0].clone();
    let before_versions = engine.state.versions.len();
    let result = engine
        .create_file_branch(
            &file.file_id,
            &file.version_id,
            "draft",
            "alice",
            &new_id("op"),
        )
        .unwrap();
    let branch = result.branch_id.unwrap();
    assert_eq!(engine.state.versions.len(), before_versions);
    let selected = engine.list_tree(&branch).unwrap();
    assert_eq!(selected[0].version_id, file.version_id);
    assert_eq!(selected[0].size, file.size);
    let manifest = HistoryManifest::from_state(&engine.state, &repo.history_tree_id).unwrap();
    let mut invalid = manifest.clone();
    invalid
        .branches
        .get_mut(&branch)
        .unwrap()
        .pending_file_versions
        .insert(file.file_id.clone(), new_id("ver"));
    assert_eq!(
        invalid.validate().unwrap_err().code(),
        "HISTORY_PACKAGE_INVALID"
    );
    let pending_commit = engine.state.branches[&branch].head_commit_id.clone();
    assert_eq!(
        manifest.commits[&pending_commit].pending_file_versions[&file.file_id],
        file.version_id
    );
    let mut invalid = manifest.clone();
    invalid
        .commits
        .get_mut(&pending_commit)
        .unwrap()
        .pending_file_versions
        .insert(file.file_id.clone(), new_id("ver"));
    assert_eq!(
        invalid.validate().unwrap_err().code(),
        "HISTORY_PACKAGE_INVALID"
    );
    let package = temp.path().join("package");
    let exported = engine
        .export_history_package(&repo.history_tree_id, &package)
        .unwrap();
    assert_eq!(exported.full_files, 1);
    assert_eq!(exported.versions, 1);
    assert_eq!(fs::read_dir(package.join("versions")).unwrap().count(), 1);
    assert_eq!(
        offline_check_history_package(&package)
            .unwrap()
            .manifest_hash,
        exported.manifest_hash
    );

    let restored_root = temp.path().join("restored");
    let mut restored = initialize(&restored_root);
    restored
        .restore_history_package(&package, "P01", 3, "alice", &new_id("op"))
        .unwrap();
    assert_eq!(
        restored.state.branches[&branch].pending_file_versions[&file.file_id],
        file.version_id
    );
    assert_eq!(restored.state.versions.len(), 1);
    for disk in &restored.config.disks {
        let stub = legacy_placeholder(
            disk,
            &repo.history_tree_id,
            &file.file_id,
            &file.name,
            &branch,
        );
        assert!(!stub.parent().unwrap().exists());
        let cache = stub
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("container.json");
        let metadata: json_codec::Value = read_json(&cache).unwrap();
        assert_eq!(
            metadata["inherited_branches"][0]["version_id"],
            file.version_id
        );
        assert!(metadata["inherited_branches"][0]["own_version_id"].is_null());
        // The optional cache can be rebuilt without manufacturing files.
        fs::remove_file(cache).unwrap();
    }
    drop(restored);
    let mut restored = Engine::open(&restored_root.join("state")).unwrap();
    for disk in &restored.config.disks {
        let stub = legacy_placeholder(
            disk,
            &repo.history_tree_id,
            &file.file_id,
            &file.name,
            &branch,
        );
        assert!(!stub.parent().unwrap().exists());
        let cache = stub
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("container.json");
        let metadata: json_codec::Value = read_json(&cache).unwrap();
        assert_eq!(
            metadata["inherited_branches"][0]["source_version_id"],
            file.version_id
        );
        assert_eq!(
            metadata["inherited_branches"][0]["version_id"],
            file.version_id
        );
    }
    assert_eq!(
        restored.list_tree(&branch).unwrap()[0].version_id,
        file.version_id
    );
    save(&mut restored, &branch, b"first branch bytes");
    assert!(
        restored.state.branches[&branch]
            .pending_file_versions
            .is_empty()
    );
    let new = restored.list_tree(&branch).unwrap()[0].clone();
    assert_eq!(
        restored.state.versions[&new.version_id].source_version_ids,
        vec![file.version_id.clone()]
    );
    assert_eq!(restored.state.versions.len(), 2);
    // Materializing current Branch state must not rewrite the old snapshot:
    // its visible content remains the frozen inherited parent, not new bytes.
    let old = restored.list_tree_at(&branch, &pending_commit).unwrap();
    assert_eq!(old[0].version_id, file.version_id);
    assert_eq!(old[0].size, file.size);
    assert_ne!(old[0].version_id, new.version_id);
    for disk in &restored.config.disks {
        assert!(
            !legacy_placeholder(
                disk,
                &repo.history_tree_id,
                &file.file_id,
                &file.name,
                &branch
            )
            .parent()
            .unwrap()
            .exists()
        );
    }
}

#[test]
fn inherited_metadata_replays_after_decision_without_preparing_payloads_or_stubs() {
    for point in ["after_finalize", "after_container_names"] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("source");
        let mut engine = initialize(&root);
        let repo = engine
            .create_repository("回放", "P01", 3, "alice", &new_id("op"))
            .unwrap();
        let source_commit = save(&mut engine, &repo.main_branch_id, b"published source");
        let file = engine.list_tree(&repo.main_branch_id).unwrap()[0].clone();
        let operation = new_id("op");
        engine.inject_fault_once(point);
        assert_eq!(
            engine
                .create_branch(&source_commit, "pending", "alice", &operation)
                .unwrap_err()
                .code(),
            "IN_DOUBT"
        );
        assert_eq!(engine.state.versions.len(), 1);
        drop(engine);
        let mut engine = Engine::open(&root.join("state")).unwrap();
        let branch = engine
            .create_branch(&source_commit, "pending", "alice", &operation)
            .unwrap();
        assert_eq!(engine.state.versions.len(), 1);
        assert_eq!(branch.pending_file_versions[&file.file_id], file.version_id);
        for disk in &engine.config.disks {
            let stub = legacy_placeholder(
                disk,
                &repo.history_tree_id,
                &file.file_id,
                &file.name,
                &branch.branch_id,
            );
            assert!(!stub.parent().unwrap().exists());
            let cache = stub
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join("container.json");
            let metadata: json_codec::Value = read_json(&cache).unwrap();
            assert_eq!(
                metadata["inherited_branches"][0]["source_version_id"],
                file.version_id
            );
            assert_eq!(
                metadata["inherited_branches"][0]["version_id"],
                file.version_id
            );
            assert_eq!(
                fs::read_dir(
                    disk.root
                        .join("histories")
                        .join(&repo.history_tree_id)
                        .join("versions")
                )
                .unwrap()
                .count(),
                1
            );
        }
        let projected = engine.list_tree(&branch.branch_id).unwrap();
        assert_eq!(projected[0].version_id, file.version_id);
        assert_eq!(projected[0].size, file.size);
    }
}
#[test]
fn split_package_restores_full_history_after_all_original_paths_are_removed() {
    let temp = tempfile::tempdir().unwrap();
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let source = temp.path().join("original");
    let mut engine = initialize(&source);
    let repository = engine
        .create_repository("来源", "P01", 3, "alice", &new_id("op"))
        .unwrap();
    let cut = save(&mut engine, &repository.main_branch_id, b"first full bytes");
    save(
        &mut engine,
        &repository.main_branch_id,
        b"second full bytes",
    );
    let plan = engine
        .create_split_plan(&repository.history_tree_id, &cut, "alice")
        .unwrap();
    let split = engine
        .commit_plan(&plan.plan_id, "alice", &new_id("op"))
        .unwrap();
    let history = split.history_tree_id.unwrap();
    let package = temp.path().join("history-package");
    let exported = engine.export_history_package(&history, &package).unwrap();
    assert_eq!(exported.full_files, 2);
    let manifest = fs::read_to_string(package.join("manifest.json")).unwrap();
    assert!(!manifest.contains("control_auth"));
    assert!(!manifest.contains("operation_principals"));
    drop(engine);
    fs::remove_dir_all(source).unwrap();
    assert_eq!(
        offline_check_history_package(&package)
            .unwrap()
            .manifest_hash,
        exported.manifest_hash
    );
    let restored_root = temp.path().join("restored");
    let mut restored = initialize(&restored_root);
    let operation = new_id("op");
    let result = restored
        .restore_history_package(&package, "P01", 3, "rescuer", &operation)
        .unwrap();
    assert_eq!(
        restored
            .restore_history_package(&package, "P01", 3, "rescuer", &operation)
            .unwrap()
            .sequence,
        result.sequence
    );
    assert_eq!(restored.state.histories.len(), 1);
    let graph = restored.graph(&history).unwrap();
    assert_eq!(graph.commits.len(), 2);
    let mut contents = Vec::new();
    for commit in &graph.commits {
        for entry in commit.entries.iter().filter(|e| e.kind == "file") {
            let mut content = restored.open_content(&entry.version_id).unwrap();
            let mut bytes = Vec::new();
            content.file.read_to_end(&mut bytes).unwrap();
            contents.push(bytes);
        }
    }
    contents.sort();
    assert_eq!(
        contents,
        vec![b"first full bytes".to_vec(), b"second full bytes".to_vec()]
    );
    let repo = &restored.state.repositories[&graph.history.repository_id];
    assert!(!repo.smb_enabled);
    assert_eq!(repo.smb_binding_generation, 1);
    assert!(result.sequence > exported.publication_sequence);
    let branch = graph.history.main_branch_id.unwrap();
    let next = save(&mut restored, &branch, b"third");
    assert!(restored.state.commits[&next].sequence > result.sequence);
    drop(restored);
    let reopened = Engine::open(&restored_root.join("state")).unwrap();
    assert_eq!(reopened.graph(&history).unwrap().commits.len(), 3);
}
/// Rebind a complete test package to its altered logical graph using a fresh
/// internally consistent receipt chain. This isolates semantic validation
/// from root/proof mismatch checks without touching the source pool.
fn rebind_test_package(
    package: &Path,
    proof_root: &Path,
    manifest: &HistoryManifest,
    mut proof: PackageProof,
    source_members: &[DiskConfig],
) {
    let members = source_members
        .iter()
        .enumerate()
        .map(|(index, member)| DiskConfig {
            root: proof_root.join(format!("member-{index}")),
            ..member.clone()
        })
        .collect::<Vec<_>>();
    let (root_hash, prepared) = graph::prepare(manifest, MAX_METADATA).unwrap();
    for object in prepared {
        object.store(&members[..1]).unwrap();
    }
    let root = load_object(&members[0], "history-manifest", &root_hash).unwrap();
    let (_, objects) =
        graph::resolve_with_objects(root, |kind, hash| load_object(&members[0], kind, hash))
            .unwrap();
    for (kind, hash, value) in objects {
        let path = if kind == "history-manifest" {
            package.join("manifest.json")
        } else {
            package_object(package, &kind, &hash).unwrap()
        };
        write_json(&path, &value, true).unwrap();
    }
    proof.manifest_hash = root_hash.clone();
    proof
        .candidate
        .history_manifest_hashes
        .insert(manifest.history.history_tree_id.clone(), root_hash);
    let candidate_hash = object_hash("candidate", &proof.candidate).unwrap();
    let sequence = proof.candidate.sequence;
    let accepts = members
        .iter()
        .map(|member| {
            append(
                member,
                "PREPARE",
                sequence,
                &candidate_hash,
                &proof.candidate.snapshot_hash,
            )
            .unwrap();
            append(member, "ACCEPT", sequence, &candidate_hash, &candidate_hash).unwrap()
        })
        .collect();
    proof.publication.candidate_hash = candidate_hash.clone();
    proof.publication.finalize = Finalize {
        candidate_hash: candidate_hash.clone(),
        accepts,
        recovery_id: None,
    };
    let finalize_hash = object_hash("finalize", &proof.publication.finalize).unwrap();
    proof.publication.finalize_receipts = members
        .iter()
        .map(|member| {
            append(
                member,
                "FINALIZE",
                sequence,
                &candidate_hash,
                &finalize_hash,
            )
            .unwrap()
        })
        .collect();
    let publication_hash = object_hash("publication", &proof.publication).unwrap();
    let receipts = members
        .iter()
        .map(|member| {
            append(
                member,
                "PUBLICATION",
                sequence,
                &candidate_hash,
                &publication_hash,
            )
            .unwrap()
        })
        .collect();
    proof.publication_seal = PublicationSeal {
        candidate_hash,
        publication_hash,
        receipts,
    };
    verify_publication_seal(
        &proof.candidate,
        &proof.publication,
        &proof.publication_seal,
    )
    .unwrap();
    write_json(&package.join("publication.json"), &proof, true).unwrap();
}

#[test]
fn tombstoned_storage_identity_is_validated_before_restore_decision() {
    let temp = tempfile::tempdir().unwrap();
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut source = initialize(&temp.path().join("source"));
    let repo = source
        .create_repository("retained identity", "P01", 3, "alice", &new_id("op"))
        .unwrap();
    save(&mut source, &repo.main_branch_id, b"retained");
    let old = source.list_tree(&repo.main_branch_id).unwrap()[0]
        .version_id
        .clone();
    save(&mut source, &repo.main_branch_id, b"current");
    let purge = source
        .create_purge_plan(&repo.history_tree_id, vec![old.clone()], "alice")
        .unwrap();
    source
        .commit_plan(&purge.plan_id, "alice", &new_id("op"))
        .unwrap();
    let package = temp.path().join("package");
    source
        .export_history_package(&repo.history_tree_id, &package)
        .unwrap();
    let (baseline, proof, _) = load_package(&package).unwrap();
    assert!(!package_version(&package, &old).exists());
    for status in ["PURGED", "MIGRATED"] {
        for malformed in ["none", "storage_name", "created_at"] {
            let label = format!("{status}-{malformed}");
            let mut manifest = baseline.clone();
            manifest.content_states.insert(old.clone(), status.into());
            let version = manifest.versions.get_mut(&old).unwrap();
            match malformed {
                "storage_name" => version.storage_name = "invalid.txt".into(),
                "created_at" => version.created_at.clear(),
                _ => (),
            }
            rebind_test_package(
                &package,
                &temp.path().join(format!("proof-{label}")),
                &manifest,
                proof.clone(),
                &source.members(),
            );
            let mut restored = initialize(&temp.path().join(format!("restored-{label}")));
            let before = canonical_json(restored.state()).unwrap();
            let journals = restored
                .members()
                .iter()
                .map(|member| fs::read(log_path(member)).ok())
                .collect::<Vec<_>>();
            let next_slot = restored.next_slot;
            if malformed == "none" {
                manifest.validate().unwrap();
                assert_eq!(
                    offline_check_history_package(&package).unwrap().tombstones,
                    1
                );
                restored
                    .restore_history_package(&package, "P01", 3, "alice", &new_id("op"))
                    .unwrap();
                assert_eq!(restored.state.versions[&old], baseline.versions[&old]);
                assert_eq!(restored.state.content_states[&old], status);
            } else {
                assert_eq!(
                    manifest.validate().unwrap_err().code(),
                    "HISTORY_PACKAGE_INVALID"
                );
                assert_eq!(
                    offline_check_history_package(&package).unwrap_err().code(),
                    "HISTORY_PACKAGE_INVALID"
                );
                assert_eq!(
                    restored
                        .restore_history_package(&package, "P01", 3, "alice", &new_id("op"))
                        .unwrap_err()
                        .code(),
                    "HISTORY_PACKAGE_INVALID"
                );
                assert_eq!(canonical_json(restored.state()).unwrap(), before);
                assert_eq!(restored.next_slot, next_slot);
                assert!(!restored.blocked);
                for (member, journal) in restored.members().iter().zip(journals) {
                    assert_eq!(fs::read(log_path(member)).ok(), journal);
                }
            }
        }
    }
}

#[test]
fn package_preserves_purge_and_rejects_missing_bytes_or_unpublished_manifest() {
    let temp = tempfile::tempdir().unwrap();
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut engine = initialize(&temp.path().join("source"));
    let repo = engine
        .create_repository("历史", "P01", 3, "alice", &new_id("op"))
        .unwrap();
    save(&mut engine, &repo.main_branch_id, b"old");
    let old = engine.list_tree(&repo.main_branch_id).unwrap()[0]
        .version_id
        .clone();
    save(&mut engine, &repo.main_branch_id, b"current");
    let purge = engine
        .create_purge_plan(&repo.history_tree_id, vec![old.clone()], "alice")
        .unwrap();
    engine
        .commit_plan(&purge.plan_id, "alice", &new_id("op"))
        .unwrap();
    let package = temp.path().join("package");
    let report = engine
        .export_history_package(&repo.history_tree_id, &package)
        .unwrap();
    assert_eq!(report.tombstones, 1);
    assert_eq!(report.full_files, 1);
    let mut restored = initialize(&temp.path().join("restored"));
    restored
        .restore_history_package(&package, "P01", 3, "alice", &new_id("op"))
        .unwrap();
    assert_eq!(
        restored.open_content(&old).err().unwrap().code(),
        "CONTENT_PURGED"
    );
    let original = fs::read(package.join("manifest.json")).unwrap();
    let manifest = load_package(&package).unwrap().0;
    let mut root: json_codec::Value = decode_strict(&original).unwrap();
    root["repository"]["name"] = json!("被修改的名字");
    write_json(&package.join("manifest.json"), &root, true).unwrap();
    assert_eq!(
        offline_check_history_package(&package).unwrap_err().code(),
        "HISTORY_PACKAGE_INVALID"
    );
    write_atomic(&package.join("manifest.json"), &original, true).unwrap();
    let version = manifest
        .versions
        .values()
        .find(|v| manifest.content_states[&v.version_id] == "AVAILABLE")
        .unwrap();
    fs::remove_file(
        package_version(&package, &version.version_id)
            .join("content")
            .join(&version.creation_name),
    )
    .unwrap();
    assert!(offline_check_history_package(&package).is_err());
}
#[test]
fn metadata_merges_labels_and_backup_paths_export_restore_without_rewriting_full_proofs() {
    let temp = tempfile::tempdir().unwrap();
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut engine = initialize(&temp.path().join("source"));
    let repo = engine
        .create_repository("metadata recovery", "P01", 1, "alice", &new_id("op"))
        .unwrap();
    save(&mut engine, &repo.main_branch_id, b"original body");
    engine
        .initialize_directory_scope(&repo.repository_id, "", "alice", &new_id("op"))
        .unwrap();
    let entry = engine
        .list_tree(&repo.main_branch_id)
        .unwrap()
        .into_iter()
        .find(|entry| entry.kind == "file")
        .unwrap();
    let original_proof = fs::read(
        version_dir(
            engine.disk("P01").unwrap(),
            &repo.history_tree_id,
            &entry.version_id,
        )
        .join("commit-proof.json"),
    )
    .unwrap();
    let branch = engine
        .file_context(&repo.repository_id, "file.txt")
        .unwrap()
        .branch_id;
    let tagged = engine
        .update_file_node_tags(crate::FileNodeTagsUpdateRequest {
            file_id: &entry.file_id,
            targets: &[FileTagTarget {
                branch_id: branch.clone(),
                source_node_id: entry.version_id.clone(),
                expected_node_id: engine.file_branch_head(&branch).unwrap(),
            }],
            add: &["release".into(), "中文".into()],
            remove: &[],
            location: None,
            principal: "alice",
            operation_id: &new_id("op"),
        })
        .unwrap()
        .result["nodes"][0]["node_id"]
        .as_str()
        .unwrap()
        .to_owned();
    engine
        .set_file_node_note(&entry.file_id, &tagged, "tag note", "alice", &new_id("op"))
        .unwrap();
    let left = engine
        .create_file_branch_from_node(
            &entry.file_id,
            &entry.version_id,
            &tagged,
            "left",
            "alice",
            &new_id("op"),
        )
        .unwrap()
        .result["branch_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let right = engine
        .create_file_branch_from_node(
            &entry.file_id,
            &entry.version_id,
            &tagged,
            "right",
            "alice",
            &new_id("op"),
        )
        .unwrap()
        .result["branch_id"]
        .as_str()
        .unwrap()
        .to_owned();
    save(&mut engine, &left, b"left body");
    save(&mut engine, &right, b"right body");
    let chosen = engine.file_branch_head(&branch).unwrap();
    let inputs = [left.as_str(), right.as_str()]
        .into_iter()
        .map(|id| FileMergeInput {
            branch_id: id.into(),
            expected_node_id: engine.file_branch_head(id).unwrap(),
        })
        .collect::<Vec<_>>();
    let merged = engine
        .merge_file_branches(crate::FileBranchMergeRequest {
            file_id: &entry.file_id,
            inputs: &inputs,
            chosen_node_id: &chosen,
            surviving_branch_id: &left,
            location: None,
            principal: "alice",
            operation_id: &new_id("op"),
        })
        .unwrap()
        .result["node_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(merged, chosen);
    let versions = canonical_json(&engine.state.versions).unwrap();
    engine
        .configure_backups(crate::BackupConfigurationRequest {
            repository_id: &repo.repository_id,
            path: "file.txt",
            target_copies: 3,
            backup_prefixes: &BTreeMap::from([
                ("B01".into(), "自己选择/备份".into()),
                ("C01".into(), "独立前缀".into()),
            ]),
            expected_generation: 0,
            principal: "alice",
            operation_id: &new_id("op"),
        })
        .unwrap();
    assert_eq!(canonical_json(&engine.state.versions).unwrap(), versions);
    assert_eq!(
        fs::read(
            version_dir(
                engine.disk("P01").unwrap(),
                &repo.history_tree_id,
                &entry.version_id
            )
            .join("commit-proof.json")
        )
        .unwrap(),
        original_proof
    );
    let obsolete = engine
        .state
        .versions
        .values()
        .filter(|version| {
            version.file_id == entry.file_id && version.version_id != entry.version_id
        })
        .map(|version| version.version_id.clone())
        .collect::<Vec<_>>();
    let purge = engine
        .create_purge_plan(&repo.history_tree_id, obsolete, "alice")
        .unwrap();
    engine
        .commit_plan(&purge.plan_id, "alice", &new_id("op"))
        .unwrap();
    let manifest = HistoryManifest::from_state(&engine.state, &repo.history_tree_id).unwrap();
    assert_eq!(manifest.file_metadata_nodes.len(), 2);
    assert_eq!(manifest.file_branch_merges.len(), 1);
    assert_eq!(manifest.file_branch_aliases[&right], left);
    let mut cycle = manifest.clone();
    cycle
        .file_metadata_nodes
        .get_mut(&tagged)
        .unwrap()
        .parent_node_ids = vec![merged.clone()];
    assert!(cycle.validate().is_err());
    let mut alias_cycle = manifest.clone();
    alias_cycle
        .file_branch_aliases
        .insert(left.clone(), right.clone());
    assert!(alias_cycle.validate().is_err());
    let mut scope = manifest.clone();
    scope
        .file_branch_merges
        .values_mut()
        .next()
        .unwrap()
        .input_branch_ids[0] = repo.main_branch_id.clone();
    assert!(scope.validate().is_err());
    let package = temp.path().join("package");
    engine
        .export_history_package(&repo.history_tree_id, &package)
        .unwrap();
    let mut restored = initialize(&temp.path().join("restored"));
    restored
        .restore_history_package(&package, "P01", 3, "alice", &new_id("op"))
        .unwrap();
    assert_eq!(
        canonical_json(&restored.state.file_metadata_nodes).unwrap(),
        canonical_json(&manifest.file_metadata_nodes).unwrap()
    );
    assert_eq!(
        canonical_json(&restored.state.file_branch_merges).unwrap(),
        canonical_json(&manifest.file_branch_merges).unwrap()
    );
    assert_eq!(
        restored.state.file_branch_aliases,
        manifest.file_branch_aliases
    );
    assert_eq!(restored.state.file_path_notes, manifest.file_path_notes);
    assert_eq!(restored.state.file_backup_paths, manifest.file_backup_paths);
    assert_eq!(
        restored.state.file_backup_policies,
        manifest.file_backup_policies
    );
    assert_eq!(
        restored.file_node_tags(&entry.file_id, &merged).unwrap(),
        BTreeSet::from(["release".into(), "中文".into()])
    );
    for disk in &restored.config.disks {
        assert!(offline_check(&disk.root).unwrap().issues.is_empty());
    }
    drop(restored);
    let restored = Engine::open(&temp.path().join("restored/state")).unwrap();
    assert_eq!(restored.file_branch_head(&left).unwrap(), merged);
}

#[test]
fn history_notes_use_the_current_write_format_without_legacy_limits() {
    let temporary = tempfile::tempdir().unwrap();
    let mut engine = initialize(temporary.path());
    let repo = engine
        .create_repository("notes", "P01", 3, "alice", &new_id("op"))
        .unwrap();
    save(&mut engine, &repo.main_branch_id, b"note target");
    let manifest = HistoryManifest::from_state(engine.state(), &repo.history_tree_id).unwrap();
    let version = manifest.versions.values().next().unwrap();
    for note in [String::new(), "x".repeat(128), "界".repeat(42)] {
        let mut valid = manifest.clone();
        valid
            .file_notes
            .insert(version.file_id.clone(), note.clone());
        valid.version_notes.insert(version.version_id.clone(), note);
        valid.validate().unwrap();
    }
    for note in [
        "x".repeat(129),
        "x".repeat(4096),
        "界".repeat(43),
        "line\nbreak".into(),
        "zero\0byte".into(),
    ] {
        for scope in ["file", "version"] {
            let mut invalid = manifest.clone();
            if scope == "file" {
                invalid
                    .file_notes
                    .insert(version.file_id.clone(), note.clone());
            } else {
                invalid
                    .version_notes
                    .insert(version.version_id.clone(), note.clone());
            }
            assert_eq!(
                invalid.validate().unwrap_err().code(),
                "HISTORY_PACKAGE_INVALID",
                "{scope}"
            );
        }
    }
}

#[test]
fn commit_namespace_envelope_rejects_malformed_entries() {
    let temporary = tempfile::tempdir().unwrap();
    fs::set_permissions(temporary.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut engine = initialize(temporary.path());
    let repo = engine
        .create_repository("namespace envelope", "P01", 3, "alice", &new_id("op"))
        .unwrap();
    save_path(
        &mut engine,
        &repo.main_branch_id,
        "folder/file.txt",
        b"valid file",
    );
    let manifest = HistoryManifest::from_state(engine.state(), &repo.history_tree_id).unwrap();
    assert!(
        manifest
            .commits
            .values()
            .flat_map(|commit| &commit.entries)
            .any(|entry| entry.kind == "directory")
    );
    assert!(
        manifest
            .commits
            .values()
            .flat_map(|commit| &commit.entries)
            .any(|entry| entry.kind == "file")
    );
    let valid_package = temporary.path().join("valid-package");
    engine
        .export_history_package(&repo.history_tree_id, &valid_package)
        .unwrap();
    offline_check_history_package(&valid_package).unwrap();

    let version = manifest.versions.values().next().unwrap();
    for case in [
        "kind",
        "entry_id",
        "file_id",
        "version_id",
        "directory_payload",
    ] {
        let mut entry = Entry {
            entry_id: new_id("entry"),
            path: "ghost".into(),
            name: "ghost".into(),
            lookup_key: "ghost".into(),
            kind: "directory".into(),
            file_id: String::new(),
            version_id: String::new(),
            size: 0,
        };
        match case {
            "kind" => entry.kind = "unsupported".into(),
            "entry_id" => entry.entry_id = "not-an-entry-id".into(),
            "file_id" | "version_id" => {
                entry.kind = "file".into();
                entry.file_id = version.file_id.clone();
                entry.version_id = version.version_id.clone();
                entry.size = version.logical_size;
                if case == "file_id" {
                    entry.file_id = "not-a-file-id".into();
                } else {
                    entry.version_id = "not-a-version-id".into();
                }
            }
            "directory_payload" => {
                entry.file_id = new_id("file");
                entry.version_id = new_id("ver");
                entry.size = 1;
            }
            _ => unreachable!(),
        }
        let mut invalid = manifest.clone();
        invalid
            .commits
            .get_mut(&manifest.history.root_commit_id)
            .unwrap()
            .entries = vec![entry];
        assert!(invalid.validate().is_err(), "{case}");
    }
}

#[test]
fn history_branch_names_match_the_current_128_byte_write_limit() {
    let temporary = tempfile::tempdir().unwrap();
    fs::set_permissions(temporary.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut engine = initialize(temporary.path());
    let repo = engine
        .create_repository("branch name limits", "P01", 3, "alice", &new_id("op"))
        .unwrap();
    let root = engine.state().histories[&repo.history_tree_id]
        .root_commit_id
        .clone();
    let name = "界".repeat(42) + "ab";
    assert_eq!(name.len(), 128);
    let branch = engine
        .create_branch(&root, &name, "alice", &new_id("op"))
        .unwrap();
    let directory = engine
        .create_directory_branches(&repo.repository_id, "", &name, "alice", &new_id("op"))
        .unwrap();
    let directory = directory.result["directory_branch_id"].as_str().unwrap();
    let manifest = HistoryManifest::from_state(engine.state(), &repo.history_tree_id).unwrap();
    let package = temporary.path().join("valid-package");
    engine
        .export_history_package(&repo.history_tree_id, &package)
        .unwrap();
    offline_check_history_package(&package).unwrap();

    for name in ["a".repeat(129), "界".repeat(43)] {
        for scope in ["branch", "directory_branch"] {
            let mut invalid = manifest.clone();
            if scope == "branch" {
                invalid.branches.get_mut(&branch.branch_id).unwrap().name = name.clone();
            } else {
                invalid.directory_branches.get_mut(directory).unwrap().name = name.clone();
            }
            assert_eq!(
                invalid.validate().unwrap_err().code(),
                "METADATA_TOO_LONG",
                "{scope}: {name}"
            );
        }
    }
}

#[test]
fn namespace_deletion_facts_validate_without_snapshot_or_content_dependencies() {
    use storage_contract::{DeletedPath, NamespaceDeletions, NamespaceEntryKind};
    let temp = tempfile::tempdir().unwrap();
    let mut engine = initialize(temp.path());
    let repo = engine
        .create_repository("deletion facts", "P01", 1, "alice", &new_id("op"))
        .unwrap();
    let mut manifest = HistoryManifest::from_state(engine.state(), &repo.history_tree_id).unwrap();
    let root = manifest.history.root_commit_id.clone();
    // An intermediate path may be absent from every snapshot; repeated paths
    // represent separate sequential operations rather than duplicate identities.
    manifest.commits.get_mut(&root).unwrap().namespace_deletions = Some(NamespaceDeletions {
        timestamp: 1,
        paths: vec![
            DeletedPath {
                path: "parent/intermediate.txt".into(),
                kind: NamespaceEntryKind::File
            };
            2
        ],
    });
    manifest.validate().unwrap();
    assert!(manifest.versions.is_empty());
    let encoded = canonical_json(&manifest).unwrap();
    let decoded: HistoryManifest = json_codec::from_slice(&encoded).unwrap();
    assert_eq!(
        decoded.commits[&root].namespace_deletions,
        manifest.commits[&root].namespace_deletions
    );
    for path in ["", "../escape", "/absolute"] {
        let mut invalid = manifest.clone();
        invalid
            .commits
            .get_mut(&root)
            .unwrap()
            .namespace_deletions
            .as_mut()
            .unwrap()
            .paths[0]
            .path = path.into();
        assert!(invalid.validate().is_err(), "accepted {path:?}");
    }
    for timestamp in [u64::MAX, i64::MAX as u64] {
        let mut invalid = manifest.clone();
        invalid
            .commits
            .get_mut(&root)
            .unwrap()
            .namespace_deletions
            .as_mut()
            .unwrap()
            .timestamp = timestamp;
        assert!(invalid.validate().is_err());
    }
    manifest
        .commits
        .get_mut(&root)
        .unwrap()
        .namespace_deletions
        .as_mut()
        .unwrap()
        .paths
        .clear();
    assert!(manifest.validate().is_err());
}

#[test]
fn namespace_deletions_survive_export_restore_and_replay_without_operation_records() {
    let temp = tempfile::tempdir().unwrap();
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut engine = initialize(temp.path());
    let repo = engine
        .create_repository("deleted namespace", "P01", 1, "alice", &new_id("op"))
        .unwrap();
    save_path(
        &mut engine,
        &repo.main_branch_id,
        "folder/file.txt",
        b"retained historical body",
    );
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "alice")
        .unwrap();
    engine
        .namespace_operation(
            &workspace.workspace_id,
            NamespaceOperation {
                kind: "delete_tree".into(),
                path: "folder".into(),
                destination: None,
            },
        )
        .unwrap();
    let request = CommitRequest {
        workspace_id: workspace.workspace_id,
        operation_id: new_id("op"),
        principal: "alice".into(),
        expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token,
        message: "remove folder".into(),
        request_digest: None,
    };
    let result = engine.commit_workspace(request.clone()).unwrap();
    let commit_id = result.commit_id.unwrap();
    let facts = engine.state().commits[&commit_id]
        .namespace_deletions
        .clone()
        .unwrap();
    let paths = facts
        .paths
        .iter()
        .map(|fact| fact.path.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(paths, BTreeSet::from(["folder", "folder/file.txt"]));
    assert_eq!(facts.paths.len(), 2);
    let count = engine.state().commits.len();
    assert_eq!(
        engine
            .commit_workspace(request)
            .unwrap()
            .commit_id
            .as_deref(),
        Some(commit_id.as_str())
    );
    assert_eq!(engine.state().commits.len(), count);
    let package = temp.path().join("deleted-package");
    let report = engine
        .export_history_package(&repo.history_tree_id, &package)
        .unwrap();
    offline_check_history_package(&package).unwrap();
    let target = tempfile::tempdir().unwrap();
    let mut restored = initialize(target.path());
    restored
        .restore_history_package(&package, "P01", 1, "alice", &new_id("op"))
        .unwrap();
    assert_eq!(
        restored.state().commits[&commit_id]
            .namespace_deletions
            .as_ref(),
        Some(&facts)
    );
    assert!(
        !restored
            .state()
            .operations
            .contains_key(&engine.state().commits[&commit_id].operation_id)
    );
    assert_eq!(restored.state().versions.len(), report.versions);
}

#[test]
fn failed_workspace_namespace_sequence_does_not_publish_deletion_facts() {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = initialize(temp.path());
    let repo = engine
        .create_repository("failed deletion", "P01", 1, "alice", &new_id("op"))
        .unwrap();
    save(&mut engine, &repo.main_branch_id, b"still present");
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "alice")
        .unwrap();
    for path in ["file.txt", "missing.txt"] {
        engine
            .namespace_operation(
                &workspace.workspace_id,
                NamespaceOperation {
                    kind: "delete".into(),
                    path: path.into(),
                    destination: None,
                },
            )
            .unwrap();
    }
    let commits = engine.state().commits.clone();
    let head = engine.state().branches[&repo.main_branch_id]
        .head_commit_id
        .clone();
    assert!(
        engine
            .commit_workspace(CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: new_id("op"),
                principal: "alice".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "must not publish".into(),
                request_digest: None,
            })
            .is_err()
    );
    assert_eq!(engine.state().commits, commits);
    assert_eq!(
        engine.state().branches[&repo.main_branch_id].head_commit_id,
        head
    );
    assert!(
        engine
            .list_tree(&repo.main_branch_id)
            .unwrap()
            .iter()
            .any(|entry| entry.path == "file.txt")
    );
}

#[test]
fn net_zero_namespace_sequence_persists_deletion_once() {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = initialize(temp.path());
    let repo = engine
        .create_repository("temporary namespace", "P01", 1, "alice", &new_id("op"))
        .unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "alice")
        .unwrap();
    for kind in ["mkdir", "delete"] {
        engine
            .namespace_operation(
                &workspace.workspace_id,
                NamespaceOperation {
                    kind: kind.into(),
                    path: "temporary".into(),
                    destination: None,
                },
            )
            .unwrap();
    }
    let request = CommitRequest {
        workspace_id: workspace.workspace_id,
        operation_id: new_id("op"),
        principal: "alice".into(),
        expected_head: workspace.base_head.clone(),
        fencing_token: workspace.fencing_token,
        message: "temporary directory".into(),
        request_digest: None,
    };
    let commit = engine
        .commit_workspace(request.clone())
        .unwrap()
        .commit_id
        .unwrap();
    assert_ne!(commit, workspace.base_head);
    assert!(engine.state().commits[&commit].entries.is_empty());
    let facts = engine.state().commits[&commit]
        .namespace_deletions
        .as_ref()
        .unwrap();
    assert_eq!(facts.paths.len(), 1);
    assert_eq!(facts.paths[0].path, "temporary");
    assert_eq!(
        facts.paths[0].kind,
        storage_contract::NamespaceEntryKind::Directory
    );
    let count = engine.state().commits.len();
    assert_eq!(
        engine.commit_workspace(request).unwrap().commit_id.as_ref(),
        Some(&commit)
    );
    assert_eq!(engine.state().commits.len(), count);
}

#[test]
fn manifest_record_projection_is_scoped_shared_and_mutation_isolated() {
    let temporary = tempfile::tempdir().unwrap();
    let mut engine = initialize(temporary.path());
    let first = engine
        .create_repository("first", "P01", 1, "alice", &new_id("op"))
        .unwrap();
    let second = engine
        .create_repository("second", "P01", 1, "alice", &new_id("op"))
        .unwrap();
    for repo in [&first, &second] {
        save_path(
            &mut engine,
            &repo.main_branch_id,
            "folder/file.txt",
            b"payload",
        );
        engine
            .initialize_directory_scope(&repo.repository_id, "folder", "alice", &new_id("op"))
            .unwrap();
    }
    let mut source = engine.state().clone();
    let mut manifest = HistoryManifest::from_state(&source, &first.history_tree_id).unwrap();
    assert!(!manifest.directory_trees.is_empty());
    let expected_commits = source
        .commits
        .iter()
        .filter(|(_, commit)| commit.history_tree_id == first.history_tree_id)
        .map(|(id, _)| id.clone())
        .collect::<Vec<_>>();
    let expected_trees = source
        .directory_trees
        .iter()
        .filter(|(_, tree)| {
            source.directory_branches[&tree.directory_branch_id].repository_id
                == first.repository_id
        })
        .map(|(id, _)| id.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        manifest.commits.keys().cloned().collect::<Vec<_>>(),
        expected_commits
    );
    assert_eq!(
        manifest.directory_trees.keys().cloned().collect::<Vec<_>>(),
        expected_trees
    );
    for (id, commit) in manifest.commits.iter() {
        assert!(std::ptr::eq(commit, &source.commits[id]));
    }
    for (id, tree) in manifest.directory_trees.iter() {
        assert!(std::ptr::eq(tree, &source.directory_trees[id]));
    }
    let before = canonical_json(&manifest).unwrap();
    let frozen = manifest.clone();
    let mut destination = State::default();
    destination.commits.extend_shared(&manifest.commits);
    destination
        .directory_trees
        .extend_shared(&manifest.directory_trees);
    let commit = manifest.commits.keys().next().unwrap().clone();
    let tree = manifest.directory_trees.keys().next().unwrap().clone();
    manifest
        .commits
        .get_mut(&commit)
        .unwrap()
        .message
        .push_str(" local");
    manifest
        .directory_trees
        .get_mut(&tree)
        .unwrap()
        .path
        .push_str("/local");
    source
        .commits
        .get_mut(&commit)
        .unwrap()
        .message
        .push_str(" source");
    source
        .directory_trees
        .get_mut(&tree)
        .unwrap()
        .path
        .push_str("/source");
    assert_eq!(
        manifest.commits[&commit].message,
        format!("{} local", frozen.commits[&commit].message)
    );
    assert_eq!(
        source.commits[&commit].message,
        format!("{} source", frozen.commits[&commit].message)
    );
    assert_eq!(
        manifest.directory_trees[&tree].path,
        format!("{}/local", frozen.directory_trees[&tree].path)
    );
    assert_eq!(
        source.directory_trees[&tree].path,
        format!("{}/source", frozen.directory_trees[&tree].path)
    );
    assert_eq!(canonical_json(&frozen).unwrap(), before);
    assert_eq!(destination.commits, frozen.commits);
    assert_eq!(destination.directory_trees, frozen.directory_trees);
    destination
        .commits
        .get_mut(&commit)
        .unwrap()
        .message
        .push_str(" destination");
    destination
        .directory_trees
        .get_mut(&tree)
        .unwrap()
        .path
        .push_str("/destination");
    assert_eq!(canonical_json(&frozen).unwrap(), before);
    drop(manifest);
    assert_eq!(canonical_json(&frozen).unwrap(), before);
}
