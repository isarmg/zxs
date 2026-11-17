use super::*;
use storage_contract::file_draft::DraftPayloadId;
use storage_contract::{BranchId, ContentHash, FileNodeId, Generation, VersionId};

#[test]
fn unborn_planner_uses_registered_root_without_making_a_historical_entry_or_node() {
    use crate::business::namespace::working::{WorkingEntry, WorkingNamespace};
    use crate::engine::MutationPlan;
    use crate::{Config, Engine};
    use name_policy::{LookupPath, Name, PortablePath};
    use std::fs;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use storage_contract::file_draft::{BirthRequest, DraftLocation};
    use storage_contract::{EntryId, FileId, Mode, OperationId, Principal, Sequence};

    let temporary = tempfile::tempdir().unwrap();
    let system = temporary.path().join("system");
    let disk = temporary.path().join("primary");
    for path in [&system, &disk] {
        fs::create_dir(path).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let config = Config::independent_roots(
        Mode::Development,
        [(Name::parse("Primary").unwrap(), disk.clone())],
    ).unwrap();
    let slot = config.disks[0].slot_id.clone();
    let mut engine = Engine::initialize(&system, config).unwrap();
    let repository = engine.create_disk_root(
        &slot, "admin", OperationId::generate().unwrap(),
    ).unwrap();
    let request = BirthRequest {
        repository: repository.repository_id.parse().unwrap(),
        path: PortablePath::parse("unborn.txt").unwrap(),
        expected_namespace_head: engine.state().branches[&repository.main_branch_id]
            .head_commit_id.parse().unwrap(),
    };
    let before = engine.state().clone();
    let principal: Principal = "admin".parse().unwrap();
    let operation = OperationId::generate().unwrap();
    let plan = engine.plan_unborn_birth(&request, &principal, operation).unwrap();
    let MutationPlan::Prepared { next, prepared, result } = plan else {
        panic!("new request must produce a private plan");
    };
    assert!(prepared.is_empty());
    assert_eq!(engine.state(), &before);
    assert!(!engine.state().operations.contains_key(&operation));
    assert_eq!(next.versions, before.versions);
    assert_eq!(next.branches, before.branches);
    assert_eq!(next.commits, before.commits);
    assert_eq!(next.file_containers.len(), before.file_containers.len() + 1);
    // Replay consumes the real planner's original allocation and complete receipt.
    // This remains logical evidence, not a cold-installed container capability.
    let replay = unborn::replay_birth(
        engine.planning_context().config, &before, &result, &principal,
    ).unwrap();
    assert_eq!(replay.state(), next.as_ref());
    let mut unrelated = next.as_ref().clone();
    unrelated.repositories.get_mut(&repository.repository_id).unwrap().name = "unrelated".into();
    assert!(!crate::publication_shape::same_transition(&before, replay.state(), &unrelated));
    for field in ["unexpected", "version_protection"] {
        let mut malformed = result.clone();
        malformed.result[field] = json_codec::json!([null]);
        assert!(unborn::replay_birth(
            engine.planning_context().config, &before, &malformed, &principal,
        ).is_err());
    }
    let mut wrong_owner = result.clone();
    wrong_owner.result["owner"] = json_codec::json!(storage_contract::DiskId::generate().unwrap());
    assert!(unborn::replay_birth(
        engine.planning_context().config, &before, &wrong_owner, &principal,
    ).is_err());
    let namespace = WorkingNamespace::from_main_state(
        &next, request.repository, engine.planning_context().config.max_entries,
    ).unwrap();
    let lookup = LookupPath::from(&request.path);
    let Some(WorkingEntry::Unborn { file, record, path }) = namespace.get(&lookup) else {
        panic!("the current union must retain this exact unborn owner");
    };
    assert_eq!(path, &request.path);
    assert_eq!(record.working.mode(), CommitMode::Manual);
    assert!(record.working.unborn().is_some());
    assert_eq!(record.created_by, result.operation_id);
    assert!(record.replicas.is_empty());
    assert!(namespace.vacant_parent(&request.path).is_err());
    assert!(namespace.require_published_subtree(Some(&lookup)).is_err());

    // A pure replay counterexample, not an admitted or physically created file.
    // The same current name may not be reallocated merely because Entry has no row.
    let replacement = unborn::Birth {
        repository: request.repository,
        path: request.path.clone(),
        expected_namespace_head: request.expected_namespace_head,
        file: FileId::generate().unwrap(),
        entry: EntryId::generate().unwrap(),
        owner: record.owner_disk,
    };
    assert_ne!(&replacement.file, file);
    assert!(unborn::project(
        &next,
        engine.planning_context().config,
        OperationId::generate().unwrap(),
        Sequence::new(next.sequence).next().unwrap(),
        unborn::Edit::Birth(&replacement),
        engine.planning_context().config.max_entries,
        engine.planning_context().config.max_file_bytes,
    ).is_err());

    // The real producer now uses the ordinary budget/EXCL/Candidate pipeline.
    // The earlier discarded logical plan never granted a physical identity.
    let published = engine.create_unborn_file(&request, &principal, operation).unwrap();
    let decoded = crate::persist::file_draft::birth::decode(&published).unwrap();
    let target = decoded.target().clone();
    let record = engine.state().file_containers[&target.file].clone();
    assert_eq!(record.created_by, operation);
    assert_eq!(record.working.mode(), CommitMode::Manual);
    assert_eq!(record.replicas.len(), 1);
    assert!(record.replicas[&record.owner_disk].entity.inode() != 0);
    assert_eq!(engine.state().versions, before.versions);
    assert_eq!(engine.state().branches, before.branches);
    assert_eq!(engine.state().commits, before.commits);
    assert!(engine.state().branches.values().all(|branch| branch.file_id.is_none()));
    let read = engine.open_unborn_working(&target).unwrap();
    assert_eq!(read.logical_size(), 0);
    assert_eq!(read.read_at(&mut [0; 1], 0).unwrap(), 0);
    drop(read);
    assert_eq!(engine.create_unborn_file(&request, &principal, operation).unwrap(), published);
    let state = std::sync::Arc::clone(engine.planning_context().state);
    let slot = engine.planning_context().next_slot;
    let duplicate = OperationId::generate().unwrap();
    assert!(engine.create_unborn_file(&request, &principal, duplicate).is_err());
    assert!(std::sync::Arc::ptr_eq(&state, engine.planning_context().state));
    assert_eq!(engine.planning_context().next_slot, slot);
    assert!(!engine.state().operations.contains_key(&duplicate));
    drop(state);
    drop(engine);

    let mut engine = Engine::open(&system).unwrap();
    assert_eq!(engine.state().file_containers[&target.file], record);
    assert_eq!(engine.state().versions, before.versions);
    assert_eq!(engine.state().branches, before.branches);
    assert_eq!(engine.state().commits, before.commits);
    assert_eq!(engine.open_unborn_working(&target).unwrap().logical_size(), 0);
    assert_eq!(engine.create_unborn_file(&request, &principal, operation).unwrap(), published);
    drop(engine);

    // Equal bytes at the same name are not the decided container occurrence.
    fn copy_tree(source: &std::path::Path, target: &std::path::Path) {
        fs::create_dir(target).unwrap();
        fs::set_permissions(target, fs::Permissions::from_mode(0o700)).unwrap();
        for item in fs::read_dir(source).unwrap() {
            let item = item.unwrap();
            let next = target.join(item.file_name());
            if item.file_type().unwrap().is_dir() {
                copy_tree(&item.path(), &next);
            } else {
                assert!(item.file_type().unwrap().is_file());
                fs::copy(item.path(), next).unwrap();
            }
        }
    }
    let container = crate::persist::layout::histories(&disk)
        .join(crate::persist::layout::namespace::directory(&record.namespace.name).as_path());
    let inode = fs::metadata(&container).unwrap();
    let parked = temporary.path().join("decided-unborn");
    fs::rename(&container, &parked).unwrap();
    copy_tree(&parked, &container);
    assert_ne!(fs::metadata(&container).unwrap().ino(), inode.ino());
    assert!(Engine::open(&system).is_err());
    fs::remove_dir_all(&container).unwrap();
    fs::rename(&parked, &container).unwrap();
    let restored = fs::metadata(&container).unwrap();
    assert_eq!((restored.dev(), restored.ino()), (inode.dev(), inode.ino()));
    let mut engine = Engine::open(&system).unwrap();
    assert_eq!(engine.state().file_containers[&target.file], record);
    assert_eq!(engine.open_unborn_working(&target).unwrap().logical_size(), 0);

    // Multiple accepted edits belong to one immutable save, not one generation
    // per frame. Truncation must mask both old blocks and the old partial tail.
    use storage_contract::file_draft::manifest::{SaveGeneration, WORKING_BLOCK_BYTES};
    use std::sync::Arc;
    let save_operation = OperationId::generate().unwrap();
    let mut writer = engine.begin_unborn_write(&target, save_operation).unwrap();
    assert_eq!(writer.write_at(&engine.planning_context(), 0, b"draft original tail").unwrap(), 19);
    let last = WORKING_BLOCK_BYTES + 7;
    assert_eq!(writer.write_at(&engine.planning_context(), last, b"x").unwrap(), 1);
    writer.truncate(&engine.planning_context(), last + 1, 3).unwrap();
    writer.truncate(&engine.planning_context(), 3, last + 1).unwrap();
    writer.write_at(&engine.planning_context(), last, b"z").unwrap();
    let prepared = Arc::new(writer.prepare(&engine.planning_context()).unwrap().unwrap());
    let save_request = prepared.request(&target).unwrap();
    assert_eq!(save_request.manifest.generation(), SaveGeneration::first());
    let source = Arc::clone(engine.planning_context().state);
    // A manifest naming genuine prepared files is still not the hot issuer.
    assert!(engine.save_unborn_working(&save_request, &principal, None).is_err());
    assert!(Arc::ptr_eq(&source, engine.planning_context().state));
    assert!(!engine.state().operations.contains_key(&save_operation));
    let saved = engine.save_unborn_working(
        &save_request, &principal, Some(Arc::clone(&prepared)),
    ).unwrap();
    drop(prepared);
    let replay = unborn::replay_save(
        engine.planning_context().config, &source, &saved, &principal,
    ).unwrap();
    assert_eq!(replay.state(), engine.state());
    let mut malformed = saved.clone();
    malformed.result["unexpected"] = json_codec::json!(true);
    assert!(unborn::replay_save(
        engine.planning_context().config, &source, &malformed, &principal,
    ).is_err());
    let mut unrelated = engine.state().clone();
    unrelated.repositories.get_mut(&repository.repository_id).unwrap().name = "unrelated".into();
    assert!(!crate::publication_shape::same_transition(&source, replay.state(), &unrelated));
    drop(source);
    let mut expected = vec![0; last as usize + 1];
    expected[..3].copy_from_slice(b"dra");
    expected[last as usize] = b'z';
    fn read_working(engine: &Engine, target: &DraftLocation, expected: &[u8]) {
        let reader = engine.open_unborn_working(target).unwrap();
        assert_eq!(reader.logical_size(), expected.len() as u64);
        let mut actual = vec![0; expected.len()];
        assert_eq!(reader.read_at(&mut actual, 0).unwrap(), actual.len());
        assert_eq!(actual, expected);
        assert_eq!(reader.full_digest().unwrap().content_hash(),
            ContentHash::from_digest(blake3::hash(expected)));
    }
    read_working(&engine, &target, &expected);
    let published_save = engine.state().file_containers[&target.file].clone();
    let state = Arc::clone(engine.planning_context().state);
    assert_eq!(engine.save_unborn_working(&save_request, &principal, None).unwrap(), saved);
    assert!(Arc::ptr_eq(&state, engine.planning_context().state));
    drop(state);
    assert_eq!(engine.state().versions, before.versions);
    assert_eq!(engine.state().branches, before.branches);
    assert_eq!(engine.state().commits, before.commits);
    assert_eq!(published_save.namespace, record.namespace);
    assert_eq!(published_save.replicas, record.replicas);
    drop(engine);

    engine = Engine::open(&system).unwrap();
    assert_eq!(engine.state().file_containers[&target.file], published_save);
    read_working(&engine, &target, &expected);
    assert_eq!(engine.save_unborn_working(&save_request, &principal, None).unwrap(), saved);
    // A second save can retain the same immutable inode with a shorter visible
    // prefix; re-extension is zero, never a revival of its older extent bytes.
    let second_operation = OperationId::generate().unwrap();
    let mut writer = engine.begin_unborn_write(&target, second_operation).unwrap();
    writer.truncate(&engine.planning_context(), last + 1, 2).unwrap();
    writer.truncate(&engine.planning_context(), 2, 7).unwrap();
    let prepared = Arc::new(writer.prepare(&engine.planning_context()).unwrap().unwrap());
    let second_request = prepared.request(&target).unwrap();
    assert_eq!(second_request.manifest.generation(), SaveGeneration::first().next().unwrap());
    let second = engine.save_unborn_working(
        &second_request, &principal, Some(prepared),
    ).unwrap();
    let expected = b"dr\0\0\0\0\0";
    read_working(&engine, &target, expected);
    drop(engine);
    engine = Engine::open(&system).unwrap();
    read_working(&engine, &target, expected);
    assert_eq!(engine.save_unborn_working(&save_request, &principal, None).unwrap(), saved);
    assert_eq!(engine.save_unborn_working(&second_request, &principal, None).unwrap(), second);
    assert_eq!(engine.state().versions, before.versions);
    assert_eq!(engine.state().branches, before.branches);
    assert_eq!(engine.state().commits, before.commits);

    let block = second_request.manifest.blocks()[0];
    let extent = container.join(crate::persist::layout::container::working_extent(
        block.extent().id(),
    ).as_path());
    let identity = fs::metadata(&extent).unwrap();
    let extent_bytes = fs::read(&extent).unwrap();
    let live = engine.open_unborn_working(&target).unwrap();
    let parked = temporary.path().join("held-working-extent");
    fs::rename(&extent, &parked).unwrap();
    fs::copy(&parked, &extent).unwrap();
    assert_ne!(fs::metadata(&extent).unwrap().ino(), identity.ino());
    assert!(live.read_at(&mut [0; 2], 0).is_err());
    drop(live);
    drop(engine);
    assert!(Engine::open(&system).is_err(), "equal bytes must not replace a recorded extent");
    assert_eq!(fs::read(&parked).unwrap(), extent_bytes);
    fs::remove_file(&extent).unwrap();
    fs::rename(&parked, &extent).unwrap();
    let restored = fs::metadata(&extent).unwrap();
    assert_eq!((restored.dev(), restored.ino()), (identity.dev(), identity.ino()));
    engine = Engine::open(&system).unwrap();
    read_working(&engine, &target, expected);

    // The explicit first-file producer materializes a genuine independent FULL
    // from this published Unborn save. This is preparation, not a decided node.
    use storage_contract::file_draft::{WorkingExpectation, WorkingSelection};
    use std::io::Read;
    let request = WorkingSelection {
        target: target.clone(),
        expected: WorkingExpectation::from_working(
            &engine.state().file_containers[&target.file].working,
        ).unwrap(),
    };
    let original = Arc::clone(engine.planning_context().state);
    let slot_before = engine.planning_context().next_slot;
    let commit_operation = OperationId::generate().unwrap();
    let first = engine.prepare_unborn_first_file(
        &request, &principal, commit_operation, "explicit first history only",
    ).unwrap();
    first.require_context(&engine.planning_context()).unwrap();
    assert_eq!(first.prepared().len(), 1);
    assert_eq!(first.content().version.file_id, target.file.to_string());
    assert_eq!(first.content().version.logical_size, expected.len() as u64);
    assert_eq!(first.content().version.content_hash,
        ContentHash::from_digest(blake3::hash(expected)));
    assert!(first.content().version.source_version_ids.is_empty());
    assert_eq!(first.state().versions.len(), original.versions.len() + 1);
    let resolved = resolve(first.state(), &target).unwrap();
    assert_eq!(resolved.base.version_id().to_string(), first.content().version.version_id);
    assert_eq!(resolved.base.node_id(), FileNodeId::Version(resolved.base.version_id()));
    assert!(matches!(first.working_after().unwrap(), FileWorkingState::ManualClean { .. }));
    assert_eq!(first.state().file_containers[&target.file].replicas, record.replicas);
    assert!(Arc::ptr_eq(&original, engine.planning_context().state));
    assert_eq!(engine.planning_context().next_slot, slot_before);
    assert!(!engine.state().operations.contains_key(&commit_operation));
    assert!(engine.state().file_containers[&target.file].working.unborn().is_some());
    assert_eq!(engine.state().versions, before.versions);
    let full_path = first.prepared()[0].source.path().to_owned();
    let full_identity = fs::metadata(&full_path).unwrap();
    assert_ne!(full_identity.ino(), fs::metadata(&extent).unwrap().ino());
    let mut full = first.prepared()[0].source.open().unwrap();
    let mut actual = Vec::new();
    full.read_to_end(&mut actual).unwrap();
    assert_eq!(actual, expected);
    // Reopening uses a fresh held-directory FD, not a shared cursor left at EOF.
    let mut again = first.prepared()[0].source.open().unwrap();
    let mut repeated = Vec::new();
    again.read_to_end(&mut repeated).unwrap();
    assert_eq!(repeated, expected);
    drop(again);
    drop(full);
    engine.state = Arc::new(engine.state().clone());
    assert!(matches!(first.require_context(&engine.planning_context()),
        Err(crate::Error::FileDraft(DraftError::SourceChanged))));
    engine.state = Arc::clone(&original);
    first.require_context(&engine.planning_context()).unwrap();
    let parked_full = temporary.path().join("independent-unborn-full");
    fs::rename(&full_path, &parked_full).unwrap();
    fs::copy(&parked_full, &full_path).unwrap();
    assert_ne!(fs::metadata(&full_path).unwrap().ino(), full_identity.ino());
    assert!(first.prepared()[0].source.open().is_err());
    assert!(first.require_context(&engine.planning_context()).is_err());
    assert!(Arc::ptr_eq(&original, engine.planning_context().state));
    assert!(!engine.state().operations.contains_key(&commit_operation));
    fs::remove_file(&full_path).unwrap();
    fs::rename(&parked_full, &full_path).unwrap();
    drop(first);
    drop(original);
    // A failed or abandoned FULL cannot clear the saved row or acquire a node.
    // Cold opening continues from the actual Save receipt, not an orphan name.
    drop(engine);
    engine = Engine::open(&system).unwrap();
    read_working(&engine, &target, expected);
    assert_eq!(engine.state().versions, before.versions);
    assert_eq!(engine.state().branches, before.branches);
    assert_eq!(engine.state().commits, before.commits);

    #[cfg(feature = "fault-injection")]
    {
        let operation = OperationId::generate().unwrap();
        let mut writer = engine.begin_unborn_write(&target, operation).unwrap();
        writer.write_at(&engine.planning_context(), 2, b"COLD").unwrap();
        let prepared = Arc::new(writer.prepare(&engine.planning_context()).unwrap().unwrap());
        let request = prepared.request(&target).unwrap();
        let original = Arc::clone(engine.planning_context().state);
        engine.inject_fault_once("after_accept_1");
        let error = engine.save_unborn_working(&request, &principal, Some(prepared)).unwrap_err();
        assert_eq!(error.code(), storage_contract::ErrorCode::InDoubt);
        let mut cause: &dyn std::error::Error = &error;
        let mut actual_cut = false;
        while let Some(source) = cause.source() {
            actual_cut |= source.to_string().contains("after_accept_1");
            cause = source;
        }
        assert!(actual_cut, "the actual native Accept cut must have been reached");
        assert!(Arc::ptr_eq(&original, engine.planning_context().state));
        drop(original);
        drop(engine);
        let extent = container.join(crate::persist::layout::container::working_extent(
            request.manifest.blocks()[0].extent().id(),
        ).as_path());
        let identity = fs::metadata(&extent).unwrap();
        let parked = temporary.path().join("accepted-unborn-extent");
        fs::rename(&extent, &parked).unwrap();
        assert!(Engine::open(&system).is_err(), "a decided missing extent is not an empty file");
        fs::rename(&parked, &extent).unwrap();
        let restored = fs::metadata(&extent).unwrap();
        assert_eq!((restored.dev(), restored.ino()), (identity.dev(), identity.ino()));
        engine = Engine::open(&system).unwrap();
        read_working(&engine, &target, b"drCOLD\0");
        let receipt = engine.state().operations[&operation].clone();
        assert_eq!(engine.save_unborn_working(&request, &principal, None).unwrap(), receipt);
        assert_eq!(engine.state().versions, before.versions);
        assert_eq!(engine.state().branches, before.branches);
        assert_eq!(engine.state().commits, before.commits);
    }

    // Only this explicit command may publish the first historical FILE node.
    // The original immutable extents remain required for exact native recovery;
    // the materialized full hash cannot reconstruct their clipped visible prefix.
    let selected = WorkingSelection {
        target: target.clone(),
        expected: WorkingExpectation::from_working(
            &engine.state().file_containers[&target.file].working,
        ).unwrap(),
    };
    let prior = engine.state().clone();
    let reader = engine.open_unborn_working(&target).unwrap();
    let mut committed_bytes = vec![0; reader.logical_size() as usize];
    assert_eq!(reader.read_at(&mut committed_bytes, 0).unwrap(), committed_bytes.len());
    drop(reader);
    let operation = OperationId::generate().unwrap();
    let message = "explicit first node after durable saves";
    #[cfg(feature = "fault-injection")]
    {
        engine.inject_fault_once("after_accept_1");
        let error = engine.commit_unborn_working(&selected, &principal, operation, message)
            .unwrap_err();
        assert_eq!(error.code(), storage_contract::ErrorCode::InDoubt);
        let mut cause: &dyn std::error::Error = &error;
        let mut actual_cut = false;
        while let Some(source) = cause.source() {
            actual_cut |= source.to_string().contains("after_accept_1");
            cause = source;
        }
        assert!(actual_cut, "first-commit must reach the real native Accept cut");
        assert_eq!(engine.state(), &prior);
        drop(engine);
        let working = prior.file_containers[&target.file].working.unborn().unwrap();
        let storage_contract::file_draft::UnbornBody::Saved { manifest } = working.body() else {
            panic!("the real earlier saves provide the accepted source manifest");
        };
        let original_extent = container.join(crate::persist::layout::container::working_extent(
            manifest.blocks()[0].extent().id(),
        ).as_path());
        let identity = fs::metadata(&original_extent).unwrap();
        let parked = temporary.path().join("first-commit-original-extent");
        fs::rename(&original_extent, &parked).unwrap();
        fs::copy(&parked, &original_extent).unwrap();
        assert_ne!(fs::metadata(&original_extent).unwrap().ino(), identity.ino());
        assert!(Engine::open(&system).is_err(), "equal bytes cannot replace the saved source");
        fs::remove_file(&original_extent).unwrap();
        assert!(Engine::open(&system).is_err(), "an accepted first node cannot infer source bytes");
        fs::rename(&parked, &original_extent).unwrap();
        let restored = fs::metadata(&original_extent).unwrap();
        assert_eq!((restored.dev(), restored.ino()), (identity.dev(), identity.ino()));
        engine = Engine::open(&system).unwrap();
    }
    let published = engine.commit_unborn_working(&selected, &principal, operation, message)
        .unwrap();
    let first = crate::persist::file_draft::unborn_first::decode(&prior, &published).unwrap();
    let version = first.content().version.clone();
    assert_eq!(version.file_id, target.file.to_string());
    assert_eq!(version.logical_size, committed_bytes.len() as u64);
    assert_eq!(version.content_hash, ContentHash::from_digest(blake3::hash(&committed_bytes)));
    assert_eq!(engine.state().versions.len(), before.versions.len() + 1);
    assert_eq!(engine.state().file_containers[&target.file].replicas, record.replicas);
    assert_eq!(engine.state().file_containers[&target.file].namespace, record.namespace);
    assert!(matches!(engine.state().file_containers[&target.file].working,
        FileWorkingState::ManualClean { .. }));
    let current = resolve(engine.state(), &target).unwrap();
    assert_eq!(current.base.version_id().to_string(), version.version_id);
    assert_eq!(current.base.node_id(), FileNodeId::Version(current.base.version_id()));
    assert!(engine.open_unborn_working(&target).is_err());
    let mut opened = engine.open_content(&version.version_id).unwrap();
    let mut bytes = Vec::new();
    opened.file.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, committed_bytes);
    drop(opened);
    let mut bad = published.clone();
    bad.result["request"]["body"]["unexpected"] = json_codec::json!(true);
    assert!(crate::persist::file_draft::unborn_first::decode(&prior, &bad).is_err());
    drop(engine);
    engine = Engine::open(&system).unwrap();
    let stable = Arc::clone(engine.planning_context().state);
    let slot = engine.planning_context().next_slot;
    assert_eq!(engine.commit_unborn_working(&selected, &principal, operation, message)
        .unwrap(), published);
    assert!(Arc::ptr_eq(&stable, engine.planning_context().state));
    assert_eq!(engine.planning_context().next_slot, slot);
    assert_eq!(engine.state().versions.len(), before.versions.len() + 1);
    let new_operation = OperationId::generate().unwrap();
    assert!(engine.commit_unborn_working(&selected, &principal, new_operation, message).is_err());
    assert!(Arc::ptr_eq(&stable, engine.planning_context().state));
    assert!(!engine.state().operations.contains_key(&new_operation));
    let mut opened = engine.open_content(&version.version_id).unwrap();
    let mut bytes = Vec::new();
    opened.file.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, committed_bytes);
    drop(opened);
    drop(stable);

    // A later genuine namespace decision changes only the location of this
    // container. First-commit replay must still consume its original saved
    // extents, not reopen the old spelling or infer the FULL from its hash.
    let versions = engine.state().versions.clone();
    let renamed = engine.file_namespace(
        &repository.repository_id,
        crate::NamespaceOperation::parse(
            crate::NamespaceOperationKind::Rename, "unborn.txt", Some("renamed.txt"),
        ).unwrap(),
        "admin", OperationId::generate().unwrap(),
    ).unwrap();
    let renamed_record = engine.state().file_containers[&target.file].clone();
    assert_eq!(renamed_record.namespace.name.as_str(), "renamed.txt");
    assert_eq!(renamed_record.replicas, record.replicas);
    assert_eq!(engine.state().versions, versions);
    assert!(!container.exists());
    // Reusing the vacant old spelling gives a distinct real Unborn identity.
    // That empty owner cannot stand in for the earlier nonempty saved source.
    let replacement_request = BirthRequest {
        repository: target.repository,
        path: target.path.clone(),
        expected_namespace_head: engine.state().branches[&repository.main_branch_id]
            .head_commit_id.parse().unwrap(),
    };
    let replacement = engine.create_unborn_file(
        &replacement_request, &principal, OperationId::generate().unwrap(),
    ).unwrap();
    let replacement = crate::persist::file_draft::birth::decode(&replacement).unwrap();
    assert_ne!(replacement.target().file, target.file);
    assert_eq!(engine.open_unborn_working(replacement.target()).unwrap().logical_size(), 0);
    assert_eq!(engine.state().versions, versions);
    let renamed_container = crate::persist::layout::histories(&disk).join(
        crate::persist::layout::namespace::directory(&renamed_record.namespace.name).as_path(),
    );
    let renamed_identity = fs::metadata(&renamed_container).unwrap();
    let replacement_identity = fs::metadata(&container).unwrap();
    drop(engine);
    let parked = temporary.path().join("renamed-first-container");
    fs::rename(&renamed_container, &parked).unwrap();
    copy_tree(&parked, &renamed_container);
    assert_ne!(fs::metadata(&renamed_container).unwrap().ino(), renamed_identity.ino());
    assert!(Engine::open(&system).is_err(), "a published name cannot adopt a replacement inode");
    assert_eq!(fs::metadata(&container).unwrap().ino(), replacement_identity.ino());
    fs::remove_dir_all(&renamed_container).unwrap();
    fs::rename(&parked, &renamed_container).unwrap();
    let restored = fs::metadata(&renamed_container).unwrap();
    assert_eq!((restored.dev(), restored.ino()),
        (renamed_identity.dev(), renamed_identity.ino()));
    engine = Engine::open(&system).unwrap();
    assert_eq!(engine.state().file_containers[&target.file], renamed_record);
    assert_eq!(engine.operation(renamed.operation_id).unwrap(), renamed);
    assert_eq!(engine.commit_unborn_working(&selected, &principal, operation, message)
        .unwrap(), published);
    assert_eq!(engine.state().versions, versions);
    assert_eq!(engine.open_unborn_working(replacement.target()).unwrap().logical_size(), 0);
    let mut opened = engine.open_content(&version.version_id).unwrap();
    let mut bytes = Vec::new();
    opened.file.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, committed_bytes);
    drop(opened);

    #[cfg(feature = "fault-injection")]
    {
        // This cut has genuinely finalized and moved the same container, but
        // has not published the new name. Its existing replay locator alone
        // can allow the original first-commit source to follow that decision.
        let rename_operation = OperationId::generate().unwrap();
        engine.inject_fault_once("after_container_names");
        let error = engine.file_namespace(
            &repository.repository_id,
            crate::NamespaceOperation::parse(
                crate::NamespaceOperationKind::Rename, "renamed.txt", Some("final.txt"),
            ).unwrap(),
            "admin", rename_operation,
        ).unwrap_err();
        assert_eq!(error.code(), storage_contract::ErrorCode::InDoubt);
        let crate::Error::Context { source, .. } = error else {
            panic!("a finalized rename retains its actual cut cause");
        };
        assert_eq!(source.code(), storage_contract::ErrorCode::InjectedFailure);
        assert!(source.to_string().contains("after_container_names"));
        drop(engine);
        engine = Engine::open(&system).unwrap();
        assert!(!engine.is_read_only());
        assert_eq!(engine.state().file_containers[&target.file].namespace.name.as_str(),
            "final.txt");
        assert_eq!(engine.state().file_containers[&target.file].replicas, record.replicas);
        assert_eq!(engine.state().versions, versions);
        assert_eq!(engine.commit_unborn_working(&selected, &principal, operation, message)
            .unwrap(), published);
        let mut opened = engine.open_content(&version.version_id).unwrap();
        let mut bytes = Vec::new();
        opened.file.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, committed_bytes);
    }
}

fn revision(value: u64) -> WorkingRevision {
    WorkingRevision::new(Generation::new(value))
}

fn base(selection: u64) -> DraftBase {
    let version = VersionId::from_random([3; 16]);
    DraftBase::new(
        BranchId::from_random([2; 16]),
        FileNodeId::Version(version),
        version,
        Generation::new(selection),
    )
}

fn payload(identity: u8, bytes: &[u8]) -> DraftPayload {
    DraftPayload::new(
        DraftPayloadId::from_random([identity; 16]),
        ContentHash::from_digest(blake3::hash(bytes)),
        bytes.len() as u64,
    )
}

#[test]
fn manual_writes_retain_first_base_and_commit_consumes_the_exact_latest_draft() {
    let automatic = FileWorkingState::Automatic {
        revision: revision(0),
    };
    let mode = change_mode(&automatic, revision(0), CommitMode::Manual).unwrap();
    assert!(mode.changed());
    assert_eq!(mode.previous(), &automatic);
    let clean = mode.into_next();
    assert_eq!(clean.revision(), revision(1));
    let first = write(&clean, revision(1), base(8), payload(4, b"first draft"))
        .unwrap()
        .into_next();
    assert!(clean.draft().is_none());
    let second = write(&first, revision(2), base(8), payload(5, b"second draft"))
        .unwrap()
        .into_next();
    let (original_base, original_payload) = first.draft().unwrap();
    let (latest_base, latest_payload) = second.draft().unwrap();
    assert_eq!(original_base, latest_base);
    assert_ne!(original_payload.payload_id(), latest_payload.payload_id());
    assert_eq!(original_payload.content_hash(), payload(4, b"first draft").content_hash());
    let prepared = commit(&second, revision(3)).unwrap();
    assert_eq!(prepared.base(), latest_base);
    assert_eq!(prepared.payload(), latest_payload);
    assert!(second.draft().is_some());
    let completed = prepared.into_change();
    assert_eq!(completed.previous(), &second);
    assert_eq!(completed.next().revision(), revision(4));
    assert_eq!(completed.next().mode(), CommitMode::Manual);
    assert!(completed.next().draft().is_none());
}

#[test]
fn stale_writes_mode_changes_and_selection_rebinding_do_not_replace_a_draft() {
    let clean = FileWorkingState::ManualClean {
        revision: revision(2),
    };
    let dirty = write(&clean, revision(2), base(1), payload(4, b"draft"))
        .unwrap()
        .into_next();
    let before = dirty.clone();
    assert!(matches!(
        write(&dirty, revision(2), base(2), payload(5, b"later")),
        Err(DraftError::RevisionChanged { expected, actual })
            if expected == revision(2) && actual == revision(3)
    ));
    let original = base(1);
    let other_version = VersionId::from_random([9; 16]);
    for replacement_base in [
        base(2),
        DraftBase::new(
            BranchId::from_random([9; 16]),
            original.node_id(),
            original.version_id(),
            original.selection_generation(),
        ),
        DraftBase::new(
            original.branch_id(),
            FileNodeId::Version(other_version),
            original.version_id(),
            original.selection_generation(),
        ),
        DraftBase::new(
            original.branch_id(),
            original.node_id(),
            other_version,
            original.selection_generation(),
        ),
    ] {
        assert!(matches!(
            write(&dirty, revision(3), replacement_base, payload(5, b"later")),
            Err(DraftError::BaseChanged)
        ));
    }
    assert!(matches!(
        change_mode(&dirty, revision(3), CommitMode::Automatic),
        Err(DraftError::Dirty)
    ));
    assert!(matches!(require_clean(&dirty), Err(DraftError::Dirty)));
    assert!(!change_mode(&dirty, revision(3), CommitMode::Manual).unwrap().changed());
    assert_eq!(dirty, before);
    let automatic = FileWorkingState::Automatic {
        revision: revision(0),
    };
    assert!(matches!(
        write(&automatic, revision(0), base(1), payload(4, b"draft")),
        Err(DraftError::ManualRequired)
    ));
}

#[test]
fn discard_preserves_mode_and_same_bytes_still_advance_the_working_fence() {
    let clean = FileWorkingState::ManualClean {
        revision: revision(1),
    };
    let first = write(&clean, revision(1), base(1), payload(4, b"same"))
        .unwrap()
        .into_next();
    let same_bytes = write(&first, revision(2), base(1), payload(5, b"same"))
        .unwrap()
        .into_next();
    assert_eq!(same_bytes.revision(), revision(3));
    assert_eq!(
        same_bytes.draft().unwrap().1.content_hash(),
        first.draft().unwrap().1.content_hash(),
    );
    let discarded = discard(&same_bytes, revision(3)).unwrap().into_next();
    assert_eq!(discarded.revision(), revision(4));
    assert_eq!(discarded.mode(), CommitMode::Manual);
    assert!(discarded.draft().is_none());
    assert!(!discard(&discarded, revision(4)).unwrap().changed());
    assert!(matches!(
        discard(&discarded, revision(3)),
        Err(DraftError::RevisionChanged { .. })
    ));
    assert!(matches!(
        commit(&discarded, revision(4)),
        Err(DraftError::DraftRequired)
    ));
}

#[test]
fn exhausted_working_revision_never_wraps_or_clears_a_dirty_body() {
    let maximum = revision(u64::MAX);
    let automatic = FileWorkingState::Automatic { revision: maximum };
    assert!(!change_mode(&automatic, maximum, CommitMode::Automatic).unwrap().changed());
    assert!(matches!(
        change_mode(&automatic, maximum, CommitMode::Manual),
        Err(DraftError::RevisionExhausted(GenerationExhausted))
    ));
    let dirty = FileWorkingState::ManualDirty {
        revision: maximum,
        base: base(1),
        payload: payload(4, b"retain"),
    };
    let before = dirty.clone();
    assert!(matches!(
        write(&dirty, maximum, base(1), payload(5, b"replacement")),
        Err(DraftError::RevisionExhausted(GenerationExhausted))
    ));
    assert!(matches!(
        commit(&dirty, maximum),
        Err(DraftError::RevisionExhausted(GenerationExhausted))
    ));
    assert!(matches!(
        discard(&dirty, maximum),
        Err(DraftError::RevisionExhausted(GenerationExhausted))
    ));
    assert_eq!(dirty, before);
}

#[test]
fn whole_working_selection_checks_all_cas_before_open_and_retains_real_published_drafts() {
    use crate::{CommitRequest, Config, Engine, ReadFile};
    use name_policy::{Name, PortablePath};
    use std::fs;
    use std::io::Write;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::sync::Arc;
    use storage_contract::file_draft::{
        DraftCommand, DraftLocation, WorkingContentExpectation, WorkingExpectation,
        WorkingSelection,
    };
    use storage_contract::{Mode, OperationId, Principal};

    fn input(bytes: &[u8]) -> ReadFile {
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(bytes).unwrap();
        file.sync_all().unwrap();
        ReadFile::from(file)
    }

    let temporary = tempfile::tempdir().unwrap();
    let system = temporary.path().join("system");
    let disk = temporary.path().join("primary");
    for path in [&system, &disk] {
        fs::create_dir(path).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let config = Config::independent_roots(
        Mode::Development,
        [(Name::parse("Primary").unwrap(), disk.clone())],
    ).unwrap();
    let slot = config.disks[0].slot_id.clone();
    let mut engine = Engine::initialize(&system, config).unwrap();
    let repository = engine.create_disk_root(
        &slot, "admin", OperationId::generate().unwrap(),
    ).unwrap();
    let workspace = engine.create_workspace(&repository.main_branch_id, "admin").unwrap();
    for name in ["first.txt", "second.txt"] {
        engine.stage_bytes(&workspace.workspace_id, name, b"real historical body").unwrap();
    }
    engine.commit_workspace(CommitRequest {
        workspace_id: workspace.workspace_id.clone(),
        operation_id: OperationId::generate().unwrap(),
        principal: "admin".into(),
        expected_head: workspace.base_head.clone(),
        fencing_token: workspace.fencing_token,
        message: "two actually owned FILEs".into(),
        request_digest: None,
    }).unwrap();
    let principal: Principal = "admin".parse().unwrap();
    let bytes: [&[u8]; 2] = [b"selected first draft", b"selected second draft"];
    let mut requests = Vec::new();
    for (name, body) in ["first.txt", "second.txt"].into_iter().zip(bytes) {
        let entry = engine.list_tree(&repository.main_branch_id).unwrap().into_iter()
            .find(|entry| entry.path() == name).unwrap();
        let target = DraftLocation {
            repository: repository.repository_id.parse().unwrap(),
            path: PortablePath::parse(name).unwrap(),
            entry: entry.entry_id.parse().unwrap(),
            file: entry.file_id.parse().unwrap(),
        };
        engine.change_file_working(&DraftCommand::SetMode {
            expected: engine.file_working_state(&target).unwrap().revision(),
            target: target.clone(), mode: CommitMode::Manual,
        }, &principal, OperationId::generate().unwrap()).unwrap();
        let write = DraftCommand::Write {
            expected: engine.file_working_state(&target).unwrap().revision(),
            base: engine.file_draft_base(&target).unwrap(),
            target: target.clone(),
            content_hash: ContentHash::from_digest(blake3::hash(body)),
            logical_size: body.len() as u64,
        };
        engine.write_file_draft(
            &write, &principal, OperationId::generate().unwrap(), input(body),
        ).unwrap();
        let working = engine.file_working_state(&target).unwrap();
        requests.push(WorkingSelection {
            target, expected: WorkingExpectation::from_working(&working).unwrap(),
        });
    }
    let state = Arc::clone(engine.planning_context().state);
    let selected = engine.select_working_sources(&requests).unwrap();
    assert_eq!(selected.sources().len(), 2);
    for ((source, expected), bytes) in selected.sources().iter().zip(&requests).zip(bytes) {
        assert_eq!(source.target(), &expected.target);
        assert_eq!(source.expected(), expected.expected);
        let mut actual = vec![0; bytes.len() + 1];
        let count = source.read_at(&mut actual, 0).unwrap();
        assert_eq!(&actual[..count], bytes);
        assert_eq!(source.read_at(&mut actual, bytes.len() as u64).unwrap(), 0);
    }
    assert!(Arc::ptr_eq(&state, engine.planning_context().state));
    let WorkingContentExpectation::Born { base, payload } = requests[0].expected.content else {
        panic!("this test deliberately uses the existing Published draft producer");
    };
    let mut changed_payload = requests.clone();
    changed_payload[0].expected.content = WorkingContentExpectation::Born {
        base,
        payload: DraftPayload::new(
            DraftPayloadId::generate().unwrap(), payload.content_hash().unwrap(), payload.logical_size(),
        ),
    };
    assert!(matches!(engine.select_working_sources(&changed_payload),
        Err(crate::Error::FileDraft(DraftError::WorkingBodyChanged))));
    assert!(matches!(engine.select_working_sources(&[requests[0].clone(), requests[0].clone()]),
        Err(crate::Error::FileDraft(DraftError::WorkingSelection))));

    let record = &engine.state().file_containers[&requests[0].target.file];
    let path = crate::persist::layout::histories(&disk)
        .join(crate::persist::layout::namespace::directory(&record.namespace.name).as_path())
        .join(crate::persist::layout::container::DRAFTS.as_path())
        .join(crate::persist::layout::container::draft_payload(payload.payload_id()).as_path());
    let identity = fs::metadata(&path).unwrap();
    let parked = temporary.path().join("held-first-draft");
    fs::rename(&path, &parked).unwrap();
    let mut stale = requests.clone();
    stale[1].expected.revision = stale[1].expected.revision.next().unwrap();
    // A stale later item wins before the first missing payload can be opened.
    assert!(matches!(engine.select_working_sources(&stale),
        Err(crate::Error::FileDraft(DraftError::RevisionChanged { .. }))));
    assert!(selected.sources()[0].require_source().is_err());
    assert!(Arc::ptr_eq(&state, engine.planning_context().state));
    drop(selected);
    fs::rename(&parked, &path).unwrap();
    let restored = fs::metadata(&path).unwrap();
    assert_eq!((restored.dev(), restored.ino()), (identity.dev(), identity.ino()));
    engine.select_working_sources(&requests).unwrap().require_engine(&engine).unwrap();
    assert_eq!(engine.state(), state.as_ref());
    drop(state);
    drop(engine);
    let engine = Engine::open(&system).unwrap();
    let selected = engine.select_working_sources(&requests).unwrap();
    selected.require_engine(&engine).unwrap();
    for (source, bytes) in selected.sources().iter().zip(bytes) {
        let mut actual = vec![0; bytes.len()];
        assert_eq!(source.read_at(&mut actual, 0).unwrap(), bytes.len());
        assert_eq!(actual, bytes);
    }
}

#[test]
fn born_working_batch_uses_one_prior_and_exact_sources_without_independent_clears() {
    use crate::engine::file_draft::PreparedWorkingBatch;
    use crate::{CommitRequest, Config, Engine, ReadFile};
    use name_policy::{Name, PortablePath};
    use std::fs;
    use std::io::{Read, Write};
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::sync::Arc;
    use storage_contract::file_draft::{
        DraftCommand, DraftLocation, WorkingContentExpectation, WorkingExpectation,
        WorkingSelection,
    };
    use storage_contract::{Mode, OperationId, Principal, Sequence};

    fn input(bytes: &[u8]) -> ReadFile {
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(bytes).unwrap();
        file.sync_all().unwrap();
        ReadFile::from(file)
    }

    let temporary = tempfile::tempdir().unwrap();
    let system = temporary.path().join("system");
    let disk = temporary.path().join("primary");
    for path in [&system, &disk] {
        fs::create_dir(path).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let config = Config::independent_roots(
        Mode::Development, [(Name::parse("Primary").unwrap(), disk.clone())],
    ).unwrap();
    let slot = config.disks[0].slot_id.clone();
    let mut engine = Engine::initialize(&system, config).unwrap();
    let repository = engine.create_disk_root(
        &slot, "admin", OperationId::generate().unwrap(),
    ).unwrap();
    let names = ["first.txt", "same.txt", "second.txt", "unselected.txt"];
    let original = b"real original version";
    let workspace = engine.create_workspace(&repository.main_branch_id, "admin").unwrap();
    for name in names {
        engine.stage_bytes(&workspace.workspace_id, name, original).unwrap();
    }
    engine.commit_workspace(CommitRequest {
        workspace_id: workspace.workspace_id.clone(),
        operation_id: OperationId::generate().unwrap(), principal: "admin".into(),
        expected_head: workspace.base_head.clone(), fencing_token: workspace.fencing_token,
        message: "four actually owned versions".into(), request_digest: None,
    }).unwrap();
    let principal: Principal = "admin".parse().unwrap();
    let bodies: [&[u8]; 4] = [
        b"first selected change", original, b"second selected change", b"leave this dirty",
    ];
    let mut requests = Vec::new();
    for (name, bytes) in names.into_iter().zip(bodies) {
        let entry = engine.list_tree(&repository.main_branch_id).unwrap().into_iter()
            .find(|entry| entry.path() == name).unwrap();
        let target = DraftLocation {
            repository: repository.repository_id.parse().unwrap(),
            path: PortablePath::parse(name).unwrap(),
            entry: entry.entry_id.parse().unwrap(), file: entry.file_id.parse().unwrap(),
        };
        engine.change_file_working(&DraftCommand::SetMode {
            target: target.clone(),
            expected: engine.file_working_state(&target).unwrap().revision(),
            mode: CommitMode::Manual,
        }, &principal, OperationId::generate().unwrap()).unwrap();
        engine.write_file_draft(&DraftCommand::Write {
            target: target.clone(),
            expected: engine.file_working_state(&target).unwrap().revision(),
            base: engine.file_draft_base(&target).unwrap(),
            logical_size: bytes.len() as u64,
            content_hash: ContentHash::from_digest(blake3::hash(bytes)),
        }, &principal, OperationId::generate().unwrap(), input(bytes)).unwrap();
        let working = engine.file_working_state(&target).unwrap();
        requests.push(WorkingSelection {
            target, expected: WorkingExpectation::from_working(&working).unwrap(),
        });
    }
    // Sources are obtained after a genuine cold read of the saved drafts.
    // This fixture does not claim a combined directory receipt was published.
    drop(engine);
    let mut engine = Engine::open(&system).unwrap();
    let before = Arc::clone(engine.planning_context().state);
    let unselected = before.file_containers[&requests[3].target.file].clone();
    let selected = Arc::new(engine.select_working_sources(&requests[..3]).unwrap());
    let operation = OperationId::generate().unwrap();
    let batch = PreparedWorkingBatch::prepare(
        &engine.planning_context(), Arc::clone(&selected), operation, &principal,
        "one combined selected-set construction",
    ).unwrap();
    batch.require_reconstruction(&engine.planning_context()).unwrap();
    assert!(Arc::ptr_eq(&before, engine.planning_context().state));
    assert_eq!(engine.state(), before.as_ref());
    assert_eq!(batch.items().len(), 3);
    assert_eq!(batch.prepared().len(), 2);
    assert_eq!(batch.state().sequence, engine.planning_context().next_slot);
    assert_eq!(batch.state().business_generation, before.business_generation + 1);
    assert_eq!(batch.state().versions.len(), before.versions.len() + 2);
    assert_eq!(batch.state().operations, before.operations);
    assert_eq!(batch.state().operation_principals, before.operation_principals);
    assert_eq!(batch.state().events, before.events);
    assert_eq!(batch.state().file_containers[&requests[3].target.file], unselected);
    assert!(batch.state().file_containers[&requests[3].target.file].working.draft().is_some());
    for (index, item) in batch.items().iter().enumerate() {
        assert_eq!(item.target(), &requests[index].target);
        assert_eq!(item.version().logical_size, bodies[index].len() as u64);
        assert_eq!(
            item.version().content_hash, ContentHash::from_digest(blake3::hash(bodies[index])),
        );
        assert_eq!(item.working_after(),
            &batch.state().file_containers[&requests[index].target.file].working);
        assert!(matches!(item.working_after(), FileWorkingState::ManualClean { .. }));
        assert!(engine.file_working_state(item.target()).unwrap().draft().is_some());
        let projected = resolve(batch.state(), item.target()).unwrap();
        assert_eq!(projected.base.version_id().to_string(), item.version().version_id);
        assert_eq!(projected.base.node_id(), item.node().unwrap());
        if index == 1 {
            assert!(matches!(item.result(), batch::WorkingBatchResult::Retained(_)));
            let WorkingContentExpectation::Born { base, .. } = requests[index].expected.content
            else { panic!("real Born fixture"); };
            assert_eq!(item.version().version_id, base.version_id().to_string());
            assert_eq!(item.node().unwrap(), base.node_id());
        } else {
            assert!(matches!(item.result(), batch::WorkingBatchResult::Published(_)));
            assert!(!before.versions.contains_key(&item.version().version_id));
        }
    }
    for (prepared, index) in batch.prepared().iter().zip([0, 2]) {
        let mut source = prepared.source.open().unwrap();
        let mut actual = Vec::new();
        source.read_to_end(&mut actual).unwrap();
        assert_eq!(actual, bodies[index]);
        assert_eq!(prepared.version.version_sequence, batch.state().sequence);
    }

    let replay = batch::replay(
        &before, engine.planning_context().config, batch.request(),
        Sequence::new(batch.state().sequence), batch.trace(), &batch.first_reads().unwrap(), batch.draft_reads(),
    ).unwrap();
    assert_eq!(replay.state(), batch.state());
    let evidence = batch.prepared().iter().map(|item| crate::journal::VersionEvidence {
        version: item.version.clone(), placement: item.placement.clone(),
    }).collect::<Vec<_>>();
    replay.require_versions(&evidence).unwrap();
    assert!(replay.require_versions(&evidence[..1]).is_err());
    let mut repeated = evidence.clone();
    repeated.push(evidence[0].clone());
    assert!(replay.require_versions(&repeated).is_err());
    let mut reversed = evidence.clone();
    reversed.reverse();
    assert!(replay.require_versions(&reversed).is_err());
    let mut wrong = batch.trace().clone();
    wrong.contents[2] = wrong.contents[0].clone();
    assert!(batch::replay(
        &before, engine.planning_context().config, batch.request(),
        Sequence::new(batch.state().sequence), &wrong, &batch.first_reads().unwrap(), batch.draft_reads(),
    ).is_err());
    let mut wrong = batch.trace().clone();
    wrong.contents[1] = wrong.contents[0].clone();
    assert!(batch::replay(
        &before, engine.planning_context().config, batch.request(),
        Sequence::new(batch.state().sequence), &wrong, &batch.first_reads().unwrap(), batch.draft_reads(),
    ).is_err());
    let mut wrong = batch.trace().clone();
    let earlier_ids = wrong.contents[0].as_ref().unwrap().allocations.clone();
    wrong.contents[2].as_mut().unwrap().allocations = earlier_ids;
    assert!(batch::replay(
        &before, engine.planning_context().config, batch.request(),
        Sequence::new(batch.state().sequence), &wrong, &batch.first_reads().unwrap(), batch.draft_reads(),
    ).is_err());
    // A fresh picker on identical values is not the same retained selection.
    let other = Arc::new(engine.select_working_sources(&requests[..3]).unwrap());
    assert!(matches!(batch.require_selection(&other),
        Err(crate::Error::FileDraft(DraftError::SourceChanged))));
    let mut substituted: Vec<_> = batch.prepared().iter().map(|prepared| {
        crate::engine::Prepared {
            version: prepared.version.clone(), placement: prepared.placement.clone(),
            source: prepared.source.clone(),
        }
    }).collect();
    substituted[0].source = crate::engine::ReadSource::selected_working(other, 0).unwrap();
    assert!(batch.require_prepared(&substituted).is_err());

    // Even value-equal State cannot reissue the actual source occurrence.
    let original_state = Arc::clone(&engine.state);
    engine.state = Arc::new(original_state.as_ref().clone());
    assert!(matches!(batch.require_context(&engine.planning_context()),
        Err(crate::Error::FileDraft(DraftError::SourceChanged))));
    engine.state = original_state;
    batch.require_context(&engine.planning_context()).unwrap();

    let WorkingContentExpectation::Born { payload, .. } = requests[2].expected.content
    else { panic!("real Born fixture"); };
    let record = &before.file_containers[&requests[2].target.file];
    let path = crate::persist::layout::histories(&disk)
        .join(crate::persist::layout::namespace::directory(&record.namespace.name).as_path())
        .join(crate::persist::layout::container::DRAFTS.as_path())
        .join(crate::persist::layout::container::draft_payload(payload.payload_id()).as_path());
    let identity = fs::metadata(&path).unwrap();
    let parked = temporary.path().join("last-selected-original");
    fs::rename(&path, &parked).unwrap();
    fs::copy(&parked, &path).unwrap();
    assert!(batch.require_context(&engine.planning_context()).is_err());
    assert!(PreparedWorkingBatch::prepare(
        &engine.planning_context(), Arc::clone(&selected), operation, &principal, "same request",
    ).is_err());
    assert_eq!(engine.state(), before.as_ref());
    // Restore only this test's replacement, not any production cleanup path.
    fs::remove_file(&path).unwrap();
    fs::rename(&parked, &path).unwrap();
    let restored = fs::metadata(&path).unwrap();
    assert_eq!((restored.dev(), restored.ino()), (identity.dev(), identity.ino()));
    // Renaming changes the inode's change time. The old admission remains
    // stale even after restoration; only a new actual full readback can retry.
    assert!(batch.require_context(&engine.planning_context()).is_err());
    let renewed = Arc::new(engine.select_working_sources(&requests[..3]).unwrap());
    let retry = PreparedWorkingBatch::prepare(
        &engine.planning_context(), renewed, operation, &principal,
        "one combined selected-set construction",
    ).unwrap();
    retry.require_reconstruction(&engine.planning_context()).unwrap();
    let expected = retry.state().clone();
    let trace = retry.trace().clone();
    let (next, prepared, handoff) = retry.into_publication(&engine.planning_context()).unwrap();
    assert_eq!(next, expected);
    assert_eq!(handoff.state(), &next);
    assert_eq!(handoff.trace(), &trace);
    assert_eq!(handoff.request().operation(), operation);
    handoff.require_context(&engine.planning_context()).unwrap();
    handoff.require_prepared(&prepared).unwrap();
    handoff.require_material().unwrap();
    assert_eq!(prepared.len(), 2);
    assert_eq!(next.file_containers[&requests[3].target.file], unselected);
    let source = Arc::clone(&engine.state);
    engine.state = Arc::new(source.as_ref().clone());
    assert!(matches!(handoff.require_context(&engine.planning_context()),
        Err(crate::Error::FileDraft(DraftError::SourceChanged))));
    engine.state = source;
    handoff.require_context(&engine.planning_context()).unwrap();
    // Moving the plan did not publish or clear any row, and a replacement
    // source remains rejected even when its actual bytes are identical.
    fs::rename(&path, &parked).unwrap();
    fs::copy(&parked, &path).unwrap();
    assert!(handoff.require_material().is_err());
    assert!(prepared[1].source.open().is_err());
    assert_eq!(engine.state(), before.as_ref());
    assert!(!engine.state().operations.contains_key(&operation));
    fs::remove_file(&path).unwrap();
    fs::rename(&parked, &path).unwrap();
}

#[test]
fn fresh_pool_format_refuses_unmarked_system_and_disk_before_recovery_writes() {
    use crate::{Config, Engine, ErrorCode, Mode};
    use name_policy::Name;
    use std::fs;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let temp = tempfile::tempdir().unwrap();
    let system = temp.path().join("system");
    let disk = temp.path().join("disk");
    let replacement_system = temp.path().join("replacement-system");
    for root in [&system, &disk] {
        fs::create_dir(root).unwrap();
        fs::set_permissions(root, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let config =
        Config::independent_roots(Mode::Development, [(Name::parse("Primary").unwrap(), disk.clone())])
            .unwrap();
    drop(Engine::initialize(&system, config).unwrap());

    let config_path = system.join("config.json");
    let seal_path = disk.join(crate::persist::layout::DISK_IDENTITY.as_path());
    let unmark = |bytes: &[u8]| {
        let mut value: json_codec::Value = json_codec::from_slice(bytes).unwrap();
        let original = value.as_object_mut().unwrap().remove("pool_semantic_format");
        assert_eq!(original.unwrap(), "manual-directory-v1");
        json_codec::to_vec(&value).unwrap()
    };
    let config = fs::read(&config_path).unwrap();
    let seal = fs::read(&seal_path).unwrap();

    let without_config_marker = unmark(&config);
    fs::write(&config_path, &without_config_marker).unwrap();
    let inode = fs::metadata(&config_path).unwrap().ino();
    let error = Engine::open(&system).err().expect("unmarked SYSTEM must reject");
    assert_eq!(error.code(), ErrorCode::RecoveryRequired);
    assert_eq!(fs::metadata(&config_path).unwrap().ino(), inode);
    assert_eq!(fs::read(&config_path).unwrap(), without_config_marker);
    assert_eq!(fs::read(&seal_path).unwrap(), seal);
    fs::write(&config_path, config).unwrap();

    let without_disk_marker = unmark(&seal);
    fs::write(&seal_path, &without_disk_marker).unwrap();
    let inode = fs::metadata(&seal_path).unwrap().ino();
    let error = Engine::restore_from_disks(&replacement_system, &[disk])
        .err()
        .expect("unmarked disk must reject explicit recovery");
    assert_eq!(error.code(), ErrorCode::DiskIdentityMismatch);
    assert!(!replacement_system.exists());
    assert_eq!(fs::metadata(&seal_path).unwrap().ino(), inode);
    assert_eq!(fs::read(&seal_path).unwrap(), without_disk_marker);
    fs::write(&seal_path, seal).unwrap();
    drop(Engine::open(&system).unwrap());
}
