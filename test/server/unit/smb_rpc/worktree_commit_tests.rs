//! Real native SMB dispatch followed by authenticated loopback HTTP publication.
//! This is not Unix FD transport, an external Samba client, or browser execution.
use super::*;
use name_policy::{PortablePath, SlotId};
use smb_protocol::protocol::FileMtime;
use std::io::Read;
use std::path::PathBuf;
use storage_contract::file_draft::FileWorkingState;
use storage_contract::snapshot_worktree::outcome::{WorktreeCommitOutcome, WorktreeCommitReceipt};
use storage_contract::snapshot_worktree::{CommitWorktree, SnapshotHandle, WorktreeStatus};

// Reuse the same real TCP helper as the ordinary native range acceptance case.
#[path = "incremental_http.rs"]
mod http;
use http::Http;

struct Fixture {
    root: tempfile::TempDir,
    system: PathBuf,
    disk: PathBuf,
    slot: SlotId,
    repository: RepositoryId,
    source: SnapshotHandle,
    first: WorktreeHandle,
    second: WorktreeHandle,
    first_mount: PortablePath,
    second_mount: PortablePath,
    password: String,
}

fn fixture() -> (Fixture, Direct) {
    let root = tempfile::tempdir().unwrap();
    let system = root.path().join("state");
    let disk = root.path().join("mounted-primary");
    let config = Config::development([(Name::parse("p").unwrap(), disk.clone())]).unwrap();
    let slot = SlotId::parse(&config.disks[0].slot_id).unwrap();
    let mut engine = Engine::initialize(&system, config).unwrap();
    let owner = engine
        .create_disk_root(slot.as_str(), "alice", OperationId::generate().unwrap())
        .unwrap();
    let repository = owner.repository_id.parse().unwrap();
    let workspace = engine
        .create_workspace(&owner.main_branch_id, "alice")
        .unwrap();
    engine
        .stage_bytes(
            &workspace.workspace_id,
            "original.txt",
            b"unchanged original",
        )
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: OperationId::generate().unwrap(),
            principal: "alice".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "source".into(),
            request_digest: None,
        })
        .unwrap();
    let context = engine.snapshot_root_context(repository).unwrap();
    let principal = "alice".parse().unwrap();
    let source = engine
        .initialize_directory_snapshot(
            &InitializeSnapshot {
                host: context.host,
                source: context.source,
                operation_id: OperationId::generate().unwrap(),
            },
            &principal,
        )
        .unwrap();
    let request = || AllocateWorktree {
        source,
        parent_directory_id: context.host.directory_id,
        expected_root_generation: context.host.expected_root_generation,
        naming: WorktreeNaming::Automatic,
        operation_id: OperationId::generate().unwrap(),
    };
    let first = engine
        .allocate_directory_worktree(&request(), &principal)
        .unwrap();
    let second = engine
        .allocate_directory_worktree(&request(), &principal)
        .unwrap();
    let first_mount = PortablePath::parse(&format!(
        "p/{}",
        engine
            .directory_worktree_status(first)
            .unwrap()
            .name
            .as_str()
    ))
    .unwrap();
    let second_mount = PortablePath::parse(&format!(
        "p/{}",
        engine
            .directory_worktree_status(second)
            .unwrap()
            .name
            .as_str()
    ))
    .unwrap();
    let mut entropy = [0; 32];
    system_entropy::fill(&mut entropy).unwrap();
    let password = entropy
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let auth = AuthStore::initialize(&system, "alice", &password).unwrap();
    let spool = root.path().join("spool");
    fs::create_dir(&spool).unwrap();
    fs::set_permissions(&spool, fs::Permissions::from_mode(0o700)).unwrap();
    fs::create_dir(spool.join("empty-directory")).unwrap();
    let direct = Direct::start(root.path(), engine, auth);
    (
        Fixture {
            root,
            system,
            disk,
            slot,
            repository,
            source,
            first,
            second,
            first_mount,
            second_mount,
            password,
        },
        direct,
    )
}

fn status(http: &Http, target: WorktreeHandle) -> WorktreeStatus {
    json_codec::from_value(http.json(
        "GET",
        &format!(
            "/api/directory-worktrees/{}?publication={}",
            target.worktree_id, target.publication,
        ),
        None,
    ))
    .unwrap()
}

fn content_route(target: WorktreeHandle, path: &str) -> String {
    let query = url_encoding::form::Serializer::new(String::new())
        .append_pair("publication", &target.publication.to_string())
        .append_pair("path", path)
        .finish();
    format!(
        "/api/directory-worktrees/{}/content?{query}",
        target.worktree_id
    )
}

fn receipt(http: &Http, fixture: &Fixture, request: &CommitWorktree) -> WorktreeCommitReceipt {
    json_codec::from_value(http.json(
        "GET",
        &format!(
            "/api/repositories/{}/directory-worktrees/commits/{}",
            fixture.repository, request.operation_id,
        ),
        None,
    ))
    .unwrap()
}

fn inode(file: &storage_engine::ReadFile) -> (u64, u64) {
    let metadata = file.metadata().unwrap();
    assert_eq!(metadata.nlink(), 1);
    (metadata.dev(), metadata.ino())
}

fn locate_inode(root: &Path, expected: (u64, u64)) -> Option<PathBuf> {
    let metadata = fs::metadata(root).unwrap();
    if (metadata.dev(), metadata.ino()) == expected {
        return Some(root.to_owned());
    }
    if metadata.is_dir() {
        for entry in fs::read_dir(root).unwrap() {
            if let Some(path) = locate_inode(&entry.unwrap().path(), expected) {
                return Some(path);
            }
        }
    }
    None
}

fn published_listing(
    direct: &mut Direct,
    mount: &str,
    target: &WorktreeFileTarget,
    version: storage_contract::VersionId,
    size: usize,
    mtime: FileMtime,
) {
    let (directory, placeholder) = direct.open(mount, false, false);
    let Reply::Directory(page) = direct.call(Request::ReadDirectory {
        handle: directory.handle,
        restart: true,
    }) else {
        panic!("published worktree directory reply");
    };
    assert!(!page.more);
    assert_eq!(page.entries.len(), 2);
    let added = page
        .entries
        .iter()
        .find(|entry| entry.path().file_name() == "added.txt")
        .unwrap();
    let DirectoryEntry::WorktreeRetained {
        target: actual,
        version: actual_version,
        size: actual_size,
        ..
    } = added
    else {
        panic!("the completed Added occurrence retains its own identity");
    };
    assert_eq!(actual, target);
    assert_eq!(*actual_version, version);
    assert_eq!(*actual_size, size as u64);
    assert_eq!(added.entry_id(), target.entry.to_string());
    assert_eq!(added.mtime(), mtime);
    let wire: DirectoryEntry = json_codec::from_slice(&json_codec::to_vec(added).unwrap()).unwrap();
    assert_eq!(wire.entry_id(), target.entry.to_string());
    assert_eq!(wire.path(), added.path());
    assert_eq!(wire.mtime(), mtime);
    direct.call(Request::Close {
        handle: directory.handle,
    });
    drop(placeholder);
}

#[test]
fn native_added_single_commit_over_tcp_preserves_other_instance_and_cold_original_result() {
    let (fixture, mut direct) = fixture();
    let first_path = format!("{}/added.txt", fixture.first_mount);
    let second_path = format!("{}/added.txt", fixture.second_mount);
    let (opened, placeholder) = direct.open(&first_path, true, true);
    assert_eq!(opened.data_io, OpenIo::NativeWorking);
    let first = listed(&mut direct, fixture.first_mount.as_str(), 0);
    direct.write(
        opened.handle,
        WorkingPosition::At { offset: 0 },
        b"saved then",
    );
    let saved = direct.flush(opened.handle);
    assert_eq!(direct.flush(opened.handle), saved);
    direct.write(opened.handle, WorkingPosition::Append {}, b" closed");
    direct.call(Request::Close {
        handle: opened.handle,
    });
    drop(placeholder);
    let expected = b"saved then closed";
    assert_eq!(
        listed(
            &mut direct,
            fixture.first_mount.as_str(),
            expected.len() as u64
        ),
        first
    );
    let (other, placeholder) = direct.open(&second_path, true, true);
    let second = listed(&mut direct, fixture.second_mount.as_str(), 0);
    assert_ne!(first.file, second.file);
    assert_ne!(first.entry, second.entry);
    let other_body = b"untouched draft";
    direct.write(other.handle, WorkingPosition::At { offset: 0 }, other_body);
    direct.flush(other.handle);
    direct.call(Request::Close {
        handle: other.handle,
    });
    drop(placeholder);
    let (versions, branches, first_record, second_record, second_status, original_inode) =
        observe(&direct, fixture.slot.as_str(), |engine| {
            let content = engine
                .open_directory_worktree_content(
                    fixture.first,
                    &LookupPath::parse("original.txt").unwrap(),
                )
                .unwrap();
            assert!(engine.state().versions.values().all(|version| {
                version.file_id != first.file.to_string()
                    && version.file_id != second.file.to_string()
            }));
            (
                engine.state().versions.clone(),
                engine.state().branches.clone(),
                engine.state().file_containers[&first.file].clone(),
                engine.state().file_containers[&second.file].clone(),
                engine.directory_worktree_status(fixture.second).unwrap(),
                inode(&content.file),
            )
        });
    assert_deleted_container(&fixture.disk, &first_record);
    assert_deleted_container(&fixture.disk, &second_record);
    drop(direct);

    let http = Http::start(
        &fixture.system,
        Engine::open(&fixture.system).unwrap(),
        &fixture.password,
    );
    let current = status(&http, fixture.first);
    assert_eq!(current.original, fixture.source);
    let request = CommitWorktree {
        target: current.target,
        expected: current.revision,
        message: "publish this one Added file".into(),
        operation_id: OperationId::generate().unwrap(),
    };
    let route = format!(
        "/api/directory-worktrees/{}/commits",
        request.target.worktree_id
    );
    let snapshot: SnapshotHandle = json_codec::from_value(http.json(
        "POST",
        &route,
        Some(json_codec::to_value(&request).unwrap()),
    ))
    .unwrap();
    assert_eq!(snapshot.repository_id, fixture.source.repository_id);
    assert_eq!(snapshot.branch_id, fixture.source.branch_id);
    assert_ne!(snapshot.node_id, fixture.source.node_id);
    assert_ne!(snapshot.publication, request.operation_id);
    let completed = receipt(&http, &fixture, &request);
    assert_eq!(completed.repository_id, fixture.repository);
    assert_eq!(completed.request, request);
    assert_eq!(
        completed.result,
        WorktreeCommitOutcome::Complete { snapshot }
    );
    assert_eq!(
        http.bytes(&content_route(fixture.first, "added.txt")),
        expected
    );
    let (_, range) = http.range(
        &content_route(fixture.first, "added.txt"),
        1,
        3,
        expected.len(),
    );
    assert_eq!(range, expected[1..4]);
    assert_eq!(
        http.bytes(&content_route(fixture.first, "original.txt")),
        b"unchanged original"
    );
    assert_eq!(status(&http, fixture.second), second_status);
    assert_eq!(status(&http, fixture.first).original, fixture.source);
    let engine = http.cold(&fixture.system);

    let entry = engine
        .directory_worktree_entry(fixture.first, &LookupPath::parse("added.txt").unwrap())
        .unwrap();
    let WorktreeEntryContent::RetainedFile {
        target,
        file,
        version,
        size,
        content_hash,
    } = entry.content
    else {
        panic!("the same committed instance must retain the Added occurrence");
    };
    assert_eq!(target, first);
    assert_eq!(file, first.file);
    assert_eq!(size, expected.len() as u64);
    assert_eq!(
        content_hash,
        storage_contract::ContentHash::from_digest(blake3::hash(expected))
    );
    assert_eq!(engine.state().versions.len(), versions.len() + 1);
    for (id, old) in versions.iter() {
        assert_eq!(engine.state().versions.get(id), Some(old));
    }
    assert_eq!(engine.state().branches.len(), branches.len() + 1);
    for (id, old) in branches.iter() {
        assert_eq!(engine.state().branches.get(id), Some(old));
    }
    let file_version = &engine.state().versions[&version.to_string()];
    assert_eq!(file_version.file_id, first.file.to_string());
    assert!(file_version.source_version_ids.is_empty());
    assert_eq!(file_version.content_hash, content_hash);
    let file_branches = engine
        .state()
        .branches
        .values()
        .filter(|branch| branch.file_id.as_deref() == Some(first.file.to_string().as_str()))
        .collect::<Vec<_>>();
    assert_eq!(file_branches.len(), 1);
    assert_eq!(
        engine
            .file_branch_head(&file_branches[0].branch_id)
            .unwrap(),
        version.to_string()
    );
    let mtime = crate::backend_read::version_mtime(file_version).unwrap();
    assert!(matches!(mtime, FileMtime::Saved { .. }));
    let retained = &engine.state().file_containers[&first.file];
    assert_eq!(retained.namespace, first_record.namespace);
    assert_eq!(retained.owner_disk, first_record.owner_disk);
    assert_eq!(retained.history_tree_id, first_record.history_tree_id);
    assert_eq!(
        retained.replicas[&retained.owner_disk].entity,
        first_record.replicas[&first_record.owner_disk].entity
    );
    assert_eq!(retained.created_by, first_record.created_by);
    assert!(matches!(
        retained.working,
        FileWorkingState::ManualClean { .. }
    ));
    assert_deleted_container(&fixture.disk, retained);
    assert_eq!(engine.state().file_containers[&second.file], second_record);
    assert_eq!(
        added_metadata(&engine, fixture.second),
        (second.clone(), other_body.len() as u64)
    );
    assert_eq!(
        engine
            .native_current_entries(fixture.repository)
            .unwrap()
            .len(),
        1
    );
    let mut version_body = engine.open_content(&version.to_string()).unwrap().file;
    let version_inode = inode(&version_body);
    let mut bytes = Vec::new();
    version_body.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, expected);
    let published = engine
        .open_directory_worktree_content(fixture.first, &LookupPath::parse("added.txt").unwrap())
        .unwrap();
    assert_ne!(
        inode(&published.file),
        version_inode,
        "FILE and snapshot own independent FULLs"
    );
    let path = locate_inode(&fixture.disk, inode(&published.file)).unwrap();
    assert!(path.components().any(|part| part.as_os_str() == ".zxs"));
    let path = locate_inode(&fixture.disk, version_inode).unwrap();
    assert!(
        path.components()
            .any(|part| part.as_os_str() == "deleted-data")
    );
    let copied = engine
        .open_directory_worktree_content(fixture.first, &LookupPath::parse("original.txt").unwrap())
        .unwrap();
    assert_ne!(
        inode(&copied.file),
        original_inode,
        "unchanged source body is copied independently"
    );
    let history = engine
        .directory_snapshot_history(fixture.repository, None)
        .unwrap();
    assert_eq!(history.nodes.len(), 2);
    assert!(history.next_cursor.is_none());
    let head = history
        .nodes
        .iter()
        .find(|node| node.snapshot == snapshot)
        .unwrap();
    assert_eq!(head.parent, Some(fixture.source));
    assert!(head.is_head);
    let final_versions = engine.state().versions.clone();
    let final_branches = engine.state().branches.clone();
    drop(version_body);
    drop(published);
    drop(copied);

    let http = Http::start(&fixture.system, engine, &fixture.password);
    assert_eq!(receipt(&http, &fixture, &request), completed);
    let retry: SnapshotHandle = json_codec::from_value(http.json(
        "POST",
        &route,
        Some(json_codec::to_value(&request).unwrap()),
    ))
    .unwrap();
    assert_eq!(retry, snapshot);
    assert_eq!(receipt(&http, &fixture, &request), completed);
    let engine = http.cold(&fixture.system);
    assert_eq!(engine.state().versions, final_versions);
    assert_eq!(engine.state().branches, final_branches);
    assert_eq!(engine.state().file_containers[&second.file], second_record);
    let auth = AuthStore::open(&fixture.system).unwrap();
    let mut direct = Direct::start(fixture.root.path(), engine, auth);
    let (opened, placeholder) = direct.open(&first_path, false, false);
    assert_eq!(opened.data_io, OpenIo::VerifiedRanges);
    assert_eq!(opened.entry_id, first.entry.to_string());
    assert_eq!(opened.version_id, Some(version.to_string()));
    assert_eq!(opened.mtime, mtime);
    assert_eq!(direct.read(opened.handle), expected);
    let Reply::Metadata(metadata) = direct.call(Request::AuthorizeRead {
        handle: opened.handle,
    }) else {
        panic!("retained Added descriptor metadata");
    };
    assert_eq!(metadata.mtime, mtime);
    let Reply::Stat(stat) = direct.call(Request::Stat { path: first_path }) else {
        panic!("retained Added stat reply");
    };
    assert_eq!(stat.entry_id, first.entry.to_string());
    assert_eq!(stat.size, expected.len() as u64);
    assert_eq!(stat.mtime, mtime);
    published_listing(
        &mut direct,
        fixture.first_mount.as_str(),
        &first,
        version,
        expected.len(),
        mtime,
    );
    direct.call(Request::Close {
        handle: opened.handle,
    });
    drop(placeholder);
    assert_eq!(
        listed(
            &mut direct,
            fixture.second_mount.as_str(),
            other_body.len() as u64
        ),
        second
    );
    let (other, placeholder) = direct.open(&second_path, false, false);
    assert_eq!(other.data_io, OpenIo::NativeWorking);
    assert_eq!(other.entry_id, second.entry.to_string());
    assert_eq!(direct.read(other.handle), other_body);
    direct.call(Request::Close {
        handle: other.handle,
    });
    drop(placeholder);
}
