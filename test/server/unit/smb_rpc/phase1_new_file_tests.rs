//! Phase-one ordinary SMB creates use one real primary and the framed RPC path.
use super::*;

fn primary_fixture() -> (Fixture, Arc<Shared>, Arc<LostCommitReply>) {
    let temp = tempfile::tempdir().unwrap();
    let config = Config::development([temp.path().join("p")]).unwrap();
    assert_eq!(config.disks.len(), 1);
    assert_eq!(config.disks[0].level, 1);
    let engine = Engine::initialize(&temp.path().join("state"), config).unwrap();
    fixture_backend(temp, engine)
}

fn mkdir(stream: &mut FrameStream, path: &str) {
    okay(
        stream,
        Request::Namespace {
            kind: smb_protocol::protocol::NamespaceOperation::Mkdir,
            path: path.into(),
            destination: None,
            replace: false,
            operation_id: new_id("op").unwrap(),
        },
    );
}

fn root_repository(shared: &Shared) -> storage_contract::Repository {
    let repositories = shared
        .backend
        .repositories_snapshot(&identity(shared))
        .unwrap();
    assert_eq!(repositories.len(), 1);
    let repository = repositories.into_iter().next().unwrap();
    assert!(repository.file_manager_root && repository.smb_enabled);
    assert_eq!(repository.primary_slot_id, "P01");
    assert_eq!(repository.target_copies, 1);
    repository
}

fn file_entry(shared: &Shared, branch: &str, path: &str) -> Entry {
    shared
        .backend
        .tree_snapshot(
            &identity(shared),
            crate::ReadTree {
                branch,
                commit: None,
            },
        )
        .unwrap()
        .entries
        .into_iter()
        .find(|entry| entry.path() == path)
        .unwrap()
}

fn annotate_source(shared: &Shared, repository: &str, source: &Entry) -> String {
    let identity = identity(shared);
    let request = || {
        crate::ReadFileHistory::parse(
            source.file_id.clone(),
            Some(repository.into()),
            Some(source.path().into()),
        )
        .unwrap()
    };
    let history = shared
        .backend
        .file_history_snapshot(&identity, request())
        .unwrap();
    let node = history.current_node_id.unwrap();
    shared
        .backend
        .update_file_node_tags(
            &identity,
            crate::UpdateFileNodeTags {
                file_id: source.file_id.clone(),
                targets: vec![storage_contract::FileTagTarget {
                    branch_id: history.selected_branch_id.unwrap(),
                    source_node_id: node.clone(),
                    expected_node_id: node,
                }],
                add: vec!["source-only".into()],
                remove: vec![],
                location: Some(storage_contract::FileLocation {
                    repository_id: repository.into(),
                    path: source.path().into(),
                }),
                operation_id: OperationId::generate().unwrap(),
            },
        )
        .unwrap();
    for target in [
        crate::NoteTarget::File(
            source.file_id.parse::<storage_contract::FileId>().unwrap().into(),
        ),
        crate::NoteTarget::Version(
            source
                .version_id
                .parse::<storage_contract::VersionId>()
                .unwrap()
                .into(),
        ),
    ] {
        shared
            .backend
            .update_note(
                &identity,
                crate::UpdateNote {
                    target,
                    note: storage_contract::Note::parse("source note".into()).unwrap(),
                    operation_id: OperationId::generate().unwrap(),
                },
            )
            .unwrap();
    }
    shared
        .backend
        .file_history_snapshot(&identity, request())
        .unwrap()
        .current_node_id
        .unwrap()
}

fn assert_initial_history(shared: &Shared, repository: &str, target: &Entry) {
    let history = shared
        .backend
        .file_history_snapshot(
            &identity(shared),
            crate::ReadFileHistory::parse(
                target.file_id.clone(),
                Some(repository.into()),
                Some(target.path().into()),
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(history.current_node_id.as_ref(), Some(&target.version_id));
    assert_eq!(history.versions.len(), 1);
    assert_eq!(history.nodes.len(), 1);
    assert_eq!(history.branches.len(), 1);
    assert!(history.merges.is_empty());
    assert!(history.note.is_empty());
}

fn read_and_close(stream: &mut FrameStream, path: &str) -> (OpenResult, Vec<u8>) {
    let (reader, _placeholder) = open(stream, path, false, false);
    let bytes = read_rpc(stream, reader.handle, 0, 4096);
    assert!(read_rpc(stream, reader.handle, bytes.len() as u64, 1).is_empty());
    okay(
        stream,
        Request::Close {
            handle: reader.handle,
        },
    );
    (reader, bytes)
}

fn exported_bytes(engine: &Engine, temp: &Fixture, version: &str) -> Vec<u8> {
    let output = temp.path().join(format!("export-{version}"));
    engine.export_version(version, &output).unwrap();
    fs::read(output).unwrap()
}

#[test]
fn phase1_primary_source_read_to_new_target_keeps_independent_history() {
    let (temp, shared, backend) = primary_fixture();
    let (mut stream, worker, reply) = wire(shared.clone(), "alice");
    assert!(matches!(reply, Response::Ok { .. }), "{reply:?}");
    assert_eq!(directory(&mut stream, ""), vec!["p"]);
    mkdir(&mut stream, "p/from");
    mkdir(&mut stream, "p/to");
    let repository = root_repository(&shared);
    let branch = &repository.main_branch_id;
    let (first, first_fd) = open(&mut stream, "p/from/source.txt", true, true);
    save(&mut stream, &first, &first_fd, b"first");
    let original = file_entry(&shared, branch, "from/source.txt");
    let (second, second_fd) = open(&mut stream, "p/from/source.txt", true, false);
    save(&mut stream, &second, &second_fd, b"later");
    let source = file_entry(&shared, branch, "from/source.txt");
    assert_eq!(source.entry_id, original.entry_id);
    assert_eq!(source.file_id, original.file_id);
    assert_ne!(source.version_id, original.version_id);
    let tagged_source_node = annotate_source(&shared, &repository.repository_id, &source);

    // A normal client reads current bytes, then creates a separate target.
    // The descriptor returned for a reader is a placeholder: use Read RPC.
    let (reader, bytes) = read_and_close(&mut stream, "p/from/source.txt");
    assert_eq!(reader.version_id.as_ref(), Some(&source.version_id));
    assert_eq!(bytes, b"later");
    let (created, created_fd) = open(&mut stream, "p/to/new.txt", true, true);
    assert!(created.version_id.is_none());
    save(&mut stream, &created, &created_fd, &bytes);
    let target = file_entry(&shared, branch, "to/new.txt");
    assert_eq!(target.entry_id, created.entry_id);
    assert_ne!(target.entry_id, source.entry_id);
    assert_ne!(target.file_id, source.file_id);
    assert_ne!(target.version_id, source.version_id);
    assert_initial_history(&shared, &repository.repository_id, &target);
    assert_eq!(file_entry(&shared, branch, "from/source.txt"), source);
    assert_eq!(read_and_close(&mut stream, "p/to/new.txt").1, bytes);
    close_wire(stream, worker);
    drop(shared);
    drop(backend);

    let engine = Engine::open(&temp.path().join("state")).unwrap();
    let tree = engine.list_tree(branch).unwrap();
    assert_eq!(
        tree.iter().find(|entry| entry.path() == source.path()),
        Some(&source)
    );
    assert_eq!(
        tree.iter().find(|entry| entry.path() == target.path()),
        Some(&target)
    );
    let source_versions = engine.file_history(&source.file_id).unwrap();
    assert_eq!(source_versions.len(), 2);
    assert_eq!(source_versions[0].version_id, original.version_id);
    assert_eq!(source_versions[1].version_id, source.version_id);
    assert_eq!(
        exported_bytes(&engine, &temp, &original.version_id),
        b"first"
    );
    assert_eq!(exported_bytes(&engine, &temp, &source.version_id), b"later");
    let target_versions = engine.file_history(&target.file_id).unwrap();
    assert_eq!(target_versions.len(), 1);
    assert_eq!(target_versions[0].version_id, target.version_id);
    assert!(target_versions[0].source_version_ids.is_empty());
    assert!(
        engine
            .file_node_parents(&target.file_id, &target.version_id)
            .unwrap()
            .is_empty()
    );
    assert!(
        engine
            .file_node_tags(&target.file_id, &target.version_id)
            .unwrap()
            .is_empty()
    );
    assert!(!engine.state().file_notes.contains_key(&target.file_id));
    assert!(
        !engine
            .state()
            .version_notes
            .contains_key(&target.version_id)
    );
    assert_eq!(engine.state().file_notes[&source.file_id], "source note");
    assert_eq!(
        engine.state().version_notes[&source.version_id],
        "source note"
    );
    assert_eq!(
        engine
            .file_node_tags(&source.file_id, &tagged_source_node)
            .unwrap(),
        BTreeSet::from(["source-only".into()])
    );
    assert_eq!(exported_bytes(&engine, &temp, &target.version_id), b"later");
}

#[test]
fn phase1_primary_empty_create_publishes_once_on_close_or_flush_then_close() {
    let (temp, shared, backend) = primary_fixture();
    let (mut stream, worker, _) = wire(shared.clone(), "alice");
    let mut saved = Vec::new();
    for (path, writable, flush_first) in [
        ("p/readonly-close.txt", false, false),
        ("p/readonly-flush.txt", false, true),
        ("p/close-only.txt", true, false),
        ("p/flush-first.txt", true, true),
    ] {
        let (created, _fd) = open(&mut stream, path, writable, true);
        assert_eq!(created.size, 0);
        assert!(created.workspace_id.is_some());
        assert!(created.version_id.is_none());
        // No PrepareWrite, descriptor writes or Dirty: creation itself is work.
        if flush_first {
            okay(
                &mut stream,
                Request::Flush {
                    handle: created.handle,
                },
            );
        }
        okay(
            &mut stream,
            Request::Close {
                handle: created.handle,
            },
        );
        let (reader, bytes) = read_and_close(&mut stream, path);
        assert_eq!(reader.entry_id, created.entry_id);
        assert_eq!(reader.size, 0);
        assert!(bytes.is_empty());
        saved.push(reader);
    }
    let repository = root_repository(&shared);
    close_wire(stream, worker);
    drop(shared);
    drop(backend);

    let engine = Engine::open(&temp.path().join("state")).unwrap();
    let entries = engine.list_tree(&repository.main_branch_id).unwrap();
    assert_eq!(entries.len(), 4);
    assert_eq!(
        entries
            .iter()
            .map(|entry| &entry.file_id)
            .collect::<BTreeSet<_>>()
            .len(),
        4
    );
    assert_eq!(engine.state().versions.len(), 4);
    for opened in saved {
        let entry = entries
            .iter()
            .find(|entry| entry.entry_id == opened.entry_id)
            .unwrap();
        let versions = engine.file_history(&entry.file_id).unwrap();
        assert_eq!(versions.len(), 1);
        assert_eq!(Some(&versions[0].version_id), opened.version_id.as_ref());
        assert_eq!(versions[0].logical_size, 0);
        assert!(versions[0].source_version_ids.is_empty());
        assert!(exported_bytes(&engine, &temp, &entry.version_id).is_empty());
    }
}

#[test]
fn phase1_primary_exclusive_create_conflicts_preserve_saved_file_and_connection() {
    let (temp, shared, backend) = primary_fixture();
    let (mut stream, worker, _) = wire(shared.clone(), "alice");
    let (created, fd) = open(&mut stream, "p/Résumé.txt", true, true);
    save(&mut stream, &created, &fd, b"keep");
    let repository = root_repository(&shared);
    let before = file_entry(&shared, &repository.main_branch_id, "Résumé.txt");
    let spool_before = fs::read_dir(&shared.spool)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<BTreeSet<_>>();
    for path in ["p/Résumé.txt", "p/RÉSUMÉ.TXT", "p/Re\u{301}sume\u{301}.txt"] {
        let (reply, descriptor) = binding_tests::exchange(
            &mut stream,
            Request::Open {
                path: path.into(),
                writable: true,
                create: true,
                truncate: true,
                exclusive: true,
                write_through: false,
            },
        );
        assert!(
            matches!(
                reply,
                Response::Error {
                    code: storage_contract::ErrorCode::NameConflict,
                    ..
                }
            ),
            "{path}: {reply:?}"
        );
        assert!(descriptor.is_none());
        assert_eq!(
            file_entry(&shared, &repository.main_branch_id, "Résumé.txt"),
            before
        );
        assert_eq!(read_and_close(&mut stream, path).1, b"keep");
        assert!(shared.file_writers.lock().unwrap().is_empty());
        assert_eq!(
            fs::read_dir(&shared.spool)
                .unwrap()
                .map(|entry| entry.unwrap().file_name())
                .collect::<BTreeSet<_>>(),
            spool_before
        );
    }
    // A refusal does not poison the stream or leave a writer lock behind.
    let (next, next_fd) = open(&mut stream, "p/next.txt", true, true);
    save(&mut stream, &next, &next_fd, b"next");
    close_wire(stream, worker);
    drop(shared);
    drop(backend);
    let engine = Engine::open(&temp.path().join("state")).unwrap();
    assert_eq!(
        engine
            .list_tree(&repository.main_branch_id)
            .unwrap()
            .iter()
            .find(|entry| entry.path() == "Résumé.txt"),
        Some(&before)
    );
    let versions = engine.file_history(&before.file_id).unwrap();
    assert_eq!(versions.len(), 1);
    assert_eq!(versions[0].version_id, before.version_id);
    assert_eq!(exported_bytes(&engine, &temp, &before.version_id), b"keep");
}
