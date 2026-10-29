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
    let pages: std::collections::BTreeSet<_> =
        crate::journal::snapshot_addresses(&members[0], &descriptor)
            .into_iter()
            .filter_map(|(kind, hash)| (kind == "state-page").then_some(hash))
            .collect();
    assert!(pages.len() >= 3);
    let mut missing = Vec::new();
    for (member, page) in members.iter().zip(pages.into_iter().take(3)) {
        let path = object_path(member, "state-page", &page);
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
fn rooted_evidence_selection_and_repair_do_not_follow_replaced_disk_names() {
    let (temp, engine, _repository) = fixture();
    let members = engine.members();
    let hash = engine.snapshot.clone().unwrap();
    let descriptor: Snapshot = load_object(&members[0], "snapshot", &hash).unwrap();
    let expected = canonical_json(engine.state()).unwrap();
    fs::write(object_path(&members[0], "snapshot", &hash), b"damaged").unwrap();
    for (index, member) in members.iter().enumerate() {
        fs::rename(&member.root, temp.path().join(format!("held-{index}"))).unwrap();
        private_dir(&member.root).unwrap();
    }
    let mut faults = 0;
    let restored =
        verified_snapshot_observed_at(&engine.disk_resources, &members, &hash, &mut |_| {
            faults += 1
        })
        .unwrap();
    assert!(faults > 0);
    assert_eq!(canonical_json(restored.state()).unwrap(), expected);
    repair_object_copies_at(
        &engine.disk_resources,
        &members,
        "snapshot",
        &hash,
        &descriptor,
    )
    .unwrap();
    for member in &members {
        let restored: Snapshot = load_object_at(
            &engine.disk_resources.get(member).unwrap().root,
            member,
            "snapshot",
            &hash,
        )
        .unwrap();
        assert_eq!(object_hash("snapshot", &restored).unwrap(), hash);
        assert!(!member.root.join("metadata").exists());
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
                &crate::journal::map_pages(&members[0], &snapshot.maps["content_states"])[0],
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
    let page =
        crate::journal::map_pages(&members[0], snapshot.maps.values().next().unwrap())[0].clone();
    let journals = members
        .iter()
        .map(|disk| fs::read(log_path(disk)).unwrap())
        .collect::<Vec<_>>();
    for disk in &members {
        fs::write(
            object_path(disk, "state-page", &page),
            b"{\"unbound\":true}",
        )
        .unwrap();
    }
    assert!(verified_snapshot(&members, &hash).is_err());
    drop(engine);
    assert!(Engine::open(&root).is_err());
    for (disk, journal) in members.iter().zip(journals) {
        assert_eq!(fs::read(log_path(disk)).unwrap(), journal);
        assert_eq!(
            fs::read(object_path(disk, "state-page", &page)).unwrap(),
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

fn repair_batch_object(index: usize) -> AuthenticatedMetadataObject {
    let value = json_codec::json!({"repair_batch_probe": index});
    let hash = object_hash("state-page", &value).unwrap();
    AuthenticatedMetadataObject::authenticate("state-page", &hash, value).unwrap()
}

#[test]
fn recovery_object_batch_repairs_missing_and_corrupt_copies() {
    let (_temp, engine, _repository) = fixture();
    let members = engine.members();
    let objects: Vec<_> = (0..4).map(repair_batch_object).collect();
    for object in &objects {
        repair_authenticated_object_at(&engine.disk_resources, &members, object).unwrap();
        fs::remove_file(object_path(&members[0], object.kind(), object.hash())).unwrap();
        fs::write(
            object_path(&members[1], object.kind(), object.hash()),
            b"corrupt",
        )
        .unwrap();
    }
    let mut batch = RecoveryObjectRepairBatch::new(&engine.disk_resources, &members).unwrap();
    for object in &objects {
        batch.repair(object).unwrap();
    }
    batch.finish().unwrap();
    for member in &members {
        for object in &objects {
            assert_eq!(
                fs::read(object_path(member, object.kind(), object.hash())).unwrap(),
                object.bytes()
            );
        }
    }
}

#[test]
fn recovery_object_batch_bounds_all_members_together() {
    let (_temp, engine, _repository) = fixture();
    let members = engine.members();
    let mut batch = RecoveryObjectRepairBatch::new(&engine.disk_resources, &members).unwrap();
    let mut previous = 0;
    let mut flushed = false;
    for index in 0..200 {
        batch.repair(&repair_batch_object(index)).unwrap();
        let held: usize = batch
            .members
            .iter()
            .map(|(_, member)| member.pending_directories())
            .sum();
        assert_eq!(held, batch.pending_directories);
        assert!(held <= MAX_RECOVERY_OBJECT_DIRECTORIES);
        flushed |= held < previous;
        previous = held;
    }
    assert!(
        flushed,
        "the fixture must cross the global directory budget"
    );
    batch.finish().unwrap();
}

#[test]
fn recovery_object_batch_rejects_replaced_directory_and_can_retry() {
    let (_temp, engine, _repository) = fixture();
    let members = engine.members();
    let object = repair_batch_object(0);
    let mut batch = RecoveryObjectRepairBatch::new(&engine.disk_resources, &members).unwrap();
    batch.repair(&object).unwrap();
    let directory = members[2].root.join("metadata/objects/state-page");
    let saved = members[2].root.join("saved-state-page");
    fs::rename(&directory, &saved).unwrap();
    let replacement_leaf = object_path(&members[2], object.kind(), object.hash());
    private_dir(replacement_leaf.parent().unwrap()).unwrap();
    assert_eq!(
        batch.finish().unwrap_err().code(),
        storage_contract::ErrorCode::UnsafePath
    );
    fs::remove_dir(replacement_leaf.parent().unwrap()).unwrap();
    fs::remove_dir(&directory).unwrap();
    fs::rename(&saved, &directory).unwrap();
    let mut retry = RecoveryObjectRepairBatch::new(&engine.disk_resources, &members).unwrap();
    retry.repair(&object).unwrap();
    retry.finish().unwrap();
}

#[test]
fn recovery_object_batch_finishes_same_inode_aba() {
    let (_temp, engine, _repository) = fixture();
    let members = engine.members();
    let object = repair_batch_object(0);
    let mut batch = RecoveryObjectRepairBatch::new(&engine.disk_resources, &members).unwrap();
    batch.repair(&object).unwrap();
    let parent = members[2].root.join("metadata/objects");
    let directory = parent.join("state-page");
    let saved = members[2].root.join("saved-state-page");
    fs::rename(&directory, &saved).unwrap();
    fs::File::open(&parent).unwrap().sync_all().unwrap();
    fs::rename(&saved, &directory).unwrap();
    batch.finish().unwrap();
    assert_eq!(
        fs::read(object_path(&members[2], object.kind(), object.hash())).unwrap(),
        object.bytes()
    );
}

fn assert_stored_json_cause(error: Error) {
    use std::error::Error as _;
    assert_eq!(error.code(), ErrorCode::ObjectCorrupt);
    let mut source = error.source();
    let mut found = false;
    while let Some(cause) = source {
        found |= cause.is::<json_codec::Error>();
        source = cause.source();
    }
    assert!(found, "stored JSON failure retains its concrete cause");
}

#[test]
fn stored_object_loaders_contextualize_syntax_and_canonical_number_failures() {
    let (_temp, engine, _repository) = fixture();
    let members = engine.members();
    let hash = engine.snapshot.as_deref().unwrap();
    let disk = &members[0];
    let root = &engine.disk_resources.get(disk).unwrap().root;
    let path = object_path(disk, "snapshot", hash);
    let original = fs::read(&path).unwrap();
    for bytes in [
        b"{truncated".as_slice(),
        br#"{"n":18446744073709551615}"#.as_slice(),
    ] {
        for member in &members {
            fs::write(object_path(member, "snapshot", hash), bytes).unwrap();
        }
        let mut observed = 0;
        for error in [
            load_authenticated_object(disk, "snapshot", hash)
                .err()
                .unwrap(),
            load_authenticated_object_at(root, disk, "snapshot", hash)
                .err()
                .unwrap(),
            load_authenticated_object_observed_at(root, disk, "snapshot", hash, &mut |count| {
                observed += count;
                Ok(())
            })
            .err()
            .unwrap(),
            load_object::<json_codec::Value>(disk, "snapshot", hash).unwrap_err(),
            load_object_at::<json_codec::Value>(root, disk, "snapshot", hash).unwrap_err(),
            load_object_observed_at::<json_codec::Value>(root, disk, "snapshot", hash, &mut |_| {
                Ok(())
            })
            .unwrap_err(),
            verified_graph_object_at(&engine.disk_resources, &members, "snapshot", hash)
                .err()
                .unwrap(),
            verified_object_at::<json_codec::Value>(
                &engine.disk_resources,
                &members,
                "snapshot",
                hash,
            )
            .unwrap_err(),
        ] {
            assert_stored_json_cause(error);
        }
        assert!(observed >= bytes.len());
    }
    fs::write(object_path(&members[1], "snapshot", hash), &original).unwrap();
    let mut faults = Vec::new();
    let recovered = verified_graph_object_observed_at(
        &engine.disk_resources,
        &members,
        "snapshot",
        hash,
        &mut |error| faults.push(error.code()),
    )
    .unwrap();
    assert_eq!(recovered.hash(), hash);
    assert_eq!(faults, [ErrorCode::ObjectCorrupt]);
    assert_eq!(fs::read(path).unwrap(), br#"{"n":18446744073709551615}"#);
}

#[test]
fn malformed_publication_seals_preserve_sources_and_replica_fallback() {
    let (_temp, engine, _repository) = fixture();
    let members = engine.members();
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
    let publication: Publication =
        load_object(&members[0], "publication", &record.object_hash).unwrap();
    let path = publication_seal_path(&members[0], &record.candidate_hash);
    let original = fs::read(&path).unwrap();
    for member in &members {
        fs::write(
            publication_seal_path(member, &record.candidate_hash),
            b"{truncated",
        )
        .unwrap();
    }
    assert_stored_json_cause(
        verified_publication_seal_at(&engine.disk_resources, &members, &candidate, &publication)
            .unwrap_err(),
    );
    fs::write(
        publication_seal_path(&members[1], &record.candidate_hash),
        original,
    )
    .unwrap();
    let mut faults = Vec::new();
    let seal = verified_publication_seal_observed_at(
        &engine.disk_resources,
        &members,
        &candidate,
        &publication,
        &mut |error| faults.push(error.code()),
    )
    .unwrap();
    assert_eq!(seal.candidate_hash, record.candidate_hash);
    assert_eq!(faults, [ErrorCode::ObjectCorrupt]);
    assert_eq!(fs::read(path).unwrap(), b"{truncated");
}
