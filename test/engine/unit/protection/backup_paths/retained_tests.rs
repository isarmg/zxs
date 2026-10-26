use super::*;
use std::os::unix::fs::PermissionsExt;

fn configure(
    engine: &mut Engine,
    repo: &Repository,
    path: &str,
    copies: u32,
    values: &[(&str, &str)],
    generation: u64,
) -> OperationResult {
    engine
        .configure_backups(BackupConfigurationRequest {
            repository_id: &repo.repository_id,
            path,
            target_copies: copies,
            backup_prefixes: &prefixes(values),
            expected_generation: generation,
            principal: "admin",
            operation_id: storage_contract::OperationId::generate(),
        })
        .unwrap()
}

fn configured_source(root: &Path) -> (Engine, Repository, Entry, String) {
    let (mut engine, repo) = initialize(root, 1, None);
    let inside = save(&mut engine, &repo, "source/inside.txt", b"inside");
    let configured = configure(&mut engine, &repo, "source", 2, &[("B01", "old")], 0);
    let policy = configured.result["policy"]["policy_id"]
        .as_str()
        .unwrap()
        .to_owned();
    (engine, repo, inside, policy)
}

#[test]
fn source_reconfiguration_preserves_one_shared_snapshot_for_moved_files() {
    let temp = tempfile::tempdir().unwrap();
    let (mut engine, repo, inside, current) = configured_source(temp.path());
    let first = save(&mut engine, &repo, "source/first.txt", b"first");
    let second = save(&mut engine, &repo, "source/second.txt", b"second");
    rename(&mut engine, &repo, "source/first.txt", "first.txt");
    rename(&mut engine, &repo, "source/second.txt", "second.txt");
    let changed = configure(
        &mut engine,
        &repo,
        "source",
        3,
        &[("B01", "new"), ("C01", "cold")],
        1,
    );
    assert_eq!(changed.result["policy"]["policy_id"], current);
    assert_eq!(changed.result["policy"]["generation"], "2");
    assert_eq!(changed.result["file_count"], "1");
    assert_eq!(engine.state.placements[&inside.version_id].target_copies, 3);
    let retained = engine.state.file_backup_policies[&first.file_id].clone();
    assert_ne!(retained, current);
    assert_eq!(engine.state.file_backup_policies[&second.file_id], retained);
    assert_eq!(engine.state.protection_policies.len(), 2);
    assert!(!engine.state.protection_policies[&retained].inheritable);
    for (entry, path) in [(&first, "first.txt"), (&second, "second.txt")] {
        let read = engine
            .backup_configuration(&repo.repository_id, path)
            .unwrap();
        assert_eq!(read["target_copies"], 2);
        assert_eq!(read["inherited"], false);
        assert_eq!(read["effective_policy_id"], retained);
        assert_eq!(engine.state.placements[&entry.version_id].target_copies, 2);
        assert_eq!(
            engine.state.file_backup_paths[&entry.file_id]["B01"],
            format!("old/{}", entry.name())
        );
    }
    let added = save(&mut engine, &repo, "source/added.txt", b"new source file");
    assert_eq!(engine.state.placements[&added.version_id].target_copies, 3);
    assert_eq!(
        engine.state.file_backup_paths[&added.file_id]["B01"],
        "new/added.txt"
    );
    let updated = save(&mut engine, &repo, "first.txt", b"moved update");
    assert_eq!(
        engine.state.placements[&updated.version_id].target_copies,
        2
    );
    // Neither an ancestor update nor a later source update can adopt the snapshot.
    configure(&mut engine, &repo, "", 1, &[], 0);
    configure(&mut engine, &repo, "source", 1, &[], 2);
    assert_eq!(engine.state.file_backup_policies[&first.file_id], retained);
    assert_eq!(engine.state.file_backup_policies[&second.file_id], retained);
    assert_eq!(
        engine
            .state
            .protection_policies
            .values()
            .filter(|policy| !policy.inheritable)
            .count(),
        1
    );
    let before = canonical_json(engine.state()).unwrap();
    for id in [&current, &retained] {
        let policy = engine.state.protection_policies[id].clone();
        let failure = engine
            .update_protection_policy(ProtectionPolicyUpdate {
                policy_id: id,
                repository_id: &repo.repository_id,
                path_prefix: &policy.path_prefix,
                target_copies: 1,
                expected_generation: policy.generation,
                principal: "admin",
                operation_id: storage_contract::OperationId::generate(),
            })
            .unwrap_err();
        assert_eq!(
            failure.code(),
            storage_contract::ErrorCode::BackupConfigurationRequired
        );
        assert_eq!(canonical_json(engine.state()).unwrap(), before);
    }
    drop(engine);
    let mut engine = Engine::open(&temp.path().join("state")).unwrap();
    assert_eq!(
        engine
            .backup_configuration(&repo.repository_id, "first.txt")
            .unwrap()["target_copies"],
        2
    );
    let updated = save(&mut engine, &repo, "second.txt", b"after reopen");
    assert_eq!(
        engine.state.placements[&updated.version_id].target_copies,
        2
    );
    configure(&mut engine, &repo, "first.txt", 1, &[], 0);
    assert!(engine.state.protection_policies.contains_key(&retained));
    configure(&mut engine, &repo, "second.txt", 1, &[], 0);
    assert!(!engine.state.protection_policies.contains_key(&retained));
    assert!(engine.state.protection_policies[&current].inheritable);
}

#[test]
fn retained_settings_replay_once_and_restore_without_becoming_inherited() {
    let temp = tempfile::tempdir().unwrap();
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let (mut engine, repo, _, current) = configured_source(&temp.path().join("original"));
    let moved = save(&mut engine, &repo, "source/moved.txt", b"moved");
    rename(&mut engine, &repo, "source/moved.txt", "moved.txt");
    let operation = storage_contract::OperationId::generate();
    let values = prefixes(&[("B01", "new"), ("C01", "cold")]);
    let request = BackupConfigurationRequest {
        repository_id: &repo.repository_id,
        path: "source",
        target_copies: 3,
        backup_prefixes: &values,
        expected_generation: 1,
        principal: "admin",
        operation_id: operation,
    };
    engine.inject_fault_once("after_finalize");
    assert_eq!(
        engine.configure_backups(request).unwrap_err().code(),
        storage_contract::ErrorCode::InDoubt
    );
    drop(engine);
    let mut engine = Engine::open(&temp.path().join("original/state")).unwrap();
    let replay = engine.configure_backups(request).unwrap();
    let retained = engine.state.file_backup_policies[&moved.file_id].clone();
    let before = canonical_json(engine.state()).unwrap();
    assert_eq!(
        canonical_json(&engine.configure_backups(request).unwrap()).unwrap(),
        canonical_json(&replay).unwrap()
    );
    assert_eq!(canonical_json(engine.state()).unwrap(), before);
    assert_eq!(
        engine
            .state
            .protection_policies
            .values()
            .filter(|policy| !policy.inheritable)
            .count(),
        1
    );
    let package = temp.path().join("package");
    engine
        .export_history_package(&repo.history_tree_id, &package)
        .unwrap();
    let destination = temp.path().join("restored");
    let mut restored = Engine::initialize(
        &destination.join("state"),
        Config::development([
            destination.join("p"),
            destination.join("b"),
            destination.join("c"),
        ]),
    )
    .unwrap();
    restored
        .restore_history_package(
            &package,
            "P01",
            2,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    assert!(restored.state.protection_policies[&current].inheritable);
    assert!(!restored.state.protection_policies[&retained].inheritable);
    assert_eq!(restored.state.protection_policies[&current].generation, 1);
    assert_eq!(restored.state.protection_policies[&retained].generation, 1);
    assert_eq!(
        restored.state.file_backup_policies[&moved.file_id],
        retained
    );
    assert_eq!(
        restored
            .backup_configuration(&repo.repository_id, "source")
            .unwrap()["policy_id"],
        current
    );
    let added = save(&mut restored, &repo, "source/restored.txt", b"restored");
    assert_eq!(
        restored.state.file_backup_paths[&added.file_id]["B01"],
        "new/restored.txt"
    );
    assert_eq!(
        restored.state.file_backup_paths[&moved.file_id]["B01"],
        "old/moved.txt"
    );
    drop(restored);
    Engine::open(&temp.path().join("restored/state")).unwrap();
}

#[test]
fn protection_policy_role_is_required_and_empty_current_rules_survive_pruning() {
    let temp = tempfile::tempdir().unwrap();
    let (mut engine, repo, _, current) = configured_source(temp.path());
    let mut value = json_codec::to_value(&engine.state.protection_policies[&current]).unwrap();
    value.as_object_mut().unwrap().remove("inheritable");
    assert!(json_codec::from_value::<ProtectionPolicy>(value.clone()).is_err());
    value["inheritable"] = Value::Null;
    assert!(json_codec::from_value::<ProtectionPolicy>(value).is_err());
    engine
        .file_namespace(
            &repo.repository_id,
            NamespaceOperation::parse(
                storage_contract::NamespaceOperationKind::Mkdir,
                "empty",
                None,
            )
            .unwrap(),
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let empty = configure(&mut engine, &repo, "empty", 1, &[], 0);
    let id = empty.result["policy"]["policy_id"].as_str().unwrap();
    configure(&mut engine, &repo, "source/inside.txt", 1, &[], 0);
    assert!(engine.state.protection_policies[id].inheritable);
    assert_eq!(
        engine
            .backup_configuration(&repo.repository_id, "empty")
            .unwrap()["policy_id"],
        id
    );
}
