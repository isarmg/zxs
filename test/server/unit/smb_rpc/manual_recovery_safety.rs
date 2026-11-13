use super::*;
use std::os::unix::fs::OpenOptionsExt;

fn opened(shared: &Arc<Shared>, branch: &str) -> (Session, File, u64, OperationId, PathBuf) {
    let mut client = session(shared.clone(), branch);
    let (opened, descriptor) = client.open("report.txt".into(), true, false, false, false, false)
        .unwrap();
    let file = File::from(descriptor);
    client.prepare_write(opened.handle, 5, 5).unwrap();
    file.write_all_at(b"first", 0).unwrap();
    client.dirty(opened.handle).unwrap();
    let (operation, record) = match client.handles.get(&opened.handle).unwrap() {
        Handle::Write(writer, _) => (
            writer.operation_id, writer.source.with_extension("work.json"),
        ),
        _ => panic!("manual writer retains a typed working record"),
    };
    (client, file, opened.handle, operation, record)
}

fn request(operation: OperationId, record: &Path, resume: bool) -> Request<OperationId> {
    Request::RecoverManualWorking {
        operation_id: operation,
        record: record.file_name().unwrap().to_str().unwrap().into(),
        resume,
    }
}

fn material(shared: &Shared) -> BTreeMap<PathBuf, Vec<u8>> {
    fs::read_dir(&shared.spool).unwrap().map(|entry| entry.unwrap().path())
        .filter(|path| path.is_file()).map(|path| {
            let bytes = fs::read(&path).unwrap();
            (path, bytes)
        }).collect()
}

#[test]
fn cold_resume_persists_reference_before_save_and_missing_frozen_material_is_query_only() {
    let (_root, shared, backend, branch) = fresh();
    let (mut client, file, _handle, operation, record) = opened(&shared, &branch);
    client.dispatch_admitted(request(operation, &record, true)).unwrap();
    let reference: Value = json_codec::from_slice(&fs::read(&record).unwrap()).unwrap();
    assert_eq!(reference["phase"]["state"], "frozen");
    assert_eq!(reference["phase"]["revision"]["operation"], json!(operation));
    // Successful resume retired its body; retain the query record elsewhere.
    assert!(!shared.spool.join(format!("manual-{operation}")).exists());
    for name in [format!("manual-{operation}.work.json")] {
        fs::rename(
            shared.spool.join(&name), shared.spool.join(format!("retained-{name}")),
        ).unwrap();
    }
    file.write_all_at(b"later", 0).unwrap();
    let before = backend.inner.test_with_engine_locked(|engine| engine.state().clone());
    let bytes = material(&shared);
    for resume in [false, true] {
        let Reply::Json(value) = client.dispatch_admitted(request(operation, &record, resume))
            .unwrap() else { panic!("recovery response is JSON"); };
        assert_eq!(value["draft_persisted"], true);
        assert_eq!(value["frozen_material_missing"], true);
        assert_eq!(value["original_receipt"]["commit_id"], Value::Null);
        assert_eq!(material(&shared), bytes);
        backend.inner.test_with_engine_locked(|engine| assert_eq!(engine.state(), &before));
    }
}

#[test]
fn partial_body_or_budget_owner_is_never_recognized_as_a_new_working_freeze() {
    for budget_only in [false, true] {
        let (_root, shared, backend, branch) = fresh();
        let (mut client, _file, _handle, operation, record) = opened(&shared, &branch);
        let name = if budget_only {
            format!("manual-{operation}.budget.json")
        } else {
            format!("manual-{operation}")
        };
        let mut partial = fs::OpenOptions::new().create_new(true).write(true).mode(0o600)
            .open(shared.spool.join(name)).unwrap();
        partial.write_all(b"incomplete original bytes").unwrap();
        partial.sync_all().unwrap();
        let bytes = material(&shared);
        let before = backend.inner.test_with_engine_locked(|engine| engine.state().clone());
        for resume in [false, true] {
            let error = client.dispatch_admitted(request(operation, &record, resume)).unwrap_err();
            assert_eq!(error.code, storage_contract::ErrorCode::RecoveryRequired);
            assert_eq!(material(&shared), bytes);
            backend.inner.test_with_engine_locked(|engine| assert_eq!(engine.state(), &before));
        }
    }
}

#[test]
fn stale_working_record_cannot_repackage_a_previously_published_operation() {
    let (_root, shared, backend, branch) = fresh();
    let (mut client, file, handle, operation, record) = opened(&shared, &branch);
    let old_working = fs::read(&record).unwrap();
    client.flush(handle).unwrap();
    fs::write(&record, old_working).unwrap();
    assert!(!shared.spool.join(format!("manual-{operation}")).exists());
    for name in [format!("manual-{operation}.work.json")] {
        fs::rename(
            shared.spool.join(&name), shared.spool.join(format!("retained-{name}")),
        ).unwrap();
    }
    file.write_all_at(b"later", 0).unwrap();
    let before = backend.inner.test_with_engine_locked(|engine| engine.state().clone());
    let bytes = material(&shared);
    for state in [
        storage_contract::CommitState::Published,
        storage_contract::CommitState::InDoubt,
        storage_contract::CommitState::RecoveryRequired,
    ] {
        // Fault at the Backend outcome boundary, retaining the actual original
        // engine receipt; this does not simulate a core journal transition.
        *backend.recovery_outcome_state.lock().unwrap() = Some(state);
        for resume in [false, true] {
            let error = client.dispatch_admitted(request(operation, &record, resume)).unwrap_err();
            assert_eq!(error.code, storage_contract::ErrorCode::RecoveryRequired);
            assert_eq!(material(&shared), bytes);
            backend.inner.test_with_engine_locked(|engine| assert_eq!(engine.state(), &before));
        }
    }
    backend.recovery_outcome_error.store(true, Ordering::SeqCst);
    for resume in [false, true] {
        assert!(client.dispatch_admitted(request(operation, &record, resume)).is_err());
        assert_eq!(material(&shared), bytes);
        backend.inner.test_with_engine_locked(|engine| assert_eq!(engine.state(), &before));
    }
}

#[test]
fn conflicting_revision_and_previous_ambiguous_working_format_are_preserved_and_rejected() {
    let (_root, shared, backend, branch) = fresh();
    let (mut client, _file, handle, operation, record) = opened(&shared, &branch);
    client.flush(handle).unwrap();
    let original: Value = json_codec::from_slice(&fs::read(&record).unwrap()).unwrap();
    let before = backend.inner.test_with_engine_locked(|engine| engine.state().clone());
    for previous_format in [false, true] {
        let mut altered = original.clone();
        if previous_format {
            altered["format"] = json!("smb-manual-working-v1");
        } else {
            altered["phase"]["revision"]["size"] = json!("6");
        }
        fs::write(&record, json_codec::to_vec(&altered).unwrap()).unwrap();
        let bytes = material(&shared);
        for resume in [false, true] {
            assert!(client.dispatch_admitted(request(operation, &record, resume)).is_err());
            assert_eq!(material(&shared), bytes);
            backend.inner.test_with_engine_locked(|engine| assert_eq!(engine.state(), &before));
        }
    }
    let mut wrong_operation = original;
    wrong_operation["origin"]["operation"] = json!(OperationId::generate().unwrap());
    fs::write(&record, json_codec::to_vec(&wrong_operation).unwrap()).unwrap();
    let bytes = material(&shared);
    assert!(client.dirty(handle).is_err());
    assert_eq!(material(&shared), bytes);
    backend.inner.test_with_engine_locked(|engine| assert_eq!(engine.state(), &before));

}

#[test]
fn complete_body_before_reference_is_not_submit_authority_and_stale_writer_cannot_regress_phase() {
    let (_root, shared, backend, branch) = fresh();
    let (mut client, file, handle, operation, record) = opened(&shared, &branch);
    let writer = match client.handles.get(&handle).unwrap() {
        Handle::Write(writer, _) => writer,
        _ => panic!("manual writer owns the crash-cut source"),
    };
    manual_recovery::freeze(&shared, &identity(&shared), writer, 4096).unwrap();
    let before = backend.inner.test_with_engine_locked(|engine| engine.state().clone());
    let bytes = material(&shared);
    // Crash cut: the complete immutable body exists, but the source has not
    // crossed its Frozen-reference barrier. The standalone RPC cannot skip it.
    assert!(client.dispatch_admitted(Request::RecoverManualDraft {
        operation_id: operation, resume: true,
    }).is_err());
    assert_eq!(material(&shared), bytes);
    backend.inner.test_with_engine_locked(|engine| assert_eq!(engine.state(), &before));
    client.dispatch_admitted(request(operation, &record, true)).unwrap();
    let frozen: Value = json_codec::from_slice(&fs::read(&record).unwrap()).unwrap();
    assert_eq!(frozen["phase"]["state"], "frozen");
    file.write_all_at(b"later", 0).unwrap();
    client.dirty(handle).unwrap();
    let after_dirty: Value = json_codec::from_slice(&fs::read(&record).unwrap()).unwrap();
    assert_eq!(after_dirty["phase"], frozen["phase"]);
    assert_eq!(client.flush(handle).unwrap_err().code,
        storage_contract::ErrorCode::WorkspaceRebaseRequired);
    let after_flush: Value = json_codec::from_slice(&fs::read(&record).unwrap()).unwrap();
    assert_eq!(after_flush["phase"], frozen["phase"]);
}

#[test]
fn unsubmitted_frozen_body_corruption_or_loss_never_adopts_matching_current_fd() {
    let (_root, shared, backend, branch) = fresh();
    let (mut client, file, handle, operation, record) = opened(&shared, &branch);
    let writer = match client.handles.get(&handle).unwrap() {
        Handle::Write(writer, _) => writer,
        _ => panic!("manual writer owns the original source"),
    };
    let revision = manual_recovery::freeze(&shared, &identity(&shared), writer, 4096).unwrap();
    if let Handle::Write(writer, _) = client.handles.get_mut(&handle).unwrap() {
        writer.container.as_mut().unwrap().revision = Some(revision);
        recovery_record(writer, "alice", &branch).unwrap();
    }
    let frozen = shared.spool.join(format!("manual-{operation}"));
    fs::write(&frozen, b"later").unwrap();
    file.write_all_at(b"later", 0).unwrap();
    let before = backend.inner.test_with_engine_locked(|engine| engine.state().clone());
    for missing in [false, true] {
        if missing {
            fs::rename(&frozen, shared.spool.join(format!("retained-{operation}"))).unwrap();
        }
        let bytes = material(&shared);
        let Reply::Json(query) = client.dispatch_admitted(request(operation, &record, false))
            .unwrap() else { panic!("original-result query is JSON"); };
        assert_eq!(query["draft_persisted"], false);
        assert!(client.dispatch_admitted(request(operation, &record, true)).is_err());
        assert_eq!(material(&shared), bytes);
        backend.inner.test_with_engine_locked(|engine| assert_eq!(engine.state(), &before));
    }
}
