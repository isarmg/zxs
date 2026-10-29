use super::*;
use crate::journal::RecordType;

#[test]
fn recovery_rejects_lost_published_journals_before_rewriting_projection() {
    for disks in [1, 3] {
        for damage in ["missing", "empty", "prefix"] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join("system");
            let mut engine = Engine::initialize(
                &root,
                Config::development((0..disks).map(|i| temp.path().join(format!("disk-{i}")))),
            )
            .unwrap();
            engine
                .create_repository(
                    &storage_contract::RepositoryName::parse("first").unwrap(),
                    "P01",
                    1,
                    "admin",
                    storage_contract::OperationId::generate(),
                )
                .unwrap();
            let members = engine.members();
            let prefix: Vec<_> = members
                .iter()
                .map(|disk| std::fs::read(log_path(disk)).unwrap())
                .collect();
            let operation = storage_contract::OperationId::generate();
            engine
                .create_repository(
                    &storage_contract::RepositoryName::parse("second").unwrap(),
                    "P01",
                    1,
                    "admin",
                    operation,
                )
                .unwrap();
            let complete: Vec<_> = members
                .iter()
                .map(|disk| std::fs::read(log_path(disk)).unwrap())
                .collect();
            drop(engine);
            let projection = std::fs::read(root.join("index.sqlite")).unwrap();
            for (disk, prefix) in members.iter().zip(&prefix) {
                match damage {
                    "missing" => std::fs::remove_file(log_path(disk)).unwrap(),
                    "empty" => std::fs::write(log_path(disk), []).unwrap(),
                    _ => std::fs::write(log_path(disk), prefix).unwrap(),
                }
            }
            for readonly in [false, true] {
                let opened = if readonly {
                    Engine::open_for_recovery(&root)
                } else {
                    Engine::open(&root)
                };
                assert_eq!(
                    opened.err().map(|error| error.code().to_owned()),
                    Some(storage_contract::ErrorCode::RecoveryRequired),
                    "{disks} disks, {damage}, readonly={readonly}"
                );
                assert_eq!(
                    std::fs::read(root.join("index.sqlite")).unwrap(),
                    projection
                );
                for (disk, prefix) in members.iter().zip(&prefix) {
                    match damage {
                        "missing" => assert!(!log_path(disk).exists()),
                        "empty" => assert!(std::fs::read(log_path(disk)).unwrap().is_empty()),
                        _ => assert_eq!(&std::fs::read(log_path(disk)).unwrap(), prefix),
                    }
                }
            }
            // Restoring the exact missing evidence, not inferring a replacement
            // history from objects, makes the unchanged publication recoverable.
            for (disk, bytes) in members.iter().zip(complete) {
                std::fs::write(log_path(disk), bytes).unwrap();
            }
            let reopened = Engine::open(&root).unwrap();
            assert_eq!(reopened.state.repositories.len(), 2);
            assert!(reopened.operation(operation).is_ok());
        }
    }
}

#[test]
fn recovery_allows_new_empty_pools_and_unpublished_preparation_artifacts() {
    for fault in [None, Some("after_payloads"), Some("after_prepare_1")] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("system");
        let mut engine =
            Engine::initialize(&root, Config::development([temp.path().join("primary")])).unwrap();
        if let Some(fault) = fault {
            engine.inject_fault_once(fault);
            assert!(
                engine
                    .create_repository(
                        &storage_contract::RepositoryName::parse("unpublished").unwrap(),
                        "P01",
                        1,
                        "admin",
                        storage_contract::OperationId::generate()
                    )
                    .is_err()
            );
            assert!(
                !list_object_hashes(&engine.members()[0], "snapshot")
                    .unwrap()
                    .is_empty()
            );
        }
        drop(engine);
        let mut reopened = Engine::open(&root).unwrap();
        assert!(reopened.state.repositories.is_empty());
        reopened
            .create_repository(
                &storage_contract::RepositoryName::parse("published").unwrap(),
                "P01",
                1,
                "admin",
                storage_contract::OperationId::generate(),
            )
            .unwrap();
    }
}

#[test]
fn recovery_rejects_dangling_journal_paths_instead_of_an_empty_history() {
    use std::os::unix::fs::symlink;

    for replace_parent in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("system");
        let mut engine =
            Engine::initialize(&root, Config::development([temp.path().join("primary")])).unwrap();
        let operation = storage_contract::OperationId::generate();
        engine
            .create_repository(
                &storage_contract::RepositoryName::parse("repo").unwrap(),
                "P01",
                1,
                "admin",
                operation,
            )
            .unwrap();
        let journal = log_path(&engine.members()[0]);
        let journal_bytes = std::fs::read(&journal).unwrap();
        let target = if replace_parent {
            journal.parent().unwrap().to_path_buf()
        } else {
            journal.clone()
        };
        drop(engine);
        let saved = temp.path().join("saved-journal");
        let missing = temp.path().join("does-not-exist");
        std::fs::rename(&target, &saved).unwrap();
        symlink(&missing, &target).unwrap();
        assert_eq!(
            Engine::open(&root)
                .err()
                .map(|error| error.code().to_owned()),
            Some(storage_contract::ErrorCode::UnsafePath),
            "an unsafe journal cannot be interpreted as a never-written pool"
        );
        assert!(!missing.exists());
        assert_eq!(std::fs::read_link(&target).unwrap(), missing);
        std::fs::remove_file(&target).unwrap();
        std::fs::rename(&saved, &target).unwrap();
        assert_eq!(std::fs::read(&journal).unwrap(), journal_bytes);
        let reopened = Engine::open(&root).unwrap();
        assert_eq!(reopened.state.repositories.len(), 1);
        assert!(reopened.operation(operation).is_ok());
    }
}

#[test]
fn recovery_repairs_exact_candidate_and_publication_replicas() {
    for interrupted in [false, true] {
        for kind in ["candidate", "publication"] {
            for missing in [false, true] {
                let temp = tempfile::tempdir().unwrap();
                let root = temp.path().join("system");
                let config = Config::development([
                    temp.path().join("a"),
                    temp.path().join("b"),
                    temp.path().join("c"),
                ]);
                let mut engine = Engine::initialize(&root, config).unwrap();
                if interrupted {
                    engine.inject_fault_once("after_publication_3");
                }
                let result = engine.create_repository(
                    &storage_contract::RepositoryName::parse("repo").unwrap(),
                    "P01",
                    3,
                    "admin",
                    storage_contract::OperationId::generate(),
                );
                assert_eq!(result.is_err(), interrupted);
                let members = engine.members();
                let record = scan(&members[0])
                    .unwrap()
                    .records
                    .into_iter()
                    .find(|(r, _)| r.record_type == RecordType::Publication)
                    .unwrap()
                    .0;
                let candidate: Candidate =
                    load_object(&members[0], "candidate", &record.candidate_hash).unwrap();
                let expected =
                    canonical_json(&load_snapshot(&members[0], &candidate.snapshot_hash).unwrap())
                        .unwrap();
                let hash = if kind == "candidate" {
                    record.candidate_hash
                } else {
                    record.object_hash
                };
                let target = object_path(&members[0], kind, &hash);
                let original = std::fs::read(&target).unwrap();
                drop(engine);
                if missing {
                    std::fs::remove_file(&target).unwrap();
                } else {
                    std::fs::write(&target, b"corrupt").unwrap();
                }
                let recovered = Engine::open(&root).unwrap();
                assert_eq!(canonical_json(recovered.state()).unwrap(), expected);
                assert_eq!(std::fs::read(&target).unwrap(), original);
            }
        }
    }
}

#[test]
fn pending_candidate_recovers_from_healthy_exact_replica() {
    for fault in ["after_accept_1", "after_accept_2"] {
        for missing in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join("system");
            let config = Config::development([
                temp.path().join("a"),
                temp.path().join("b"),
                temp.path().join("c"),
            ]);
            let mut engine = Engine::initialize(&root, config).unwrap();
            engine.inject_fault_once(fault);
            let operation = storage_contract::OperationId::generate();
            assert!(
                engine
                    .create_repository(
                        &storage_contract::RepositoryName::parse("repo").unwrap(),
                        "P01",
                        3,
                        "admin",
                        operation
                    )
                    .is_err()
            );
            let members = engine.members();
            let record = scan(&members[0])
                .unwrap()
                .records
                .into_iter()
                .find(|(r, _)| r.record_type == RecordType::Accept)
                .unwrap()
                .0;
            let target = object_path(&members[0], "candidate", &record.candidate_hash);
            let original = std::fs::read(&target).unwrap();
            drop(engine);
            if missing {
                std::fs::remove_file(&target).unwrap();
            } else {
                std::fs::write(&target, b"corrupt").unwrap();
            }
            let recovered = Engine::open(&root).unwrap();
            assert_eq!(
                recovered.operation(operation).unwrap().commit_state,
                storage_contract::CommitState::Published
            );
            assert_eq!(recovered.state.repositories.len(), 1);
            assert_eq!(std::fs::read(&target).unwrap(), original);
            drop(recovered);
            assert_eq!(Engine::open(&root).unwrap().state.repositories.len(), 1);
        }
    }
}

#[test]
fn recovery_rejects_all_digest_mismatched_object_replicas_without_rewriting_them() {
    for kind in ["candidate", "publication"] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("system");
        let config = Config::development([
            temp.path().join("a"),
            temp.path().join("b"),
            temp.path().join("c"),
        ]);
        let mut engine = Engine::initialize(&root, config).unwrap();
        engine
            .create_repository(
                &storage_contract::RepositoryName::parse("repo").unwrap(),
                "P01",
                3,
                "admin",
                storage_contract::OperationId::generate(),
            )
            .unwrap();
        let members = engine.members();
        let record = scan(&members[0])
            .unwrap()
            .records
            .into_iter()
            .find(|(r, _)| r.record_type == RecordType::Publication)
            .unwrap()
            .0;
        let hash = if kind == "candidate" {
            record.candidate_hash
        } else {
            record.object_hash
        };
        let mut wrong: json_codec::Value =
            read_json(&object_path(&members[0], kind, &hash)).unwrap();
        if kind == "candidate" {
            wrong["operation_id"] = json_codec::json!(storage_contract::OperationId::generate());
        } else {
            wrong["candidate_hash"] = json_codec::json!("0".repeat(64));
        }
        let wrong = canonical_json(&wrong).unwrap();
        let journals = members
            .iter()
            .map(|d| std::fs::read(log_path(d)).unwrap())
            .collect::<Vec<_>>();
        drop(engine);
        for member in &members {
            std::fs::write(object_path(member, kind, &hash), &wrong).unwrap();
        }
        assert_eq!(
            Engine::open(&root).err().unwrap().code(),
            storage_contract::ErrorCode::ObjectCorrupt
        );
        for (member, journal) in members.iter().zip(journals) {
            assert_eq!(
                std::fs::read(object_path(member, kind, &hash)).unwrap(),
                wrong
            );
            assert_eq!(std::fs::read(log_path(member)).unwrap(), journal);
        }
    }
}

#[test]
fn recovery_rejects_flat_objects_without_repair_or_deletion() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("system");
    let config = Config::development([
        temp.path().join("a"),
        temp.path().join("b"),
        temp.path().join("c"),
    ]);
    let mut engine = Engine::initialize(&root, config).unwrap();
    engine
        .create_repository(
            &storage_contract::RepositoryName::parse("repo").unwrap(),
            "P01",
            3,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let members = engine.members();
    let record = scan(&members[0])
        .unwrap()
        .records
        .into_iter()
        .find(|(r, _)| r.record_type == RecordType::Publication)
        .unwrap()
        .0;
    let hash = record.candidate_hash;
    let expected = std::fs::read(object_path(&members[0], "candidate", &hash)).unwrap();
    let flat = members[0]
        .root
        .join("metadata/objects/candidate")
        .join(format!("{hash}.json"));
    std::fs::write(&flat, b"{}").unwrap();
    assert!(load_object::<Candidate>(&members[0], "candidate", &hash).is_err());
    drop(engine);
    assert_eq!(
        Engine::open(&root).err().unwrap().code(),
        storage_contract::ErrorCode::FormatUnsupported
    );
    assert_eq!(std::fs::read(flat).unwrap(), b"{}");
    assert_eq!(
        std::fs::read(object_path(&members[0], "candidate", &hash)).unwrap(),
        expected
    );
}

#[test]
fn object_repair_does_not_replace_symlink_or_hardlink() {
    use std::os::unix::fs::symlink;
    for hardlink in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let config = Config::development([
            temp.path().join("a"),
            temp.path().join("b"),
            temp.path().join("c"),
        ]);
        let engine = Engine::initialize(&temp.path().join("system"), config).unwrap();
        let members = engine.members();
        let value = json_codec::json!({"data": "verified"});
        let hash = store_object(&members[0], "candidate", &value).unwrap();
        let target = object_path(&members[0], "candidate", &hash);
        std::fs::remove_file(&target).unwrap();
        let external = temp.path().join("external");
        std::fs::write(&external, b"untouched").unwrap();
        if hardlink {
            std::fs::hard_link(&external, &target).unwrap();
        } else {
            symlink(&external, &target).unwrap();
        }
        assert!(
            repair_object_copies_at(&engine.disk_resources, &members, "candidate", &hash, &value)
                .is_err()
        );
        assert_eq!(std::fs::read(&external).unwrap(), b"untouched");
        assert_eq!(std::fs::read(&target).unwrap(), b"untouched");
    }
}

#[test]
fn bound_snapshot_repair_preserves_alternate_page_partition() {
    let temp = tempfile::tempdir().unwrap();
    let config = Config::development([
        temp.path().join("a"),
        temp.path().join("b"),
        temp.path().join("c"),
    ]);
    let mut engine = Engine::initialize(&temp.path().join("system"), config).unwrap();
    let members = engine.members();
    let repository = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("partition fixture").unwrap(),
            "P01",
            3,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let mut state = engine.state.as_ref().clone();
    let template = state.branches[&repository.main_branch_id].clone();
    for n in 0..300 {
        let mut branch = template.clone();
        branch.branch_id = new_id("br");
        branch.name = format!("branch-{n}");
        state.branches.insert(branch.branch_id.clone(), branch);
    }
    let canonical = crate::journal::prepare_snapshot_fixture(&state, MAX_METADATA).unwrap();
    canonical.store(&members[..1], &state).unwrap();
    std::fs::remove_file(object_path(&members[0], "snapshot", &canonical.hash)).unwrap();
    let mut alternate = canonical.descriptor.clone();
    // The current reader accepts a route-correct extra index. Repair must keep
    // its exact root, not silently normalize it through the current encoder.
    let field = "repositories";
    let page: json_codec::Value =
        load_object(&members[0], "state-page", alternate.maps[field].hash()).unwrap();
    assert_eq!(page.as_object().unwrap().len(), 1);
    let key = page.as_object().unwrap().keys().next().unwrap();
    let digit = blake3::hash(key.as_bytes()).to_hex().to_string()[..1].to_owned();
    let index = json_codec::json!({"prefix":"","children":{digit:alternate.maps[field]}});
    let index_hash = store_object(&members[0], "metadata-index", &index).unwrap();
    alternate.maps.insert(
        field.into(),
        json_codec::from_value(json_codec::json!({"kind":"metadata-index","hash":index_hash}))
            .unwrap(),
    );
    let hash = store_object(&members[0], "snapshot", &alternate).unwrap();
    let modern_hash = store_snapshot(&[], &state).unwrap();
    assert_ne!(
        hash, modern_hash,
        "fixture must distinguish alternate/current page partitions"
    );
    engine.repair_bound_snapshot(&hash).unwrap();
    for member in &members {
        assert_eq!(
            canonical_json(&load_snapshot(member, &hash).unwrap()).unwrap(),
            canonical_json(&state).unwrap()
        );
        assert!(!object_path(member, "snapshot", &modern_hash).exists());
        assert_eq!(
            std::fs::read(object_path(member, "snapshot", &hash)).unwrap(),
            std::fs::read(object_path(&members[0], "snapshot", &hash)).unwrap()
        );
    }
}

#[test]
fn repair_cache_is_bounded_and_overflow_remains_uncached() {
    let mut repaired = BTreeSet::new();
    for n in 0..MAX_REPAIRED_PAGES {
        remember_repaired_page(&mut repaired, &format!("{n:064x}"));
    }
    let overflow = format!("{:064x}", MAX_REPAIRED_PAGES);
    remember_repaired_page(&mut repaired, &overflow);
    assert_eq!(repaired.len(), MAX_REPAIRED_PAGES);
    assert!(!repaired.contains(&overflow));
    assert!(repaired.contains(&format!("{:064x}", 0)));
}

#[test]
fn failed_replica_repair_does_not_cache_page_success() {
    let temp = tempfile::tempdir().unwrap();
    let config = Config::development([
        temp.path().join("a"),
        temp.path().join("b"),
        temp.path().join("c"),
    ]);
    let engine = Engine::initialize(&temp.path().join("system"), config).unwrap();
    let members = engine.members();
    let mut state = State::default();
    state
        .file_notes
        .insert("file_test".into(), "unchanged".into());
    let hash = store_snapshot(&members, &state).unwrap();
    let snapshot: Snapshot = load_object(&members[0], "snapshot", &hash).unwrap();
    let page =
        crate::journal::map_pages(&members[0], snapshot.maps.values().next().unwrap())[0].clone();
    let target = object_path(&members[2], "state-page", &page);
    let original = std::fs::read(&target).unwrap();
    std::fs::remove_file(&target).unwrap();
    std::fs::create_dir(&target).unwrap();
    let mut repaired = BTreeSet::new();
    assert_eq!(
        repair_snapshot_copies(
            &engine.disk_resources,
            &members,
            &members,
            &hash,
            &mut repaired
        )
        .unwrap_err()
        .code(),
        storage_contract::ErrorCode::UnsafePath
    );
    assert!(!repaired.contains(&format!("state-page/{page}")));
    std::fs::remove_dir(&target).unwrap();
    repair_snapshot_copies(
        &engine.disk_resources,
        &members,
        &members,
        &hash,
        &mut repaired,
    )
    .unwrap();
    assert!(repaired.contains(&format!("state-page/{page}")));
    assert_eq!(std::fs::read(&target).unwrap(), original);
}

#[test]
fn shared_pages_are_repaired_once_per_recovery_and_rechecked_next_time() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("system");
    let config = Config::development([
        temp.path().join("a"),
        temp.path().join("b"),
        temp.path().join("c"),
    ]);
    let mut engine = Engine::initialize(&root, config).unwrap();
    engine
        .create_repository(
            &storage_contract::RepositoryName::parse("files").unwrap(),
            "P01",
            3,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let branch = engine
        .state
        .repositories
        .values()
        .next()
        .unwrap()
        .main_branch_id
        .clone();
    let workspace = engine.create_workspace(&branch, "admin").unwrap();
    engine
        .stage_bytes(
            &workspace.workspace_id,
            "shared.txt",
            b"retained historical content",
        )
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: storage_contract::OperationId::generate(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "shared page fixture".into(),
            request_digest: None,
        })
        .unwrap();
    for n in 0..6 {
        engine
            .create_repository(
                &storage_contract::RepositoryName::parse(&format!("repo{n}")).unwrap(),
                "P01",
                3,
                "admin",
                storage_contract::OperationId::generate(),
            )
            .unwrap();
    }
    let members = engine.members();
    let expected = canonical_json(engine.state()).unwrap();
    drop(engine);

    let mut references = 0;
    let mut pages = BTreeSet::new();
    let mut shared = None;
    for (record, _) in scan(&members[0]).unwrap().records {
        if record.record_type != RecordType::Publication {
            continue;
        }
        let candidate: Candidate =
            load_object(&members[0], "candidate", &record.candidate_hash).unwrap();
        let snapshot: Snapshot =
            load_object(&members[0], "snapshot", &candidate.snapshot_hash).unwrap();
        for hash in crate::journal::snapshot_addresses(&members[0], &snapshot)
            .into_iter()
            .filter_map(|(kind, hash)| (kind == "state-page").then_some(hash))
        {
            references += 1;
            if !pages.insert(hash.clone()) {
                shared = Some(hash.clone());
            }
        }
    }
    let shared = shared.expect("fixture must reference an unchanged historical page");
    let path = object_path(&members[0], "state-page", &shared);
    let original = std::fs::read(&path).unwrap();
    assert!(references > pages.len());
    // Both recovery invocations must independently verify the damaged page:
    // an in-memory cache may not turn into persisted trust after restart.
    for corrupt in [false, true] {
        for member in &members {
            let checkpoint = member.root.join("metadata/replay-checkpoint.json");
            if checkpoint.exists() {
                std::fs::remove_file(checkpoint).unwrap();
            }
        }
        if corrupt {
            std::fs::write(&path, b"corrupt").unwrap();
        } else {
            std::fs::remove_file(&path).unwrap();
        }
        PAGE_REPAIR_COUNT.with(|count| count.set(0));
        let engine = Engine::open(&root).unwrap();
        assert_eq!(canonical_json(engine.state()).unwrap(), expected);
        assert_eq!(std::fs::read(&path).unwrap(), original);
        let repairs = PAGE_REPAIR_COUNT.with(|count| count.get());
        assert_eq!(repairs, pages.len());
        assert!(repairs < references);
        eprintln!("repaired {repairs} unique pages instead of {references} references");
        drop(engine);
    }
}

#[test]
fn single_storage_disk_publishes_reopens_rebuilds_index_and_restores_system() {
    use std::io::Read;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("system");
    let disk = temp.path().join("only-storage");
    let config = Config::development([disk.clone()]);
    let mut engine = Engine::initialize(&root, config).unwrap();
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("single").unwrap(),
            "P01",
            1,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "only.txt", b"one durable copy")
        .unwrap();
    let result = engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: storage_contract::OperationId::generate(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "single disk".into(),
            request_digest: None,
        })
        .unwrap();
    assert_eq!(
        result.commit_state,
        storage_contract::CommitState::Published
    );
    let version = engine.list_tree(&repo.main_branch_id).unwrap()[0]
        .version_id
        .clone();
    let state = canonical_json(engine.state()).unwrap();
    drop(engine);
    std::fs::remove_file(root.join("index.sqlite")).unwrap();
    let engine = Engine::open(&root).unwrap();
    assert!(!engine.blocked);
    assert_eq!(canonical_json(engine.state()).unwrap(), state);
    let mut bytes = Vec::new();
    engine
        .open_content(&version)
        .unwrap()
        .file
        .read_to_end(&mut bytes)
        .unwrap();
    assert_eq!(bytes, b"one durable copy");
    drop(engine);
    let restored = Engine::restore_from_disks(&temp.path().join("new-system"), &[disk]).unwrap();
    assert_eq!(canonical_json(restored.state()).unwrap(), state);
    let mut bytes = Vec::new();
    restored
        .open_content(&version)
        .unwrap()
        .file
        .read_to_end(&mut bytes)
        .unwrap();
    assert_eq!(bytes, b"one durable copy");
}

#[test]
fn configured_metadata_majority_does_not_shrink_when_disks_disappear() {
    for (count, lost, expected_available) in
        [(1, 1, false), (2, 1, false), (3, 1, true), (4, 2, false)]
    {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("system");
        let disks: Vec<_> = (0..count)
            .map(|n| temp.path().join(format!("disk-{n}")))
            .collect();
        let mut engine = Engine::initialize(&root, Config::development(disks.clone())).unwrap();
        engine
            .create_repository(
                &storage_contract::RepositoryName::parse("majority").unwrap(),
                "P01",
                1,
                "admin",
                storage_contract::OperationId::generate(),
            )
            .unwrap();
        drop(engine);
        for disk in disks.iter().rev().take(lost) {
            std::fs::remove_dir_all(disk).unwrap();
        }
        match Engine::open(&root) {
            Ok(engine) => {
                assert!(expected_available);
                assert!(engine.blocked);
            }
            Err(error) => {
                assert!(!expected_available);
                assert_eq!(
                    error.code(),
                    storage_contract::ErrorCode::MetadataQuorumUnavailable
                );
            }
        }
        if !expected_available && count > 1 {
            assert!(
                Engine::restore_from_disks(&temp.path().join("restored"), &disks[..count - lost])
                    .is_err()
            );
        }
    }
}

#[test]
fn publication_proofs_require_exact_nonempty_unique_membership() {
    for count in [1, 2, 3, 4] {
        let temp = tempfile::tempdir().unwrap();
        let disks = (0..count).map(|n| temp.path().join(format!("disk-{n}")));
        let mut engine =
            Engine::initialize(&temp.path().join("system"), Config::development(disks)).unwrap();
        engine
            .create_repository(
                &storage_contract::RepositoryName::parse("proof").unwrap(),
                "P01",
                1,
                "admin",
                storage_contract::OperationId::generate(),
            )
            .unwrap();
        let member = &engine.members()[0];
        let record = scan(member)
            .unwrap()
            .records
            .into_iter()
            .find(|(r, _)| r.record_type == RecordType::Publication)
            .unwrap()
            .0;
        let candidate: Candidate =
            load_object(member, "candidate", &record.candidate_hash).unwrap();
        let publication: Publication =
            load_object(member, "publication", &record.object_hash).unwrap();
        let seal: PublicationSeal =
            read_json(&publication_seal_path(member, &record.candidate_hash)).unwrap();
        verify_publication_seal(&candidate, &publication, &seal).unwrap();
        let mut bad = candidate.clone();
        bad.metadata_disk_ids.clear();
        assert!(engine.verify_candidate_membership(&bad).is_err());
        assert!(verify_publication_seal(&bad, &publication, &seal).is_err());
        bad.metadata_disk_ids = vec![candidate.metadata_disk_ids[0].clone(); count + 1];
        assert!(engine.verify_candidate_membership(&bad).is_err());
        assert!(verify_publication_seal(&bad, &publication, &seal).is_err());
        let mut incomplete = seal.clone();
        incomplete.receipts.pop();
        assert!(verify_publication_seal(&candidate, &publication, &incomplete).is_err());
        if count > 1 {
            bad.metadata_disk_ids = vec![candidate.metadata_disk_ids[0].clone()];
            assert!(engine.verify_candidate_membership(&bad).is_err());
        }
    }
}

#[test]
fn single_storage_disk_completes_pending_accept_but_never_invents_lost_metadata() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("system");
    let mut engine =
        Engine::initialize(&root, Config::development([temp.path().join("disk")])).unwrap();
    let operation = storage_contract::OperationId::generate();
    engine.inject_fault_once("after_accept_1");
    assert!(
        engine
            .create_repository(
                &storage_contract::RepositoryName::parse("pending").unwrap(),
                "P01",
                1,
                "admin",
                operation
            )
            .is_err()
    );
    let member = engine.members().remove(0);
    drop(engine);
    let engine = Engine::open(&root).unwrap();
    assert_eq!(
        engine.operation(operation).unwrap().commit_state,
        storage_contract::CommitState::Published
    );
    let record = scan(&member)
        .unwrap()
        .records
        .into_iter()
        .find(|(r, _)| r.record_type == RecordType::Accept)
        .unwrap()
        .0;
    let candidate_path = object_path(&member, "candidate", &record.candidate_hash);
    drop(engine);
    std::fs::write(candidate_path, b"corrupted sole metadata copy").unwrap();
    assert!(Engine::open(&root).is_err());
}

#[test]
fn self_consistent_one_member_seal_cannot_downgrade_configured_three_members() {
    let temp = tempfile::tempdir().unwrap();
    let config = Config::development((0..3).map(|n| temp.path().join(format!("disk-{n}"))));
    let engine = Engine::initialize(&temp.path().join("system"), config.clone()).unwrap();
    // Build an internally consistent proof that claims the same pool and first
    // disk, but omits two configured members. Hash consistency is not authority.
    let mut reduced = config;
    reduced.disks.truncate(1);
    reduced.disks[0].root = temp.path().join("forged-disk");
    let mut forged = Engine::initialize(&temp.path().join("forged-system"), reduced).unwrap();
    forged
        .create_repository(
            &storage_contract::RepositoryName::parse("forged").unwrap(),
            "P01",
            1,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let member = forged.members().remove(0);
    let record = scan(&member)
        .unwrap()
        .records
        .into_iter()
        .find(|(r, _)| r.record_type == RecordType::Publication)
        .unwrap()
        .0;
    let candidate: Candidate = load_object(&member, "candidate", &record.candidate_hash).unwrap();
    let publication: Publication =
        load_object(&member, "publication", &record.object_hash).unwrap();
    let seal: PublicationSeal =
        read_json(&publication_seal_path(&member, &record.candidate_hash)).unwrap();
    verify_publication_seal(&candidate, &publication, &seal).unwrap();
    assert_eq!(candidate.pool_id, engine.config.pool_id);
    assert_eq!(
        engine
            .verify_candidate_membership(&candidate)
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::PublicationInvalid
    );
    let target = engine.members().remove(0);
    store_object(&target, "candidate", &candidate).unwrap();
    append(
        &target,
        RecordType::Accept,
        candidate.sequence,
        &record.candidate_hash,
        &record.candidate_hash,
    )
    .unwrap();
    let root = engine.root.clone();
    drop(engine);
    assert_eq!(
        Engine::open(&root).err().unwrap().code(),
        storage_contract::ErrorCode::PublicationInvalid
    );
}

#[test]
fn nonmetadata_payload_disk_offline_proofs_require_saved_membership() {
    let temp = tempfile::tempdir().unwrap();
    let mut config = Config::development((0..4).map(|n| temp.path().join(format!("disk-{n}"))));
    config.disks[3].metadata_member = false;
    let payload_disk = config.disks[3].clone();
    let mut engine = Engine::initialize(&temp.path().join("system"), config).unwrap();
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("offline").unwrap(),
            "P01",
            4,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let mut old = None;
    for content in [b"old", b"new"] {
        let workspace = engine
            .create_workspace(&repo.main_branch_id, "admin")
            .unwrap();
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
                message: "offline proof".into(),
                request_digest: None,
            })
            .unwrap();
        if old.is_none() {
            old = Some(
                engine.list_tree(&repo.main_branch_id).unwrap()[0]
                    .version_id
                    .clone(),
            );
        }
    }
    let old = old.unwrap();
    let version = engine.state.versions[&old].clone();
    let directory = version_dir(&payload_disk, &repo.history_tree_id, &old);
    verify_local_publication(&directory, &version).unwrap();
    assert!(offline_check(&payload_disk.root).unwrap().issues.is_empty());
    let exported = temp.path().join("exported.txt");
    offline_export(&payload_disk.root, &old, &exported).unwrap();
    assert_eq!(std::fs::read(exported).unwrap(), b"old");
    let anchor = payload_disk
        .root
        .join("metadata/pool-membership/config.json");
    let saved = std::fs::read(&anchor).unwrap();
    std::fs::remove_file(&anchor).unwrap();
    assert!(verify_local_publication(&directory, &version).is_err());
    assert!(offline_export(&payload_disk.root, &old, &temp.path().join("denied.txt")).is_err());
    assert!(!anchor.exists());
    std::fs::write(&anchor, &saved).unwrap();
    let plan = engine
        .create_purge_plan(&repo.history_tree_id, vec![old], "admin")
        .unwrap();
    engine
        .commit_plan(
            &plan.plan_id,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    verify_tombstone(&directory, &version).unwrap();
    std::fs::remove_file(&anchor).unwrap();
    assert!(verify_tombstone(&directory, &version).is_err());
    assert!(!anchor.exists());
}

#[test]
fn empty_unmounted_root_is_unavailable_without_writes() {
    for lost in [1, 2] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("system");
        let config = Config::development((0..3).map(|n| temp.path().join(format!("disk-{n}"))));
        let mut engine = Engine::initialize(&root, config.clone()).unwrap();
        let repo = engine
            .create_repository(
                &storage_contract::RepositoryName::parse("majority").unwrap(),
                "P01",
                1,
                "admin",
                storage_contract::OperationId::generate(),
            )
            .unwrap();
        drop(engine);
        for disk in config.disks.iter().rev().take(lost) {
            std::fs::remove_dir_all(&disk.root).unwrap();
            private_dir(&disk.root).unwrap();
        }
        for recovery in [false, true] {
            let opened = if recovery {
                Engine::open_for_recovery(&root)
            } else {
                Engine::open(&root)
            };
            if lost == 1 {
                let engine = opened.unwrap();
                assert!(engine.blocked);
                assert!(engine.state.repositories.contains_key(&repo.repository_id));
            } else {
                assert_eq!(
                    opened.err().unwrap().code(),
                    storage_contract::ErrorCode::MetadataQuorumUnavailable
                );
            }
            for disk in config.disks.iter().rev().take(lost) {
                assert_eq!(std::fs::read_dir(&disk.root).unwrap().count(), 0);
            }
        }
    }
}

#[test]
fn unavailable_root_classification_rejects_unsafe_or_conflicting_roots() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    for fault in ["nonempty", "wrong-seal", "symlink", "permissions"] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("system");
        let config = Config::development((0..3).map(|n| temp.path().join(format!("disk-{n}"))));
        drop(Engine::initialize(&root, config.clone()).unwrap());
        let disk = &config.disks[2];
        match fault {
            "nonempty" => {
                std::fs::remove_file(disk.root.join("disk.json")).unwrap();
            }
            "wrong-seal" => {
                std::fs::copy(
                    config.disks[0].root.join("disk.json"),
                    disk.root.join("disk.json"),
                )
                .unwrap();
            }
            "symlink" => {
                let moved = temp.path().join("moved");
                std::fs::rename(&disk.root, &moved).unwrap();
                symlink(&moved, &disk.root).unwrap();
            }
            "permissions" => {
                std::fs::set_permissions(&disk.root, std::fs::Permissions::from_mode(0o755))
                    .unwrap();
            }
            _ => unreachable!(),
        }
        assert!(Engine::open(&root).is_err(), "{fault}");
        assert!(Engine::open_for_recovery(&root).is_err(), "{fault}");
    }
}

#[test]
fn missing_payload_only_replica_preserves_history_but_not_new_copy_admission() {
    use std::io::Read;
    for checkpoint in [false, true] {
        for empty_root in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join("system");
            let mut config =
                Config::development((0..4).map(|n| temp.path().join(format!("disk-{n}"))));
            config.disks[3].metadata_member = false;
            let mut engine = Engine::initialize(&root, config.clone()).unwrap();
            let repo = engine
                .create_repository(
                    &storage_contract::RepositoryName::parse("history").unwrap(),
                    "P01",
                    4,
                    "admin",
                    storage_contract::OperationId::generate(),
                )
                .unwrap();
            let mut versions = Vec::new();
            for content in [b"old", b"new"] {
                let workspace = engine
                    .create_workspace(&repo.main_branch_id, "admin")
                    .unwrap();
                engine
                    .stage_bytes(&workspace.workspace_id, "file.txt", content)
                    .unwrap();
                engine
                    .commit_workspace(CommitRequest {
                        workspace_id: workspace.workspace_id.clone(),
                        operation_id: storage_contract::OperationId::generate(),
                        principal: "admin".into(),
                        expected_head: workspace.base_head,
                        fencing_token: workspace.fencing_token,
                        message: "save".into(),
                        request_digest: None,
                    })
                    .unwrap();
                versions.push(
                    engine.list_tree(&repo.main_branch_id).unwrap()[0]
                        .version_id
                        .clone(),
                );
            }
            let before = canonical_json(engine.state()).unwrap();
            drop(engine);
            // Generate an eligible checkpoint while every replica still exists.
            drop(Engine::open(&root).unwrap());
            for disk in config.disks.iter().filter(|disk| disk.metadata_member) {
                let path = disk.root.join("metadata/replay-checkpoint.json");
                assert!(path.exists());
                if !checkpoint {
                    std::fs::remove_file(path).unwrap();
                }
            }
            let missing = &config.disks[3].root;
            std::fs::remove_dir_all(missing).unwrap();
            if empty_root {
                private_dir(missing).unwrap();
            }
            for recovery in [false, true] {
                let mut engine = if recovery {
                    Engine::open_for_recovery(&root)
                } else {
                    Engine::open(&root)
                }
                .unwrap();
                assert_eq!(canonical_json(engine.state()).unwrap(), before);
                for (version, expected) in versions.iter().zip([b"old", b"new"]) {
                    let mut bytes = Vec::new();
                    engine
                        .open_content(version)
                        .unwrap()
                        .file
                        .read_to_end(&mut bytes)
                        .unwrap();
                    assert_eq!(&bytes, expected);
                }
                let workspace = engine
                    .create_workspace(&repo.main_branch_id, "admin")
                    .unwrap();
                engine
                    .stage_bytes(&workspace.workspace_id, "file.txt", b"third")
                    .unwrap();
                let logs: Vec<_> = engine
                    .members()
                    .iter()
                    .map(|disk| std::fs::read(log_path(disk)).unwrap())
                    .collect();
                let error = engine
                    .commit_workspace(CommitRequest {
                        workspace_id: workspace.workspace_id.clone(),
                        operation_id: storage_contract::OperationId::generate(),
                        principal: "admin".into(),
                        expected_head: workspace.base_head,
                        fencing_token: workspace.fencing_token,
                        message: "must retain four copies".into(),
                        request_digest: None,
                    })
                    .unwrap_err();
                assert_ne!(error.code(), storage_contract::ErrorCode::InDoubt);
                engine
                    .abort_workspace(&workspace.workspace_id, "admin")
                    .unwrap();
                assert_eq!(
                    engine
                        .members()
                        .iter()
                        .map(|disk| std::fs::read(log_path(disk)).unwrap())
                        .collect::<Vec<_>>(),
                    logs
                );
                assert_eq!(canonical_json(engine.state()).unwrap(), before);
                if empty_root {
                    assert_eq!(std::fs::read_dir(missing).unwrap().count(), 0);
                } else {
                    assert!(!missing.exists());
                }
            }
        }
    }
}

#[test]
fn replay_receipts_survive_multiple_tail_slot_advances_for_checkpoint() {
    let temp = tempfile::tempdir().unwrap();
    let disk = Config::development([temp.path().join("disk")])
        .disks
        .remove(0);
    private_dir(log_path(&disk).parent().unwrap()).unwrap();
    let mut writer = JournalWriter::create(&disk).unwrap();
    writer
        .append(
            &disk,
            RecordType::Prepare,
            1,
            &"a".repeat(64),
            &"b".repeat(64),
        )
        .unwrap();
    drop(writer);
    let opened = OpenJournal::scan(&disk).unwrap();
    let mut replayed = opened.scan.records.clone();
    let mut writer = opened.into_writer().unwrap();
    for sequence in 1..=4 {
        let candidate = if sequence == 1 {
            "a".repeat(64)
        } else {
            format!("{sequence:064x}")
        };
        writer
            .append(
                &disk,
                RecordType::Publication,
                sequence,
                &candidate,
                &"b".repeat(64),
            )
            .unwrap();
        retain_replayed_records(&mut replayed, writer.records());
    }
    let expected = scan(&disk).unwrap().records;
    assert_eq!(
        canonical_json(&replayed).unwrap(),
        canonical_json(&expected).unwrap()
    );
    assert_eq!(
        replayed
            .iter()
            .filter(|(r, _)| r.record_type == RecordType::Publication)
            .count(),
        4
    );
    assert_eq!(writer.records().len(), 1);
}

#[test]
fn transient_seal_read_failure_requires_reopening_missing_journal_admission() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("system");
    let config = Config::development((0..3).map(|i| temp.path().join(format!("disk-{i}"))));
    let mut engine = Engine::initialize(&root, config.clone()).unwrap();
    engine
        .create_repository(
            &storage_contract::RepositoryName::parse("published").unwrap(),
            "P01",
            1,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    drop(engine);
    let mut engine =
        Engine::prepare_workspace_recovery(root.clone(), config.clone().try_into().unwrap())
            .unwrap();
    let seal = config.disks[0].root.join("disk.json");
    let unavailable = seal.with_extension("unavailable");
    std::fs::rename(&seal, &unavailable).unwrap();
    engine.recover_mode(RecoveryMode::PreservePending).unwrap();
    std::fs::rename(&unavailable, &seal).unwrap();
    assert!(engine.is_read_only());
    assert_eq!(
        engine
            .prepare_degraded_recovery(
                storage_contract::OperationId::generate(),
                "admin",
                "retry after transient I/O"
            )
            .unwrap_err()
            .code(),
        ErrorCode::RecoveryRequired
    );
    drop(engine);
    let reopened = Engine::open_for_recovery(&root).unwrap();
    reopened.require_recovery_locks().unwrap();
    assert_eq!(reopened.journals.len(), 3);
}

#[test]
fn preservation_mode_keeps_partially_written_publication_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("system");
    let config = Config::development((0..3).map(|i| temp.path().join(format!("disk-{i}"))));
    let mut engine = Engine::initialize(&root, config.clone()).unwrap();
    engine
        .create_repository(
            &storage_contract::RepositoryName::parse("published").unwrap(),
            "P01",
            1,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    drop(engine);
    let path = log_path(&config.disks[0]);
    let complete = std::fs::read(&path).unwrap();
    std::fs::write(&path, &complete[..complete.len() - 5]).unwrap();
    let torn = std::fs::read(&path).unwrap();
    assert_eq!(
        Engine::open_for_recovery(&root).err().unwrap().code(),
        ErrorCode::RecoveryRequired
    );
    assert_eq!(std::fs::read(&path).unwrap(), torn);
    let recovered = Engine::open(&root).unwrap();
    assert!(!recovered.is_read_only());
    assert_eq!(std::fs::read(&path).unwrap(), complete);
}
