//! Count actual replacements, not dirty flags or filesystem timestamp changes.
use super::*;
use crate::smb_rpc::recovery_record::persist;

fn writer(client: &Session, handle: u64) -> &Writer {
    match &client.handles[&handle] {
        Handle::Write(writer) => writer,
        _ => panic!("expected writer"),
    }
}

fn record(source: &Path) -> Value {
    json_codec::from_slice(&fs::read(source.with_extension("work.json")).unwrap()).unwrap()
}

#[test]
fn unchanged_dirty_records_replace_once_and_keep_authorization_and_budget_growth() {
    for count in [1, 8, 64] {
        let (_temporary, shared, backend, branch) = fixture();
        let mut client = session(shared, &branch);
        let (opened, file) = open_file(&mut client, true);
        let source = writer(&client, opened.handle).source.clone();
        let operation = writer(&client, opened.handle).operation_id;
        let start = persist::replacements();
        backend.take_authorizations();
        for index in 0..count {
            let end = 4 + index as u64;
            client
                .dispatch(Request::PrepareWrite {
                    handle: opened.handle,
                    end,
                    bytes: 1,
                })
                .unwrap();
            file.write_all_at(b"x", end - 1).unwrap();
            client.dirty(opened.handle).unwrap();
            let budget: Value =
                json_codec::from_slice(&fs::read(source.with_extension("budget.json")).unwrap())
                    .unwrap();
            assert_eq!(budget["admitted_end"], end.to_string());
        }
        assert_eq!(persist::replacements() - start, 1);
        assert_eq!(backend.take_authorizations(), vec![branch.clone(); count]);
        assert_eq!(writer(&client, opened.handle).operation_id, operation);
        assert_eq!(record(&source)["dirty"], true);
        assert_eq!(record(&source)["descriptor_changes_unconfirmed"], true);
        backend.revoked.store(true, Ordering::SeqCst);
        let error = client.dirty(opened.handle).unwrap_err();
        assert_eq!(error.code, storage_contract::ErrorCode::Forbidden);
        assert_eq!(persist::replacements() - start, 1);
        assert_eq!(backend.take_authorizations(), vec![branch]);
        assert_eq!(file.metadata().unwrap().len(), 3 + count as u64);
        assert_eq!(backend.uploads.load(Ordering::SeqCst), 0);
        drop(client);
        assert!(source.is_file());
        assert!(source.with_extension("budget.json").is_file());
    }
}

#[test]
fn successful_flush_and_write_through_persist_each_new_generation() {
    for write_through in [false, true] {
        let (_temporary, shared, backend, branch) = fixture();
        let mut client = session(shared, &branch);
        let (opened, descriptor) = client
            .open(
                "report.txt".into(),
                true,
                false,
                false,
                false,
                write_through,
            )
            .unwrap();
        let opened: OpenResult = json_codec::from_value(opened).unwrap();
        let file = File::from(descriptor.unwrap());
        let source = writer(&client, opened.handle).source.clone();
        let first = writer(&client, opened.handle).operation_id;
        let start = persist::replacements();
        file.write_all_at(b"one", 0).unwrap();
        client.dirty(opened.handle).unwrap();
        client.flush(opened.handle).unwrap();
        assert_eq!(persist::replacements() - start, 2);
        assert_eq!(record(&source)["published"], true);
        file.write_all_at(b"two", 0).unwrap();
        client.dirty(opened.handle).unwrap();
        let next = writer(&client, opened.handle).operation_id;
        assert_ne!(next, first);
        assert_ne!(
            writer(&client, opened.handle).workspace.workspace_id,
            opened.workspace_id.as_deref().unwrap()
        );
        if !write_through {
            let before = persist::replacements();
            client.dirty(opened.handle).unwrap();
            assert_eq!(persist::replacements(), before);
        }
        client.flush(opened.handle).unwrap();
        // First dirty + publication, then old published-dirty evidence,
        // newly authorized workspace evidence, and its publication evidence.
        assert_eq!(persist::replacements() - start, 5);
        assert_eq!(record(&source)["operation_id"], next.to_string());
        assert_eq!(record(&source)["dirty"], false);
        assert_eq!(backend.uploads.load(Ordering::SeqCst), 2);
        client.close(opened.handle, false).unwrap();
        assert!(!source.exists());
        assert!(!source.with_extension("work.json").exists());
        assert!(!source.with_extension("budget.json").exists());
    }
}

#[test]
fn create_and_truncate_reuse_open_record_but_detach_refreshes_retained_state() {
    for create in [false, true] {
        let (_temporary, shared, _backend, branch) = fixture();
        let mut client = session(shared, &branch);
        let (opened, descriptor) = client
            .open(
                if create { "fresh.txt" } else { "report.txt" }.into(),
                true,
                create,
                !create,
                false,
                false,
            )
            .unwrap();
        let opened: OpenResult = json_codec::from_value(opened).unwrap();
        let file = File::from(descriptor.unwrap());
        let source = writer(&client, opened.handle).source.clone();
        let operation = writer(&client, opened.handle).operation_id;
        client
            .dispatch(Request::PrepareWrite {
                handle: opened.handle,
                end: 3,
                bytes: 3,
            })
            .unwrap();
        file.write_all_at(b"new", 0).unwrap();
        let start = persist::replacements();
        client.dirty(opened.handle).unwrap();
        assert_eq!(persist::replacements(), start);
        client.detach(opened.handle).unwrap();
        assert!(client.handles.is_empty());
        let detached = record(&source);
        assert_eq!(detached["handle_state"], "detached");
        assert_eq!(detached["operation_id"], operation.to_string());
        assert_eq!(detached["retained_bytes_synced"], true);
        assert_eq!(fs::read(&source).unwrap(), b"new");
        assert!(source.with_extension("budget.json").is_file());
    }
}

#[test]
fn failed_record_replacement_never_certifies_the_generation() {
    let (_temporary, shared, _backend, branch) = fixture();
    let mut client = session(shared, &branch);
    let (opened, file) = open_file(&mut client, true);
    let source = writer(&client, opened.handle).source.clone();
    let operation = writer(&client, opened.handle).operation_id;
    file.write_all_at(b"new", 0).unwrap();
    let start = persist::replacements();
    persist::fail_after_replace();
    let error = client
        .dispatch(Request::Dirty {
            handle: opened.handle,
        })
        .unwrap_err();
    assert!(std::error::Error::source(&error.error).is_some());
    assert_eq!(
        error.operation_id.as_deref(),
        Some(operation.to_string().as_str())
    );
    assert_eq!(record(&source)["dirty"], true);
    assert_eq!(fs::read(&source).unwrap(), b"new");
    client.dirty(opened.handle).unwrap();
    assert_eq!(persist::replacements() - start, 2);
    client.dirty(opened.handle).unwrap();
    assert_eq!(persist::replacements() - start, 2);
    assert_eq!(writer(&client, opened.handle).operation_id, operation);
    client.close(opened.handle, false).unwrap();
}

#[test]
fn changed_missing_or_replaced_records_cross_fresh_persistence_barriers() {
    for change in [
        "missing",
        "replacement",
        "corrupt",
        "truncate",
        "grow",
        "rewritten",
    ] {
        let (_temporary, shared, _backend, branch) = fixture();
        let mut client = session(shared, &branch);
        let (opened, file) = open_file(&mut client, true);
        file.write_all_at(b"new", 0).unwrap();
        client.dirty(opened.handle).unwrap();
        let source = writer(&client, opened.handle).source.clone();
        let path = source.with_extension("work.json");
        let expected = fs::read(&path).unwrap();
        let start = persist::replacements();
        match change {
            "missing" => fs::remove_file(&path).unwrap(),
            "replacement" => {
                fs::rename(&path, path.with_extension("moved")).unwrap();
                fs::write(&path, &expected).unwrap();
                fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
            }
            "corrupt" => fs::write(&path, vec![b'x'; expected.len()]).unwrap(),
            "truncate" => fs::write(&path, b"{").unwrap(),
            "grow" => fs::write(&path, vec![b'x'; expected.len() + 1]).unwrap(),
            "rewritten" => {
                fs::write(&path, &expected).unwrap();
                File::open(&path)
                    .unwrap()
                    .set_times(fs::FileTimes::new().set_modified(std::time::UNIX_EPOCH))
                    .unwrap();
            }
            _ => unreachable!(),
        }
        client.dirty(opened.handle).unwrap();
        assert_eq!(persist::replacements() - start, 1, "{change}");
        assert_eq!(fs::read(&path).unwrap(), expected);
        client.dirty(opened.handle).unwrap();
        assert_eq!(persist::replacements() - start, 1, "{change}");
        client.close(opened.handle, false).unwrap();
    }
}

#[test]
fn cached_recovery_record_does_not_admit_replaced_spool_or_unsafe_record() {
    for change in ["spool", "directory", "symlink", "hardlink"] {
        let (_temporary, shared, backend, branch) = fixture();
        let mut client = session(shared.clone(), &branch);
        let (opened, file) = open_file(&mut client, true);
        file.write_all_at(b"new", 0).unwrap();
        client.dirty(opened.handle).unwrap();
        let source = writer(&client, opened.handle).source.clone();
        let path = source.with_extension("work.json");
        let operation = writer(&client, opened.handle).operation_id;
        let expected = fs::read(&path).unwrap();
        match change {
            "spool" => {
                fs::rename(&shared.spool, shared.spool.with_extension("moved")).unwrap();
                fs::create_dir(&shared.spool).unwrap();
                fs::set_permissions(&shared.spool, fs::Permissions::from_mode(0o700)).unwrap();
            }
            "directory" => {
                fs::remove_file(&path).unwrap();
                fs::create_dir(&path).unwrap();
            }
            "symlink" => {
                fs::remove_file(&path).unwrap();
                std::os::unix::fs::symlink(&source, &path).unwrap();
            }
            "hardlink" => fs::hard_link(&path, path.with_extension("alias")).unwrap(),
            _ => unreachable!(),
        }
        let error = client
            .dispatch(Request::Dirty {
                handle: opened.handle,
            })
            .unwrap_err();
        assert!(std::error::Error::source(&error.error).is_some());
        assert_eq!(
            error.operation_id.as_deref(),
            Some(operation.to_string().as_str())
        );
        let mut bytes = [0; 3];
        file.read_exact_at(&mut bytes, 0).unwrap();
        assert_eq!(&bytes, b"new");
        assert_eq!(backend.uploads.load(Ordering::SeqCst), 0);
        if change == "spool" {
            assert_eq!(
                fs::read(
                    shared
                        .spool
                        .with_extension("moved")
                        .join(path.file_name().unwrap())
                )
                .unwrap(),
                expected
            );
            assert!(!path.exists());
        }
    }
}

#[test]
fn lost_publication_reply_keeps_original_operation_and_later_bytes() {
    let (_temporary, shared, backend, branch) = fixture();
    let mut client = session(shared, &branch);
    let (opened, file) = open_file(&mut client, true);
    file.write_all_at(b"one", 0).unwrap();
    client.dirty(opened.handle).unwrap();
    let source = writer(&client, opened.handle).source.clone();
    let original = writer(&client, opened.handle).operation_id;
    let start = persist::replacements();
    backend.fail.store(true, Ordering::SeqCst);
    client.flush(opened.handle).unwrap_err();
    file.write_all_at(b"two", 0).unwrap();
    client.dirty(opened.handle).unwrap_err();
    assert_eq!(persist::replacements(), start);
    assert_eq!(writer(&client, opened.handle).operation_id, original);
    assert_eq!(record(&source)["operation_id"], original.to_string());
    assert_eq!(backend.uploads.load(Ordering::SeqCst), 1);
    assert_eq!(
        client.flush(opened.handle).unwrap_err().code,
        storage_contract::ErrorCode::WorkspaceRebaseRequired
    );
    assert_eq!(persist::replacements() - start, 1);
    assert_eq!(fs::read(&source).unwrap(), b"two");
    assert_eq!(record(&source)["published"], true);
    assert_eq!(record(&source)["dirty"], true);
    client.dirty(opened.handle).unwrap();
    assert_ne!(writer(&client, opened.handle).operation_id, original);
    client.flush(opened.handle).unwrap();
    assert_eq!(backend.uploads.load(Ordering::SeqCst), 2);
    client.close(opened.handle, false).unwrap();
}
