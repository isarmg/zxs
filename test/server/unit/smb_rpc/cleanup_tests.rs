//! Durable cleanup is separate from the original save's authoritative outcome.
use super::*;

fn writer_source(client: &Session, handle: u64) -> (PathBuf, OperationId) {
    let Handle::Write(writer) = &client.handles[&handle] else {
        panic!("writer required")
    };
    assert!(Arc::ptr_eq(
        &writer.directory,
        &client.shared.budget.directory
    ));
    (writer.source.clone(), writer.operation_id)
}

fn assert_removed(source: &Path) {
    for path in [
        source.to_owned(),
        source.with_extension("work.json"),
        source.with_extension("budget.json"),
    ] {
        assert!(!path.exists(), "retained {}", path.display());
    }
}

#[test]
fn completed_close_unchanged_close_and_abort_explicitly_clean_their_spool() {
    for (write, abort) in [(true, false), (false, false), (true, true)] {
        let (_temp, shared, backend, branch) = fixture();
        let mut client = session(shared.clone(), &branch);
        let (opened, file) = open_file(&mut client, true);
        let (source, operation) = writer_source(&client, opened.handle);
        if write {
            file.write_all_at(b"new", 0).unwrap();
            client.dirty(opened.handle).unwrap();
        }
        client.close(opened.handle, abort).unwrap();
        assert_removed(&source);
        assert!(client.handles.is_empty());
        assert!(shared.file_writers.lock().unwrap().is_empty());
        if write && !abort {
            assert_eq!(
                backend
                    .operation_outcome(&client.identity, operation)
                    .unwrap()
                    .unwrap()
                    .commit_state,
                storage_contract::CommitState::Published
            );
        }
        // Same source can be reserved again only after the previous charge is removed.
        shared
            .budget
            .reserve(&client.identity.principal, &source, 0, 0)
            .unwrap()
            .cleanup(budget::Evidence::Reservation)
            .unwrap();
    }
}

#[test]
fn failed_open_record_preparation_releases_its_explicit_reservation() {
    let (_temp, shared, _backend, branch) = fixture();
    let mut client = session(shared.clone(), &branch);
    recovery_record::persist::fail_after_replace();
    assert!(
        client
            .open("report.txt".into(), true, false, false, false, false)
            .is_err()
    );
    assert!(client.handles.is_empty());
    assert!(shared.file_writers.lock().unwrap().is_empty());
    let names = fs::read_dir(&shared.spool)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        names,
        BTreeSet::from([".budget.lock".into(), "empty-directory".into()])
    );
    let (opened, _file) = open_file(&mut client, true);
    client.close(opened.handle, false).unwrap();
}

#[test]
fn published_close_cleanup_failure_keeps_receipt_not_a_false_detached_record() {
    for abort in [false, true] {
        let (_temp, shared, backend, branch) = fixture();
        let mut client = session(shared.clone(), &branch);
        let (opened, file) = open_file(&mut client, true);
        file.write_all_at(b"new", 0).unwrap();
        client.dirty(opened.handle).unwrap();
        client.flush(opened.handle).unwrap();
        let (source, operation) = writer_source(&client, opened.handle);
        // Fail the third unlink, after source/work have actually been removed.
        fs::remove_file(source.with_extension("budget.json")).unwrap();
        fs::create_dir(source.with_extension("budget.json")).unwrap();
        let request = if abort {
            Request::Abort {
                handle: opened.handle,
            }
        } else {
            Request::Close {
                handle: opened.handle,
            }
        };
        let error = client.dispatch(request).unwrap_err();
        assert_eq!(error.code, storage_contract::ErrorCode::RecoveryRequired);
        assert!(!error.retryable);
        assert_eq!(error.operation_id, Some(operation.to_string()));
        assert!(std::error::Error::source(&error.error).is_some());
        assert!(!source.exists());
        assert!(!source.with_extension("work.json").exists());
        assert!(client.handles.is_empty());
        assert!(shared.file_writers.lock().unwrap().is_empty());
        assert_eq!(
            backend
                .operation_outcome(&client.identity, operation)
                .unwrap()
                .unwrap()
                .commit_state,
            storage_contract::CommitState::Published
        );
        assert_eq!(
            shared
                .budget
                .reserve(&client.identity.principal, &shared.spool.join("next"), 0, 0)
                .err()
                .unwrap()
                .code,
            storage_contract::ErrorCode::SmbSpoolUnavailable
        );
    }
}

#[test]
#[ignore = "requires budget_io_eio.c in an isolated LD_PRELOAD child"]
fn real_close_directory_sync_failure_keeps_published_outcome_and_consumes_handle() {
    let root = PathBuf::from(std::env::var_os("ZXS_SMB_BUDGET_FAULT_ROOT").unwrap());
    let (_temp, mut shared, backend, branch) = fixture();
    fs::create_dir(root.join("empty-directory")).unwrap();
    let owner = Arc::get_mut(&mut shared).unwrap();
    owner.spool = root.clone();
    owner.budget = budget::Budget::recover(
        &root,
        crate::SpoolLimits {
            bytes_per_account: 1 << 20,
            bytes_total: 1 << 20,
            recovery_reserve_bytes: 1,
        },
    )
    .unwrap();
    let mut client = session(shared.clone(), &branch);
    let (opened, file) = open_file(&mut client, true);
    file.write_all_at(b"new", 0).unwrap();
    client.dirty(opened.handle).unwrap();
    client.flush(opened.handle).unwrap();
    let (source, operation) = writer_source(&client, opened.handle);
    fs::write(root.join(".inject"), b"enable cleanup fsync EIO").unwrap();
    let error = client
        .dispatch(Request::Close {
            handle: opened.handle,
        })
        .unwrap_err();
    assert_eq!(error.code, storage_contract::ErrorCode::RecoveryRequired);
    assert!(!error.retryable);
    assert_eq!(error.operation_id, Some(operation.to_string()));
    let mut cause: &dyn std::error::Error = &error.error;
    while let Some(source) = cause.source() {
        cause = source;
    }
    assert_eq!(
        cause.downcast_ref::<io::Error>().unwrap().raw_os_error(),
        Some(5)
    );
    assert_removed(&source);
    assert!(client.handles.is_empty());
    assert!(shared.file_writers.lock().unwrap().is_empty());
    assert_eq!(
        backend
            .operation_outcome(&client.identity, operation)
            .unwrap()
            .unwrap()
            .commit_state,
        storage_contract::CommitState::Published
    );
    assert_eq!(
        shared
            .budget
            .reserve(&client.identity.principal, &root.join("next"), 0, 0)
            .err()
            .unwrap()
            .code,
        storage_contract::ErrorCode::SmbSpoolUnavailable
    );
}

#[test]
fn completed_namespace_cleans_only_its_evidence_and_budget() {
    let (_temp, shared, backend, branch) = fixture();
    let mut client = session(shared.clone(), &branch);
    let (opened, _file) = open_file(&mut client, true);
    let (writer, _) = writer_source(&client, opened.handle);
    let operation = OperationId::generate();
    client
        .dispatch(Request::Namespace {
            kind: smb_protocol::protocol::NamespaceOperation::Mkdir,
            path: "new-directory".into(),
            destination: None,
            replace: false,
            operation_id: operation.to_string(),
        })
        .unwrap();
    assert!(
        !shared
            .spool
            .join(format!("{operation}.namespace.json"))
            .exists()
    );
    assert!(
        !shared
            .spool
            .join(format!("{operation}.budget.json"))
            .exists()
    );
    assert!(writer.is_file());
    assert!(writer.with_extension("work.json").is_file());
    assert!(writer.with_extension("budget.json").is_file());
    assert_eq!(
        backend
            .operation_outcome(&client.identity, operation)
            .unwrap()
            .unwrap()
            .commit_state,
        storage_contract::CommitState::Published
    );
    let replay = client
        .dispatch(Request::Namespace {
            kind: smb_protocol::protocol::NamespaceOperation::Mkdir,
            path: "new-directory".into(),
            destination: None,
            replace: false,
            operation_id: operation.to_string(),
        })
        .unwrap_err();
    assert_eq!(
        replay.code,
        storage_contract::ErrorCode::OperationIdConflict
    );
    client.close(opened.handle, false).unwrap();
}

#[test]
fn namespace_path_lock_rejection_cleans_its_unused_reservation() {
    let (_temp, shared, _backend, branch) = fixture();
    let mut client = session(shared.clone(), &branch);
    let (opened, _file) = open_file(&mut client, true);
    let (source, _) = writer_source(&client, opened.handle);
    let operation = OperationId::generate();
    let error = client
        .dispatch(Request::Namespace {
            kind: smb_protocol::protocol::NamespaceOperation::Unlink,
            path: "report.txt".into(),
            destination: None,
            replace: false,
            operation_id: operation.to_string(),
        })
        .unwrap_err();
    assert_eq!(error.code, storage_contract::ErrorCode::FileWriteBusy);
    assert!(
        !shared
            .spool
            .join(format!("{operation}.budget.json"))
            .exists()
    );
    assert!(source.is_file());
    client.close(opened.handle, false).unwrap();
}

#[test]
fn live_cleanup_rejects_replaced_payload_and_preserves_both_files() {
    let (_temp, shared, backend, branch) = fixture();
    let mut client = session(shared.clone(), &branch);
    let (opened, file) = open_file(&mut client, true);
    file.write_all_at(b"new", 0).unwrap();
    client.dirty(opened.handle).unwrap();
    client.flush(opened.handle).unwrap();
    let (source, operation) = writer_source(&client, opened.handle);
    let retained = shared.spool.join("original-payload");
    fs::rename(&source, &retained).unwrap();
    fs::write(&source, b"replacement").unwrap();
    fs::set_permissions(&source, fs::Permissions::from_mode(0o600)).unwrap();
    let error = client
        .dispatch(Request::Close {
            handle: opened.handle,
        })
        .unwrap_err();
    assert_eq!(error.code, storage_contract::ErrorCode::RecoveryRequired);
    assert!(!error.retryable);
    assert_eq!(error.operation_id, Some(operation.to_string()));
    assert_eq!(fs::read(&source).unwrap(), b"replacement");
    assert_eq!(fs::read(&retained).unwrap(), b"new");
    assert!(source.with_extension("work.json").is_file());
    assert!(source.with_extension("budget.json").is_file());
    let record: Value =
        json_codec::from_slice(&fs::read(source.with_extension("work.json")).unwrap()).unwrap();
    assert_eq!(record["handle_state"], "active");
    assert!(client.handles.is_empty());
    assert!(shared.file_writers.lock().unwrap().is_empty());
    assert_eq!(
        backend
            .operation_outcome(&client.identity, operation)
            .unwrap()
            .unwrap()
            .commit_state,
        storage_contract::CommitState::Published
    );
    assert_eq!(
        shared
            .budget
            .reserve(&client.identity.principal, &shared.spool.join("next"), 0, 0)
            .err()
            .unwrap()
            .code,
        storage_contract::ErrorCode::SmbSpoolUnavailable
    );
}
