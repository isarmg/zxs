//! Public engine baseline, not first-account/helper startup or FILE-local Draft support.
use std::{
    fs,
    io::Read,
    os::unix::fs::{MetadataExt, PermissionsExt},
};
use storage_contract::{BranchName, CommitState, ErrorCode, Mode, OperationId, WorkspaceState};
use storage_engine::{CommitRequest, Config, Engine, Entry, ValidatedConfig, Workspace};

fn request(workspace: &Workspace, message: &str) -> CommitRequest {
    CommitRequest {
        workspace_id: workspace.workspace_id.clone(),
        operation_id: OperationId::generate().unwrap(),
        principal: "admin".into(),
        expected_head: workspace.base_head.clone(),
        fencing_token: workspace.fencing_token,
        message: message.into(),
        request_digest: None,
    }
}

fn entry(engine: &Engine, branch: &str) -> Entry {
    let entries = engine.list_tree(branch).unwrap();
    assert_eq!(entries.len(), 1);
    entries.into_iter().next().unwrap()
}

fn body(engine: &Engine, version: &str) -> Vec<u8> {
    let mut opened = engine.open_content(version).unwrap();
    let mut bytes = Vec::new();
    opened.file.read_to_end(&mut bytes).unwrap();
    bytes
}

fn staged_body(engine: &Engine, workspace: &str, item: &str) -> Vec<u8> {
    let mut opened = engine.open_workspace_content(workspace, item, "admin").unwrap();
    let mut bytes = Vec::new();
    opened.file.read_to_end(&mut bytes).unwrap();
    bytes
}

// This uses the already published first FILE, not a second bootstrap fixture.
fn unsupported_container_lifecycle_preserves_first_file(
    engine: &mut Engine,
    repository: &storage_engine::Repository,
    first: &Entry,
    original: &CommitRequest,
    container: &std::path::Path,
) {
    use storage_contract::file_relocation::{NamespaceEntryId, ObjectMoveRequest};
    use storage_contract::history_window::{HistoryFileId, HistoryRepositoryId};
    use storage_engine::{DeleteFileRequest, FileDeletionTarget, UndoFileDeletionRequest};

    let repository_id = HistoryRepositoryId::parse(&repository.repository_id).unwrap();
    let file_id = HistoryFileId::parse(&first.file_id).unwrap();
    let path = name_policy::LookupPath::from(&first.path);
    let target = FileDeletionTarget {
        repository_id: &repository_id,
        file_id: &file_id,
        path: &path,
    };
    let before = storage_engine::canonical_json(engine.state()).unwrap();
    let file_database = fs::read(container.join(".zxs-file.sqlite")).unwrap();
    let old_body = body(engine, &first.version_id);
    let saved = engine.operation(original.operation_id).unwrap();
    let deletion = OperationId::generate().unwrap();
    assert_eq!(engine.soft_delete_file(DeleteFileRequest {
        target, principal: "admin", operation_id: deletion,
    }).unwrap_err().code(), ErrorCode::UnsupportedOperation);
    // Existing ownership/digest rules still win before unsupported new writes.
    assert_eq!(engine.soft_delete_file(DeleteFileRequest {
        target, principal: "admin", operation_id: original.operation_id,
    }).unwrap_err().code(), ErrorCode::IdempotencyKeyReused);
    let undo = OperationId::generate().unwrap();
    let deletion_id = storage_contract::FileDeletionId::generate().unwrap().to_string();
    assert_eq!(engine.undo_file_deletion(UndoFileDeletionRequest {
        file_id: &file_id,
        deletion_node_id: &deletion_id,
        principal: "admin",
        operation_id: undo,
    }).unwrap_err().code(), ErrorCode::FileNodeNotFound);
    let movement = OperationId::generate().unwrap();
    let request = ObjectMoveRequest {
        source_repository_id: repository_id.clone(),
        source_entry_id: NamespaceEntryId::parse(&first.entry_id).unwrap(),
        source_path: path,
        target_slot_id: repository.primary_slot_id.clone(),
        target_repository_id: Some(repository_id),
        target_path: name_policy::PortablePath::parse("moved-notes.txt").unwrap(),
        replace: false,
        target_entry_id: None,
    };
    assert_eq!(engine.move_object(&request, "admin", movement).unwrap_err().code(),
        ErrorCode::UnsupportedOperation);
    for operation in [deletion, undo, movement] {
        assert!(!engine.state().operations.contains_key(&operation));
    }
    assert!(engine.file_deletion(&first.file_id).is_none());
    assert_eq!(storage_engine::canonical_json(engine.state()).unwrap(), before);
    assert_eq!(fs::read(container.join(".zxs-file.sqlite")).unwrap(), file_database);
    assert_eq!(body(engine, &first.version_id), old_body);
    assert_eq!(engine.commit_workspace(original.clone()).unwrap(), saved);
    assert_eq!(storage_engine::canonical_json(engine.state()).unwrap(), before);
}

fn copy_directory(source: &std::path::Path, target: &std::path::Path) {
    fs::create_dir(target).unwrap();
    fs::set_permissions(target, fs::Permissions::from_mode(0o700)).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let destination = target.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_directory(&entry.path(), &destination);
        } else {
            fs::copy(entry.path(), destination).unwrap();
        }
    }
}

#[cfg(feature = "fault-injection")]
fn file_database_rows(path: &std::path::Path)
    -> std::collections::BTreeMap<(String, String), json_codec::Value>
{
    use sqlite_access::{Connection, Row, SqliteConnection};
    use sqlite_access::sqlite::{SqliteConnectOptions, SqliteOwnedBuf};
    let bytes = fs::read(path).unwrap();
    platform_io::sqlite::block_on_sqlite_connection(async {
        let mut connection = SqliteConnection::connect_with(
            &SqliteConnectOptions::new().in_memory(true),
        ).await.unwrap();
        connection.deserialize(None, SqliteOwnedBuf::try_from(bytes.as_slice()).unwrap(), true)
            .await.unwrap();
        let values = sqlite_access::query("SELECT kind,id,body FROM records ORDER BY kind,id")
            .fetch_all(&mut connection).await.unwrap();
        let rows = values.into_iter().map(|row| {
            let body: String = row.try_get("body").unwrap();
            ((row.try_get("kind").unwrap(), row.try_get("id").unwrap()),
                json_codec::from_str(&body).unwrap())
        }).collect();
        connection.close().await.unwrap();
        rows
    })
}

#[test]
fn empty_held_pool_stages_reopens_commits_and_selects_real_file_history() {
    let temporary = tempfile::tempdir().unwrap();
    let system = temporary.path().join("system");
    let disk = temporary.path().join("primary");
    for path in [&system, &disk] {
        fs::create_dir(path).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(fs::read_dir(path).unwrap().count(), 0);
    }
    let identity = fs::metadata(&disk).unwrap();
    let selected = fs::File::open(&disk).unwrap();
    let config = Config::independent_roots(
        Mode::Development, [(name_policy::Name::parse("Primary").unwrap(), disk.clone())],
    ).unwrap();
    let registration = config.disks[0].clone();
    let mut engine = Engine::initialize_held(
        &system,
        ValidatedConfig::try_from(config).unwrap(),
        vec![selected],
    )
    .unwrap();
    assert_eq!(engine.state().sequence, 0);
    assert!(engine.state().versions.is_empty());
    let storage_roots = engine.config().storage_roots.clone();
    assert_eq!(storage_roots.len(), 1);
    let disk_id = registration.disk_id.parse::<storage_contract::DiskId>().unwrap();
    let root_binding = &storage_roots[&disk_id];
    assert_eq!(root_binding.registration.disk_id, registration.disk_id);
    assert_eq!(root_binding.root_entity.device(), identity.dev());
    assert_eq!(root_binding.root_entity.inode(), identity.ino());
    assert!(!disk.join("histories").exists());
    assert!(disk.join("zxs-data/pool-membership/config.json").is_file());
    assert!(!disk.join("metadata/pool-membership/config.json").exists());
    for (name, bound) in [
        ("common-data", &root_binding.roots.common_data),
        ("deleted-data", &root_binding.roots.deleted_data),
        ("zxs-data", &root_binding.roots.zxs_data),
    ] {
        let observed = fs::metadata(disk.join(name)).unwrap();
        assert_eq!((observed.dev(), observed.ino()),
            (bound.entity.device(), bound.entity.inode()));
    }
    drop(engine);
    engine = Engine::open(&system).unwrap();
    assert!(!engine.is_read_only());
    assert_eq!(engine.state().sequence, 0);
    assert_eq!(engine.config().storage_roots, storage_roots);
    assert_eq!(engine.config().disks[0].disk_id, registration.disk_id);
    assert_eq!(engine.config().disks[0].binding_generation, registration.binding_generation);
    let reopened = fs::metadata(&disk).unwrap();
    assert_eq!((reopened.dev(), reopened.ino()), (identity.dev(), identity.ino()));

    let root_operation = OperationId::generate().unwrap();
    let repository = engine
        .create_disk_root(&registration.slot_id, "admin", root_operation)
        .unwrap();
    let namespace_origin = engine.operation(root_operation).unwrap()
        .result["file_manager_root_origin"].clone();
    assert_eq!(namespace_origin["genesis"],
        json_codec::to_value(root_binding).unwrap());
    let first_workspace = engine.create_workspace(&repository.main_branch_id, "admin").unwrap();
    engine.stage_bytes(&first_workspace.workspace_id, "notes.txt", b"first body").unwrap();
    let first_request = request(&first_workspace, "first explicit save");
    let unknown = disk.join("common-data/notes.txt");
    fs::create_dir(&unknown).unwrap();
    fs::set_permissions(&unknown, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(unknown.join("not-owned"), b"unknown user directory").unwrap();
    let before_collision = storage_engine::canonical_json(engine.state()).unwrap();
    assert_eq!(engine.commit_workspace(first_request.clone()).unwrap_err().code(),
        ErrorCode::ContainerLayoutInvalid);
    assert_eq!(storage_engine::canonical_json(engine.state()).unwrap(), before_collision);
    assert_eq!(fs::read(unknown.join("not-owned")).unwrap(), b"unknown user directory");
    fs::remove_file(unknown.join("not-owned")).unwrap();
    fs::remove_dir(&unknown).unwrap();
    let first_receipt = engine.commit_workspace(first_request.clone()).unwrap();
    assert_eq!(first_receipt.commit_state, CommitState::Published);
    let first = entry(&engine, &repository.main_branch_id);
    let first_version = engine.state().versions[&first.version_id].clone();
    assert_eq!(body(&engine, &first.version_id), b"first body");
    let file_id = first.file_id.parse::<storage_contract::FileId>().unwrap();
    let container_record = engine.state().file_containers[&file_id].clone();
    assert_eq!(container_record.owner_disk, disk_id);
    assert_eq!(container_record.created_by, first_request.operation_id);
    assert_eq!(container_record.namespace.repository.to_string(), repository.repository_id);
    assert_eq!(container_record.namespace.branch.to_string(), repository.main_branch_id);
    assert_eq!(container_record.namespace.entry.to_string(), first.entry_id);
    assert_eq!(container_record.namespace.name.as_str(), "notes.txt");
    assert_eq!(container_record.history_tree_id.to_string(), repository.history_tree_id);
    assert!(matches!(container_record.working,
        storage_contract::file_draft::FileWorkingState::Automatic { .. }));
    let replica = &container_record.replicas[&disk_id];
    assert_eq!(replica.root_id, root_binding.root_id);
    let container = disk.join("common-data/notes.txt");
    assert!(!disk.join("zxs-data")
        .join(format!("container-prepare-{}", replica.preparation)).exists());
    assert!(!disk.join("common-data").join(&repository.history_tree_id)
        .join("file-containers").exists());
    let storage_id = engine.state().versions[&first.version_id].storage_id.to_string();
    assert_eq!(storage_id.len(), 15);
    assert_eq!(fs::read(container.join("versions").join(format!("{storage_id}-notes.txt")))
        .unwrap(), b"first body");
    let container_identity = fs::metadata(&container).unwrap();
    assert_eq!((replica.entity.device(), replica.entity.inode()),
        (container_identity.dev(), container_identity.ino()));
    assert_ne!(replica.entity.inode(), 0);
    unsupported_container_lifecycle_preserves_first_file(
        &mut engine, &repository, &first, &first_request, &container,
    );
    let before_container_fault = storage_engine::canonical_json(engine.state()).unwrap();
    let parked_container = temporary.path().join("parked-original-container");
    fs::rename(&container, &parked_container).unwrap();
    copy_directory(&parked_container, &container);
    assert_eq!(engine.open_content(&first.version_id).err().unwrap().code(),
        ErrorCode::ContentUnavailable);
    assert_eq!(storage_engine::canonical_json(engine.state()).unwrap(), before_container_fault);
    // Equal FILE/BODY bytes in a different entity are not the published container.
    assert_eq!(fs::read(container.join(".zxs-file.sqlite")).unwrap(),
        fs::read(parked_container.join(".zxs-file.sqlite")).unwrap());
    drop(engine);
    assert!(Engine::open(&system).is_err());
    fs::remove_dir_all(&container).unwrap();
    fs::rename(&parked_container, &container).unwrap();
    engine = Engine::open(&system).unwrap();
    assert_eq!(engine.state().file_containers[&file_id], container_record);
    assert_eq!(body(&engine, &first.version_id), b"first body");

    // The new root is an actual ordinary FILE writer/reader destination, not
    // an empty sidecar alongside a silently retained histories layout.
    assert!(disk.join("zxs-data/common-data").join(&repository.history_tree_id)
        .join("versions").join(&first.version_id).join("version.json").is_file());
    assert!(!disk.join("common-data").join(&repository.history_tree_id).exists());
    assert!(!disk.join("histories").exists());
    // Complete matching local proofs at the retired address are not a second
    // format. The published reader must use only management metadata.
    let metadata = disk.join("zxs-data/common-data").join(&repository.history_tree_id)
        .join("versions").join(&first.version_id);
    let retired_history = disk.join("common-data").join(&repository.history_tree_id);
    let retired_versions = retired_history.join("versions");
    fs::create_dir(&retired_history).unwrap();
    fs::create_dir(&retired_versions).unwrap();
    for path in [&retired_history, &retired_versions] {
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let retired_metadata = retired_versions.join(&first.version_id);
    fs::rename(&metadata, &retired_metadata).unwrap();
    assert_eq!(engine.open_content(&first.version_id).err().unwrap().code(),
        ErrorCode::ContentUnavailable);
    drop(engine);
    let without_management_proof = Engine::open(&system).unwrap();
    assert_eq!(without_management_proof.open_content(&first.version_id).err().unwrap().code(),
        ErrorCode::ContentUnavailable);
    assert_eq!(storage_engine::canonical_json(without_management_proof.state()).unwrap(),
        before_container_fault);
    drop(without_management_proof);
    fs::rename(&retired_metadata, &metadata).unwrap();
    fs::remove_dir(&retired_versions).unwrap();
    fs::remove_dir(&retired_history).unwrap();
    engine = Engine::open(&system).unwrap();
    assert_eq!(body(&engine, &first.version_id), b"first body");

    // A live resource must not follow a replacement common-data name. A cold
    // identity check alone would miss this after-open substitution.
    let before_replacement = storage_engine::canonical_json(engine.state()).unwrap();
    let common = disk.join("common-data");
    let parked_common = disk.join("parked-live-common");
    fs::rename(&common, &parked_common).unwrap();
    fs::create_dir(&common).unwrap();
    fs::set_permissions(&common, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(engine.open_content(&first.version_id).err().unwrap().code(),
        ErrorCode::ContentUnavailable);
    assert_eq!(fs::read_dir(&common).unwrap().count(), 0);
    assert_eq!(storage_engine::canonical_json(engine.state()).unwrap(), before_replacement);
    fs::remove_dir(&common).unwrap();
    fs::rename(&parked_common, &common).unwrap();
    assert_eq!(body(&engine, &first.version_id), b"first body");

    let branch = engine
        .create_file_branch(
            &first.file_id,
            &first.version_id,
            &BranchName::parse("edited").unwrap(),
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap()
        .branch_id
        .unwrap();
    let original_branch = engine.state().file_selections[&first.file_id].clone();
    assert_ne!(branch, original_branch);
    assert_eq!(engine.state().versions.len(), 1);
    assert_eq!(engine.file_branch_head(&branch).unwrap(), first.version_id);
    engine
        .select_file_branch(&first.file_id, &branch, "admin", OperationId::generate().unwrap())
        .unwrap();
    let workspace = engine.create_workspace(&branch, "admin").unwrap();
    let before_staging = storage_engine::canonical_json(engine.state()).unwrap();
    let staged = engine
        .stage_bytes(&workspace.workspace_id, "notes.txt", b"second body, explicitly committed")
        .unwrap();
    let commit = request(&workspace, "explicit save after restart");
    assert_eq!(storage_engine::canonical_json(engine.state()).unwrap(), before_staging);
    assert_eq!(engine.file_branch_head(&branch).unwrap(), first.version_id);
    assert_eq!(body(&engine, &first.version_id), b"first body");

    // This is the existing leased Workspace contract. A restart must not turn
    // upload completion into a history commit or discard the explicit input.
    drop(engine);
    engine = Engine::open(&system).unwrap();
    let recovered = engine.workspace(&workspace.workspace_id).unwrap();
    assert_eq!(recovered.state, WorkspaceState::Open);
    assert_eq!(recovered.base_head, workspace.base_head);
    assert_eq!(recovered.fencing_token, workspace.fencing_token);
    assert_eq!(
        storage_engine::canonical_json(&recovered.files[&staged.item_id]).unwrap(),
        storage_engine::canonical_json(&staged).unwrap(),
    );
    assert_eq!(engine.file_branch_head(&branch).unwrap(), first.version_id);
    assert_eq!(engine.state().versions.len(), 1);
    assert_eq!(
        staged_body(&engine, &workspace.workspace_id, &staged.item_id),
        b"second body, explicitly committed",
    );
    let before_commit = storage_engine::canonical_json(engine.state()).unwrap();
    let mut wrong_actor = commit.clone();
    wrong_actor.principal = "another actor".into();
    assert_eq!(
        engine.commit_workspace(wrong_actor).unwrap_err().code(),
        ErrorCode::PermissionDenied,
    );
    assert_eq!(storage_engine::canonical_json(engine.state()).unwrap(), before_commit);
    assert!(!engine.state().operations.contains_key(&commit.operation_id));

    let receipt = engine.commit_workspace(commit.clone()).unwrap();
    assert_eq!(receipt.commit_state, CommitState::Published);
    let second = entry(&engine, &repository.main_branch_id);
    assert_eq!(second.file_id, first.file_id);
    assert_eq!(second.entry_id, first.entry_id);
    assert_ne!(second.version_id, first.version_id);
    assert_eq!(engine.state().versions.len(), 2);
    assert_eq!(engine.state().versions[&first.version_id], first_version);
    assert_eq!(engine.state().file_containers[&file_id], container_record);
    assert_eq!(engine.file_branch_head(&branch).unwrap(), second.version_id);
    assert_eq!(
        engine.file_node_version(&first.file_id, &second.version_id).unwrap(),
        second.version_id,
    );
    assert_eq!(
        engine.file_node_parents(&first.file_id, &second.version_id).unwrap(),
        vec![first.version_id.clone()],
    );
    for version in [&first.version_id, &second.version_id] {
        let version_row = &engine.state().versions[version];
        assert_eq!(version_row.file_id, first.file_id);
        assert_eq!(version_row.history_tree_id, repository.history_tree_id);
        assert_eq!(version_row.creation_primary_slot_id, registration.slot_id);
        let placement = &engine.state().placements[version];
        assert_eq!(placement.primary_slot_id, registration.slot_id);
        assert_eq!(placement.route_slot_ids, vec![registration.slot_id.clone()]);
        assert_eq!(
            placement.binding_generations[&registration.slot_id],
            registration.binding_generation,
        );
    }
    assert_eq!(body(&engine, &first.version_id), b"first body");
    assert_eq!(body(&engine, &second.version_id), b"second body, explicitly committed");

    let select_operation = OperationId::generate().unwrap();
    let selection = engine
        .select_file_branch(&first.file_id, &original_branch, "admin", select_operation)
        .unwrap();
    assert_eq!(entry(&engine, &repository.main_branch_id).version_id, first.version_id);
    assert_eq!(engine.state().versions.len(), 2);
    drop(engine);
    engine = Engine::open(&system).unwrap();
    assert!(!engine.is_read_only());
    assert_eq!(entry(&engine, &repository.main_branch_id).version_id, first.version_id);
    assert_eq!(engine.file_branch_head(&branch).unwrap(), second.version_id);
    assert_eq!(body(&engine, &first.version_id), b"first body");
    assert_eq!(body(&engine, &second.version_id), b"second body, explicitly committed");
    let before_retry = storage_engine::canonical_json(engine.state()).unwrap();
    assert_eq!(engine.commit_workspace(commit.clone()).unwrap(), receipt);
    assert_eq!(engine.commit_workspace(first_request).unwrap(), first_receipt);
    assert_eq!(
        engine
            .select_file_branch(&first.file_id, &original_branch, "admin", select_operation)
            .unwrap(),
        selection,
    );
    assert_eq!(storage_engine::canonical_json(engine.state()).unwrap(), before_retry);
    let mut changed_request = commit;
    changed_request.message = "different request under the same operation".into();
    assert_eq!(
        engine.commit_workspace(changed_request).unwrap_err().code(),
        ErrorCode::IdempotencyKeyReused,
    );
    assert_eq!(storage_engine::canonical_json(engine.state()).unwrap(), before_retry);
    assert_eq!(engine.config().storage_roots, storage_roots);
    drop(engine);

    let saved_config = fs::read(system.join("config.json")).unwrap();
    let saved_seal = fs::read(disk.join("disk.json")).unwrap();
    let saved_journal = fs::read(disk.join("metadata/application-log/journal.bin")).unwrap();
    for name in ["common-data", "deleted-data", "zxs-data"] {
        let named = disk.join(name);
        let parked = disk.join(format!("parked-{name}"));
        fs::rename(&named, &parked).unwrap();
        fs::create_dir(&named).unwrap();
        fs::set_permissions(&named, fs::Permissions::from_mode(0o700)).unwrap();
        let replacement = fs::metadata(&named).unwrap();
        assert_eq!(Engine::open(&system).err().unwrap().code(), ErrorCode::DiskIdentityMismatch);
        assert_eq!(fs::read_dir(&named).unwrap().count(), 0);
        let after = fs::metadata(&named).unwrap();
        assert_eq!((after.dev(), after.ino()), (replacement.dev(), replacement.ino()));
        assert_eq!(fs::read(system.join("config.json")).unwrap(), saved_config);
        assert_eq!(fs::read(disk.join("disk.json")).unwrap(), saved_seal);
        assert_eq!(fs::read(disk.join("metadata/application-log/journal.bin")).unwrap(),
            saved_journal);
        fs::remove_dir(&named).unwrap();
        fs::rename(&parked, &named).unwrap();
        let reopened = Engine::open(&system).unwrap();
        assert_eq!(reopened.config().storage_roots, storage_roots);
        assert_eq!(body(&reopened, &second.version_id), b"second body, explicitly committed");
        drop(reopened);
    }
}


#[cfg(feature = "fault-injection")]
#[test]
fn fresh_container_preparation_survives_exact_decision_cuts_without_adopting_names() {
    for point in [
        "after_payloads", "after_accept_1", "after_finalize", "after_container_names",
        "after_local_publications",
    ] {
        let temporary = tempfile::tempdir().unwrap();
        let system = temporary.path().join("system");
        let disk = temporary.path().join("primary");
        for path in [&system, &disk] {
            fs::create_dir(path).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let config = Config::independent_roots(
            Mode::Development, [(name_policy::Name::parse("Primary").unwrap(), disk.clone())],
        ).unwrap();
        let slot = config.disks[0].slot_id.clone();
        let mut engine = Engine::initialize_held(
            &system, ValidatedConfig::try_from(config).unwrap(), vec![fs::File::open(&disk).unwrap()],
        ).unwrap();
        let repository = engine.create_disk_root(&slot, "admin", OperationId::generate().unwrap())
            .unwrap();
        let workspace = engine.create_workspace(&repository.main_branch_id, "admin").unwrap();
        engine.stage_bytes(&workspace.workspace_id, "ordinary.txt", b"recover prepared body")
            .unwrap();
        let request = request(&workspace, "ordinary physical preparation");
        let before = storage_engine::canonical_json(engine.state()).unwrap();
        engine.inject_fault_once(point);
        let error = engine.commit_workspace(request.clone()).unwrap_err();
        let injected = if point == "after_payloads" {
            assert_eq!(error.code(), ErrorCode::InjectedFailure, "{point}: {error}");
            &error
        } else {
            assert_eq!(error.code(), ErrorCode::InDoubt, "{point}: {error}");
            let storage_engine::Error::Context { source, .. } = &error else {
                panic!("missing decided error context at {point}: {error}");
            };
            source.as_ref()
        };
        assert_eq!(injected.code(), ErrorCode::InjectedFailure, "{point}: {error}");
        assert_eq!(
            injected.message().as_ref(), format!("interrupted at {point}").as_str(),
            "wrong injected cause at {point}: {error}",
        );
        assert_eq!(storage_engine::canonical_json(engine.state()).unwrap(), before);
        let named = disk.join("common-data/ordinary.txt");
        let pending_stage = if matches!(
            point, "after_payloads" | "after_accept_1" | "after_finalize",
        ) {
            assert!(!named.exists());
            let stages = fs::read_dir(disk.join("zxs-data")).unwrap()
                .map(|entry| entry.unwrap().path())
                .filter(|path| path.file_name().unwrap().to_str().unwrap()
                    .starts_with("container-prepare-"))
                .collect::<Vec<_>>();
            assert_eq!(stages.len(), 1);
            let stage = stages.into_iter().next().unwrap();
            let identity = fs::metadata(&stage).unwrap();
            let bodies = fs::read_dir(stage.join("versions")).unwrap()
                .map(|entry| entry.unwrap().path()).collect::<Vec<_>>();
            assert_eq!(bodies.len(), 1);
            let body_name = bodies[0].file_name().unwrap().to_owned();
            assert_eq!(fs::read(&bodies[0]).unwrap(), b"recover prepared body");
            Some((stage, (identity.dev(), identity.ino()), body_name))
        } else {
            None
        };
        drop(engine);
        if point == "after_accept_1" {
            // An actual ACCEPT binds the preparation inode, not just its name
            // and complete bytes. Cold recovery cannot adopt an equal clone.
            let (stage, original_identity, body_name) = pending_stage.as_ref().unwrap();
            let parked = temporary.path().join("parked-accepted-preparation");
            fs::rename(stage, &parked).unwrap();
            copy_directory(&parked, stage);
            let replacement = fs::metadata(stage).unwrap();
            assert_ne!((replacement.dev(), replacement.ino()), *original_identity);
            let journal = disk.join("metadata/application-log/journal.bin");
            let unchanged_journal = fs::read(&journal).unwrap();
            assert_eq!(Engine::open(&system).err().unwrap().code(),
                ErrorCode::ContainerLayoutInvalid);
            assert_eq!(fs::read(&journal).unwrap(), unchanged_journal);
            assert!(!named.exists());
            for root in [stage.as_path(), parked.as_path()] {
                assert_eq!(fs::read(root.join("versions").join(body_name)).unwrap(),
                    b"recover prepared body");
            }
            let retained = fs::metadata(&parked).unwrap();
            assert_eq!((retained.dev(), retained.ino()), *original_identity);
            fs::remove_dir_all(stage).unwrap();
            fs::rename(&parked, stage).unwrap();

            fs::create_dir(&named).unwrap();
            fs::set_permissions(&named, fs::Permissions::from_mode(0o700)).unwrap();
            fs::write(named.join("not-owned"), b"do not adopt this name").unwrap();
            let identity = fs::metadata(&named).unwrap();
            let journal = disk.join("metadata/application-log/journal.bin");
            let before_journal = fs::read(&journal).unwrap();
            assert_eq!(Engine::open(&system).err().unwrap().code(),
                ErrorCode::ContainerLayoutInvalid);
            assert_eq!(fs::read(named.join("not-owned")).unwrap(), b"do not adopt this name");
            assert_eq!(fs::read(&journal).unwrap(), before_journal);
            let after = fs::metadata(&named).unwrap();
            assert_eq!((after.dev(), after.ino()), (identity.dev(), identity.ino()));
            fs::remove_file(named.join("not-owned")).unwrap();
            fs::remove_dir(&named).unwrap();
        }
        let mut reopened = Engine::open(&system).unwrap();
        let receipt = reopened.commit_workspace(request.clone()).unwrap();
        assert_eq!(receipt.commit_state, CommitState::Published);
        let item = entry(&reopened, &repository.main_branch_id);
        assert_eq!(body(&reopened, &item.version_id), b"recover prepared body");
        let record = reopened.state().file_containers
            [&item.file_id.parse::<storage_contract::FileId>().unwrap()].clone();
        let identity = fs::metadata(&named).unwrap();
        let replica = &record.replicas[&record.owner_disk];
        assert_eq!((identity.dev(), identity.ino()),
            (replica.entity.device(), replica.entity.inode()));
        assert!(!disk.join("zxs-data")
            .join(format!("container-prepare-{}", replica.preparation)).exists());
        if point == "after_payloads" {
            // There was no Candidate/ACCEPT for that old preparation. A retry
            // makes its own one, and neither adopts nor deletes the orphan.
            let (orphan, identity, body_name) = pending_stage.as_ref().unwrap();
            assert_ne!(orphan.file_name().unwrap(),
                std::ffi::OsStr::new(&format!("container-prepare-{}", replica.preparation)));
            let retained = fs::metadata(orphan).unwrap();
            assert_eq!((retained.dev(), retained.ino()), *identity);
            assert_eq!(fs::read(orphan.join("versions").join(body_name)).unwrap(),
                b"recover prepared body");
        }
        drop(reopened);
        let mut again = Engine::open(&system).unwrap();
        let before_retry = storage_engine::canonical_json(again.state()).unwrap();
        assert_eq!(again.commit_workspace(request).unwrap(), receipt);
        assert_eq!(storage_engine::canonical_json(again.state()).unwrap(), before_retry);
        assert_eq!(body(&again, &item.version_id), b"recover prepared body");
    }
}

#[cfg(feature = "fault-injection")]
#[test]
fn fresh_backup_replica_publishes_exact_named_prefix_and_recovers_its_preparation() {
    for cut in [None, Some("after_accept_1"), Some("after_container_names")] {
        let temporary = tempfile::tempdir().unwrap();
        let system = temporary.path().join("system");
        let roots = ["owner-mount", "replica-mount", "third-level-mount"]
            .map(|name| temporary.path().join(name));
        for root in std::iter::once(&system).chain(roots.iter()) {
            fs::create_dir(root).unwrap();
            fs::set_permissions(root, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let config = Config::development(["Alpha", "Beta", "Gamma"].into_iter()
            .zip(roots.iter()).map(|(name, root)| {
                (name_policy::Name::parse(name).unwrap(), root.clone())
            })).unwrap();
        let owner = config.disks[0].disk_id.parse::<storage_contract::DiskId>().unwrap();
        let backup = config.disks[1].disk_id.parse::<storage_contract::DiskId>().unwrap();
        let slot = config.disks[0].slot_id.clone();
        let component = format!("Alpha--{}", &owner.to_string()["disk_".len()..]);
        let prefix = roots[1].join(&component);
        let mut engine = Engine::initialize_held(
            &system, ValidatedConfig::try_from(config).unwrap(),
            roots.iter().map(|root| fs::File::open(root).unwrap()).collect()).unwrap();
        let repository = engine.create_disk_root(&slot, "admin", OperationId::generate().unwrap())
            .unwrap();
        let policy = storage_contract::new_id("policy").unwrap();
        engine.update_protection_policy(storage_engine::ProtectionPolicyUpdate {
            policy_id: &policy, repository_id: &repository.repository_id, path_prefix: "",
            target_copies: 2, expected_generation: 0, principal: "admin",
            operation_id: OperationId::generate().unwrap(),
        }).unwrap();
        let workspace = engine.create_workspace(&repository.main_branch_id, "admin").unwrap();
        engine.stage_bytes(&workspace.workspace_id, "one.txt", b"owner and actual B replica")
            .unwrap();
        let first_request = request(&workspace, "publish real automatic B path");
        let before = storage_engine::canonical_json(engine.state()).unwrap();
        let journals = || roots.iter().map(|root| {
            fs::read(root.join("metadata/application-log/journal.bin")).unwrap()
        }).collect::<Vec<_>>();
        let journal_before = journals();
        fs::create_dir(&prefix).unwrap();
        fs::set_permissions(&prefix, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(prefix.join("unknown-owner"), b"not a registered source directory").unwrap();
        assert_eq!(engine.commit_workspace(first_request.clone()).unwrap_err().code(),
            ErrorCode::ContainerLayoutInvalid);
        assert_eq!(storage_engine::canonical_json(engine.state()).unwrap(), before);
        assert_eq!(journals(), journal_before);
        assert_eq!(fs::read(prefix.join("unknown-owner")).unwrap(),
            b"not a registered source directory");
        fs::remove_file(prefix.join("unknown-owner")).unwrap();
        fs::remove_dir(&prefix).unwrap();

        if let Some(point) = cut {
            engine.inject_fault_once(point);
            let error = engine.commit_workspace(first_request.clone()).unwrap_err();
            assert_eq!(error.code(), ErrorCode::CommitInDoubt);
            let mut source: Option<&(dyn std::error::Error + 'static)> = Some(&error);
            let mut exact = false;
            while let Some(cause) = source {
                if let Some(injected) = cause.downcast_ref::<storage_engine::Error>() {
                    if injected.code() == ErrorCode::InjectedFailure {
                        exact = injected.message().as_ref()
                            == format!("interrupted at {point}").as_str();
                        break;
                    }
                }
                source = cause.source();
            }
            assert!(exact, "missing actual {point}: {error}");
        } else {
            assert_eq!(engine.commit_workspace(first_request.clone()).unwrap().commit_state,
                CommitState::Published);
        }
        drop(engine);
        if cut == Some("after_accept_1") {
            assert!(!prefix.exists());
            let preparations = fs::read_dir(roots[1].join("zxs-data")).unwrap()
                .map(|entry| entry.unwrap().path())
                .filter(|path| path.file_name().unwrap().to_str().unwrap()
                    .starts_with("source-prepare-"))
                .collect::<Vec<_>>();
            assert_eq!(preparations.len(), 1);
            let stage = &preparations[0];
            let real = fs::metadata(stage).unwrap();
            let parked = temporary.path().join("real-prefix-preparation");
            fs::rename(stage, &parked).unwrap();
            copy_directory(&parked, stage);
            assert_ne!((fs::metadata(stage).unwrap().dev(), fs::metadata(stage).unwrap().ino()),
                (real.dev(), real.ino()));
            let journal_before = journals();
            assert_eq!(Engine::open(&system).err().unwrap().code(),
                ErrorCode::ContainerLayoutInvalid);
            assert_eq!(journals(), journal_before);
            assert!(!prefix.exists());
            fs::remove_dir_all(stage).unwrap();
            fs::rename(&parked, stage).unwrap();
        }
        engine = Engine::open(&system).unwrap();
        assert!(!engine.is_read_only());
        let receipt = engine.commit_workspace(first_request.clone()).unwrap();
        assert_eq!(receipt.commit_state, CommitState::Published);
        let first = entry(&engine, &repository.main_branch_id);
        let file = first.file_id.parse::<storage_contract::FileId>().unwrap();
        let record = engine.state().file_containers[&file].clone();
        assert_eq!(record.owner_disk, owner);
        assert!(record.replicas[&owner].prefixes.is_empty());
        assert_eq!(record.replicas[&backup].prefixes.len(), 1);
        let binding = record.replicas[&backup].prefixes[0].clone();
        assert_eq!(binding.source_disk, owner);
        let observed = fs::metadata(&prefix).unwrap();
        assert_eq!((binding.entity.device(), binding.entity.inode()),
            (observed.dev(), observed.ino()));
        assert!(!roots[1].join("zxs-data")
            .join(format!("source-prepare-{}", binding.preparation)).exists());
        let version = &engine.state().versions[&first.version_id];
        let body_leaf = format!("{}-one.txt", version.storage_id);
        assert_eq!(fs::read(prefix.join("common-data/one.txt/versions").join(&body_leaf)).unwrap(),
            b"owner and actual B replica");
        assert_eq!(fs::read(roots[0].join("common-data/one.txt/versions").join(&body_leaf))
            .unwrap(), b"owner and actual B replica");
        assert!(prefix.join("zxs-data/common-data").join(&repository.history_tree_id)
            .join("versions").join(&first.version_id).join("version.json").is_file());
        assert!(!prefix.join(&repository.history_tree_id).exists());
        assert_eq!(fs::read_dir(roots[2].join("common-data")).unwrap().count(), 0);
        assert_eq!(body(&engine, &first.version_id), b"owner and actual B replica");
        // Force the ordinary authenticated reader to consume B, rather than
        // accepting a correct directory tree that no real reader can use.
        let owner_body = roots[0].join("common-data/one.txt/versions").join(&body_leaf);
        let parked_body = temporary.path().join("temporarily-unavailable-owner-body");
        fs::rename(&owner_body, &parked_body).unwrap();
        assert_eq!(body(&engine, &first.version_id), b"owner and actual B replica");
        fs::rename(&parked_body, &owner_body).unwrap();

        // A second real file shares the published source identity, never mkdirs
        // or adopts the prefix again. Metadata-only follow-ups retain it too.
        let second = engine.create_workspace(&repository.main_branch_id, "admin").unwrap();
        engine.stage_bytes(&second.workspace_id, "two.txt", b"second file under same B prefix")
            .unwrap();
        let second_request = request(&second, "reuse published source entity");
        engine.commit_workspace(second_request.clone()).unwrap();
        let entries = engine.list_tree(&repository.main_branch_id).unwrap();
        let second_file = entries.iter()
            .find(|entry| entry.path.as_str() == "two.txt").unwrap();
        let second_id = second_file.file_id.parse::<storage_contract::FileId>().unwrap();
        assert_eq!(engine.state().file_containers[&second_id].replicas[&backup].prefixes,
            vec![binding.clone()]);
        drop(engine);

        let parked = temporary.path().join("real-published-prefix");
        fs::rename(&prefix, &parked).unwrap();
        copy_directory(&parked, &prefix);
        assert_eq!(fs::read(prefix.join("common-data/one.txt/versions").join(&body_leaf)).unwrap(),
            fs::read(parked.join("common-data/one.txt/versions").join(&body_leaf)).unwrap());
        let journal_before = journals();
        assert_eq!(Engine::open(&system).err().unwrap().code(), ErrorCode::ContainerLayoutInvalid);
        assert_eq!(journals(), journal_before);
        fs::remove_dir_all(&prefix).unwrap();
        fs::rename(&parked, &prefix).unwrap();

        // Keep the authenticated source prefix itself untouched. Matching bytes
        // at a replacement lifecycle root do not carry its recorded identity.
        let common = prefix.join("common-data");
        let parked_common = temporary.path().join("real-B-common-area");
        fs::rename(&common, &parked_common).unwrap();
        copy_directory(&parked_common, &common);
        assert_eq!(fs::read(common.join("one.txt/versions").join(&body_leaf)).unwrap(),
            fs::read(parked_common.join("one.txt/versions").join(&body_leaf)).unwrap());
        let journal_before = journals();
        assert_eq!(Engine::open(&system).err().unwrap().code(), ErrorCode::ContainerLayoutInvalid);
        assert_eq!(journals(), journal_before);
        fs::remove_dir_all(&common).unwrap();
        fs::rename(&parked_common, &common).unwrap();

        engine = Engine::open(&system).unwrap();
        let before_retry = storage_engine::canonical_json(engine.state()).unwrap();
        engine.commit_workspace(second_request).unwrap();
        assert_eq!(storage_engine::canonical_json(engine.state()).unwrap(), before_retry);
        assert_eq!(engine.state().file_containers[&file], record);
        assert_eq!(body(&engine, &first.version_id), b"owner and actual B replica");


        // C is introduced only after B has been published and cold-read above.
        // Its direct source is B, so its address is Beta--B/Alpha--A, not A alone.
        let third = engine.config().disks[2].disk_id.parse::<storage_contract::DiskId>()
            .unwrap();
        let beta_component = format!("Beta--{}", &backup.to_string()["disk_".len()..]);
        let outer = roots[2].join(&beta_component);
        let nested = outer.join(&component);
        engine.update_protection_policy(storage_engine::ProtectionPolicyUpdate {
            policy_id: &policy, repository_id: &repository.repository_id, path_prefix: "",
            target_copies: 3, expected_generation: 1, principal: "admin",
            operation_id: OperationId::generate().unwrap(),
        }).unwrap();
        let third_workspace = engine.create_workspace(&repository.main_branch_id, "admin")
            .unwrap();
        engine.stage_bytes(&third_workspace.workspace_id, "one.txt",
            b"new version copied A to B to C").unwrap();
        let third_request = request(&third_workspace, "extend exact B source to actual C");
        let before_c = storage_engine::canonical_json(engine.state()).unwrap();
        let journal_before = journals();
        fs::create_dir(&outer).unwrap();
        fs::set_permissions(&outer, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(outer.join("unknown-source"), b"not the published Beta directory").unwrap();
        assert_eq!(engine.commit_workspace(third_request.clone()).unwrap_err().code(),
            ErrorCode::ContainerLayoutInvalid);
        assert_eq!(storage_engine::canonical_json(engine.state()).unwrap(), before_c);
        assert_eq!(journals(), journal_before);
        assert_eq!(engine.state().file_containers[&file], record);
        assert_eq!(fs::read(outer.join("unknown-source")).unwrap(),
            b"not the published Beta directory");
        fs::remove_file(outer.join("unknown-source")).unwrap();
        fs::remove_dir(&outer).unwrap();
        if let Some(point) = cut {
            engine.inject_fault_once(point);
            let error = engine.commit_workspace(third_request.clone()).unwrap_err();
            assert_eq!(error.code(), ErrorCode::CommitInDoubt);
            let mut source: Option<&(dyn std::error::Error + 'static)> = Some(&error);
            let mut exact = false;
            while let Some(cause) = source {
                if let Some(injected) = cause.downcast_ref::<storage_engine::Error>() {
                    if injected.code() == ErrorCode::InjectedFailure {
                        exact = injected.message().as_ref()
                            == format!("interrupted at {point}").as_str();
                        break;
                    }
                }
                source = cause.source();
            }
            assert!(exact, "missing actual C {point}: {error}");
        } else {
            assert_eq!(engine.commit_workspace(third_request.clone()).unwrap().commit_state,
                CommitState::Published);
        }
        drop(engine);
        if cut == Some("after_accept_1") {
            assert!(!outer.exists());
            let preparations = fs::read_dir(roots[2].join("zxs-data")).unwrap()
                .map(|entry| entry.unwrap().path())
                .filter(|path| path.file_name().unwrap().to_str().unwrap()
                    .starts_with("source-prepare-"))
                .collect::<Vec<_>>();
            assert_eq!(preparations.len(), 2);
            // Both the direct-source and inherited-source preparations are
            // identity-bound independently, even while neither name is public.
            for (index, stage) in preparations.iter().enumerate() {
                let parked = temporary.path().join(format!("real-C-prefix-stage-{index}"));
                let real = fs::metadata(stage).unwrap();
                fs::rename(stage, &parked).unwrap();
                copy_directory(&parked, stage);
                assert_ne!((fs::metadata(stage).unwrap().dev(), fs::metadata(stage).unwrap().ino()),
                    (real.dev(), real.ino()));
                let journal_before = journals();
                assert_eq!(Engine::open(&system).err().unwrap().code(),
                    ErrorCode::ContainerLayoutInvalid);
                assert_eq!(journals(), journal_before);
                assert!(!outer.exists());
                fs::remove_dir_all(stage).unwrap();
                fs::rename(&parked, stage).unwrap();
            }
        }
        engine = Engine::open(&system).unwrap();
        assert!(!engine.is_read_only());
        let third_receipt = engine.commit_workspace(third_request.clone()).unwrap();
        assert_eq!(third_receipt.commit_state, CommitState::Published);
        let latest = engine.list_tree(&repository.main_branch_id).unwrap().into_iter()
            .find(|entry| entry.path.as_str() == "one.txt").unwrap();
        assert_eq!(latest.file_id, first.file_id);
        assert_ne!(latest.version_id, first.version_id);
        let extended = engine.state().file_containers[&file].clone();
        assert_eq!(extended.owner_disk, owner);
        assert_eq!(extended.replicas[&owner], record.replicas[&owner]);
        assert_eq!(extended.replicas[&backup], record.replicas[&backup]);
        let third_prefixes = extended.replicas[&third].prefixes.clone();
        assert_eq!(third_prefixes.iter().map(|prefix| prefix.source_disk).collect::<Vec<_>>(),
            vec![backup, owner]);
        for (bound, path) in third_prefixes.iter().zip([&outer, &nested]) {
            let observed = fs::metadata(path).unwrap();
            assert_eq!((bound.entity.device(), bound.entity.inode()),
                (observed.dev(), observed.ino()));
            assert!(!roots[2].join("zxs-data")
                .join(format!("source-prepare-{}", bound.preparation)).exists());
        }
        assert!(!roots[2].join(&component).exists());
        assert!(!roots[2].join("common-data").join(&beta_component).exists());
        assert!(!roots[1].join("common-data").join(&component).exists());
        let latest_version = &engine.state().versions[&latest.version_id];
        let latest_leaf = format!("{}-one.txt", latest_version.storage_id);
        assert_eq!(fs::read(nested.join("common-data/one.txt/versions").join(&latest_leaf)).unwrap(),
            b"new version copied A to B to C");
        assert!(nested.join("zxs-data/common-data").join(&repository.history_tree_id)
            .join("versions").join(&latest.version_id).join("version.json").is_file());
        assert!(!outer.join("zxs-data/common-data").join(&repository.history_tree_id).exists());
        assert_eq!(body(&engine, &first.version_id), b"owner and actual B replica");
        // Only C has a readable BODY here; the normal authenticated reader must
        // still use the exact final two-level prefix, not a byte-matching copy.
        let upstream_bodies = [
            roots[0].join("common-data/one.txt/versions").join(&latest_leaf),
            prefix.join("common-data/one.txt/versions").join(&latest_leaf),
        ];
        let parked_bodies = [temporary.path().join("A-latest-body"),
            temporary.path().join("B-latest-body")];
        for (source, parked) in upstream_bodies.iter().zip(&parked_bodies) {
            fs::rename(source, parked).unwrap();
        }
        assert_eq!(body(&engine, &latest.version_id), b"new version copied A to B to C");
        for (source, parked) in upstream_bodies.iter().zip(&parked_bodies) {
            fs::rename(parked, source).unwrap();
        }
        // A later actual file must retain both published C prefix entities.
        let fourth = engine.create_workspace(&repository.main_branch_id, "admin").unwrap();
        engine.stage_bytes(&fourth.workspace_id, "three.txt", b"shared exact C ancestry")
            .unwrap();
        let fourth_request = request(&fourth, "reuse both published C source identities");
        engine.commit_workspace(fourth_request.clone()).unwrap();
        let fourth_file = engine.list_tree(&repository.main_branch_id).unwrap().into_iter()
            .find(|entry| entry.path.as_str() == "three.txt").unwrap();
        let fourth_id = fourth_file.file_id.parse::<storage_contract::FileId>().unwrap();
        assert_eq!(engine.state().file_containers[&fourth_id].replicas[&third].prefixes,
            third_prefixes);
        drop(engine);
        // Replacing either inherited A or direct B on C is rejected, even with
        // every byte copied. Restoring the actual entity permits normal recovery.
        for (index, path) in [&nested, &outer].into_iter().enumerate() {
            let parked = temporary.path().join(format!("real-C-published-prefix-{index}"));
            fs::rename(path, &parked).unwrap();
            copy_directory(&parked, path);
            let journal_before = journals();
            assert_eq!(Engine::open(&system).err().unwrap().code(),
                ErrorCode::ContainerLayoutInvalid);
            assert_eq!(journals(), journal_before);
            fs::remove_dir_all(path).unwrap();
            fs::rename(&parked, path).unwrap();
        }
        engine = Engine::open(&system).unwrap();
        let before_retry = storage_engine::canonical_json(engine.state()).unwrap();
        engine.commit_workspace(third_request).unwrap();
        engine.commit_workspace(fourth_request).unwrap();
        assert_eq!(storage_engine::canonical_json(engine.state()).unwrap(), before_retry);
        assert_eq!(engine.state().file_containers[&file], extended);
        assert_eq!(body(&engine, &latest.version_id), b"new version copied A to B to C");
        assert_eq!(body(&engine, &first.version_id), b"owner and actual B replica");


        // C is already active and cold-admitted. V3 is a new ordinary save,
        // separate from C's introduction above; it must advance the actual FILE
        // branch and preserve V1/V2 and the container's independently edited tags.
        use storage_contract::file_container_tags::{
            Context, ReadRequest, ReplaceCommand, Tag, TagSet,
        };
        let tag_read = ReadRequest {
            file_id: file,
            context: Context {
                repository_id: repository.repository_id.parse().unwrap(),
                path: name_policy::PortablePath::parse("one.txt").unwrap(),
            },
        };
        let tag_command = ReplaceCommand {
            file_id: file, context: tag_read.context.clone(),
            expected_generation: engine.file_container_tags(&tag_read).unwrap().generation,
            tags: TagSet::new([Tag::parse("C-active".into()).unwrap()]).unwrap(),
        };
        engine.replace_container_tags(&tag_command, &"admin".parse().unwrap(),
            OperationId::generate().unwrap()).unwrap();
        let expected_tags = engine.state().container_tags[&file].clone();
        let selected_branch = engine.state().file_selections[&first.file_id].clone();
        assert_eq!(engine.file_branch_head(&selected_branch).unwrap(), latest.version_id);
        let v3_workspace = engine.create_workspace(&repository.main_branch_id, "admin").unwrap();
        engine.stage_bytes(&v3_workspace.workspace_id, "one.txt", b"V3 after C is active")
            .unwrap();
        let v3_request = request(&v3_workspace, "ordinary third version with active C");
        assert_eq!(engine.commit_workspace(v3_request.clone()).unwrap().commit_state,
            CommitState::Published);
        let v3 = engine.list_tree(&repository.main_branch_id).unwrap().into_iter()
            .find(|entry| entry.path.as_str() == "one.txt").unwrap();
        assert_eq!(v3.file_id, first.file_id);
        assert_ne!(v3.version_id, latest.version_id);
        assert_eq!(engine.file_branch_head(&selected_branch).unwrap(), v3.version_id);
        assert_eq!(engine.file_node_parents(&first.file_id, &v3.version_id).unwrap(),
            vec![latest.version_id.clone()]);
        assert_eq!(engine.state().file_containers[&file], extended);
        assert_eq!(engine.state().container_tags[&file], expected_tags);
        let v3_leaf = format!("{}-one.txt", engine.state().versions[&v3.version_id].storage_id);
        let c_body = nested.join("common-data/one.txt/versions").join(&v3_leaf);
        let a_body = roots[0].join("common-data/one.txt/versions").join(&v3_leaf);
        let b_body = prefix.join("common-data/one.txt/versions").join(&v3_leaf);
        drop(engine);
        engine = Engine::open(&system).unwrap();
        assert_eq!(engine.file_branch_head(&selected_branch).unwrap(), v3.version_id);
        assert_eq!(engine.file_container_tags(&tag_read).unwrap().tags, tag_command.tags);
        let c_rows = file_database_rows(&nested.join("common-data/one.txt/.zxs-file.sqlite"));
        for source in [&prefix, &nested] {
            let version = source.join("zxs-data/common-data").join(&repository.history_tree_id)
                .join("versions").join(&v3.version_id);
            assert!(version.join("version.json").is_file());
        }
        for disk in [&roots[1], &roots[2]] {
            assert!(!disk.join("zxs-data").join(&repository.history_tree_id)
                .join("versions").join(&v3.version_id).exists());
        }
        for container in [roots[0].join("common-data/one.txt"), prefix.join("common-data/one.txt")] {
            assert_eq!(file_database_rows(&container.join(".zxs-file.sqlite")), c_rows);
        }
        assert_eq!(c_rows[&("file".into(), first.file_id.clone())]["container_tags"],
            json_codec::to_value(&expected_tags).unwrap());
        assert_eq!(c_rows[&("branch".into(), selected_branch.clone())]["head_node_id"],
            v3.version_id);
        for version in [&first.version_id, &latest.version_id, &v3.version_id] {
            let row = &c_rows[&("version".into(), version.clone())];
            assert_eq!(row["version"], json_codec::to_value(&engine.state().versions[version])
                .unwrap());
            assert_eq!(row["parent_node_ids"], json_codec::to_value(
                engine.file_node_parents(&first.file_id, version).unwrap()).unwrap());
        }
        let parked_a = temporary.path().join("V3-unavailable-A");
        let parked_b = temporary.path().join("V3-unavailable-B");
        fs::rename(&a_body, &parked_a).unwrap();
        fs::rename(&b_body, &parked_b).unwrap();
        let expected = fs::metadata(&c_body).unwrap();
        let mut opened = engine.open_content(&v3.version_id).unwrap();
        let observed = opened.file.metadata().unwrap();
        assert_eq!((observed.dev(), observed.ino()), (expected.dev(), expected.ino()));
        let mut bytes = Vec::new();
        opened.file.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"V3 after C is active");
        drop(opened);
        fs::rename(&parked_a, &a_body).unwrap();
        fs::rename(&parked_b, &b_body).unwrap();
        let before_retry = storage_engine::canonical_json(engine.state()).unwrap();
        engine.commit_workspace(v3_request).unwrap();
        assert_eq!(storage_engine::canonical_json(engine.state()).unwrap(), before_retry);
        assert_eq!(body(&engine, &first.version_id), b"owner and actual B replica");
        assert_eq!(body(&engine, &latest.version_id), b"new version copied A to B to C");
    }
}
