use super::*;
use crate::{
    DirectoryImportProgress, DirectoryImportStopOutcome, DirectoryImportStopRequest, WorkspaceState,
};

fn stopped_request(
    engine: &Engine,
    progress: &DirectoryImportProgress,
) -> DirectoryImportStopRequest {
    DirectoryImportStopRequest {
        principal: "admin".into(),
        target: engine
            .registered_disks()
            .find(|disk| disk.slot_id == "P01")
            .unwrap()
            .identity(),
        import_operation_id: progress.operation_id(),
        root_operation_id: progress.root_operation_id(),
        workspace_id: progress.workspace_id().map(str::to_owned),
        commit_request: progress.request().cloned(),
        stop_operation_id: operation(),
    }
}
fn interrupt_import(
    engine: &mut Engine,
    source: &Path,
    stop: usize,
    persist_failed_intent: bool,
) -> DirectoryImportProgress {
    let plan = inspect(source)
        .prepare_new_disk_import(engine, "P01")
        .unwrap();
    let digest = confirmation(&plan);
    let mut calls = 0;
    let mut retained = None;
    plan.execute_with_progress(engine, "admin", operation(), digest, |progress| {
        calls += 1;
        if calls != stop || persist_failed_intent {
            retained = Some(progress.clone());
        }
        if calls == stop {
            return Err(Error::new(
                ErrorCode::IoError,
                "simulated interrupted workflow boundary",
            ));
        }
        Ok(())
    })
    .unwrap_err();
    retained.unwrap()
}
fn stopped(outcome: DirectoryImportStopOutcome) -> crate::OperationResult {
    let DirectoryImportStopOutcome::Stopped(result) = outcome else {
        panic!("unexpected publication")
    };
    result
}

#[test]
fn stop_after_cold_restart_preserves_root_source_and_all_staged_bytes() {
    for (stop, persist_intent, abort_before_restart) in [
        (2, true, false),  // Root intent, no root creation.
        (3, false, false), // Root created; workspace intent not retained.
        (3, true, false),  // Workspace identity retained before creation.
        (4, true, false),  // Empty workspace created.
        (5, true, false),  // All content staged; request retained before publication.
        (5, true, true),   // Stop closed the workspace, but its receipt was not yet published.
    ] {
        let temp = tempfile::tempdir().unwrap();
        let source = make_source(temp.path());
        let original = contents(&source);
        let mut engine = engine(temp.path());
        let progress = interrupt_import(&mut engine, &source, stop, persist_intent);
        let request = stopped_request(&engine, &progress);
        let staged = request
            .workspace_id
            .as_ref()
            .and_then(|id| engine.workspace(id).ok())
            .map(|workspace| {
                workspace
                    .files
                    .values()
                    .map(|file| {
                        (
                            file.source_path.clone(),
                            fs::read(&file.source_path).unwrap(),
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if abort_before_restart {
            engine
                .abort_workspace(request.workspace_id.as_ref().unwrap(), "admin")
                .unwrap();
        }
        drop(engine);
        let mut engine = Engine::open(&temp.path().join("system")).unwrap();
        assert!(
            engine
                .directory_import_stop_result(&request)
                .unwrap()
                .is_none()
        );
        let result = stopped(engine.stop_directory_import(&request).unwrap());
        assert_eq!(result.kind, "directory.import.stop");
        assert_eq!(result.operation_id, request.stop_operation_id);
        assert!(
            !engine
                .state()
                .operations
                .contains_key(&request.import_operation_id)
        );
        assert_eq!(engine.state().repositories.is_empty(), stop == 2);
        assert_eq!(contents(&source), original);
        for (path, bytes) in &staged {
            assert_eq!(fs::read(path).unwrap(), *bytes);
        }
        if let Some(id) = &request.workspace_id {
            if stop >= 4 {
                assert_eq!(engine.workspace(id).unwrap().state, WorkspaceState::Aborted);
            } else {
                assert_eq!(
                    engine.workspace(id).unwrap_err().code(),
                    ErrorCode::WorkspaceNotFound
                );
            }
        }
        let sequence = engine.state().sequence;
        assert_eq!(
            stopped(engine.stop_directory_import(&request).unwrap()),
            result
        );
        assert_eq!(engine.state().sequence, sequence);
        drop(engine);
        let mut engine = Engine::open(&temp.path().join("system")).unwrap();
        assert_eq!(
            stopped(
                engine
                    .directory_import_stop_result(&request)
                    .unwrap()
                    .unwrap()
            ),
            result
        );
        assert_eq!(
            stopped(engine.stop_directory_import(&request).unwrap()),
            result
        );
        let plan = inspect(&source)
            .prepare_new_disk_import(&engine, "P01")
            .unwrap();
        let digest = confirmation(&plan);
        let error = plan
            .execute(&mut engine, "admin", request.import_operation_id, digest)
            .unwrap_err();
        assert_eq!(error.cause().code(), ErrorCode::OperationIdConflict);
        assert_eq!(contents(&source), original);
    }
}

#[test]
fn stop_rejects_wrong_binding_and_active_preparation_before_any_abort() {
    let temp = tempfile::tempdir().unwrap();
    let source = make_source(temp.path());
    let mut engine = engine(temp.path());
    let progress = interrupt_import(&mut engine, &source, 4, true);
    let request = stopped_request(&engine, &progress);
    let workspace = request.workspace_id.as_ref().unwrap();
    let sequence = engine.state().sequence;
    let mut foreign = request.clone();
    foreign.principal = "other".into();
    assert!(engine.stop_directory_import(&foreign).is_err());
    foreign = request.clone();
    foreign.target.binding_generation += 1;
    assert_eq!(
        engine.stop_directory_import(&foreign).unwrap_err().code(),
        ErrorCode::PlanStale
    );
    foreign = request.clone();
    foreign.root_operation_id = Some(operation());
    assert_eq!(
        engine.stop_directory_import(&foreign).unwrap_err().code(),
        ErrorCode::RecoveryRequired
    );
    engine.blocked = true;
    assert_eq!(
        engine.stop_directory_import(&request).unwrap_err().code(),
        ErrorCode::RecoveryRequired
    );
    engine.blocked = false;
    let held = engine
        .prepare_upload(
            workspace,
            &crate::new_id("entry").unwrap(),
            "file",
            &source.join("binary.bin"),
            1,
        )
        .unwrap();
    assert_eq!(
        engine.stop_directory_import(&request).unwrap_err().code(),
        ErrorCode::CommitPreparationBusy
    );
    drop(held);
    assert_eq!(engine.state().sequence, sequence);
    assert_eq!(
        engine.workspace(workspace).unwrap().state,
        WorkspaceState::Open
    );
    let result = stopped(engine.stop_directory_import(&request).unwrap());
    foreign = request.clone();
    foreign.target.binding_generation += 1;
    assert_eq!(
        engine
            .directory_import_stop_result(&foreign)
            .unwrap_err()
            .code(),
        ErrorCode::IdempotencyKeyReused
    );
    assert_eq!(
        stopped(
            engine
                .directory_import_stop_result(&request)
                .unwrap()
                .unwrap()
        ),
        result
    );
}

#[test]
fn stopping_an_exact_published_import_returns_original_receipt_without_mutation() {
    let temp = tempfile::tempdir().unwrap();
    let source = make_source(temp.path());
    let mut engine = engine(temp.path());
    let plan = inspect(&source)
        .prepare_new_disk_import(&engine, "P01")
        .unwrap();
    let digest = confirmation(&plan);
    let mut retained = None;
    let result = plan
        .execute_with_progress(&mut engine, "admin", operation(), digest, |progress| {
            retained = Some(progress.clone());
            Ok(())
        })
        .unwrap();
    let request = stopped_request(&engine, &retained.unwrap());
    drop(engine);
    let mut engine = Engine::open(&temp.path().join("system")).unwrap();
    let sequence = engine.state().sequence;
    let DirectoryImportStopOutcome::Published(observed) =
        engine.stop_directory_import(&request).unwrap()
    else {
        panic!("published import must not become stopped")
    };
    assert_eq!(observed, result);
    assert_eq!(engine.state().sequence, sequence);
    assert!(
        !engine
            .state()
            .operations
            .contains_key(&request.stop_operation_id)
    );
    assert_eq!(
        engine
            .workspace(request.workspace_id.as_ref().unwrap())
            .unwrap()
            .state,
        WorkspaceState::Committed
    );
    let mut wrong = request.clone();
    wrong.commit_request.as_mut().unwrap().fencing_token += 1;
    assert_eq!(
        engine.stop_directory_import(&wrong).unwrap_err().code(),
        ErrorCode::RecoveryRequired
    );
    wrong = request.clone();
    wrong.commit_request = None;
    assert_eq!(
        engine.stop_directory_import(&wrong).unwrap_err().code(),
        ErrorCode::PlanInvalid
    );
}

#[test]
fn stopped_import_identity_is_fenced_at_generic_native_mutation_admission() {
    let temp = tempfile::tempdir().unwrap();
    let source = make_source(temp.path());
    let mut engine = engine(temp.path());
    let progress = interrupt_import(&mut engine, &source, 4, true);
    let request = stopped_request(&engine, &progress);
    let receipt = stopped(engine.stop_directory_import(&request).unwrap());
    let workspace = engine.create_workspace(&branch(&engine), "admin").unwrap();
    let sequence = engine.state().sequence;
    let error = engine
        .commit_workspace(crate::CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: request.import_operation_id,
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "Try a fresh native save".into(),
            request_digest: None,
        })
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::OperationIdConflict);
    assert_eq!(engine.state().sequence, sequence);
    assert_eq!(
        engine
            .create_repository(
                &crate::RepositoryName::parse("other").unwrap(),
                "P01",
                1,
                "admin",
                request.import_operation_id
            )
            .unwrap_err()
            .code(),
        ErrorCode::OperationIdConflict
    );
    assert_eq!(engine.state().sequence, sequence);
    assert_eq!(
        stopped(
            engine
                .directory_import_stop_result(&request)
                .unwrap()
                .unwrap()
        ),
        receipt
    );
}
