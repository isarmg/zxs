//! Committed owner transfers retain the original history and copy each FULL
//! into one independent destination container under the original operation.
use super::*;
use storage_contract::{
    file_relocation::{NamespaceEntryId, OBJECT_MOVE_KIND, ObjectMoveFact, ObjectMoveRequest},
    history_window::HistoryRepositoryId,
};

struct Body {
    path: PathBuf,
    inode: u64,
    bytes: Vec<u8>,
}

struct RetainedVersion {
    version: FileVersion,
    copies: Vec<Body>,
}

fn save(engine: &mut Engine, repository: &Repository, files: &[(&str, &[u8])]) {
    let workspace = engine
        .create_workspace(&repository.main_branch_id, "alice")
        .unwrap();
    for (path, bytes) in files {
        engine
            .stage_bytes(&workspace.workspace_id, path, bytes)
            .unwrap();
    }
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: storage_contract::OperationId::generate().unwrap(),
            principal: "alice".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "move recovery source".into(),
            request_digest: None,
        })
        .unwrap();
}

fn fixture() -> (tempfile::TempDir, Engine, Repository, DirectoryBranch) {
    let temporary = tempfile::tempdir().unwrap();
    let config = Config::development([
        temporary.path().join("p"),
        temporary.path().join("b"),
        temporary.path().join("c"),
    ])
    .unwrap();
    let mut engine = Engine::initialize(&temporary.path().join("state"), config).unwrap();
    let repository = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("移动恢复").unwrap(),
            "P01",
            3,
            "alice",
            storage_contract::OperationId::generate().unwrap(),
        )
        .unwrap();
    save(
        &mut engine,
        &repository,
        &[("a/f.txt", b"frozen F1"), ("b/keep.txt", b"untouched")],
    );
    for path in ["a", "b"] {
        crate::directory_git_fixture::checkpoint_as(&mut engine, &repository, path, "alice");
    }
    let retained = engine
        .create_directory_branches(
            &repository.repository_id,
            Some(&name_policy::LookupPath::parse("a").unwrap()),
            &storage_contract::BranchName::parse("retained").unwrap(),
            "alice",
            storage_contract::OperationId::generate().unwrap(),
        )
        .unwrap()
        .result["directory_branch_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let retained = engine.state.directory_branches[&retained].clone();
    // The exported history includes both the frozen commit and a later save.
    save(&mut engine, &repository, &[("a/f.txt", b"saved F2")]);
    (temporary, engine, repository, retained)
}

fn request(repository: &Repository, source: &Entry) -> ObjectMoveRequest {
    ObjectMoveRequest {
        source_repository_id: HistoryRepositoryId::parse(&repository.repository_id).unwrap(),
        source_entry_id: NamespaceEntryId::parse(&source.entry_id).unwrap(),
        source_path: name_policy::LookupPath::parse("a/f.txt").unwrap(),
        target_slot_id: repository.primary_slot_id.clone(),
        target_repository_id: Some(
            HistoryRepositoryId::parse(&repository.repository_id).unwrap(),
        ),
        target_path: name_policy::PortablePath::parse("b/f.txt").unwrap(),
        replace: false,
        target_entry_id: None,
    }
}

fn capture_versions(engine: &Engine, file: &Entry) -> Vec<RetainedVersion> {
    let versions = engine
        .state
        .versions
        .values()
        .filter(|version| version.file_id == file.file_id)
        .map(|version| RetainedVersion {
            version: version.clone(),
            copies: engine
                .config
                .disks
                .iter()
                .map(|disk| {
                    let directory = disk
                        .root
                        .join("histories")
                        .join(&version.history_tree_id)
                        .join("versions")
                        .join(&version.version_id);
                    let path = content_path(&directory, version).unwrap();
                    Body {
                        inode: fs::metadata(&path).unwrap().ino(),
                        bytes: fs::read(&path).unwrap(),
                        path,
                    }
                })
                .collect(),
        })
        .collect::<Vec<_>>();
    assert_eq!(
        versions
            .iter()
            .map(|version| version.copies[0].bytes.as_slice())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([b"frozen F1".as_slice(), b"saved F2".as_slice()])
    );
    versions
}

fn assert_views(
    engine: &Engine,
    previous: &State,
    repository: &Repository,
    retained: &DirectoryBranch,
    source: &Entry,
) -> Entry {
    let tree = engine.namespace_tree(&repository.main_branch_id).unwrap();
    assert!(tree.get("a/f.txt").is_none());
    let target = tree.get("b/f.txt").unwrap().clone();
    assert_ne!(target.entry_id, source.entry_id);
    assert_ne!(target.file_id, source.file_id);
    assert_ne!(target.version_id, source.version_id);
    assert_eq!(engine.state.file_locations[&source.file_id], "a/f.txt");
    assert_eq!(engine.state.file_locations[&target.file_id], "b/f.txt");
    assert!(engine.file_deletion(&source.file_id).is_none());
    for path in ["a", "b"] {
        let status = engine
            .directory_git_status(&repository.repository_id, path)
            .unwrap();
        let branch =
            &engine.state.directory_branches[status.directory_branch_id.as_ref().unwrap()];
        assert!(!status.staged && status.unstaged && !status.needs_restage);
        if path == "a" {
            assert!(
                !branch
                    .entries
                    .iter()
                    .any(|entry| entry.file_id == source.file_id)
            );
        } else {
            assert!(branch.entries.contains(&target));
            let keep = branch
                .entries
                .iter()
                .find(|entry| entry.path() == "b/keep.txt")
                .unwrap();
            assert!(
                previous.directory_branches[&branch.directory_branch_id]
                    .entries
                    .contains(keep)
            );
        }
    }
    assert_eq!(
        engine.state.directory_branches[&retained.directory_branch_id],
        *retained
    );
    assert_eq!(
        engine.state.directory_git_repositories,
        previous.directory_git_repositories
    );
    for (id, tree) in &previous.directory_trees {
        assert_eq!(&engine.state.directory_trees[id], tree);
    }
    target
}

fn assert_independent_body(
    disk: &DiskConfig,
    original: &RetainedVersion,
    target: &FileVersion,
    body: &Body,
) {
    let history = disk.root.join("histories").join(&target.history_tree_id);
    let source_directory = history.join("versions").join(&original.version.version_id);
    let target_directory = history.join("versions").join(&target.version_id);
    let source_body = content_path(&source_directory, &original.version).unwrap();
    let target_body = content_path(&target_directory, target).unwrap();
    assert_eq!(source_body, body.path);
    assert_eq!(fs::metadata(&source_body).unwrap().ino(), body.inode);
    assert_ne!(fs::metadata(&target_body).unwrap().ino(), body.inode);
    assert!(
        target_body
            .to_string_lossy()
            .contains("/namespace/b/f.txt/versions/")
    );
    for (path, version, location) in [
        (&source_body, &original.version, "a/f.txt"),
        (&target_body, target, "b/f.txt"),
    ] {
        assert_eq!(fs::metadata(path).unwrap().nlink(), 1);
        assert_eq!(fs::read(path).unwrap(), body.bytes);
        let named = container(&history, &version.file_id).unwrap().unwrap();
        assert_eq!(path.parent().unwrap().parent().unwrap(), named);
        assert_eq!(fs::read_dir(named.join("versions")).unwrap().count(), 2);
        let cache: json_codec::Value = read_json(&named.join("container.json")).unwrap();
        assert_eq!(cache["file_id"], version.file_id);
        assert_eq!(cache["location"], location);
    }
}

fn assert_copied_history(
    engine: &Engine,
    receipt: &OperationResult,
    target: &Entry,
    originals: &[RetainedVersion],
) {
    let result = crate::business::object_move::read_result(receipt).unwrap();
    assert_eq!(result.version_mapping.len(), originals.len());
    assert_eq!(
        engine
            .state
            .versions
            .values()
            .filter(|version| version.file_id == target.file_id)
            .count(),
        originals.len()
    );
    for original in originals {
        assert_eq!(original.copies.len(), 3);
        let mapped = &result.version_mapping[&original.version.version_id];
        let copied = &engine.state.versions[mapped];
        assert_eq!(copied.file_id, target.file_id);
        assert_ne!(copied.version_id, original.version.version_id);
        assert_eq!(copied.history_tree_id, original.version.history_tree_id);
        assert_eq!(copied.content_hash, original.version.content_hash);
        assert_eq!(copied.logical_size, original.version.logical_size);
        assert_eq!(
            engine.state.versions[&original.version.version_id],
            original.version
        );
        for (disk, body) in engine.config.disks.iter().zip(&original.copies) {
            assert_independent_body(disk, original, copied, body);
        }
    }
}

fn assert_receipt(receipt: &OperationResult, request: &ObjectMoveRequest, target: &Entry) {
    assert_eq!(receipt.kind, OBJECT_MOVE_KIND);
    assert_eq!(receipt.commit_state, storage_contract::CommitState::Published);
    assert_eq!(receipt.request_digest, request.digest("alice").unwrap());
    assert_eq!(
        receipt.protection_at_publish,
        storage_contract::ProtectionAtPublish::TargetMet
    );
    assert_eq!(
        receipt.current_health,
        storage_contract::CurrentHealth::Healthy
    );
    let movement: ObjectMoveFact =
        json_codec::from_value(receipt.result["movement"].clone()).unwrap();
    assert_eq!(movement.request, *request);
    assert_eq!(movement.principal, "alice");
    assert_eq!(movement.target_entry_id.as_str(), target.entry_id);
}

#[test]
fn decided_cross_directory_move_recovers_both_views_and_independent_containers() {
    for fault in ["after_finalize", "after_container_names"] {
        let (temporary, mut engine, repository, retained) = fixture();
        let source = engine
            .namespace_tree(&repository.main_branch_id)
            .unwrap()
            .get("a/f.txt")
            .unwrap()
            .clone();
        let originals = capture_versions(&engine, &source);
        assert_eq!(originals.len(), 2);
        let request = request(&repository, &source);
        let operation = storage_contract::OperationId::generate().unwrap();
        let previous = engine.state().clone();
        engine.inject_fault_once(fault);
        assert_eq!(
            engine
                .move_object(&request, "alice", operation)
                .unwrap_err()
                .code(),
            storage_contract::ErrorCode::InDoubt,
            "{fault}"
        );
        assert_eq!(engine.state(), &previous, "{fault}");
        drop(engine);
        let mut engine = Engine::open(&temporary.path().join("state")).unwrap();
        let recovered = engine.operation(operation).unwrap();
        let target = assert_views(&engine, &previous, &repository, &retained, &source);
        assert_receipt(&recovered, &request, &target);
        let result = crate::business::object_move::read_result(&recovered).unwrap();
        assert_eq!(target.version_id, result.version_mapping[&source.version_id]);
        assert_copied_history(&engine, &recovered, &target, &originals);
        let published = engine.state().clone();
        let target_bodies = capture_versions(&engine, &target);
        // Retry the original request after its source occurrence has vanished.
        assert_eq!(
            engine.move_object(&request, "alice", operation).unwrap(),
            recovered
        );
        assert_eq!(engine.state(), &published);
        assert_eq!(engine.state.operations.len(), previous.operations.len() + 1);
        assert_eq!(
            engine.state.versions.len(),
            previous.versions.len() + originals.len()
        );
        drop(engine);
        let engine = Engine::open(&temporary.path().join("state")).unwrap();
        assert_eq!(engine.operation(operation).unwrap(), recovered);
        assert_eq!(engine.state(), &published);
        assert_copied_history(&engine, &recovered, &target, &originals);
        for version in target_bodies {
            for body in version.copies {
                assert_eq!(fs::metadata(body.path).unwrap().ino(), body.inode);
            }
        }
    }
}
