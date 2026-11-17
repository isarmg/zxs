use super::*;

#[test]
fn cache_encoder_rejects_before_accept_and_reopens_cleanly() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("state");
    let config = Config::independent_roots(
        storage_contract::Mode::Development,
        [(name_policy::Name::parse("Primary").unwrap(), temp.path().join("a"))],
    ).unwrap();
    let slot = config.disks[0].slot_id.clone();
    let mut engine = Engine::initialize(&root, config).unwrap();
    let repo = engine.create_disk_root(
        &slot, "alice", storage_contract::OperationId::generate().unwrap(),
    ).unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "alice")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "file.txt", b"bytes")
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: storage_contract::OperationId::generate().unwrap(),
            principal: "alice".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "save".into(),
            request_digest: None,
        })
        .unwrap();
    let mut unobserved = engine.state().clone();
    let file = *unobserved.file_containers.keys().next().unwrap();
    let mut record = unobserved.file_containers[&file].clone();
    let disk = *record.replicas.keys().next().unwrap();
    record.replicas.get_mut(&disk).unwrap().entity =
        storage_contract::file_container::ContainerEntity::new(0, 0);
    unobserved.file_containers.insert(file, record);
    assert_eq!(crate::engine::fresh_layout::containers::validate_state(&unobserved)
        .unwrap_err().code(), storage_contract::ErrorCode::StateInvalid);
    let before = canonical_json(engine.state()).unwrap();
    let file = engine.state().versions.values().next().unwrap().file_id.clone();
    // Use the actual metadata planner, including its permanent request, actor,
    // event and whole-State proof. Cloning an arbitrary old receipt can select
    // workspace.commit without its new operation principal and fail path-audit
    // admission before reaching the cache encoder.
    let note = storage_contract::Note::parse(
        "n".repeat(storage_contract::MAX_NOTE_BYTES),
    ).unwrap();
    let operation = storage_contract::OperationId::generate().unwrap();
    let plan = engine.plan_mutation_digest(
        "file.metadata",
        "alice",
        operation,
        crate::persist::file_metadata::note_input(&file, None, &note),
        None,
        |engine, next, _| {
            let request = crate::business::file_metadata::Request::Note {
                file: file.parse()?, version: None, note: &note,
            };
            let replacement = crate::business::file_metadata::plan(
                engine.state(), engine.current_namespaces.as_deref(), request,
            )?;
            replacement.apply(next);
            Ok(crate::persist::file_metadata::outcome(&replacement))
        },
    ).unwrap();
    let crate::engine::MutationPlan::Prepared { next, prepared, result } = plan else {
        panic!("fresh metadata operation must produce an unpublished plan");
    };
    assert!(prepared.is_empty());
    assert_eq!(next.file_notes.get(&file).map(String::as_str), Some(note.as_str()));
    assert_eq!(canonical_json(engine.state()).unwrap(), before);
    let next = *next;
    let logs: Vec<_> = engine
        .members()
        .iter()
        .map(|disk| std::fs::read(log_path(disk)).unwrap())
        .collect();
    let error = engine
        .planning_context()
        .prepare_publication_bounded(next.clone(), Vec::new(), result.clone(), MAX_METADATA, 128)
        .err()
        .unwrap();
    assert_eq!(error.code(), storage_contract::ErrorCode::ResourceLimit, "{error:?}");
    assert!(!engine.blocked);
    assert_eq!(canonical_json(engine.state()).unwrap(), before);
    assert_eq!(
        engine
            .members()
            .iter()
            .map(|disk| std::fs::read(log_path(disk)).unwrap())
            .collect::<Vec<_>>(),
        logs
    );
    drop(engine);
    let engine = Engine::open(&root).unwrap();
    assert_eq!(canonical_json(engine.state()).unwrap(), before);
    assert_eq!(
        engine.operation(result.operation_id).unwrap_err().code(),
        storage_contract::ErrorCode::OperationNotFound
    );
    drop(engine);
    let mut engine = Engine::open_for_recovery(&root).unwrap();
    assert_eq!(canonical_json(engine.state()).unwrap(), before);
    let detached = engine
        .planning_context()
        .prepare_publication_bounded(
            next.clone(), Vec::new(), result.clone(), MAX_METADATA, MAX_METADATA,
        ).unwrap();
    assert!(detached.candidate.versions.is_empty());
    // Equal contents/snapshot/sequence cannot replace the actual source
    // occurrence between the shared planner and the first decision write.
    let source = std::sync::Arc::clone(&engine.state);
    engine.state = std::sync::Arc::new((*source).clone());
    assert_eq!(engine.require_detached_publication(&detached).unwrap_err().code(),
        storage_contract::ErrorCode::PreparationStale);
    assert_eq!(engine.members().iter()
        .map(|disk| std::fs::read(log_path(disk)).unwrap()).collect::<Vec<_>>(), logs);
    engine.state = source;
    engine.require_detached_publication(&detached).unwrap();
    // FILE-only preparation must retain its container even with no BODY cursor.
    // Matching cache/body bytes on a new inode cannot cross first PREPARE.
    fn copy_tree(source: &std::path::Path, target: &std::path::Path) {
        std::fs::create_dir(target).unwrap();
        std::fs::set_permissions(target, std::fs::metadata(source).unwrap().permissions()).unwrap();
        for entry in std::fs::read_dir(source).unwrap() {
            let entry = entry.unwrap();
            let destination = target.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_tree(&entry.path(), &destination);
            } else {
                std::fs::copy(entry.path(), destination).unwrap();
            }
        }
    }
    let container = engine.config.disks[0].root.join("common-data/file.txt");
    let parked = temp.path().join("parked-metadata-container");
    std::fs::rename(&container, &parked).unwrap();
    copy_tree(&parked, &container);
    assert_eq!(std::fs::read(container.join(".zxs-file.sqlite")).unwrap(),
        std::fs::read(parked.join(".zxs-file.sqlite")).unwrap());
    assert_eq!(engine.decide_publication(detached).unwrap_err().code(),
        storage_contract::ErrorCode::PreparationStale);
    assert!(!engine.blocked);
    assert_eq!(canonical_json(engine.state()).unwrap(), before);
    assert_eq!(engine.members().iter()
        .map(|disk| std::fs::read(log_path(disk)).unwrap()).collect::<Vec<_>>(), logs);
    std::fs::remove_dir_all(&container).unwrap();
    std::fs::rename(&parked, &container).unwrap();
    let detached = engine.planning_context().prepare_publication_bounded(
        next, Vec::new(), result.clone(), MAX_METADATA, MAX_METADATA,
    ).unwrap();
    engine.inject_fault_once("after_accept_1");
    assert_eq!(
        engine.decide_publication(detached).unwrap_err().code(),
        storage_contract::ErrorCode::InDoubt
    );
    drop(engine);
    let engine = Engine::open(&root).unwrap();
    assert_eq!(
        engine.operation(result.operation_id).unwrap().operation_id,
        result.operation_id
    );
}

#[test]
fn payload_cache_rejection_preserves_names_and_retries_installed_payloads() {
    use std::{os::unix::fs::MetadataExt, path::PathBuf};

    // Follow the admitted source-area and container, not the retired
    // disk-global version locator. This observes the same roots as publication.
    fn paths(engine: &Engine, version: &FileVersion) -> Vec<(PathBuf, PathBuf, PathBuf)> {
        engine.config.disks.iter().map(|disk| {
            let scope = crate::engine::fresh_layout::containers::observe_replica(
                &engine.config, &engine.disk_resources, engine.state(),
                version.file_id.parse().unwrap(), disk,
            ).unwrap();
            let material = crate::disk::VersionMaterial::current(&scope, version).unwrap();
            (
                material.directory().unwrap(),
                material.body_path().unwrap(),
                scope.container().unwrap().path().to_owned(),
            )
        }).collect()
    }

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("state");
    let config = Config::development([
        (name_policy::Name::parse("Primary").unwrap(), temp.path().join("a")),
        (name_policy::Name::parse("Backup").unwrap(), temp.path().join("b")),
        (name_policy::Name::parse("Archive").unwrap(), temp.path().join("c")),
    ]).unwrap();
    let slot = config.disks[0].slot_id.clone();
    let mut engine = Engine::initialize(&root, config).unwrap();
    // Only the actual genesis-root publication binds the user namespace to
    // common-data. A generic logical repository is not a physical RootOrigin.
    let repo = engine.create_disk_root(
        &slot, "alice", storage_contract::OperationId::generate().unwrap(),
    ).unwrap();
    let policy = new_id("policy").unwrap();
    engine.update_protection_policy(crate::ProtectionPolicyUpdate {
        policy_id: &policy, repository_id: &repo.repository_id, path_prefix: "",
        target_copies: 3, expected_generation: 0, principal: "alice",
        operation_id: storage_contract::OperationId::generate().unwrap(),
    }).unwrap();
    let workspace = engine.create_workspace(&repo.main_branch_id, "alice").unwrap();
    engine.stage_bytes(&workspace.workspace_id, "file.txt", b"published bytes").unwrap();
    engine.commit_workspace(CommitRequest {
        workspace_id: workspace.workspace_id,
        operation_id: storage_contract::OperationId::generate().unwrap(),
        principal: "alice".into(), expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token, message: "save".into(), request_digest: None,
    }).unwrap();
    let old_entry = engine.list_tree(&repo.main_branch_id).unwrap().remove(0);
    let old_version = engine.state.versions[&old_entry.version_id].clone();
    let original_paths = paths(&engine, &old_version).into_iter().map(|(dir, body, container)| {
        let cache = std::fs::read(container.join("container.json")).unwrap();
        let database = std::fs::read(container.join(".zxs-file.sqlite")).unwrap();
        let identity = std::fs::metadata(&container).unwrap();
        (dir, body, container, cache, database, (identity.dev(), identity.ino()))
    }).collect::<Vec<_>>();
    assert_eq!(original_paths.len(), 3);
    let before = canonical_json(engine.state()).unwrap();
    let original_snapshot = engine.snapshot.clone();
    let original_slot = engine.next_slot;
    let logs = engine.members().iter()
        .map(|disk| std::fs::read(log_path(disk)).unwrap()).collect::<Vec<_>>();

    // Build a real second content save, including its exact permanent receipt,
    // actor, path audit and file-branch/node changes. Whole-container rename has
    // a separate, still-unimplemented physical transition; it is not this budget test.
    let workspace = engine.create_workspace(&repo.main_branch_id, "alice").unwrap();
    engine.stage_bytes(&workspace.workspace_id, "file.txt", b"replacement bytes").unwrap();
    let request = CommitRequest {
        workspace_id: workspace.workspace_id,
        operation_id: storage_contract::OperationId::generate().unwrap(),
        principal: "alice".into(), expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token, message: "replace contents".into(),
        request_digest: None,
    };
    let plan = engine.plan_workspace_commit_inner(request.clone(), None).unwrap();
    let planned_workspace = plan.workspace;
    let crate::engine::MutationPlan::Prepared { next, prepared: pending, result } = plan.mutation
    else { panic!("new workspace save must produce an unpublished plan"); };
    let next = *next;
    assert_eq!(pending.len(), 1);
    let version = pending[0].version.clone();
    assert_eq!(version.file_id, old_entry.file_id);
    assert_eq!(version.source_version_ids, vec![old_entry.version_id.clone()]);
    assert_eq!(pending[0].placement.route_slot_ids.len(), 3);
    let new_paths = paths(&engine, &version);
    let copy_pending = || pending.iter().map(|payload| Prepared {
        version: payload.version.clone(), placement: payload.placement.clone(),
        source: payload.source.clone(),
    }).collect::<Vec<_>>();
    let retry_pending = copy_pending();

    // Exact metadata admission precedes even the first new payload preparation.
    let error = engine.planning_context().prepare_publication_bounded(
        next.clone(), copy_pending(), result.clone(), 128, MAX_METADATA,
    ).err().unwrap();
    assert_eq!(error.code(), storage_contract::ErrorCode::ResourceLimit, "{error:?}");
    assert!(!engine.blocked);
    assert_eq!(canonical_json(engine.state()).unwrap(), before);
    assert_eq!(engine.snapshot, original_snapshot);
    assert_eq!(engine.next_slot, original_slot);
    for (directory, body, _) in &new_paths {
        assert!(!directory.exists());
        assert!(!body.exists());
    }
    for (_, body, container, cache, database, identity) in &original_paths {
        assert_eq!(std::fs::read(body).unwrap(), b"published bytes");
        assert_eq!(std::fs::read(container.join("container.json")).unwrap(), *cache);
        assert_eq!(std::fs::read(container.join(".zxs-file.sqlite")).unwrap(), *database);
        let current = std::fs::metadata(container).unwrap();
        assert_eq!((current.dev(), current.ino()), *identity);
    }
    assert_eq!(engine.members().iter()
        .map(|disk| std::fs::read(log_path(disk)).unwrap()).collect::<Vec<_>>(), logs);

    let error = engine.planning_context().prepare_publication_bounded(
        next.clone(), pending, result.clone(), MAX_METADATA, 128,
    ).err().unwrap();
    assert_eq!(error.code(), storage_contract::ErrorCode::ResourceLimit, "{error:?}");
    assert!(!engine.blocked);
    assert_eq!(canonical_json(engine.state()).unwrap(), before);
    assert_eq!(engine.snapshot, original_snapshot);
    assert_eq!(engine.next_slot, original_slot);
    assert_eq!(engine.list_tree(&repo.main_branch_id).unwrap(), vec![old_entry.clone()]);
    assert_eq!(engine.members().iter()
        .map(|disk| std::fs::read(log_path(disk)).unwrap()).collect::<Vec<_>>(), logs);
    assert_eq!(paths(&engine, &old_version), original_paths.iter()
        .map(|(dir, body, container, ..)| (dir.clone(), body.clone(), container.clone()))
        .collect::<Vec<_>>());
    for (_, body, container, cache, database, identity) in &original_paths {
        assert_eq!(std::fs::read(body).unwrap(), b"published bytes");
        assert_eq!(std::fs::read(container.join("container.json")).unwrap(), *cache);
        assert_eq!(std::fs::read(container.join(".zxs-file.sqlite")).unwrap(), *database);
        let current = std::fs::metadata(container).unwrap();
        assert_eq!((current.dev(), current.ino()), *identity);
    }
    let installed = engine.config.disks.iter().zip(&new_paths).map(|(disk, (dir, body, _))| {
        // BODY installation happened, but no local decision proof or namespace
        // change did. Verify the actual source-scoped descriptor and BODY FD.
        let scope = crate::engine::fresh_layout::containers::observe_replica(
            &engine.config, &engine.disk_resources, engine.state(),
            version.file_id.parse().unwrap(), disk,
        ).unwrap();
        let material = crate::disk::VersionMaterial::current(&scope, &version).unwrap();
        let verified = material.verify_payload().unwrap();
        drop(verified);
        assert_eq!(std::fs::read(body).unwrap(), b"replacement bytes");
        assert!(!dir.join(crate::persist::layout::VERSION_COMMIT_PROOF.as_path()).exists());
        let identity = std::fs::metadata(body).unwrap();
        (identity.dev(), identity.ino())
    }).collect::<Vec<_>>();
    drop(engine);
    let mut engine = Engine::open(&root).unwrap();
    assert_eq!(canonical_json(engine.state()).unwrap(), before);
    assert_eq!(engine.operation(result.operation_id).unwrap_err().code(),
        storage_contract::ErrorCode::OperationNotFound);
    let expected_state = canonical_json(&next).unwrap();
    let expected_snapshot = prepare_snapshot(&next, MAX_METADATA).unwrap().hash;
    let expected_versions = canonical_json(&retry_pending.iter().map(|payload| VersionEvidence {
        version: payload.version.clone(), placement: payload.placement.clone(),
    }).collect::<Vec<_>>()).unwrap();
    let detached = engine.planning_context().prepare_publication_bounded(
        next, retry_pending, result.clone(), MAX_METADATA, MAX_METADATA,
    ).unwrap();
    assert_eq!(canonical_json(&detached.next).unwrap(), expected_state);
    assert_eq!(detached.candidate.snapshot_hash, expected_snapshot);
    assert_eq!(canonical_json(&detached.candidate.versions).unwrap(), expected_versions);
    assert_eq!(detached.candidate.operation_id, result.operation_id);
    engine.write_workspace_commit_request(&planned_workspace, &request, None).unwrap();
    let published = engine.decide_publication(detached).unwrap();
    engine.complete_workspace_commit(planned_workspace, published.clone()).unwrap();
    let saved = engine.list_tree(&repo.main_branch_id).unwrap().remove(0);
    assert_eq!(saved.path.as_str(), "file.txt");
    assert_eq!(saved.file_id, old_entry.file_id);
    assert_eq!(saved.version_id, version.version_id);
    for ((_, body, container), identity) in paths(&engine, &version).iter().zip(&installed) {
        assert_eq!(container.file_name().unwrap(), "file.txt");
        assert_eq!(std::fs::read(body).unwrap(), b"replacement bytes");
        let current = std::fs::metadata(body).unwrap();
        assert_eq!((current.dev(), current.ino()), *identity);
    }
    for (_, body, container, ..) in &original_paths {
        assert_eq!(std::fs::read(body).unwrap(), b"published bytes");
        assert!(!container.with_file_name("renamed.txt").exists());
    }
    drop(engine);
    let mut engine = Engine::open(&root).unwrap();
    assert_eq!(engine.operation(result.operation_id).unwrap(), published);
    assert_eq!(engine.list_tree(&repo.main_branch_id).unwrap()[0].version_id, version.version_id);
    let completed = canonical_json(engine.state()).unwrap();
    let snapshot = engine.snapshot.clone();
    let slot = engine.next_slot;
    assert_eq!(engine.commit_workspace(request).unwrap(), published);
    assert_eq!(canonical_json(engine.state()).unwrap(), completed);
    assert_eq!(engine.snapshot, snapshot);
    assert_eq!(engine.next_slot, slot);
}


#[test]
fn complete_physical_requirements_accumulate_on_the_same_admitted_disk() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("state");
    let mut config = Config::independent_roots(
        storage_contract::Mode::Development,
        [(name_policy::Name::parse("Primary").unwrap(), temp.path().join("a"))],
    ).unwrap();
    config.disks[0].capacity_bytes = Some(128 * 1024);
    let slot = config.disks[0].slot_id.clone();
    let mut engine = Engine::initialize(&root, config).unwrap();
    engine.create_disk_root(&slot, "alice", storage_contract::OperationId::generate().unwrap())
        .unwrap();
    let before = canonical_json(engine.state()).unwrap();
    let logs = engine.members().iter()
        .map(|disk| std::fs::read(log_path(disk)).unwrap()).collect::<Vec<_>>();
    let context = engine.planning_context();
    let disk = &context.config.disks[0];
    let mut first = PhysicalRequirements::new(&context);
    first.add(disk, 80 * 1024).unwrap();
    first.check().unwrap();
    let mut second = PhysicalRequirements::new(&context);
    second.add(disk, 80 * 1024).unwrap();
    second.check().unwrap();
    first.add(disk, 80 * 1024).unwrap();
    assert!(matches!(first.check(),
        Err(Error::FreshLayout(super::super::fresh_layout::FreshLayoutError::PhysicalCapacity {
            ..
        }))));
    let mut overflow = PhysicalRequirements::new(&context);
    overflow.add(disk, u64::MAX).unwrap();
    assert!(matches!(overflow.add(disk, 1),
        Err(Error::FreshLayout(
            super::super::fresh_layout::FreshLayoutError::PhysicalBytesOverflow,
        ))));
    assert_eq!(canonical_json(engine.state()).unwrap(), before);
    assert_eq!(engine.members().iter()
        .map(|disk| std::fs::read(log_path(disk)).unwrap()).collect::<Vec<_>>(), logs);
    // These are the same live free-space/configured-limit gates as ordinary
    // publication. This does not prove cross-session quota or census coverage.
}
