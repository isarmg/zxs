//! Actual ordinary namespace transactions, including their historical replay.
use crate::{CommitRequest, Config, Engine, FileVersion, NamespaceOperation, NamespaceOperationKind};
use std::collections::BTreeMap;
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use storage_contract::{ContentHash, DiskId, FileId, OperationId, VersionId};

#[derive(Debug, Eq, PartialEq)]
struct BodyIdentity {
    container: (u64, u64),
    body: (u64, u64),
    bytes: Vec<u8>,
}

fn save(engine: &mut Engine, branch: &str, name: &str, bytes: &[u8]) {
    let workspace = engine.create_workspace(branch, "alice").unwrap();
    engine.stage_bytes(&workspace.workspace_id, name, bytes).unwrap();
    engine.commit_workspace(CommitRequest {
        workspace_id: workspace.workspace_id,
        operation_id: OperationId::generate().unwrap(),
        principal: "alice".into(),
        expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token,
        message: "content".into(),
        request_digest: None,
    }).unwrap();
}

fn fixture(copies: u32) -> (tempfile::TempDir, PathBuf, Engine, String, String, FileId) {
    let temporary = tempfile::tempdir().unwrap();
    let system = temporary.path().join("system");
    let config = Config::development([
        (name_policy::Name::parse("Primary").unwrap(), temporary.path().join("a")),
        (name_policy::Name::parse("Backup").unwrap(), temporary.path().join("b")),
        (name_policy::Name::parse("Archive").unwrap(), temporary.path().join("c")),
    ]).unwrap();
    let slot = config.disks[0].slot_id.clone();
    let mut engine = Engine::initialize(&system, config).unwrap();
    let repository = engine.create_disk_root(&slot, "alice", OperationId::generate().unwrap())
        .unwrap();
    if copies != 1 {
        let policy = crate::new_id("policy").unwrap();
        engine.update_protection_policy(crate::ProtectionPolicyUpdate {
            policy_id: &policy,
            repository_id: &repository.repository_id,
            path_prefix: "",
            target_copies: copies,
            expected_generation: 0,
            principal: "alice",
            operation_id: OperationId::generate().unwrap(),
        }).unwrap();
    }
    save(&mut engine, &repository.main_branch_id, "file.txt", b"first immutable body");
    save(&mut engine, &repository.main_branch_id, "file.txt", b"second immutable body");
    let entry = engine.list_tree(&repository.main_branch_id).unwrap().remove(0);
    let file = entry.file_id.parse().unwrap();
    use storage_contract::file_container_tags::{Context, ReplaceCommand, Tag, TagSet};
    engine.replace_container_tags(&ReplaceCommand {
        file_id: file,
        context: Context {
            repository_id: repository.repository_id.parse().unwrap(),
            path: name_policy::PortablePath::parse("file.txt").unwrap(),
        },
        expected_generation: storage_contract::Generation::new(0),
        tags: TagSet::new([Tag::parse("retained label".into()).unwrap()]).unwrap(),
    }, &"alice".parse().unwrap(), OperationId::generate().unwrap()).unwrap();
    (temporary, system, engine, repository.repository_id, repository.main_branch_id, file)
}

fn rename(source: &str, target: &str) -> NamespaceOperation {
    NamespaceOperation::parse(NamespaceOperationKind::Rename, source, Some(target)).unwrap()
}

fn versions(engine: &Engine, file: FileId) -> Vec<FileVersion> {
    engine.state().versions.values().filter(|version| version.file_id == file.to_string())
        .cloned().collect()
}

fn observe(
    engine: &Engine, file: FileId, name: &str,
) -> BTreeMap<(DiskId, VersionId), BodyIdentity> {
    let mut found = BTreeMap::new();
    let replicas = &engine.state().file_containers[&file].replicas;
    for disk in &engine.config.disks {
        if !replicas.contains_key(&disk.disk_id.parse::<DiskId>().unwrap()) {
            continue;
        }
        let scope = crate::engine::fresh_layout::containers::observe_replica(
            &engine.config, &engine.disk_resources, engine.state(), file, disk,
        ).unwrap();
        assert_eq!(scope.name().as_str(), name);
        let container = scope.container().unwrap();
        assert_eq!(container.path().file_name().unwrap().to_str(), Some(name));
        let entity = container.directory_identity(container.path()).unwrap();
        for version in versions(engine, file) {
            let path = scope.body_path(&version).unwrap();
            let expected = format!("{}-{}", version.storage_id, version.creation_name);
            assert_eq!(path.file_name().unwrap().to_str(), Some(expected.as_str()));
            if name != version.creation_name {
                assert!(!path.with_file_name(format!("{}-{name}",version.storage_id)).exists());
            }
            let mut body = container.open_read_regular(&path).unwrap();
            let metadata = body.metadata().unwrap();
            let mut bytes = Vec::new();
            body.read_to_end(&mut bytes).unwrap();
            assert_eq!(ContentHash::from_digest(blake3::hash(&bytes)), version.content_hash);
            found.insert((disk.disk_id.parse().unwrap(), version.version_id.parse().unwrap()),
                BodyIdentity {
                    container: (entity.device, entity.inode),
                    body: (metadata.dev(), metadata.ino()), bytes,
                });
        }
    }
    assert_eq!(found.len(), replicas.len() * versions(engine, file).len());
    found
}

#[test]
fn same_parent_rename_preserves_full_history_and_each_replica() {
    let (_temporary, system, mut engine, repository, branch, file) = fixture(1);
    let original = observe(&engine, file, "file.txt");
    let before_versions = engine.state().versions.clone();
    let before_tags = engine.state().container_tags.clone();
    let file_key = file.to_string();
    let before_heads = engine.state().branches.values()
        .filter(|branch| branch.file_id.as_deref() == Some(file_key.as_str()))
        .map(|branch| (branch.branch_id.clone(), branch.clone())).collect::<BTreeMap<_, _>>();
    let operation = OperationId::generate().unwrap();
    let result = engine.file_namespace(
        &repository, rename("file.txt", "renamed.txt"), "alice", operation,
    ).unwrap();
    assert_eq!(engine.state().versions, before_versions);
    assert_eq!(engine.state().container_tags, before_tags);
    for (id, before) in &before_heads {
        let after = &engine.state().branches[id];
        assert_eq!(after.head_commit_id, before.head_commit_id);
        assert_eq!(after.file_node_id, before.file_node_id);
        assert_eq!(after.generation, before.generation);
    }
    let facts = crate::persist::path_changes::read(&result).unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].file_id, file);
    assert_eq!(facts[0].before_version_id, facts[0].after_version_id);
    assert_eq!(observe(&engine, file, "renamed.txt"), original);
    drop(engine);
    engine = Engine::open(&system).unwrap();
    assert!(!engine.is_read_only());
    let state = engine.state().clone();
    assert_eq!(engine.file_namespace(
        &repository, rename("file.txt", "renamed.txt"), "alice", operation,
    ).unwrap(), result);
    assert_eq!(engine.state(), &state);
    assert_eq!(observe(&engine, file, "renamed.txt"), original);

    // A legal successor adds new replica occurrences after the first rename.
    // The lineage must carry A, not reject B/C or backdate them into A's history.
    engine.configure_automatic_backups(crate::AutomaticBackupConfigurationRequest {
        repository_id: repository.parse().unwrap(), path: None, target_copies: 3,
        expected_generation: storage_contract::Generation::new(0),
        principal: &"alice".parse().unwrap(), operation_id: OperationId::generate().unwrap(),
    }).unwrap();
    assert_eq!(engine.state().versions, before_versions);
    let expanded = observe(&engine, file, "renamed.txt");
    assert_eq!(expanded.len(), 6);
    for (key, identity) in &original {
        assert_eq!(expanded.get(key), Some(identity));
    }
    let original = expanded;
    drop(engine);
    engine = Engine::open(&system).unwrap();
    assert_eq!(observe(&engine, file, "renamed.txt"), original);

    engine.file_namespace(&repository, rename("renamed.txt", "final.txt"), "alice",
        OperationId::generate().unwrap()).unwrap();
    // An unrelated new FileId may now occupy the first spelling. Historical
    // recovery must follow the proved terminal inode, not try that old name.
    save(&mut engine, &branch, "file.txt", b"unrelated new identity");
    let new_file = engine.list_tree(&branch).unwrap().into_iter()
        .find(|entry| entry.name() == "file.txt").unwrap().file_id.parse::<FileId>().unwrap();
    assert_ne!(new_file, file);
    drop(engine);
    engine = Engine::open(&system).unwrap();
    assert_eq!(observe(&engine, file, "final.txt"), original);
    assert_eq!(versions(&engine, file).len(), 2);
    assert_eq!(engine.operation(operation).unwrap(), result);
    for version in versions(&engine, file) {
        let mut body = engine.open_content(&version.version_id).unwrap();
        let mut bytes = Vec::new();
        body.file.read_to_end(&mut bytes).unwrap();
        assert_eq!(ContentHash::from_digest(blake3::hash(&bytes)), version.content_hash);
    }
}

#[test]
fn new_main_name_creates_only_its_new_body_and_non_main_actions_never_rename_earlier_versions() {
    let (temp, system, mut engine, repository, main, file) = fixture(1);
    let before = observe(&engine,file,"file.txt");
    let original = versions(&engine,file);
    assert_eq!(original.len(),2);
    engine.file_namespace(&repository,rename("file.txt","renamed.txt"),"alice",
        OperationId::generate().unwrap()).unwrap();
    assert_eq!(observe(&engine,file,"renamed.txt"),before);
    save(&mut engine,&main,"renamed.txt",b"only this version uses the new name");
    let after = observe(&engine,file,"renamed.txt");
    for (k,v) in &before { assert_eq!(after.get(k),Some(v)); }
    let versions_after = versions(&engine,file);
    assert_eq!(versions_after.len(),3);
    assert_eq!(versions_after.iter().filter(|v|v.creation_name=="file.txt").count(),2);
    assert_eq!(versions_after.iter().filter(|v|v.creation_name=="renamed.txt").count(),1);
    let view = engine.metadata_view();
    assert_eq!(view.installed_version_name(&original[0]).unwrap(),
        format!("{}-file.txt", original[0].storage_id));
    let mut forged = original[0].clone();
    forged.creation_name = "renamed.txt".into();
    assert_eq!(view.installed_version_name(&forged).unwrap_err().code(),
        storage_contract::ErrorCode::VersionNotFound);
    drop(view);

    let key=file.to_string();
    let branch=engine.create_file_branch(&key,&original[0].version_id,
        &storage_contract::BranchName::parse("review").unwrap(),"alice",OperationId::generate().unwrap()).unwrap();
    let branch=branch.result["branch_id"].as_str().unwrap().to_owned();
    engine.rename_file_branch(&key,&branch,&storage_contract::BranchName::parse("other").unwrap(),
        "alice",OperationId::generate().unwrap()).unwrap();
    engine.select_file_branch(&key,&branch,"alice",OperationId::generate().unwrap()).unwrap();
    let session=engine.create_workspace(&branch,"alice").unwrap();
    let denied=engine.namespace_operation(&session.workspace_id,rename("renamed.txt","not-the-main.txt")).unwrap_err();
    assert_eq!(denied.code(),storage_contract::ErrorCode::FileBranchScope);
    engine.abort_workspace(&session.workspace_id,"alice").unwrap();
    assert_eq!(observe(&engine,file,"renamed.txt"),after);
    assert_eq!(versions(&engine,file).len(),3);
    drop(engine);
    let engine=Engine::open(&system).unwrap();
    assert_eq!(observe(&engine,file,"renamed.txt"),after);
    let package=temp.path().join("original-names-export");
    engine.export_history_package(&original[0].history_tree_id,&package).unwrap();
    crate::offline_check_history_package(&package).unwrap();
    for version in versions_after {
        let dir=package.join("versions").join(&version.version_id).join("content");
        let mut reader=engine.open_content(&version.version_id).unwrap();
        let mut bytes=Vec::new();
        reader.file.read_to_end(&mut bytes).unwrap();
        assert_eq!(std::fs::read(dir.join(&version.creation_name)).unwrap(),bytes);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(),1);
        assert_eq!(ContentHash::from_digest(blake3::hash(&bytes)),version.content_hash);
    }
}

#[test]
fn same_parent_rename_refuses_collisions_before_decision_without_overwrite() {
    let (_temporary, _system, mut engine, repository, _branch, file) = fixture(3);
    let original = observe(&engine, file, "file.txt");
    let disk = &engine.config.disks[1];
    let scope = crate::engine::fresh_layout::containers::observe_replica(
        &engine.config, &engine.disk_resources, engine.state(), file, disk,
    ).unwrap();
    let collision = scope.container().unwrap().path().parent().unwrap().join("taken.txt");
    std::fs::write(&collision, b"unrelated user entry").unwrap();
    let identity = std::fs::metadata(&collision).unwrap();
    drop(scope);
    let before = engine.state().clone();
    let snapshot = engine.snapshot.clone();
    let slot = engine.next_slot;
    let logs = engine.members().iter()
        .map(|disk| std::fs::read(crate::journal::log_path(disk)).unwrap()).collect::<Vec<_>>();
    let operation = OperationId::generate().unwrap();
    let error = engine.file_namespace(
        &repository, rename("file.txt", "taken.txt"), "alice", operation,
    ).unwrap_err();
    assert!(matches!(&error, crate::Error::FreshLayout(
        crate::engine::FreshLayoutError::ContainerIdentity,
    )), "{error:?}");
    assert!(!engine.blocked);
    assert_eq!(engine.state(), &before);
    assert_eq!(engine.snapshot, snapshot);
    assert_eq!(engine.next_slot, slot);
    assert_eq!(engine.members().iter()
        .map(|disk| std::fs::read(crate::journal::log_path(disk)).unwrap()).collect::<Vec<_>>(),
        logs);
    assert_eq!(std::fs::read(&collision).unwrap(), b"unrelated user entry");
    let after = std::fs::metadata(&collision).unwrap();
    assert_eq!((after.dev(), after.ino()), (identity.dev(), identity.ino()));
    assert_eq!(observe(&engine, file, "file.txt"), original);
    assert_eq!(engine.operation(operation).unwrap_err().code(),
        storage_contract::ErrorCode::OperationNotFound);
}

#[cfg(feature = "fault-injection")]
#[test]
fn same_parent_rename_replays_exact_finalized_cut_without_new_content() {
    for point in [
        "after_accept_1", "after_finalize",
        "ordinary-container-renamed", "after_container_names", "after_publication_1",
    ] {
        let (_temporary, system, mut engine, repository, _branch, file) = fixture(3);
        let original = observe(&engine, file, "file.txt");
        let source = crate::engine::fresh_layout::containers::observe_replica(
            &engine.config, &engine.disk_resources, engine.state(), file, &engine.config.disks[0],
        ).unwrap();
        let old_path = source.container().unwrap().path().to_owned();
        let new_path = old_path.parent().unwrap().join("renamed.txt");
        let retained = versions(&engine, file).into_iter().next().unwrap();
        let renamed_body = new_path.join(crate::persist::layout::VERSIONS.as_path()).join(
            format!("{}-{}", retained.storage_id, retained.creation_name),
        );
        drop(source);
        let versions = engine.state().versions.clone();
        let operation = OperationId::generate().unwrap();
        engine.inject_fault_once(point);
        let error = engine.file_namespace(
            &repository, rename("file.txt", "renamed.txt"), "alice", operation,
        ).unwrap_err();
        assert_eq!(error.code(), storage_contract::ErrorCode::InDoubt, "{point}: {error:?}");
        let crate::Error::Context { source, .. } = error else {
            panic!("decided failure must retain its concrete cause");
        };
        assert_eq!(source.code(), storage_contract::ErrorCode::InjectedFailure, "{point}");
        assert!(source.to_string().contains(point), "{point}: {source}");
        drop(engine);
        if point == "after_accept_1" {
            // Matching inode and bytes at the new spelling are not FINALIZE.
            // Do not adopt a physical move performed outside the decision tail.
            std::fs::rename(&old_path, &new_path).unwrap();
            assert!(Engine::open(&system).is_err());
            assert!(!old_path.exists());
            let moved = std::fs::metadata(&new_path).unwrap();
            assert!(original.values().any(|body| body.container == (moved.dev(), moved.ino())));
            std::fs::rename(&new_path, &old_path).unwrap();
        }
        if point == "after_container_names" {
            // Intent cleanup is complete but the decision is not Published yet.
            // With no rename left to do, cold recovery must still reject bad BODY.
            use std::io::{Seek, Write};
            use std::os::unix::fs::PermissionsExt;
            let original = std::fs::read(&renamed_body).unwrap();
            let metadata = std::fs::metadata(&renamed_body).unwrap();
            assert_eq!(metadata.mode() & 0o222, 0, "installed BODY is immutable");
            // Acquire a test-only corruption FD, then restore the original mode
            // before cold admission. Never replace the original BODY inode.
            std::fs::set_permissions(&renamed_body, std::fs::Permissions::from_mode(
                metadata.mode() | 0o200,
            )).unwrap();
            let mut body = std::fs::OpenOptions::new().write(true).open(&renamed_body).unwrap();
            body.set_permissions(metadata.permissions()).unwrap();
            let held = body.metadata().unwrap();
            assert_eq!((held.dev(), held.ino()), (metadata.dev(), metadata.ino()));
            let mut corrupt = original.clone();
            corrupt[0] ^= 0x40;
            body.write_all(&corrupt).unwrap();
            body.sync_all().unwrap();
            assert_eq!(std::fs::metadata(&renamed_body).unwrap().mode(), metadata.mode());
            assert!(Engine::open(&system).is_err());
            assert_eq!(std::fs::read(&renamed_body).unwrap(), corrupt);
            body.rewind().unwrap();
            body.write_all(&original).unwrap();
            body.sync_all().unwrap();
            let restored = std::fs::metadata(&renamed_body).unwrap();
            assert_eq!((restored.dev(), restored.ino()), (metadata.dev(), metadata.ino()));
            assert_eq!(restored.mode(), metadata.mode());
        }
        if point == "after_publication_1" {
            // Publication may need missing receipts repaired, but its already
            // completed namespace must not be silently re-created from an old name.
            std::fs::rename(&new_path, &old_path).unwrap();
            assert!(Engine::open(&system).is_err());
            assert!(!new_path.exists());
            std::fs::rename(&old_path, &new_path).unwrap();
        }
        engine = Engine::open(&system).unwrap();
        assert!(!engine.is_read_only(), "{point}");
        assert_eq!(engine.state().versions, versions, "{point}");
        assert_eq!(observe(&engine, file, "renamed.txt"), original, "{point}");
        let published = engine.operation(operation).unwrap();
        let sequence = engine.state().sequence;
        assert_eq!(engine.file_namespace(
            &repository, rename("file.txt", "renamed.txt"), "alice", operation,
        ).unwrap(), published, "{point}");
        assert_eq!(engine.state().sequence, sequence, "{point}");
        drop(engine);
        engine = Engine::open(&system).unwrap();
        assert_eq!(observe(&engine, file, "renamed.txt"), original, "{point}");
        assert_eq!(engine.operation(operation).unwrap(), published, "{point}");
    }
}
