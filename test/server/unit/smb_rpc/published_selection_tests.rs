//! A first-created SMB file must retain its published write destination.
use super::*;

fn first_save(
    client: &mut Session,
    backend: &LostCommitReply,
    path: &str,
    create: bool,
    lose_reply: bool,
) -> (OpenResult, File) {
    let (opened, descriptor) = client
        .open(path.into(), true, create, false, false, false)
        .unwrap();
    let opened: OpenResult = json_codec::from_value(opened).unwrap();
    let file = File::from(descriptor.unwrap());
    // A genuinely unpublished new path has no captured file context yet.
    client
        .dispatch(Request::PrepareWrite {
            handle: opened.handle,
            end: 3,
            bytes: 3,
        })
        .unwrap();
    file.write_all_at(b"one", 0).unwrap();
    client.dirty(opened.handle).unwrap();
    let operation = match &client.handles[&opened.handle] {
        Handle::Write(writer) => writer.operation_id,
        _ => unreachable!(),
    };
    if lose_reply {
        backend.fail.store(true, Ordering::SeqCst);
        assert_eq!(
            client.flush(opened.handle).unwrap_err().code,
            storage_contract::ErrorCode::InjectedFailure
        );
    }
    let uploads = backend.uploads.load(Ordering::SeqCst);
    let saved = client.flush(opened.handle).unwrap();
    assert_eq!(saved["commit_state"], "PUBLISHED");
    assert_eq!(saved["operation_id"], operation.to_string());
    if lose_reply {
        assert_eq!(backend.uploads.load(Ordering::SeqCst), uploads);
    }
    (opened, file)
}

fn select_same_version_draft(shared: &Shared, branch: &str, path: &str) -> Entry {
    let identity = &identity(shared);
    let tree = shared
        .backend
        .tree_snapshot(
            identity,
            crate::tests::tree_request(&(json!({"branch_id":branch}))),
        )
        .map(|snapshot| json_codec::to_value(snapshot).unwrap())
        .unwrap();
    let entries: Vec<Entry> = json_codec::from_value(tree["entries"].clone()).unwrap();
    let entry = entries
        .into_iter()
        .find(|entry| entry.path == path)
        .unwrap();
    let created = shared
        .backend
        .create_file_branch(
            identity,
            crate::CreateFileBranch {
                file_id: entry.file_id.clone(),
                source_version_id: entry.version_id.clone(),
                source_node_id: None,
                name: storage_contract::BranchName::parse("same version draft").unwrap(),
                operation_id: storage_contract::OperationId::generate(),
            },
        )
        .and_then(crate::operation_wire::operation_wire)
        .unwrap();
    assert_eq!(created["result"]["materialized"], false);
    shared
        .backend
        .select_file_branch(
            identity,
            crate::SelectFileBranch {
                file_id: entry.file_id.clone(),
                branch_id: created["result"]["branch_id"].as_str().unwrap().into(),
                location: None,
                operation_id: storage_contract::OperationId::generate(),
            },
        )
        .unwrap();
    entry
}

#[test]
fn new_file_published_handle_rejects_same_version_file_branch_switch() {
    for lose_reply in [false, true] {
        let (_temporary, shared, backend, branch) = fixture();
        let mut client = session(shared.clone(), &branch);
        let (opened, file) = first_save(&mut client, &backend, "fresh.txt", true, lose_reply);
        let original = select_same_version_draft(&shared, &branch, "fresh.txt");
        let selected = client
            .find(&name_policy::LookupPath::parse("fresh.txt").unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(
            (&selected.entry_id, &selected.version_id, &selected.path),
            (&original.entry_id, &original.version_id, &original.path)
        );
        let uploads = backend.uploads.load(Ordering::SeqCst);
        assert_eq!(
            client
                .dispatch(Request::PrepareWrite {
                    handle: opened.handle,
                    end: 3,
                    bytes: 3
                })
                .unwrap_err()
                .code,
            storage_contract::ErrorCode::FileBranchConflict
        );
        // An already admitted FD write still has to fail the Dirty boundary.
        file.write_all_at(b"bad", 0).unwrap();
        assert_eq!(
            client.dirty(opened.handle).unwrap_err().code,
            storage_contract::ErrorCode::FileBranchConflict
        );
        assert_eq!(backend.uploads.load(Ordering::SeqCst), uploads);
        let writer = match &client.handles[&opened.handle] {
            Handle::Write(writer) => writer,
            _ => unreachable!(),
        };
        assert!(writer.dirty && writer.committed && writer.source.is_file());
        assert_eq!(
            writer.workspace.workspace_id,
            opened.workspace_id.as_deref().unwrap()
        );
        let current = client
            .find(&name_policy::LookupPath::parse("fresh.txt").unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(current.version_id, original.version_id);
        let content = backend
            .content(&client.identity, &current.version_id)
            .unwrap();
        let mut bytes = [0; 3];
        content.file.read_exact_at(&mut bytes, 0).unwrap();
        assert_eq!(&bytes, b"one");
        client.close(opened.handle, true).unwrap();
    }
}

#[test]
fn published_handles_can_write_again_without_selection_change() {
    for create in [false, true] {
        for lose_reply in [false, true] {
            let (_temporary, shared, backend, branch) = fixture();
            let mut client = session(shared, &branch);
            let path = if create { "fresh.txt" } else { "report.txt" };
            let (opened, file) = first_save(&mut client, &backend, path, create, lose_reply);
            let first_operation = match &client.handles[&opened.handle] {
                Handle::Write(writer) => writer.operation_id,
                _ => unreachable!(),
            };
            client
                .dispatch(Request::PrepareWrite {
                    handle: opened.handle,
                    end: 3,
                    bytes: 3,
                })
                .unwrap();
            file.write_all_at(b"two", 0).unwrap();
            client.dirty(opened.handle).unwrap();
            let writer = match &client.handles[&opened.handle] {
                Handle::Write(writer) => writer,
                _ => unreachable!(),
            };
            assert_ne!(
                writer.workspace.workspace_id,
                opened.workspace_id.as_deref().unwrap()
            );
            assert_ne!(writer.operation_id, first_operation);
            let record: Value = json_codec::from_slice(
                &fs::read(writer.source.with_extension("work.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(record["workspace_id"], writer.workspace.workspace_id);
            assert_eq!(record["operation_id"], writer.operation_id.to_string());
            assert_eq!(record["published"], false);
            assert_eq!(record["dirty"], true);
            let saved = client.flush(opened.handle).unwrap();
            assert_eq!(saved["commit_state"], "PUBLISHED");
            let current = client
                .find(&name_policy::LookupPath::parse(path).unwrap())
                .unwrap()
                .unwrap();
            assert_eq!(current.entry_id, opened.entry_id);
            let content = backend
                .content(&client.identity, &current.version_id)
                .unwrap();
            let mut bytes = [0; 3];
            content.file.read_exact_at(&mut bytes, 0).unwrap();
            assert_eq!(&bytes, b"two");
            client.close(opened.handle, false).unwrap();
        }
    }
}

#[test]
fn unpublished_dirty_preserves_generation_and_records_before_authorization_failure() {
    let (_temporary, shared, backend, branch) = fixture();
    let mut client = session(shared, &branch);
    let (opened, file) = open_file(&mut client, true);
    let (source, operation) = match &client.handles[&opened.handle] {
        Handle::Write(writer) => (writer.source.clone(), writer.operation_id),
        _ => unreachable!(),
    };
    let record_path = source.with_extension("work.json");
    for bytes in [b"one", b"two"] {
        file.write_all_at(bytes, 0).unwrap();
        client.dirty(opened.handle).unwrap();
        let record: Value = json_codec::from_slice(&fs::read(&record_path).unwrap()).unwrap();
        assert_eq!(
            record["workspace_id"],
            opened.workspace_id.as_deref().unwrap()
        );
        assert_eq!(record["operation_id"], operation.to_string());
        assert_eq!(record["dirty"], true);
        assert_eq!(record["published"], false);
    }
    // Remove the previous record so it cannot satisfy this observation.
    fs::remove_file(&record_path).unwrap();
    backend.revoked.store(true, Ordering::SeqCst);
    file.write_all_at(b"end", 0).unwrap();
    client.dirty(opened.handle).unwrap_err();
    let record: Value = json_codec::from_slice(&fs::read(&record_path).unwrap()).unwrap();
    assert_eq!(record["operation_id"], operation.to_string());
    assert_eq!(record["dirty"], true);
    assert_eq!(backend.uploads.load(Ordering::SeqCst), 0);
    assert_eq!(fs::read(source).unwrap(), b"end");
}

#[test]
fn failed_initial_dirty_record_retains_source_and_retry_records_generation() {
    let (_temporary, shared, _backend, branch) = fixture();
    let mut client = session(shared, &branch);
    let (opened, file) = open_file(&mut client, true);
    let source = match &client.handles[&opened.handle] {
        Handle::Write(writer) => writer.source.clone(),
        _ => unreachable!(),
    };
    let record_path = source.with_extension("work.json");
    fs::remove_file(&record_path).unwrap();
    fs::create_dir(&record_path).unwrap();
    file.write_all_at(b"new", 0).unwrap();
    let error = client.dirty(opened.handle).unwrap_err();
    assert!(std::error::Error::source(&error).is_some());
    assert_eq!(fs::read(&source).unwrap(), b"new");
    fs::remove_dir(&record_path).unwrap();
    client.dirty(opened.handle).unwrap();
    let record: Value = json_codec::from_slice(&fs::read(&record_path).unwrap()).unwrap();
    assert_eq!(
        record["workspace_id"],
        opened.workspace_id.as_deref().unwrap()
    );
    assert_eq!(record["dirty"], true);
    client.close(opened.handle, false).unwrap();
}

#[test]
fn failed_new_generation_record_is_retried_without_another_workspace() {
    let (_temporary, shared, backend, branch) = fixture();
    let mut client = session(shared, &branch);
    let (opened, file) = first_save(&mut client, &backend, "report.txt", false, false);
    let source = match &client.handles[&opened.handle] {
        Handle::Write(writer) => writer.source.clone(),
        _ => unreachable!(),
    };
    let record_path = source.with_extension("work.json");
    *backend.obstruct_work_record.lock().unwrap() = Some(record_path.clone());
    file.write_all_at(b"two", 0).unwrap();
    let error = client.dirty(opened.handle).unwrap_err();
    assert!(std::error::Error::source(&error).is_some());
    let (workspace, operation) = match &client.handles[&opened.handle] {
        Handle::Write(writer) => {
            assert!(writer.dirty && !writer.committed);
            (writer.workspace.workspace_id.clone(), writer.operation_id)
        }
        _ => unreachable!(),
    };
    assert_ne!(Some(&workspace), opened.workspace_id.as_ref());
    assert_eq!(fs::read(&source).unwrap(), b"two");
    fs::remove_dir(&record_path).unwrap();
    client.dirty(opened.handle).unwrap();
    let record: Value = json_codec::from_slice(&fs::read(&record_path).unwrap()).unwrap();
    assert_eq!(record["workspace_id"], workspace);
    assert_eq!(record["operation_id"], operation.to_string());
    assert_eq!(record["dirty"], true);
    client.close(opened.handle, false).unwrap();
}
