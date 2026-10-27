use super::*;
use std::fs;

fn fixture() -> (tempfile::TempDir, Engine, Workspace) {
    let temp = tempfile::tempdir().unwrap();
    let config = Config::development([
        temp.path().join("p"),
        temp.path().join("b"),
        temp.path().join("t"),
    ]);
    let mut engine = Engine::initialize(&temp.path().join("state"), config).unwrap();
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("reads").unwrap(),
            "P01",
            3,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "body", b"original")
        .unwrap();
    (temp, engine, workspace)
}

#[test]
fn verified_replica_holds_checked_inode_and_rewinds_before_returning() {
    let (_temp, mut engine, workspace) = fixture();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: storage_contract::OperationId::generate(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "save".into(),
            request_digest: None,
        })
        .unwrap();
    let version_id = engine.list_tree(&workspace.branch_id).unwrap()[0]
        .version_id
        .clone();
    let (mut held, version, directory) = engine.verified_replica(&version_id).unwrap();
    let path = crate::container_storage::content_path(&directory, &version).unwrap();
    let replacement = path.with_extension("replacement");
    fs::write(&replacement, b"modified").unwrap();
    fs::rename(replacement, &path).unwrap();
    let mut bytes = Vec::new();
    held.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"original");
    // A new open must reject the damaged first replica and return a healthy copy.
    bytes.clear();
    engine
        .open_content(&version_id)
        .unwrap()
        .file
        .read_to_end(&mut bytes)
        .unwrap();
    assert_eq!(bytes, b"original");
    for disk in engine.members() {
        fs::remove_file(disk.root.join("disk.json")).unwrap();
    }
    for error in [
        engine.open_content(&version_id).err().unwrap(),
        engine
            .prepare_content(&version_id)
            .unwrap()
            .open()
            .err()
            .unwrap(),
    ] {
        assert_eq!(
            error.code(),
            storage_contract::ErrorCode::ContentUnavailable
        );
        assert!(std::error::Error::source(&error).is_some());
    }
}

#[test]
fn workspace_read_rejects_length_mismatch_even_when_hash_matches() {
    let (_temp, engine, workspace) = fixture();
    let staged = engine.workspace(&workspace.workspace_id).unwrap();
    let item = staged.files.keys().next().unwrap();
    for size in [7, 9] {
        let mut plan = engine
            .prepare_workspace_content(&workspace.workspace_id, item, "admin")
            .unwrap();
        plan.0.size = size;
        assert_eq!(
            plan.open().err().unwrap().code(),
            storage_contract::ErrorCode::ChecksumMismatch
        );
        assert!(engine.reads.lock().unwrap().is_empty());
    }
}

#[test]
fn partial_write_does_not_authenticate_corrupted_staging_bytes() {
    let (_temp, mut engine, workspace) = fixture();
    let before = engine.workspace(&workspace.workspace_id).unwrap();
    let (item, staged) = before.files.iter().next().unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&staged.source_path, fs::Permissions::from_mode(0o600)).unwrap();
    for damaged in [&b"tampered"[..], &b"short"[..], &b"original-extra"[..]] {
        fs::write(&staged.source_path, damaged).unwrap();
        assert_eq!(
            engine
                .stage_range(&workspace.workspace_id, item, "body", 7, b"!", None)
                .unwrap_err()
                .code(),
            storage_contract::ErrorCode::ChecksumMismatch
        );
        assert_eq!(
            canonical_json(&engine.workspace(&workspace.workspace_id).unwrap()).unwrap(),
            canonical_json(&before).unwrap()
        );
        assert_eq!(fs::read(&staged.source_path).unwrap(), damaged);
        assert!(engine.reads.lock().unwrap().is_empty());
    }
    fs::write(&staged.source_path, b"original").unwrap();
    let updated = engine
        .stage_range(&workspace.workspace_id, item, "body", 7, b"!", None)
        .unwrap();
    assert_eq!(fs::read(updated.source_path).unwrap(), b"origina!");
}

#[test]
fn partial_write_from_published_replica_keeps_verified_untouched_bytes() {
    let (_temp, mut engine, workspace) = fixture();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: storage_contract::OperationId::generate(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "save".into(),
            request_digest: None,
        })
        .unwrap();
    let next = engine
        .create_workspace(&workspace.branch_id, "admin")
        .unwrap();
    let staged = engine
        .stage_range(&next.workspace_id, &new_id("entry"), "body", 0, b"O", None)
        .unwrap();
    assert_eq!(fs::read(staged.source_path).unwrap(), b"Original");
    assert!(engine.reads.lock().unwrap().is_empty());
}

#[test]
fn copy_build_rejects_replacement_after_replica_verification_before_candidate_mutation() {
    let (_temp, mut engine, workspace) = fixture();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: storage_contract::OperationId::generate(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "save".into(),
            request_digest: None,
        })
        .unwrap();
    let id = engine.list_tree(&workspace.branch_id).unwrap()[0]
        .version_id
        .clone();
    let (source, original) = engine.valid_replica(&id).unwrap();
    let mut candidate = engine.state().clone();
    let before = canonical_json(&candidate).unwrap();
    let mut pending = Vec::new();
    for replacement in [&b"modified"[..], &b"short"[..]] {
        let substitute = source.with_extension("replacement");
        fs::write(&substitute, replacement).unwrap();
        fs::rename(&substitute, &source).unwrap();
        let error = engine
            .build_copied_version(
                &mut candidate,
                &mut pending,
                crate::business::VersionBuild {
                    history: &original.history_tree_id,
                    file_id: &new_id("file"),
                    path: &name_policy::PortablePath::parse("copy").unwrap(),
                    source: &source,
                    primary: &original.creation_primary_slot_id,
                    copies: 3,
                    source_ids: Vec::new(),
                    checkpoint: true,
                },
                &original,
            )
            .unwrap_err();
        assert_eq!(error.code(), storage_contract::ErrorCode::SourceChanged);
        assert_eq!(canonical_json(&candidate).unwrap(), before);
        assert!(pending.is_empty());
    }
}

#[test]
fn read_leases_release_once_across_plan_moves_and_engine_drop() {
    let (_temp, mut engine, workspace) = fixture();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: storage_contract::OperationId::generate(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "save".into(),
            request_digest: None,
        })
        .unwrap();
    let id = engine.list_tree(&workspace.branch_id).unwrap()[0]
        .version_id
        .clone();
    let reads = Arc::clone(&engine.reads);
    let full = engine.open_content(&id).unwrap();
    let plan = engine.prepare_content(&id).unwrap();
    assert_eq!(reads.lock().unwrap()[&id], 2);
    let range = plan.open().unwrap();
    assert_eq!(reads.lock().unwrap()[&id], 2);
    let checksums = engine.prepare_content(&id).unwrap();
    assert_eq!(reads.lock().unwrap()[&id], 3);
    assert!(!checksums.checksums().unwrap().is_empty());
    assert_eq!(reads.lock().unwrap()[&id], 2);
    drop(engine);
    drop(full);
    assert_eq!(reads.lock().unwrap()[&id], 1);
    drop(range);
    assert!(reads.lock().unwrap().is_empty());
}

#[test]
fn workspace_read_leases_release_cancelled_and_opened_plans_independently() {
    let (_temp, engine, workspace) = fixture();
    let staged = engine.workspace(&workspace.workspace_id).unwrap();
    let item = staged.files.keys().next().unwrap();
    let plan = engine
        .prepare_workspace_content(&workspace.workspace_id, item, "admin")
        .unwrap();
    let cancelled = engine
        .prepare_workspace_content(&workspace.workspace_id, item, "admin")
        .unwrap();
    let key = plan.0.lease.version.clone();
    assert_eq!(engine.reads.lock().unwrap()[&key], 2);
    drop(cancelled);
    let opened = plan.open().unwrap();
    assert_eq!(engine.reads.lock().unwrap()[&key], 1);
    drop(opened);
    assert!(engine.reads.lock().unwrap().is_empty());
    assert_eq!(
        engine
            .prepare_workspace_content(&workspace.workspace_id, item, "other")
            .err()
            .unwrap()
            .code(),
        storage_contract::ErrorCode::PermissionDenied
    );
    assert!(engine.reads.lock().unwrap().is_empty());
}

#[test]
#[should_panic(expected = "invariant: every read lease has a registered count")]
fn read_lease_drop_exposes_missing_registration() {
    let (_temp, engine, workspace) = fixture();
    let staged = engine.workspace(&workspace.workspace_id).unwrap();
    let item = staged.files.keys().next().unwrap();
    let plan = engine
        .prepare_workspace_content(&workspace.workspace_id, item, "admin")
        .unwrap();
    engine.reads.lock().unwrap().remove(&plan.0.lease.version);
    drop(plan);
}

#[test]
#[should_panic(expected = "invariant: a live read lease has a positive count")]
fn read_lease_drop_exposes_zero_registration() {
    let (_temp, engine, workspace) = fixture();
    let staged = engine.workspace(&workspace.workspace_id).unwrap();
    let item = staged.files.keys().next().unwrap();
    let plan = engine
        .prepare_workspace_content(&workspace.workspace_id, item, "admin")
        .unwrap();
    *engine
        .reads
        .lock()
        .unwrap()
        .get_mut(&plan.0.lease.version)
        .unwrap() = 0;
    drop(plan);
}

#[test]
#[should_panic(expected = "invariant: every placement references an existing version")]
fn accounting_exposes_dangling_internal_placement() {
    let (_temp, mut engine, workspace) = fixture();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: storage_contract::OperationId::generate(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: String::new(),
            request_digest: None,
        })
        .unwrap();
    let id = engine.state.versions.keys().next().unwrap().clone();
    std::sync::Arc::make_mut(&mut engine.state)
        .versions
        .remove(&id);
    engine.used_bytes("P01");
}

fn commit_fixture() -> (tempfile::TempDir, Engine, FileVersion) {
    let (temp, mut engine, workspace) = fixture();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: storage_contract::OperationId::generate(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "save".into(),
            request_digest: None,
        })
        .unwrap();
    let id = &engine.list_tree(&workspace.branch_id).unwrap()[0].version_id;
    let version = engine.state.versions[id].clone();
    (temp, engine, version)
}

fn replace_replica(engine: &Engine, version: &FileVersion, slot: &str, bytes: &[u8]) {
    let directory = version_dir(
        engine.disk(slot).unwrap(),
        &version.history_tree_id,
        &version.version_id,
    );
    let path = crate::container_storage::content_path(&directory, version).unwrap();
    let replacement = path.with_extension("replacement");
    fs::write(&replacement, bytes).unwrap();
    fs::rename(replacement, path).unwrap();
}

#[test]
fn export_verifies_while_copying_and_falls_back_from_bad_replica() {
    for damaged in [
        b"tampered".as_slice(),
        b"short".as_slice(),
        b"original-extra",
    ] {
        let (temp, engine, version) = commit_fixture();
        let placement = &engine.state.placements[&version.version_id];
        replace_replica(&engine, &version, &placement.route_slot_ids[0], damaged);
        let destination = temp.path().join("exports/body");
        engine
            .export_version(&version.version_id, &destination)
            .unwrap();
        assert_eq!(fs::read(&destination).unwrap(), b"original");
        assert_eq!(
            fs::read_dir(destination.parent().unwrap()).unwrap().count(),
            1
        );
    }
}

#[test]
fn export_rejects_bad_manifest_and_seal_before_copying_replica() {
    let (temp, engine, version) = commit_fixture();
    let placement = &engine.state.placements[&version.version_id];
    let first = engine.disk(&placement.route_slot_ids[0]).unwrap();
    let first_dir = version_dir(first, &version.history_tree_id, &version.version_id);
    let checks = first_dir.join("checksums.bin");
    fs::remove_file(&checks).unwrap();
    fs::write(checks, b"damaged").unwrap();
    let second = engine.disk(&placement.route_slot_ids[1]).unwrap();
    fs::remove_file(second.root.join("disk.json")).unwrap();
    let destination = temp.path().join("exports/body");
    engine
        .export_version(&version.version_id, &destination)
        .unwrap();
    assert_eq!(fs::read(&destination).unwrap(), b"original");
    assert_eq!(
        fs::read_dir(destination.parent().unwrap()).unwrap().count(),
        1
    );
}

#[test]
fn export_all_corrupt_replicas_leave_no_destination_or_temporary_copy() {
    let (temp, engine, version) = commit_fixture();
    for slot in &engine.state.placements[&version.version_id].route_slot_ids {
        replace_replica(&engine, &version, slot, b"tampered");
    }
    let destination = temp.path().join("exports/body");
    let error = engine
        .export_version(&version.version_id, &destination)
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::ContentUnavailable);
    assert!(std::error::Error::source(&error).is_some());
    assert!(!destination.exists());
    assert_eq!(
        fs::read_dir(destination.parent().unwrap()).unwrap().count(),
        0
    );
}

#[test]
fn export_destination_failure_keeps_its_error_and_existing_bytes() {
    let (temp, engine, version) = commit_fixture();
    let destination = temp.path().join("export");
    fs::write(&destination, b"existing destination").unwrap();
    let error = engine
        .export_version(&version.version_id, &destination)
        .unwrap_err();
    assert!(matches!(
        error,
        Error::Contract(storage_contract::Error::Io { ref source, .. })
            if source.kind() == std::io::ErrorKind::AlreadyExists
    ));
    assert_eq!(fs::read(destination).unwrap(), b"existing destination");
    assert!(!fs::read_dir(temp.path()).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_str()
            .unwrap()
            .starts_with(".zxs-full-")
    }));
}

#[test]
fn prepare_rejects_source_growth_after_planning_without_installing_payload() {
    let (_temp, engine, workspace) = fixture();
    let plan = engine
        .plan_workspace_commit_inner(
            CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: storage_contract::OperationId::generate(),
                principal: "admin".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: String::new(),
                request_digest: None,
            },
            None,
        )
        .unwrap();
    let crate::engine::MutationPlan::Prepared { prepared, .. } = plan.mutation else {
        panic!("new workspace content has a payload plan");
    };
    let payload = &prepared[0];
    let replacement = payload.source.with_extension("replacement");
    fs::write(&replacement, b"original but longer").unwrap();
    fs::rename(replacement, &payload.source).unwrap();
    assert_eq!(
        engine.prepare_payload(payload).unwrap_err().code(),
        ErrorCode::SourceChanged
    );
    for slot in &payload.placement.route_slot_ids {
        let directory = version_dir(
            engine.disk(slot).unwrap(),
            &payload.version.history_tree_id,
            &payload.version.version_id,
        );
        assert!(!directory.exists());
    }
}

#[test]
fn content_plan_keeps_admitted_roots_and_limits_after_rebinding_and_engine_drop() {
    let (temp, mut engine, version) = commit_fixture();
    let baseline = engine.effective_workspace_limits();
    let plan = engine.prepare_content(&version.version_id).unwrap();
    assert!(std::ptr::eq(&*plan.config, &*engine.config));
    let original_root = plan.config.disks[0].root.clone();
    let mut raised = baseline;
    raised.max_retained_workspaces += 1;
    engine.config = engine
        .config
        .clone()
        .with_disk_roots(
            (0..3)
                .map(|i| temp.path().join(format!("unused-{i}")))
                .collect(),
        )
        .unwrap()
        .with_workspace_limits(Some(raised))
        .unwrap();
    assert_eq!(engine.effective_workspace_limits(), raised);
    drop(engine);
    assert_eq!(plan.config.disks[0].root, original_root);
    assert_eq!(plan.config.effective_workspace_limits(), baseline);
    let mut bytes = Vec::new();
    plan.open().unwrap().file.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"original");
}

#[test]
fn detached_content_plan_checks_current_member_identity_and_preserves_cause() {
    let (temp, engine, version) = commit_fixture();
    let plan = engine.prepare_content(&version.version_id).unwrap();
    let roots: Vec<_> = engine
        .config
        .disks
        .iter()
        .map(|disk| disk.root.clone())
        .collect();
    drop(engine);
    for (index, root) in roots.iter().enumerate() {
        let removed = temp.path().join(format!("removed-{index}"));
        fs::rename(root, &removed).unwrap();
        fs::create_dir(root).unwrap();
        fs::copy(removed.join("disk.json"), root.join("disk.json")).unwrap();
    }
    let error = plan.open().err().unwrap();
    assert_eq!(error.code(), ErrorCode::ContentUnavailable);
    assert!(std::error::Error::source(&error).is_some());
}
