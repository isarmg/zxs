use super::*;
use std::fs;

fn fixture() -> (tempfile::TempDir, Engine, Workspace) {
    let temp = tempfile::tempdir().unwrap();
    let config = Config::development([
        temp.path().join("p"),
        temp.path().join("b"),
        temp.path().join("t"),
    ])
    .unwrap();
    let mut engine = Engine::initialize(&temp.path().join("state"), config).unwrap();
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("reads").unwrap(),
            "P01",
            3,
            "admin",
            storage_contract::OperationId::generate().unwrap(),
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

fn committed_fixture() -> (tempfile::TempDir, Engine, String) {
    let (temp, mut engine, workspace) = fixture();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: storage_contract::OperationId::generate().unwrap(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "held disk".into(),
            request_digest: None,
        })
        .unwrap();
    let version = engine.list_tree(&workspace.branch_id).unwrap()[0]
        .version_id
        .clone();
    (temp, engine, version)
}

#[test]
fn live_reads_and_detached_plan_keep_the_admitted_disks_after_root_replacement() {
    use std::os::unix::fs::PermissionsExt;
    let (_temp, engine, version) = committed_fixture();
    let plan = engine.prepare_content(&version).unwrap();
    for disk in &engine.config.disks {
        fs::rename(&disk.root, disk.root.with_extension("held")).unwrap();
        fs::create_dir(&disk.root).unwrap();
        fs::set_permissions(&disk.root, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(disk.root.join("replacement-only"), b"not an admitted disk").unwrap();
    }
    let mut bytes = Vec::new();
    engine
        .open_content(&version)
        .unwrap()
        .file
        .read_to_end(&mut bytes)
        .unwrap();
    assert_eq!(bytes, b"original");
    bytes.clear();
    plan.open().unwrap().file.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"original");
    assert!(engine.disk_status().is_err());
    assert!(
        engine
            .prepare_disk_observation()
            .sample()
            .iter()
            .all(|disk| !disk.status.online)
    );
    assert!(
        engine
            .config
            .disks
            .iter()
            .all(|disk| { fs::read_dir(&disk.root).unwrap().count() == 1 })
    );
}

#[test]
fn a_content_plan_does_not_retain_writer_locks() {
    let (temp, engine, version) = committed_fixture();
    let plan = engine.prepare_content(&version).unwrap();
    drop(engine);
    let reopened = Engine::open(&temp.path().join("state")).unwrap();
    let mut bytes = Vec::new();
    plan.open().unwrap().file.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"original");
    assert!(reopened.open_content(&version).is_ok());
}

#[test]
fn staging_and_detached_commit_use_the_admitted_disks_after_root_replacement() {
    use std::os::unix::fs::PermissionsExt;
    let (_temp, mut engine, workspace) = fixture();
    for disk in &engine.config.disks {
        fs::rename(&disk.root, disk.root.with_extension("held")).unwrap();
        fs::create_dir(&disk.root).unwrap();
        fs::set_permissions(&disk.root, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(disk.root.join("replacement-only"), b"not admitted").unwrap();
    }
    engine
        .stage_bytes(&workspace.workspace_id, "second", b"new content")
        .unwrap();
    let operation = storage_contract::OperationId::generate().unwrap();
    let prepared = engine
        .prepare_workspace_commit(
            CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: operation,
                principal: "admin".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "same held disks".into(),
                request_digest: None,
            },
            None,
        )
        .unwrap()
        .execute()
        .unwrap();
    engine.finish_workspace_commit(prepared).unwrap();
    assert!(engine.operation(operation).is_ok());
    let entries = engine.list_tree(&workspace.branch_id).unwrap();
    assert_eq!(entries.len(), 2);
    for entry in entries {
        let mut bytes = Vec::new();
        engine
            .open_content(&entry.version_id)
            .unwrap()
            .file
            .read_to_end(&mut bytes)
            .unwrap();
        assert!(bytes == b"original" || bytes == b"new content");
    }
    for disk in &engine.config.disks {
        assert_eq!(fs::read_dir(&disk.root).unwrap().count(), 1);
        assert!(
            crate::journal::scan_at(&engine.disk_resources.get(disk).unwrap().root, disk)
                .unwrap()
                .records
                .len()
                >= 8
        );
    }
}

#[test]
fn detached_upload_keeps_its_admitted_workspace_directory_after_root_replacement() {
    use std::os::unix::fs::PermissionsExt;
    let (temp, mut engine, workspace) = fixture();
    let source = temp.path().join("upload-source");
    fs::write(&source, b"detached content").unwrap();
    let item = storage_contract::new_id("entry").unwrap();
    let preparation = engine
        .prepare_upload(&workspace.workspace_id, &item, "detached", &source, 1)
        .unwrap();
    for disk in &engine.config.disks {
        fs::rename(&disk.root, disk.root.with_extension("held")).unwrap();
        fs::create_dir(&disk.root).unwrap();
        fs::set_permissions(&disk.root, fs::Permissions::from_mode(0o700)).unwrap();
    }
    engine
        .finish_upload(preparation.execute().unwrap())
        .unwrap();
    let mut bytes = Vec::new();
    engine
        .open_workspace_content(&workspace.workspace_id, &item, "admin")
        .unwrap()
        .file
        .read_to_end(&mut bytes)
        .unwrap();
    assert_eq!(bytes, b"detached content");
    assert!(
        engine
            .config
            .disks
            .iter()
            .all(|disk| fs::read_dir(&disk.root).unwrap().next().is_none())
    );
}

#[test]
fn caller_paths_and_staged_sources_keep_distinct_origins_after_root_replacement() {
    use std::os::unix::fs::PermissionsExt;
    let (_temp, mut engine, workspace) = fixture();
    let staged = engine
        .workspace(&workspace.workspace_id)
        .unwrap()
        .files
        .values()
        .next()
        .unwrap()
        .clone();
    let disk = engine.disk("P01").unwrap().clone();
    let input = disk.root.join("caller-input");
    fs::write(&input, b"old named input").unwrap();
    fs::rename(&disk.root, disk.root.with_extension("held")).unwrap();
    fs::create_dir(&disk.root).unwrap();
    fs::set_permissions(&disk.root, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(&input, b"new named input").unwrap();
    fs::create_dir_all(staged.source_path.parent().unwrap()).unwrap();
    fs::write(&staged.source_path, b"replacement staged bytes").unwrap();

    let staged_external = engine
        .stage_file(&workspace.workspace_id, "external-stage", &input)
        .unwrap();
    let item = new_id("entry").unwrap();
    let upload = engine
        .prepare_upload(&workspace.workspace_id, &item, "external-upload", &input, 1)
        .unwrap();
    // External admission retains its selected file, not a later named replacement.
    let later = input.with_extension("later");
    fs::write(&later, b"later named input").unwrap();
    fs::rename(later, &input).unwrap();
    engine.finish_upload(upload.execute().unwrap()).unwrap();
    for (item, expected) in [
        (staged.item_id.as_str(), &b"original"[..]),
        (staged_external.item_id.as_str(), &b"new named input"[..]),
        (item.as_str(), &b"new named input"[..]),
    ] {
        let mut bytes = Vec::new();
        engine
            .open_workspace_content(&workspace.workspace_id, item, "admin")
            .unwrap()
            .file
            .read_to_end(&mut bytes)
            .unwrap();
        assert_eq!(bytes, expected);
    }
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: OperationId::generate().unwrap(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "explicit source provenance".into(),
            request_digest: None,
        })
        .unwrap();
    for entry in engine.list_tree(&workspace.branch_id).unwrap() {
        let mut bytes = Vec::new();
        engine
            .open_content(&entry.version_id)
            .unwrap()
            .file
            .read_to_end(&mut bytes)
            .unwrap();
        assert_eq!(
            bytes,
            if entry.path() == "body" {
                &b"original"[..]
            } else {
                &b"new named input"[..]
            }
        );
    }
    assert!(!disk.root.join("disk.json").exists());
    assert_eq!(fs::read(&input).unwrap(), b"later named input");
    assert_eq!(
        fs::read(staged.source_path).unwrap(),
        b"replacement staged bytes"
    );
}

#[test]
fn held_reads_still_read_tombstones_and_corrupt_bytes_from_the_old_disk() {
    let (_temp, engine, version_id) = committed_fixture();
    let version = engine.state.versions.get(&version_id).unwrap();
    for disk in &engine.config.disks {
        let directory = version_dir(disk, &version.history_tree_id, &version_id);
        let body = crate::container_storage::content_path(&directory, version).unwrap();
        let moved = disk.root.with_extension("held");
        fs::rename(&disk.root, &moved).unwrap();
        let body = moved.join(body.strip_prefix(&disk.root).unwrap());
        let replacement = body.with_extension("damaged");
        fs::write(&replacement, b"modified").unwrap();
        fs::rename(replacement, &body).unwrap();
    }
    assert_eq!(
        engine.open_content(&version_id).err().unwrap().code(),
        ErrorCode::ContentUnavailable
    );
    assert_eq!(
        engine.prepare_content(&version_id).unwrap().health(3),
        storage_contract::CurrentHealth::Unavailable
    );
    for disk in &engine.config.disks {
        let directory = version_dir(disk, &version.history_tree_id, &version_id);
        let moved = disk.root.with_extension("held");
        fs::write(
            moved
                .join(directory.strip_prefix(&disk.root).unwrap())
                .join("purged.json"),
            b"present",
        )
        .unwrap();
    }
    let error = engine
        .prepare_content(&version_id)
        .unwrap()
        .open()
        .err()
        .unwrap();
    assert_eq!(error.code(), ErrorCode::ContentUnavailable);
    assert!(std::error::Error::source(&error).is_some());
}

#[test]
fn verified_replica_holds_checked_inode_and_rewinds_before_returning() {
    let (_temp, mut engine, workspace) = fixture();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: storage_contract::OperationId::generate().unwrap(),
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
        assert!(engine.reads.lock().unwrap().counts().unwrap().is_empty());
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
        assert!(engine.reads.lock().unwrap().counts().unwrap().is_empty());
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
            operation_id: storage_contract::OperationId::generate().unwrap(),
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
        .stage_range(
            &next.workspace_id,
            &new_id("entry").unwrap(),
            "body",
            0,
            b"O",
            None,
        )
        .unwrap();
    assert_eq!(fs::read(staged.source_path).unwrap(), b"Original");
    assert!(engine.reads.lock().unwrap().counts().unwrap().is_empty());
}

#[test]
fn copy_build_rejects_replacement_after_replica_verification_before_candidate_mutation() {
    let (_temp, mut engine, workspace) = fixture();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: storage_contract::OperationId::generate().unwrap(),
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
        let substitute = source.path().with_extension("replacement");
        fs::write(&substitute, replacement).unwrap();
        fs::rename(&substitute, source.path()).unwrap();
        let error = engine
            .build_copied_version(
                &mut candidate,
                &mut pending,
                crate::business::VersionBuild {
                    history: &original.history_tree_id,
                    file_id: &new_id("file").unwrap(),
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
            operation_id: storage_contract::OperationId::generate().unwrap(),
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
    assert_eq!(reads.lock().unwrap().counts().unwrap()[&id], 2);
    let range = plan.open().unwrap();
    assert_eq!(reads.lock().unwrap().counts().unwrap()[&id], 2);
    let checksums = engine.prepare_content(&id).unwrap();
    assert_eq!(reads.lock().unwrap().counts().unwrap()[&id], 3);
    assert!(!checksums.checksums().unwrap().is_empty());
    assert_eq!(reads.lock().unwrap().counts().unwrap()[&id], 2);
    drop(engine);
    drop(full);
    assert_eq!(reads.lock().unwrap().counts().unwrap()[&id], 1);
    drop(range);
    assert!(reads.lock().unwrap().counts().unwrap().is_empty());
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
    assert_eq!(engine.reads.lock().unwrap().counts().unwrap()[&key], 2);
    drop(cancelled);
    let opened = plan.open().unwrap();
    assert_eq!(engine.reads.lock().unwrap().counts().unwrap()[&key], 1);
    drop(opened);
    assert!(engine.reads.lock().unwrap().counts().unwrap().is_empty());
    assert_eq!(
        engine
            .prepare_workspace_content(&workspace.workspace_id, item, "other")
            .err()
            .unwrap()
            .code(),
        storage_contract::ErrorCode::PermissionDenied
    );
    assert!(engine.reads.lock().unwrap().counts().unwrap().is_empty());
}

#[test]
fn read_lease_drop_fences_missing_registration() {
    let (_temp, engine, workspace) = fixture();
    let staged = engine.workspace(&workspace.workspace_id).unwrap();
    let item = staged.files.keys().next().unwrap();
    let plan = engine
        .prepare_workspace_content(&workspace.workspace_id, item, "admin")
        .unwrap();
    engine
        .reads
        .lock()
        .unwrap()
        .counts
        .remove(&plan.0.lease.version);
    drop(plan);
    assert_eq!(
        engine.reads.lock().unwrap().counts().unwrap_err().code(),
        ErrorCode::RecoveryRequired
    );
    assert_eq!(
        engine
            .prepare_workspace_content(&workspace.workspace_id, item, "admin")
            .err()
            .unwrap()
            .code(),
        ErrorCode::RecoveryRequired
    );
}

#[test]
fn read_lease_drop_fences_zero_registration() {
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
        .counts
        .get_mut(&plan.0.lease.version)
        .unwrap() = 0;
    drop(plan);
    assert_eq!(
        engine.reads.lock().unwrap().counts().unwrap_err().code(),
        ErrorCode::RecoveryRequired
    );
    assert_eq!(
        engine
            .prepare_workspace_content(&workspace.workspace_id, item, "admin")
            .err()
            .unwrap()
            .code(),
        ErrorCode::RecoveryRequired
    );
}

#[test]
#[should_panic(expected = "invariant: every placement references an existing version")]
fn accounting_exposes_dangling_internal_placement() {
    let (_temp, mut engine, workspace) = fixture();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: storage_contract::OperationId::generate().unwrap(),
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
            operation_id: storage_contract::OperationId::generate().unwrap(),
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

#[test]
fn checksum_admission_rejects_unsupported_and_over_budget_descriptors_before_io() {
    let (_temp, _engine, mut version) = commit_fixture();
    for width in [0, 1, u32::MAX] {
        version.checksum_chunk_bytes = width;
        assert_eq!(
            checksum_manifest_length(&version).unwrap_err().code(),
            ErrorCode::ChecksumFormatInvalid
        );
    }
    version.checksum_chunk_bytes = 65536;
    version.logical_size = 1 << 40;
    assert_eq!(
        checksum_manifest_length(&version).unwrap(),
        MAX_CHECKSUM_MANIFEST
    );
    version.logical_size += 1;
    assert_eq!(
        checksum_manifest_length(&version).unwrap_err().code(),
        ErrorCode::ResourceLimit
    );
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
                operation_id: storage_contract::OperationId::generate().unwrap(),
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
    let replacement = payload.source.path().with_extension("replacement");
    fs::write(&replacement, b"original but longer").unwrap();
    fs::rename(replacement, payload.source.path()).unwrap();
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
fn detached_content_plan_keeps_its_members_when_their_names_are_replaced() {
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
    let mut bytes = Vec::new();
    plan.open().unwrap().file.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"original");
}

#[test]
fn version_health_fallback_observes_shared_membership_but_not_missing_local_bodies() {
    for shared_membership in [false, true] {
        let (_temp, mut engine, workspace) = fixture();
        engine
            .commit_workspace(CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: storage_contract::OperationId::generate().unwrap(),
                principal: "admin".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "health observation".into(),
                request_digest: None,
            })
            .unwrap();
        let version_id = engine.list_tree(&workspace.branch_id).unwrap()[0]
            .version_id
            .clone();
        let version = &engine.state.versions[&version_id];
        let disk = engine
            .disk(&engine.state.placements[&version_id].route_slot_ids[0])
            .unwrap();
        let path = if shared_membership {
            disk.root.join("metadata/pool-membership/config.json")
        } else {
            let directory = version_dir(disk, &version.history_tree_id, &version_id);
            crate::container_storage::content_path(&directory, version).unwrap()
        };
        let epoch = engine.metadata_integrity.capture();
        fs::remove_file(path).unwrap();
        assert_eq!(engine.version_health(&version_id).unwrap(), "DEGRADED");
        assert_eq!(
            epoch.matches(&engine.metadata_integrity),
            !shared_membership
        );
        assert!(!engine.blocked);
    }
}

#[test]
fn poisoned_read_lease_drop_during_unwind_preserves_registry() {
    let (_temp, engine, workspace) = fixture();
    let staged = engine.workspace(&workspace.workspace_id).unwrap();
    let item = staged.files.keys().next().unwrap();
    let plan = engine
        .prepare_workspace_content(&workspace.workspace_id, item, "admin")
        .unwrap();
    let key = plan.0.lease.version.clone();
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = engine.reads.lock().unwrap();
        panic!("poison");
    }));
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _plan = plan;
            panic!("outer unwind");
        }))
        .is_err()
    );
    assert!(engine.reads.is_poisoned());
    assert_eq!(
        engine.reads.lock().err().unwrap().into_inner().counts[&key],
        1
    );
}

#[test]
fn invalid_read_release_keeps_purge_fenced_after_the_owner_is_gone() {
    let (_temp, mut engine, version) = committed_fixture();
    let history = engine.state.versions[&version].history_tree_id.clone();
    let plan = engine.prepare_content(&version).unwrap();
    assert_eq!(
        engine
            .create_purge_plan(&history, vec![version.clone()], "admin")
            .unwrap_err()
            .code(),
        ErrorCode::ContentPinned
    );
    engine.reads.lock().unwrap().counts.remove(&version);
    drop(plan);
    assert_eq!(
        engine
            .create_purge_plan(&history, vec![version], "admin")
            .unwrap_err()
            .code(),
        ErrorCode::RecoveryRequired
    );
}

fn read_bindings(engine: &Engine) -> Vec<DiskIoBinding> {
    engine
        .config
        .disks
        .iter()
        .map(|disk| {
            engine
                .disk_resources
                .get(disk)
                .unwrap()
                .root
                .io_binding()
                .unwrap()
                .clone()
        })
        .collect()
}

#[test]
fn prepared_content_and_checksum_reads_cannot_open_after_all_disks_quiesce() {
    let (_temp, engine, version) = committed_fixture();
    let bindings = read_bindings(&engine);
    let baseline = bindings
        .iter()
        .map(|binding| binding.status().unwrap().ordinary)
        .collect::<Vec<_>>();
    let content = engine.prepare_content(&version).unwrap();
    let checksums = engine.prepare_content(&version).unwrap();
    let placement = engine.state.placements[&version].route_slot_ids.clone();
    for (binding, before) in bindings.iter().zip(baseline) {
        assert_eq!(
            binding.status().unwrap().ordinary,
            before,
            "plans acquire no I/O"
        );
        drop(binding.begin_quiesce().unwrap().unwrap());
        assert!(binding.status().unwrap().drained());
    }
    assert_eq!(
        content.open().err().unwrap().code(),
        ErrorCode::ContentUnavailable
    );
    assert_eq!(
        checksums.checksums().unwrap_err().code(),
        ErrorCode::ContentUnavailable
    );
    assert_eq!(
        engine.open_content(&version).err().unwrap().code(),
        ErrorCode::ContentUnavailable
    );
    assert_eq!(
        engine.read_checksums(&version).unwrap_err().code(),
        ErrorCode::ContentUnavailable
    );
    assert_eq!(engine.state.placements[&version].route_slot_ids, placement);
    assert_eq!(engine.config.disks.len(), bindings.len());
    for binding in &bindings {
        assert!(binding.status().unwrap().drained());
    }
}

#[test]
fn range_replica_resources_are_cancelled_per_disk_even_while_idle() {
    let (_temp, engine, version) = committed_fixture();
    let mut content = engine.prepare_content(&version).unwrap().open().unwrap();
    let mut clone = content.file.try_clone().unwrap();
    let primary = engine
        .disk_resources
        .get(
            engine
                .disk(&engine.state.placements[&version].route_slot_ids[0])
                .unwrap(),
        )
        .unwrap()
        .root
        .io_binding()
        .unwrap();
    assert!(primary.status().unwrap().ordinary > 0);
    drop(primary.begin_quiesce().unwrap().unwrap());
    // Neither the original HTTP/SMB owner nor its clone must be polled or
    // dropped to close idle descriptors. The control result remains truthful.
    assert!(primary.status().unwrap().drained());
    assert_eq!(
        content.file.check_running().unwrap_err().code(),
        ErrorCode::DiskUnavailable
    );
    assert_eq!(
        clone.read(&mut [0]).unwrap_err().kind(),
        std::io::ErrorKind::PermissionDenied
    );
    assert!(!content.fallback_files.is_empty());
    let mut bytes = Vec::new();
    content.fallback_files[0].read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"original");
    for binding in read_bindings(&engine) {
        drop(binding.begin_quiesce().unwrap());
        assert!(binding.status().unwrap().drained());
    }
    for file in &mut content.fallback_files {
        assert_eq!(
            file.read(&mut [0]).unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied
        );
    }
}

#[test]
fn detached_read_plan_skips_a_quiesced_replica_without_changing_placement() {
    let (_temp, engine, version) = committed_fixture();
    let content = engine.prepare_content(&version).unwrap();
    let checksums = engine.prepare_content(&version).unwrap();
    let placement = engine.state.placements[&version].route_slot_ids.clone();
    let primary = engine
        .disk_resources
        .get(engine.disk(&placement[0]).unwrap())
        .unwrap();
    let binding = primary.root.io_binding().unwrap();
    drop(binding.begin_quiesce().unwrap().unwrap());
    let mut bytes = Vec::new();
    content
        .open()
        .unwrap()
        .file
        .read_to_end(&mut bytes)
        .unwrap();
    assert_eq!(bytes, b"original");
    assert!(!checksums.checksums().unwrap().is_empty());
    assert!(binding.status().unwrap().drained());
    assert_eq!(engine.state.placements[&version].route_slot_ids, placement);
}

#[test]
fn workspace_read_plan_keeps_its_disk_gate_after_preparation() {
    let (_temp, engine, workspace) = fixture();
    let staged = engine.workspace(&workspace.workspace_id).unwrap();
    let item = staged.files.keys().next().unwrap();
    let plan = engine
        .prepare_workspace_content(&workspace.workspace_id, item, "admin")
        .unwrap();
    let bindings = read_bindings(&engine);
    let held = bindings
        .iter()
        .find(|binding| binding.status().unwrap().ordinary > 0)
        .unwrap();
    drop(held.begin_quiesce().unwrap().unwrap());
    assert!(held.status().unwrap().drained());
    let error = plan.open().err().unwrap();
    assert_eq!(error.code(), ErrorCode::ContentUnavailable);
}

#[test]
fn external_read_source_does_not_borrow_an_unrelated_disk_gate() {
    let (temp, engine, _workspace) = fixture();
    let path = temp.path().join("external-source");
    fs::write(&path, b"local bytes").unwrap();
    let mut file = ReadSource::external(path).open_leased().unwrap();
    for binding in read_bindings(&engine) {
        drop(binding.begin_quiesce().unwrap().unwrap());
        assert!(binding.status().unwrap().drained());
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"local bytes");
}

#[test]
fn reopening_the_engine_keeps_old_read_plans_in_the_same_disk_revocation_domain() {
    let (temp, engine, version) = committed_fixture();
    let plan = engine.prepare_content(&version).unwrap();
    let checksums = engine.prepare_content(&version).unwrap();
    let mut full = engine.open_content(&version).unwrap();
    let mut range = engine.prepare_content(&version).unwrap().open().unwrap();
    let old_bindings = read_bindings(&engine);
    drop(engine);
    let error = Engine::open(&temp.path().join("state"))
        .err()
        .expect("live reads retain writer ownership");
    assert_eq!(error.code(), ErrorCode::WriterAlreadyActive);
    // A failed reopen must not revoke authorized reads to obtain writer locks.
    let mut first = [0; 1];
    full.file.read_exact(&mut first).unwrap();
    assert_eq!(first, [b'o']);
    range.file.read_exact(&mut first).unwrap();
    assert_eq!(first, [b'o']);
    drop(full);
    drop(range);
    let reopened = Engine::open(&temp.path().join("state")).unwrap();
    for binding in read_bindings(&reopened) {
        drop(binding.begin_quiesce().unwrap().unwrap());
        assert!(binding.status().unwrap().drained());
    }
    for binding in old_bindings {
        assert!(binding.status().unwrap().quiescing);
        assert!(binding.status().unwrap().drained());
    }
    assert_eq!(
        plan.open().err().unwrap().code(),
        ErrorCode::ContentUnavailable
    );
    assert_eq!(
        checksums.checksums().unwrap_err().code(),
        ErrorCode::ContentUnavailable
    );
}

#[test]
fn an_engine_reopen_cannot_reactivate_a_quiescing_binding_held_by_old_readers() {
    let (temp, engine, version) = committed_fixture();
    let plan = engine.prepare_content(&version).unwrap();
    let bindings = read_bindings(&engine);
    for binding in &bindings {
        drop(binding.begin_quiesce().unwrap().unwrap());
    }
    drop(engine);
    // Startup may reject this still-owned runtime transition or reconstruct a
    // metadata view, but neither outcome may mint a fresh active payload gate.
    match Engine::open(&temp.path().join("state")) {
        Ok(reopened) => {
            assert_eq!(
                reopened.open_content(&version).err().unwrap().code(),
                ErrorCode::ContentUnavailable
            );
            for binding in read_bindings(&reopened) {
                assert!(binding.status().unwrap().quiescing);
            }
        }
        Err(error) => {
            let mut cause: Option<&(dyn std::error::Error + 'static)> = Some(&error);
            let mut unavailable = false;
            while let Some(source) = cause {
                unavailable |= source
                    .downcast_ref::<Error>()
                    .is_some_and(|source| source.code() == ErrorCode::DiskUnavailable)
                    || source
                        .downcast_ref::<storage_contract::Error>()
                        .is_some_and(|source| source.code() == ErrorCode::DiskUnavailable);
                cause = source.source();
            }
            assert!(
                unavailable,
                "startup failed for a reason other than disk quiescing: {error:?}"
            );
        }
    }
    assert_eq!(
        plan.open().err().unwrap().code(),
        ErrorCode::ContentUnavailable
    );
    for binding in bindings {
        assert!(binding.status().unwrap().drained());
    }
}

#[test]
fn generic_managed_copy_source_cannot_escape_its_read_gate() {
    let (_temp, engine, version) = committed_fixture();
    let (source, _) = engine.valid_replica(&version).unwrap();
    let ReadSource::Disk { owner, .. } = &source else {
        panic!("published replica lost its managed provenance");
    };
    let binding = owner.root.io_binding().unwrap();
    let before_operations = binding.status().unwrap().ordinary;
    let before_descriptors = payload_descriptor_count(source.path());
    let mut original = source.open().unwrap();
    let mut duplicate = original.try_clone().unwrap();
    // A cloned descriptor retains the same operation, while both real resources
    // remain independently revocable. Operation count is not an FD count.
    assert_eq!(binding.status().unwrap().ordinary, before_operations + 1);
    assert_eq!(
        payload_descriptor_count(source.path()),
        before_descriptors + 2
    );
    let mut first = [0; 1];
    original.read_exact(&mut first).unwrap();
    assert_eq!(first, [b'o']);
    let expectation = platform_io::payload::Expectation {
        bytes: 8,
        blake3: Some(blake3::hash(b"original")),
    };
    assert_eq!(original.verify_full(expectation).unwrap().bytes, 8);
    drop(binding.begin_quiesce().unwrap().unwrap());
    assert!(binding.status().unwrap().drained());
    assert_eq!(
        source.open().err().unwrap().code(),
        ErrorCode::DiskUnavailable
    );
    assert_eq!(payload_descriptor_count(source.path()), 0);
    for file in [&mut original, &mut duplicate] {
        assert!(matches!(
            file.verify_full(expectation),
            Err(platform_io::payload::Error::Io(error))
                if error.kind() == std::io::ErrorKind::PermissionDenied
        ));
        assert_eq!(
            file.read(&mut first).unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied
        );
    }
}

fn payload_descriptor_count(path: &std::path::Path) -> usize {
    fs::read_dir("/proc/self/fd")
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|entry| match fs::read_link(entry) {
            Ok(target) => target == path,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => panic!("cannot inspect test descriptor: {error}"),
        })
        .count()
}

#[test]
fn raw_payload_helpers_now_retire_real_files_on_idle_quiesce() {
    use std::io::Seek;
    for mode in 0..3 {
        let (_temp, engine, id) = committed_fixture();
        let disk = &engine.config.disks[0];
        let owner = engine.disk_resources.get(disk).unwrap();
        let version = &engine.state.versions[&id];
        let directory = version_dir(disk, &version.history_tree_id, &id);
        let path = crate::container_storage::content_path_at(
            &owner.root,
            &directory,
            version,
            &mut |_| Ok(()),
        )
        .unwrap();
        let mut file = match mode {
            0 => owner.open_payload(&directory, version).unwrap().0,
            1 => owner.verify_payload(&directory, version).unwrap(),
            _ => open_payload_file_observed_at(&owner.root, &directory, version, &mut |_| Ok(()))
                .unwrap(),
        };
        file.rewind().unwrap();
        let mut body = Vec::new();
        file.read_to_end(&mut body).unwrap();
        assert_eq!(body, b"original");
        assert!(payload_descriptor_count(&path) > 0);
        let binding = owner.root.io_binding().unwrap();
        drop(binding.begin_quiesce().unwrap().unwrap());
        assert!(binding.status().unwrap().drained());
        assert_eq!(payload_descriptor_count(&path), 0);
        assert_eq!(
            file.metadata().unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied
        );
        assert_eq!(
            owner
                .open_payload(&directory, version)
                .err()
                .unwrap()
                .code(),
            ErrorCode::DiskUnavailable
        );
        assert_eq!(fs::read(&path).unwrap(), b"original");
    }
}

#[test]
fn existing_payload_proof_cannot_sync_or_reopen_after_quiesce() {
    let (_temp, engine, id) = committed_fixture();
    let disk = &engine.config.disks[0];
    let owner = engine.disk_resources.get(disk).unwrap();
    let version = &engine.state.versions[&id];
    let directory = version_dir(disk, &version.history_tree_id, &id);
    let proof = VerifiedExisting::read_at(&owner.root, &directory, version).unwrap();
    proof
        .sync_at(&owner.root, &directory, proof.path())
        .unwrap();
    let path = proof.path().to_owned();
    assert!(payload_descriptor_count(&path) > 0);
    let binding = owner.root.io_binding().unwrap();
    drop(binding.begin_quiesce().unwrap().unwrap());
    assert!(binding.status().unwrap().drained());
    assert_eq!(payload_descriptor_count(&path), 0);
    assert_eq!(
        proof
            .sync_at(&owner.root, &directory, &path)
            .unwrap_err()
            .code(),
        ErrorCode::DiskUnavailable
    );
    assert_eq!(fs::read(path).unwrap(), b"original");
}

#[test]
fn payload_verification_cancellation_cannot_return_a_verified_file() {
    let (_temp, engine, id) = committed_fixture();
    let disk = &engine.config.disks[0];
    let owner = engine.disk_resources.get(disk).unwrap();
    let version = &engine.state.versions[&id];
    let directory = version_dir(disk, &version.history_tree_id, &id);
    let (file, _, manifest) = owner.open_payload(&directory, version).unwrap();
    let binding = owner.root.io_binding().unwrap();
    let mut observed = 0;
    let result = verify_open_payload_observed(file, manifest, version, &mut |bytes| {
        observed += bytes;
        if bytes > 0 {
            drop(binding.begin_quiesce().unwrap().unwrap());
        }
        Ok(())
    });
    assert_eq!(observed, 8);
    assert!(
        matches!(result, Err(Error::Contract(storage_contract::Error::Io { source, .. }))
        if source.kind() == std::io::ErrorKind::PermissionDenied)
    );
    assert!(binding.status().unwrap().drained());
}
