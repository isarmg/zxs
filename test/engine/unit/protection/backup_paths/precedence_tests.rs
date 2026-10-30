use super::*;

fn path_policy(
    engine: &mut Engine,
    repo: &Repository,
    id: &str,
    path: &str,
    copies: u32,
    generation: u64,
) -> Result<OperationResult> {
    engine.update_protection_policy(ProtectionPolicyUpdate {
        policy_id: id,
        repository_id: &repo.repository_id,
        path_prefix: path,
        target_copies: copies,
        expected_generation: generation,
        principal: "admin",
        operation_id: storage_contract::OperationId::generate(),
    })
}

fn configure_scope(
    engine: &mut Engine,
    repo: &Repository,
    path: &str,
    copies: u32,
    generation: u64,
) -> String {
    let values = if copies == 1 {
        BTreeMap::new()
    } else {
        prefixes(&[("B01", "backup")])
    };
    engine
        .configure_backups(BackupConfigurationRequest {
            repository_id: &repo.repository_id,
            path,
            target_copies: copies,
            backup_prefixes: &values,
            expected_generation: generation,
            principal: "admin",
            operation_id: storage_contract::OperationId::generate(),
        })
        .unwrap()
        .result["policy"]["policy_id"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn generic_policy_collisions_follow_the_current_directory_or_file_scope_path() {
    for (source, destination, write_path) in [
        ("source", "renamed", "renamed/new.txt"),
        ("source/file.txt", "renamed.txt", "renamed.txt"),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let (mut engine, repo) = initialize(temp.path(), 1, None);
        save(&mut engine, &repo, "source/file.txt", b"before");
        let scoped = configure_scope(&mut engine, &repo, source, 2, 0);
        rename(&mut engine, &repo, source, destination);

        let generic = new_id("policy");
        let before = canonical_json(engine.state()).unwrap();
        let error = path_policy(
            &mut engine,
            &repo,
            &generic,
            &destination.to_uppercase(),
            1,
            0,
        )
        .unwrap_err();
        assert_eq!(error.code(), storage_contract::ErrorCode::NameCollision);
        assert_eq!(canonical_json(engine.state()).unwrap(), before);

        // A moved stable scope no longer reserves its old spelling.
        path_policy(&mut engine, &repo, &generic, source, 1, 0).unwrap();
        let before = canonical_json(engine.state()).unwrap();
        let error = path_policy(&mut engine, &repo, &generic, destination, 1, 1).unwrap_err();
        assert_eq!(error.code(), storage_contract::ErrorCode::NameCollision);
        assert_eq!(canonical_json(engine.state()).unwrap(), before);
        let read = engine
            .backup_configuration(&repo.repository_id, destination)
            .unwrap();
        super::detached_parity::check(&engine, &repo.repository_id, destination, &read);
        assert_eq!(read.policy_id.as_deref().unwrap(), scoped);
        assert_eq!(read.effective_policy_id.as_deref().unwrap(), scoped);
        assert_eq!(read.target_copies, 2);
        assert_eq!(read.backup_prefixes["B01"], "backup");
        let written = save(&mut engine, &repo, write_path, b"after");
        assert_eq!(
            engine.state.placements[&written.version_id].target_copies,
            2
        );
        assert_eq!(engine.state.file_backup_policies[&written.file_id], scoped);
    }
}

#[test]
fn renamed_stable_scope_wins_equal_depth_in_either_creation_and_id_order() {
    for generic_first in [false, true] {
        for generic_id in [
            "policy_00000000000000000000000000000000",
            "policy_ffffffffffffffffffffffffffffffff",
        ] {
            let temp = tempfile::tempdir().unwrap();
            let (mut engine, repo) = initialize(temp.path(), 1, None);
            save(&mut engine, &repo, "source/file.txt", b"before");
            if generic_first {
                path_policy(&mut engine, &repo, generic_id, "renamed", 1, 0).unwrap();
            }
            let scoped = configure_scope(&mut engine, &repo, "source", 2, 0);
            if !generic_first {
                path_policy(&mut engine, &repo, generic_id, "renamed", 1, 0).unwrap();
            }
            let single = save(&mut engine, &repo, "source/single.txt", b"single");
            let single_policy = configure_scope(&mut engine, &repo, "source/single.txt", 1, 0);
            let child_policy = new_id("policy");
            path_policy(&mut engine, &repo, &child_policy, "renamed/child", 3, 0).unwrap();
            rename(&mut engine, &repo, "source", "renamed");

            let read = engine
                .backup_configuration(&repo.repository_id, "renamed")
                .unwrap();
            super::detached_parity::check(&engine, &repo.repository_id, "renamed", &read);
            assert_eq!(read.policy_id.as_deref().unwrap(), scoped);
            assert_eq!(read.effective_policy_id.as_deref().unwrap(), scoped);
            assert_eq!(read.target_copies, 2);
            assert_eq!(read.backup_prefixes["B01"], "backup");
            assert_eq!(
                json_codec::json!(read.route_slot_ids),
                json!(["P01", "B01"])
            );
            let added = save(&mut engine, &repo, "renamed/new.txt", b"new");
            assert_eq!(engine.state.placements[&added.version_id].target_copies, 2);
            assert_eq!(engine.state.file_backup_policies[&added.file_id], scoped);
            assert_eq!(
                engine.state.file_backup_paths[&added.file_id]["B01"],
                "backup/new.txt"
            );

            // Specific child rules and explicit one-copy file bindings still win.
            let child = save(&mut engine, &repo, "renamed/child/new.txt", b"child");
            assert_eq!(engine.state.placements[&child.version_id].target_copies, 3);
            let read = engine
                .backup_configuration(&repo.repository_id, "renamed/child")
                .unwrap();
            super::detached_parity::check(&engine, &repo.repository_id, "renamed/child", &read);
            assert_eq!(read.effective_policy_id.as_deref().unwrap(), child_policy);
            assert_eq!(read.target_copies, 3);
            assert_eq!(configure_scope(&mut engine, &repo, "renamed", 2, 1), scoped);
            assert_eq!(engine.state.placements[&child.version_id].target_copies, 3);
            assert_eq!(
                engine.state.file_backup_policies[&single.file_id],
                single_policy
            );
            drop(engine);

            let mut engine = Engine::open(&temp.path().join("state")).unwrap();
            let after = save(&mut engine, &repo, "renamed/after.txt", b"after reopen");
            assert_eq!(engine.state.placements[&after.version_id].target_copies, 2);
            assert_eq!(engine.state.file_backup_policies[&after.file_id], scoped);
            let single = save(&mut engine, &repo, "renamed/single.txt", b"still single");
            assert_eq!(engine.state.placements[&single.version_id].target_copies, 1);
            assert_eq!(
                engine.state.file_backup_policies[&single.file_id],
                single_policy
            );
        }
    }
}

#[test]
fn root_scope_adopts_existing_path_rule_and_rejects_parallel_rules() {
    for generic_first in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let (mut engine, repo) = initialize(temp.path(), 1, None);
        let generic = new_id("policy");
        if generic_first {
            path_policy(&mut engine, &repo, &generic, "", 1, 0).unwrap();
        }
        let scoped = configure_scope(&mut engine, &repo, "", 2, u64::from(generic_first));
        if generic_first {
            assert_eq!(scoped, generic);
        }
        assert_eq!(engine.state.protection_policies.len(), 1);
        let error = path_policy(&mut engine, &repo, &new_id("policy"), "", 1, 0).unwrap_err();
        assert_eq!(error.code(), storage_contract::ErrorCode::NameCollision);
        let read = engine
            .backup_configuration(&repo.repository_id, "")
            .unwrap();
        super::detached_parity::check(&engine, &repo.repository_id, "", &read);
        assert_eq!(read.policy_id.as_deref().unwrap(), scoped);
        assert_eq!(read.effective_policy_id.as_deref().unwrap(), scoped);
        assert_eq!(read.generation, u64::from(generic_first) + 1);
        assert_eq!(read.target_copies, 2);
        let added = save(&mut engine, &repo, "root.txt", b"new root file");
        assert_eq!(engine.state.placements[&added.version_id].target_copies, 2);
        assert_eq!(engine.state.file_backup_policies[&added.file_id], scoped);
        assert_eq!(
            engine.state.file_backup_paths[&added.file_id]["B01"],
            "backup/root.txt"
        );
    }
}

#[test]
fn exact_path_rule_does_not_hide_effective_file_binding_or_retained_settings() {
    let temp = tempfile::tempdir().unwrap();
    let (mut engine, repo) = initialize(temp.path(), 1, None);
    let file = save(&mut engine, &repo, "source/file.txt", b"before");
    let source_policy = configure_scope(&mut engine, &repo, "source", 1, 0);
    let path_policy_id = new_id("policy");
    path_policy(&mut engine, &repo, &path_policy_id, "moved.txt", 3, 0).unwrap();
    rename(&mut engine, &repo, "source/file.txt", "moved.txt");
    for retained in [false, true] {
        if retained {
            configure_scope(&mut engine, &repo, "source", 2, 1);
            assert_ne!(
                engine.state.file_backup_policies[&file.file_id],
                source_policy
            );
        }
        let read = engine
            .backup_configuration(&repo.repository_id, "moved.txt")
            .unwrap();
        super::detached_parity::check(&engine, &repo.repository_id, "moved.txt", &read);
        assert_eq!(read.policy_id.as_deref().unwrap(), path_policy_id);
        assert_eq!(read.generation, 1);
        assert_eq!(
            read.effective_policy_id.as_deref().unwrap(),
            engine.state.file_backup_policies[&file.file_id]
        );
        assert_eq!(read.effective_generation, 1);
        assert_eq!(read.inherited, !retained);
        assert_eq!(read.target_copies, 1);
        assert_eq!(json_codec::json!(read.backup_prefixes), json!({}));
        assert_eq!(json_codec::json!(read.route_slot_ids), json!(["P01"]));
        let updated = save(&mut engine, &repo, "moved.txt", b"still one copy");
        assert_eq!(
            engine.state.placements[&updated.version_id].target_copies,
            1
        );
    }
    assert_eq!(
        configure_scope(&mut engine, &repo, "moved.txt", 2, 1),
        path_policy_id
    );
    let read = engine
        .backup_configuration(&repo.repository_id, "moved.txt")
        .unwrap();
    super::detached_parity::check(&engine, &repo.repository_id, "moved.txt", &read);
    assert_eq!(read.effective_policy_id.as_deref().unwrap(), path_policy_id);
    assert_eq!(read.generation, 2);
    assert!(!read.inherited);
    assert_eq!(read.target_copies, 2);
}
