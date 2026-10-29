//! Published SMB saves retain their original identity until reconciliation completes.
use super::published_reconciliation_failure::{obstruct_terminal_mirror, restore_terminal_mirror};
use super::*;

#[test]
fn terminal_mirror_failure_retains_active_and_detached_smb_work_after_publication() {
    for (close_first, change_after_error) in [(false, false), (false, true), (true, false)] {
        let (temporary, shared, backend, branch) = fixture();
        let mut client = session(shared.clone(), &branch);
        let (opened, file) = open_file(&mut client, true);
        file.write_all_at(b"new", 0).unwrap();
        client.dirty(opened.handle).unwrap();
        let (source, workspace, operation) = match &client.handles[&opened.handle] {
            Handle::Write(writer) => (
                writer.source.clone(),
                writer.workspace.clone(),
                writer.operation_id,
            ),
            _ => unreachable!(),
        };
        let (before, operations, obstruction) = backend.inner.test_with_engine_locked(|engine| {
            (
                engine.state().sequence,
                engine.state().operations.len(),
                obstruct_terminal_mirror(engine, &workspace.workspace_id),
            )
        });
        let request = if close_first {
            Request::Close {
                handle: opened.handle,
            }
        } else {
            Request::Flush {
                handle: opened.handle,
            }
        };
        let mut failure = client.dispatch(request).unwrap_err();
        let manifest = obstruction.join().unwrap();
        assert_eq!(failure.code, storage_contract::ErrorCode::RecoveryRequired);
        assert_eq!(failure.status, ::http::StatusCode::SERVICE_UNAVAILABLE);
        assert!(!failure.retryable);
        assert_eq!(
            failure.operation_id.as_deref(),
            Some(operation.to_string().as_str())
        );
        assert!(failure.message.contains("published"));
        assert!(failure.message.contains(&operation.to_string()));
        let receipt = backend.operation(&client.identity, operation).unwrap();
        assert_eq!(
            receipt.commit_state,
            storage_contract::CommitState::Published
        );
        assert_eq!(receipt.sequence, before + 1);
        assert_eq!(
            receipt.protection_at_publish,
            storage_contract::ProtectionAtPublish::TargetMet
        );
        let Some(crate::http_boundary::ApiErrorSource::Engine(
            storage_engine::Error::PublishedWorkspaceReconciliation(context),
        )) = failure.error.source.as_deref()
        else {
            panic!("the mapped error must retain its exact published workspace context");
        };
        assert_eq!(context.result().operation_id, operation);
        assert_eq!(context.result().request_digest, receipt.request_digest);
        assert_eq!(context.workspace_id(), workspace.workspace_id);
        assert_eq!(context.principal(), client.identity.principal);
        use std::error::Error as _;
        let mut cause = context.source();
        let mut concrete_filesystem_source = false;
        while let Some(source) = cause {
            concrete_filesystem_source |=
                source.is::<platform_io::fs_safety::Error>() || source.is::<std::io::Error>();
            cause = source.source();
        }
        assert!(concrete_filesystem_source);
        assert!(
            !json_codec::to_string(&failure.error)
                .unwrap()
                .contains(temporary.path().to_str().unwrap())
        );
        let crate::http_boundary::ApiErrorSource::Engine(published) =
            *failure.error.source.take().unwrap()
        else {
            unreachable!()
        };
        let classified = crate::engine_backend::error(
            published
                .context(
                    storage_contract::ErrorCode::UnsafePath,
                    "/private/outer-context",
                )
                .retryable(),
        );
        assert_eq!(classified.status, ::http::StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            classified.code,
            storage_contract::ErrorCode::RecoveryRequired
        );
        assert!(!classified.retryable);
        assert!(classified.message.contains("already published"));
        assert!(!classified.message.contains("/private/outer-context"));
        let ordinary = crate::engine_backend::error(storage_engine::Error::new(
            storage_contract::ErrorCode::UnsafePath,
            "ordinary prepublication path refusal",
        ));
        assert_eq!(ordinary.status, ::http::StatusCode::BAD_REQUEST);
        assert_eq!(ordinary.code, storage_contract::ErrorCode::UnsafePath);
        assert_eq!(backend.uploads.load(Ordering::SeqCst), 1);
        assert_eq!(
            backend.ready().unwrap_err().code,
            storage_contract::ErrorCode::RecoveryRequired
        );
        assert_eq!(fs::read(&source).unwrap(), b"new");

        if !close_first {
            let assert_active = |client: &Session, expected_bytes: &[u8]| {
                let writer = match &client.handles[&opened.handle] {
                    Handle::Write(writer) => writer,
                    _ => unreachable!(),
                };
                assert!(writer.dirty && !writer.committed);
                assert_eq!(writer.operation_id, operation);
                assert_eq!(writer.workspace.workspace_id, workspace.workspace_id);
                assert_eq!(writer.workspace.fencing_token, workspace.fencing_token);
                assert_eq!(writer.workspace.base_head, workspace.base_head);
                assert_eq!(writer.source, source);
                assert_eq!(fs::read(&source).unwrap(), expected_bytes);
                let active: Value =
                    json_codec::from_slice(&fs::read(source.with_extension("work.json")).unwrap())
                        .unwrap();
                assert_eq!(active["handle_state"], "active");
                assert_eq!(active["dirty"], true);
                assert_eq!(active["published"], false);
                assert_eq!(active["operation_id"], operation.to_string());
                assert_eq!(active["workspace_id"], workspace.workspace_id);
                assert_eq!(active["expected_head"], workspace.base_head);
                assert_eq!(active["fencing_token"], workspace.fencing_token.to_string());
                assert_eq!(active["source_path"], source.to_str().unwrap());
            };
            assert_active(&client, b"new");

            // Dirty acknowledges changed bytes, not publication. It may succeed
            // while reconciliation is pending, but must retain this generation.
            if change_after_error {
                file.write_all_at(b"end", 0).unwrap();
                assert_eq!(client.dirty(opened.handle).unwrap(), json!({"dirty": true}));
                assert_active(&client, b"end");
            }
            for request in [
                Request::Flush {
                    handle: opened.handle,
                },
                Request::Close {
                    handle: opened.handle,
                },
            ] {
                let is_flush = matches!(request, Request::Flush { .. });
                let failure = client.dispatch(request).unwrap_err();
                assert_eq!(failure.code, storage_contract::ErrorCode::RecoveryRequired);
                assert_eq!(failure.status, ::http::StatusCode::SERVICE_UNAVAILABLE);
                assert!(!failure.retryable);
                assert_eq!(
                    failure.operation_id.as_deref(),
                    Some(operation.to_string().as_str())
                );
                if is_flush {
                    assert_active(&client, if change_after_error { b"end" } else { b"new" });
                }
            }
        }

        assert!(!client.handles.contains_key(&opened.handle));
        assert!(client.writer_locks.is_empty());
        assert!(shared.file_writers.lock().unwrap().is_empty());
        assert_eq!(
            fs::read(&source).unwrap(),
            if change_after_error { b"end" } else { b"new" }
        );
        assert!(source.with_extension("budget.json").is_file());
        let detached: Value =
            json_codec::from_slice(&fs::read(source.with_extension("work.json")).unwrap()).unwrap();
        assert_eq!(detached["handle_state"], "detached");
        assert_eq!(detached["detach_error"]["code"], "RECOVERY_REQUIRED");
        assert_eq!(detached["retained_bytes_synced"], true);
        assert_eq!(detached["operation_id"], operation.to_string());
        assert_eq!(detached["workspace_id"], workspace.workspace_id);
        assert_eq!(
            detached["fencing_token"],
            workspace.fencing_token.to_string()
        );
        assert_eq!(backend.uploads.load(Ordering::SeqCst), 1);
        backend.inner.test_with_engine_locked(|engine| {
            assert!(engine.is_read_only());
            assert_eq!(engine.state().sequence, before + 1);
            assert_eq!(engine.state().operations.len(), operations + 1);
            let retained = engine.workspace(&workspace.workspace_id).unwrap();
            assert_ne!(retained.state, storage_contract::WorkspaceState::Aborted);
            assert_eq!(retained.fencing_token, workspace.fencing_token);
            assert_eq!(retained.files.len(), 1);
            for staged in retained.files.values() {
                assert_eq!(fs::read(&staged.source_path).unwrap(), b"new");
            }
        });
        assert!(manifest.with_file_name("commit-request.json").is_file());
        let entry = client
            .find(&name_policy::LookupPath::parse("report.txt").unwrap())
            .unwrap()
            .unwrap();
        let content = backend
            .content(&client.identity, &entry.version_id)
            .unwrap();
        let mut bytes = [0; 3];
        content.file.read_exact_at(&mut bytes, 0).unwrap();
        assert_eq!(&bytes, b"new");
        restore_terminal_mirror(&manifest);
    }
}

#[test]
fn explicit_abort_after_completed_publication_does_not_erase_lost_reply_receipt() {
    let (_temporary, shared, backend, branch) = fixture();
    let mut client = session(shared, &branch);
    let (opened, file) = open_file(&mut client, true);
    file.write_all_at(b"new", 0).unwrap();
    client.dirty(opened.handle).unwrap();
    let (operation, workspace, source) = match &client.handles[&opened.handle] {
        Handle::Write(writer) => (
            writer.operation_id,
            writer.workspace.clone(),
            writer.source.clone(),
        ),
        _ => unreachable!(),
    };
    backend.fail.store(true, Ordering::SeqCst);
    assert_eq!(
        client.flush(opened.handle).unwrap_err().code,
        storage_contract::ErrorCode::InjectedFailure
    );
    let receipt = backend.operation(&client.identity, operation).unwrap();
    assert_eq!(
        receipt.commit_state,
        storage_contract::CommitState::Published
    );
    backend
        .abort_workspace(&client.identity, &workspace.workspace_id)
        .unwrap();
    let aborted = backend
        .workspace(&client.identity, &workspace.workspace_id)
        .unwrap();
    assert_eq!(aborted.state, storage_contract::WorkspaceState::Aborted);
    assert_eq!(aborted.fencing_token, workspace.fencing_token + 2);

    // Abort closes a workspace, not its permanent publication fact. A lost
    // reply still reconciles by the original ID after this explicit action.
    let saved = client.flush(opened.handle).unwrap();
    assert_eq!(saved["commit_state"], "PUBLISHED");
    assert_eq!(saved["operation_id"], operation.to_string());
    assert_eq!(saved["request_digest"], receipt.request_digest);
    assert_eq!(backend.uploads.load(Ordering::SeqCst), 1);
    client.close(opened.handle, false).unwrap();
    assert!(!source.exists());
    assert!(!source.with_extension("work.json").exists());
    assert!(!source.with_extension("budget.json").exists());
    let retained = backend.operation(&client.identity, operation).unwrap();
    assert_eq!(retained.commit_state, receipt.commit_state);
    assert_eq!(retained.sequence, receipt.sequence);
    assert_eq!(retained.request_digest, receipt.request_digest);
    let workspace = backend
        .workspace(&client.identity, &workspace.workspace_id)
        .unwrap();
    assert_eq!(workspace.state, aborted.state);
    assert_eq!(workspace.fencing_token, aborted.fencing_token);
}

#[test]
fn failed_publication_reconciliation_retries_without_restaging_or_second_version() {
    for failure in [RECONCILIATION_SNAPSHOT, RECONCILIATION_CONTENT] {
        for lost_reply in [false, true] {
            let (_temp, shared, backend, branch) = fixture();
            let mut one = session(shared, &branch);
            let (opened, file) = open_file(&mut one, true);
            file.write_all_at(b"new", 0).unwrap();
            one.dirty(opened.handle).unwrap();
            let operation_id = match &one.handles[&opened.handle] {
                Handle::Write(writer) => writer.operation_id,
                _ => unreachable!(),
            };
            backend
                .reconciliation_failure
                .store(failure, Ordering::SeqCst);
            if lost_reply {
                backend.fail.store(true, Ordering::SeqCst);
                assert_eq!(
                    one.flush(opened.handle).unwrap_err().code,
                    storage_contract::ErrorCode::InjectedFailure
                );
            }
            assert_eq!(
                one.flush(opened.handle).unwrap_err().code,
                storage_contract::ErrorCode::MetadataUnavailable
            );
            let published = backend
                .operation(&one.identity, operation_id)
                .and_then(crate::operation_wire::operation_wire)
                .unwrap();
            assert_eq!(published["commit_state"], "PUBLISHED");
            assert_eq!(backend.uploads.load(Ordering::SeqCst), 1);

            let saved = one.flush(opened.handle).unwrap();
            assert_eq!(saved["commit_state"], "PUBLISHED");
            assert_eq!(backend.uploads.load(Ordering::SeqCst), 1);
            let entry = one
                .find(&name_policy::LookupPath::parse("report.txt").unwrap())
                .unwrap()
                .unwrap();
            assert_eq!(
                super::version_count(backend.as_ref(), &one.identity, &entry.file_id),
                2
            );
            one.close(opened.handle, false).unwrap();
        }
    }
}

#[test]
fn failed_publication_reconciliation_preserves_later_bytes_for_authorized_rebase() {
    for failure in [RECONCILIATION_SNAPSHOT, RECONCILIATION_CONTENT] {
        let (_temp, shared, backend, branch) = fixture();
        let mut one = session(shared, &branch);
        let (opened, file) = open_file(&mut one, true);
        file.write_all_at(b"new", 0).unwrap();
        one.dirty(opened.handle).unwrap();
        backend
            .reconciliation_failure
            .store(failure, Ordering::SeqCst);
        assert_eq!(
            one.flush(opened.handle).unwrap_err().code,
            storage_contract::ErrorCode::MetadataUnavailable
        );
        file.write_all_at(b"end", 0).unwrap();
        assert_eq!(
            one.flush(opened.handle).unwrap_err().code,
            storage_contract::ErrorCode::WorkspaceRebaseRequired
        );
        let writer = match &one.handles[&opened.handle] {
            Handle::Write(writer) => writer,
            _ => unreachable!(),
        };
        assert!(writer.committed && writer.dirty);
        assert_eq!(fs::read(&writer.source).unwrap(), b"end");
        let record: Value =
            json_codec::from_slice(&fs::read(writer.source.with_extension("work.json")).unwrap())
                .unwrap();
        assert_eq!(record["published"], true);
        assert_eq!(record["dirty"], true);
        assert_eq!(backend.uploads.load(Ordering::SeqCst), 1);
        let entry = one
            .find(&name_policy::LookupPath::parse("report.txt").unwrap())
            .unwrap()
            .unwrap();
        let content = backend.content(&one.identity, &entry.version_id).unwrap();
        let mut bytes = [0; 3];
        content.file.read_exact_at(&mut bytes, 0).unwrap();
        assert_eq!(&bytes, b"new");

        one.dirty(opened.handle).unwrap();
        one.flush(opened.handle).unwrap();
        assert_eq!(backend.uploads.load(Ordering::SeqCst), 2);
        let entry = one
            .find(&name_policy::LookupPath::parse("report.txt").unwrap())
            .unwrap()
            .unwrap();
        let content = backend.content(&one.identity, &entry.version_id).unwrap();
        content.file.read_exact_at(&mut bytes, 0).unwrap();
        assert_eq!(&bytes, b"end");
        one.close(opened.handle, false).unwrap();
    }
}
