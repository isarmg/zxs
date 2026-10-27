use super::*;
use crate::journal::RecordType;
use std::fs;

fn fixture() -> (tempfile::TempDir, Engine, Repository) {
    let temp = tempfile::tempdir().unwrap();
    let engine_root = temp.path().join("system");
    let mut engine = Engine::initialize(
        &engine_root,
        Config::development([
            temp.path().join("a"),
            temp.path().join("b"),
            temp.path().join("c"),
        ]),
    )
    .unwrap();
    let repository = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("replica recovery").unwrap(),
            "P01",
            3,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    (temp, engine, repository)
}

fn save(engine: &mut Engine, branch: &str, content: &[u8]) -> String {
    let workspace = engine.create_workspace(branch, "admin").unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "file.txt", content)
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: storage_contract::OperationId::generate(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "replica fixture".into(),
            request_digest: None,
        })
        .unwrap();
    engine.list_tree(branch).unwrap()[0].version_id.clone()
}

#[test]
fn snapshot_recovers_disjoint_page_replicas_without_read_repair() {
    let (_temp, engine, _repository) = fixture();
    let root = engine.root.clone();
    let members = engine.members();
    let hash = engine.snapshot.clone().unwrap();
    let expected = canonical_json(engine.state()).unwrap();
    let descriptor: Snapshot = load_object(&members[0], "snapshot", &hash).unwrap();
    let pages: std::collections::BTreeSet<_> = descriptor.pages.values().flatten().collect();
    assert!(pages.len() >= 3);
    let mut missing = Vec::new();
    for (member, page) in members.iter().zip(pages.into_iter().take(3)) {
        let path = object_path(member, "state-page", page);
        let bytes = fs::read(&path).unwrap();
        fs::remove_file(&path).unwrap();
        missing.push((path, bytes));
    }
    assert!(
        members
            .iter()
            .all(|member| load_snapshot(member, &hash).is_err())
    );
    let state = verified_snapshot(&members, &hash).unwrap();
    assert_eq!(canonical_json(state.state()).unwrap(), expected);
    assert!(missing.iter().all(|(path, _)| !path.exists()));
    drop(engine);
    let recovered = Engine::open(&root).unwrap();
    assert_eq!(canonical_json(recovered.state()).unwrap(), expected);
    for (path, bytes) in missing {
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
}

#[test]
fn purged_state_reopens_with_one_damaged_and_one_offline_metadata_member() {
    for kind in [
        "candidate",
        "publication",
        "snapshot",
        "seal",
        "content-state",
    ] {
        let (temp, mut engine, repository) = fixture();
        let old = save(&mut engine, &repository.main_branch_id, b"old");
        save(&mut engine, &repository.main_branch_id, b"new");
        let purge = engine
            .create_purge_plan(&repository.history_tree_id, vec![old.clone()], "admin")
            .unwrap();
        engine
            .commit_plan(
                &purge.plan_id,
                "admin",
                storage_contract::OperationId::generate(),
            )
            .unwrap();
        let root = engine.root.clone();
        let members = engine.members();
        let expected = canonical_json(engine.state()).unwrap();
        let record = scan(&members[0])
            .unwrap()
            .records
            .into_iter()
            .rev()
            .find(|(record, _)| record.record_type == RecordType::Publication)
            .unwrap()
            .0;
        let candidate: Candidate =
            load_object(&members[0], "candidate", &record.candidate_hash).unwrap();
        let snapshot: Snapshot =
            load_object(&members[0], "snapshot", &candidate.snapshot_hash).unwrap();
        let damaged = match kind {
            "candidate" => object_path(&members[0], kind, &record.candidate_hash),
            "publication" => object_path(&members[0], kind, &record.object_hash),
            "snapshot" => object_path(&members[0], kind, &candidate.snapshot_hash),
            "seal" => publication_seal_path(&members[0], &record.candidate_hash),
            _ => object_path(
                &members[0],
                "state-page",
                &snapshot.pages["content_states"][0],
            ),
        };
        drop(engine);
        fs::write(&damaged, b"corrupt redundant metadata").unwrap();
        fs::rename(&members[2].root, temp.path().join("offline")).unwrap();
        let recovered = Engine::open(&root).unwrap_or_else(|error| panic!("{kind}: {error}"));
        assert!(recovered.is_read_only());
        assert_eq!(canonical_json(recovered.state()).unwrap(), expected);
        assert_eq!(
            recovered.state.content_states[&old],
            storage_contract::ContentState::Purged
        );
        assert_eq!(fs::read(damaged).unwrap(), b"corrupt redundant metadata");
    }
}

#[test]
fn migrated_content_does_not_consume_live_payload_budget() {
    let (_temp, mut engine, repository) = fixture();
    let mut config = engine.config().clone();
    for disk in &mut config.disks {
        disk.capacity_bytes = Some(40);
    }
    engine.config = ValidatedConfig::try_from(config).unwrap();
    let original = save(&mut engine, &repository.main_branch_id, b"firstold");
    let cut = engine.state.branches[&repository.main_branch_id]
        .head_commit_id
        .clone();
    save(&mut engine, &repository.main_branch_id, b"next-old");
    assert_eq!(engine.used_bytes("P01"), 16);
    let split = engine
        .create_split_plan(&repository.history_tree_id, &cut, "admin")
        .unwrap();
    let result = engine
        .commit_plan(
            &split.plan_id,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    assert_eq!(
        engine.state.content_states[&original],
        storage_contract::ContentState::Migrated
    );
    assert_eq!(engine.used_bytes("P01"), 16);
    let repository =
        engine.state.repositories[result.result["repository_id"].as_str().unwrap()].clone();
    // The old copies have been removed. A 17-byte save is admitted into the
    // 40-byte slot budget (16 retained + 17 new), not double-counted as 49.
    save(
        &mut engine,
        &repository.main_branch_id,
        b"new payload bytes",
    );
    assert_eq!(engine.used_bytes("P01"), 33);
    assert!(
        engine
            .disk_status()
            .unwrap()
            .iter()
            .all(|disk| disk.used_bytes == 33)
    );
}

#[test]
fn corrupt_local_proof_does_not_prevent_authoritative_recovery() {
    for name in ["commit-proof.json", "publication-proof.json"] {
        let (_temp, mut engine, repository) = fixture();
        let id = save(&mut engine, &repository.main_branch_id, b"retained");
        let root = engine.root.clone();
        let version = engine.state.versions[&id].clone();
        let disk = engine.config.disks[0].clone();
        let directory = version_dir(&disk, &version.history_tree_id, &id);
        let proof = directory.join(name);
        let expected = fs::read(&proof).unwrap();
        drop(engine);
        fs::write(&proof, b"corrupt redundant local proof").unwrap();
        let recovered = Engine::open(&root).unwrap();
        assert_eq!(fs::read(proof).unwrap(), expected);
        assert!(recovered.open_content(&id).is_ok());
        let retained = fs::read_dir(directory.join("proof-quarantine"))
            .unwrap()
            .map(|entry| fs::read(entry.unwrap().path()).unwrap())
            .collect::<Vec<_>>();
        assert!(
            retained
                .iter()
                .any(|bytes| bytes == b"corrupt redundant local proof")
        );
    }
}

#[test]
fn restore_rejects_two_independently_agreed_pool_configurations() {
    let (first_temp, first, _) = fixture();
    let (second_temp, second, _) = fixture();
    let mut disks = first.members()[..2]
        .iter()
        .map(|disk| disk.root.clone())
        .collect::<Vec<_>>();
    disks.extend(second.members()[..2].iter().map(|disk| disk.root.clone()));
    let root = first_temp.path().join("restored");
    drop(first);
    drop(second);
    let restored = Engine::restore_from_disks(&root, &disks);
    assert!(
        matches!(restored, Err(ref error) if error.code() == storage_contract::ErrorCode::ConfigConflict)
    );
    assert!(!root.join("config.json").exists());
    drop(second_temp);
}

#[test]
fn snapshot_rejects_unverifiable_page_closure_without_repair() {
    let (_temp, engine, _) = fixture();
    let root = engine.root.clone();
    let members = engine.members();
    let hash = engine.snapshot.clone().unwrap();
    let snapshot: Snapshot = load_object(&members[0], "snapshot", &hash).unwrap();
    let page = snapshot.pages.values().flatten().next().unwrap();
    let journals = members
        .iter()
        .map(|disk| fs::read(log_path(disk)).unwrap())
        .collect::<Vec<_>>();
    for disk in &members {
        fs::write(object_path(disk, "state-page", page), b"{\"unbound\":true}").unwrap();
    }
    assert!(verified_snapshot(&members, &hash).is_err());
    drop(engine);
    assert!(Engine::open(&root).is_err());
    for (disk, journal) in members.iter().zip(journals) {
        assert_eq!(fs::read(log_path(disk)).unwrap(), journal);
        assert_eq!(
            fs::read(object_path(disk, "state-page", page)).unwrap(),
            b"{\"unbound\":true}"
        );
    }
}

#[test]
fn local_proof_recovery_rejects_symlinks_without_touching_targets() {
    let (temp, mut engine, repository) = fixture();
    let id = save(&mut engine, &repository.main_branch_id, b"retained");
    let root = engine.root.clone();
    let version = &engine.state.versions[&id];
    let directory = version_dir(&engine.config.disks[0], &version.history_tree_id, &id);
    let proof = directory.join("publication-proof.json");
    let original = fs::read(&proof).unwrap();
    let target = temp.path().join("outside-proof.json");
    drop(engine);
    fs::rename(&proof, &target).unwrap();
    std::os::unix::fs::symlink(&target, &proof).unwrap();
    assert!(
        matches!(Engine::open(&root), Err(error) if error.code() == storage_contract::ErrorCode::UnsafePath)
    );
    assert_eq!(fs::read(target).unwrap(), original);
    assert!(
        fs::symlink_metadata(proof)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(!directory.join("proof-quarantine").exists());
}

#[test]
fn restore_accepts_one_agreed_pool_and_rejects_foreign_extra_input() {
    let (temp, engine, repository) = fixture();
    let (foreign_temp, foreign, _) = fixture();
    let mut disks = engine.members()[..2]
        .iter()
        .map(|disk| disk.root.clone())
        .collect::<Vec<_>>();
    disks.push(foreign.members()[0].root.clone());
    let root = temp.path().join("restored");
    drop(engine);
    drop(foreign);
    assert!(
        matches!(Engine::restore_from_disks(&root, &disks), Err(error) if error.code() == storage_contract::ErrorCode::ConfigConflict)
    );
    assert!(!root.join("config.json").exists());
    disks.pop();
    let restored = Engine::restore_from_disks(&root, &disks).unwrap();
    assert!(
        restored
            .state
            .repositories
            .contains_key(&repository.repository_id)
    );
    drop(foreign_temp);
}

#[test]
fn repair_rejects_unsafe_unreadable_entry_instead_of_replacing_it() {
    let (_temp, engine, repo) = fixture();
    let members = engine.members();
    let hash = object_hash("repository", &repo).unwrap();
    let path = object_path(&members[0], "repository", &hash);
    crate::disk::private_dir(path.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&path, &path).unwrap();
    assert!(repair_object_copies(&members, "repository", &hash, &repo).is_err());
    assert!(fs::symlink_metadata(path).unwrap().file_type().is_symlink());
}
