use crate::{Engine, OperationId};
use storage_contract::file_lifecycle::{Detail, Request, Timestamp};
use storage_contract::history_window::{
    HistoryCollection, HistoryFileId, HistoryPageLimit, HistoryVersionId, HistoryWindowCancellation,
    HistoryWindowRequest,
};

fn actual() -> (tempfile::TempDir, Engine, HistoryFileId, HistoryVersionId) {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &temp.path().join("state"),
        crate::Config::development([temp.path().join("disk")]).unwrap(),
    )
    .unwrap();
    let repository = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("observed").unwrap(),
            "P01",
            1,
            "admin",
            crate::OperationId::generate().unwrap(),
        )
        .unwrap();
    let workspace = engine
        .create_workspace(&repository.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "file.txt", b"retained source")
        .unwrap();
    engine
        .commit_workspace(crate::CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: crate::OperationId::generate().unwrap(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "observed source".into(),
            request_digest: None,
        })
        .unwrap();
    let entry = engine
        .list_tree(&repository.main_branch_id)
        .unwrap()
        .into_iter()
        .find(|entry| entry.path() == "file.txt")
        .unwrap();
    (
        temp,
        engine,
        HistoryFileId::parse(&entry.file_id).unwrap(),
        HistoryVersionId::parse(&entry.version_id).unwrap(),
    )
}

fn visible_request(file: &HistoryFileId) -> HistoryWindowRequest {
    let mut request = HistoryWindowRequest {
        file_id: file.clone(), collection: HistoryCollection::Nodes, location: None,
        history_root: None, cursor: None, anchor_node_id: None,
        limit: HistoryPageLimit::for_collection(64, &HistoryCollection::Nodes).unwrap(),
    };
    request.file_id = file.clone();
    request.history_root = None;
    request.location = None;
    request
}


fn initialize_file_branch_owner(engine: &mut Engine, version: &HistoryVersionId) {
    let history = &engine.state.versions[version.as_str()].history_tree_id;
    let repository = &engine.state.repositories[&engine.state.histories[history].repository_id];
    let repository_id = repository.repository_id.clone();
    let expected_namespace_head = engine.state.branches[&repository.main_branch_id]
        .head_commit_id
        .clone();
    engine
        .initialize_directory_git(
            &repository_id,
            &storage_contract::directory_git::DirectoryGitInitializeBody {
                path: String::new(),
                expected_namespace_head,
                operation_id: crate::OperationId::generate().unwrap(),
            },
            "admin",
        )
        .unwrap();
}


fn request_for(engine: &Engine, file: &HistoryFileId, limit: usize) -> Request {
    let read = engine.capture_history_window(
        visible_request(file), HistoryWindowCancellation::new(),
    ).unwrap();
    Request::parse(file.as_str(), &read.history_root.to_string(), None, None, None, limit).unwrap()
}

#[test]
fn file_lifecycle_real_delete_undo_paginate_reopen_and_reject_changed_source() {
    let (temp, mut engine, file, version) = actual();
    initialize_file_branch_owner(&mut engine, &version);
    let initial = request_for(&engine, &file, 1);
    assert_lifecycle_sources_agree(&engine.state, &file);
    let read = engine
        .capture_file_lifecycle(initial, HistoryWindowCancellation::new())
        .unwrap();
    assert!(read.page().unwrap().events.is_empty());
    let context = read.read.context().clone();
    let path = name_policy::LookupPath::parse("file.txt").unwrap();
    let delete = OperationId::generate().unwrap();
    engine
        .soft_delete_file(crate::DeleteFileRequest {
            target: crate::FileDeletionTarget {
                repository_id: &context.repository_id,
                file_id: &file,
                path: &path,
            },
            principal: "admin",
            operation_id: delete,
        })
        .unwrap();
    assert!(engine.recheck_file_lifecycle(&read).is_err());
    let deletion = engine.file_deletion(file.as_str()).unwrap().clone();
    assert_lifecycle_sources_agree(&engine.state, &file);
    let hidden_request = request_for(&engine, &file, 1);
    let old_location = Request::parse(
        file.as_str(),
        &hidden_request.history_root.to_string(),
        Some(context.repository_id.as_str()),
        Some("file.txt"),
        None,
        1,
    )
    .unwrap();
    assert!(
        engine
            .capture_file_lifecycle(old_location, HistoryWindowCancellation::new())
            .is_err()
    );
    let mut hidden = engine
        .capture_file_lifecycle(hidden_request.clone(), HistoryWindowCancellation::new())
        .unwrap();
    let hidden_page = hidden.page().unwrap();
    assert!(hidden_page.context.path.is_none());
    assert_eq!(hidden_page.events[0].operation_id, delete);
    assert_eq!(
        hidden_page.events[0].occurred_at,
        Some(Timestamp::from_unix_seconds(deletion.created_at)),
    );
    hidden.source.operations.remove(&delete);
    assert_eq!(
        hidden.page().unwrap_err().code(),
        storage_contract::ErrorCode::PublicationInvalid,
    );
    let undo = OperationId::generate().unwrap();
    engine
        .undo_file_deletion(crate::UndoFileDeletionRequest {
            file_id: &file,
            deletion_node_id: &deletion.node_id,
            principal: "admin",
            operation_id: undo,
        })
        .unwrap();
    assert!(engine.recheck_file_lifecycle(&hidden).is_err());
    assert_lifecycle_sources_agree(&engine.state, &file);
    drop(read);
    drop(hidden);
    drop(engine);
    let mut engine = Engine::open(&temp.path().join("state")).unwrap();
    assert_lifecycle_sources_agree(&engine.state, &file);
    let mut request = request_for(&engine, &file, 1);
    let read = engine
        .capture_file_lifecycle(request.clone(), HistoryWindowCancellation::new())
        .unwrap();
    let first = read.page().unwrap();
    engine.recheck_file_lifecycle(&read).unwrap();
    assert_eq!(first.events[0].operation_id, delete);
    request.cursor = first.next_cursor.clone();
    assert!(request.cursor.is_some());
    let read = engine
        .capture_file_lifecycle(request.clone(), HistoryWindowCancellation::new())
        .unwrap();
    let second = read.page().unwrap();
    second.validate(&request).unwrap();
    assert_eq!(second.events[0].operation_id, undo);
    assert!(matches!(second.events[0].detail, Detail::Undo { .. }));
    assert!(second.events[0].occurred_at.is_none());
    assert!(second.next_cursor.is_none());
    assert_eq!(first.events[0].source_id(), second.events[0].source_id());
    let mut wrong_sequence = request.clone();
    wrong_sequence.cursor.as_mut().unwrap().after_sequence = second.events[0].sequence;
    assert!(
        engine
            .capture_file_lifecycle(wrong_sequence, HistoryWindowCancellation::new())
            .unwrap()
            .page()
            .is_err()
    );
    let stale_request = request.clone();
    engine
        .create_repository(
            &storage_contract::RepositoryName::parse("unrelated").unwrap(),
            "P01",
            1,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    assert_eq!(
        engine.recheck_file_lifecycle(&read).unwrap_err().code(),
        storage_contract::ErrorCode::HistoryChanged,
    );
    assert!(
        engine
            .capture_file_lifecycle(stale_request, HistoryWindowCancellation::new())
            .unwrap()
            .page()
            .is_err()
    );
    request = request_for(&engine, &file, 1);
    request.cursor = first.next_cursor.clone();
    let binding = request.binding(&second.context, engine.state.sequence);
    request.cursor.as_mut().unwrap().binding = binding;
    request.cursor.as_mut().unwrap().after_operation_id = OperationId::generate().unwrap();
    assert!(
        engine
            .capture_file_lifecycle(request, HistoryWindowCancellation::new())
            .unwrap()
            .page()
            .is_err()
    );
    let cancel = HistoryWindowCancellation::new();
    let read = engine
        .capture_file_lifecycle(request_for(&engine, &file, 1), cancel.clone())
        .unwrap();
    cancel.cancel();
    assert_eq!(
        read.page().unwrap_err().code(),
        storage_contract::ErrorCode::OperationAborted,
    );
}

fn assert_lifecycle_sources_agree(state: &crate::State, file: &HistoryFileId) {
    let projection = crate::state_projection::ProjectionSnapshot::new(state);
    let expected = crate::file_deletion::retained::events(projection.view(), file.as_str())
        .unwrap();
    let source = crate::state_projection::lifecycles::LifecycleSource::capture(
        state,
        file,
        || Ok(()),
    )
    .unwrap();
    let actual = crate::file_deletion::retained::events_from_source(
        source.view(),
        || Ok(()),
        Ok,
    )
    .unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn file_lifecycle_scoped_source_rejects_unrelated_malformed_ownership() {
    let (_temp, mut engine, file, version) = actual();
    initialize_file_branch_owner(&mut engine, &version);
    let original = crate::state_projection::ProjectionSnapshot::new(&engine.state);
    assert!(
        crate::file_deletion::retained::events(original.view(), "invalid-file-text")
            .unwrap()
            .is_empty()
    );
    for kind in ["file.delete", crate::business::history_copy::RETAINED_KIND] {
        let mut state = engine.state.as_ref().clone();
        let mut malformed = state.operations.values().next().unwrap().clone();
        malformed.operation_id = OperationId::generate().unwrap();
        malformed.kind = kind.to_owned();
        // Neither ownership classifier can associate this corrupt receipt with
        // the requested file. Ignoring it would falsely prove a complete prefix.
        malformed.result = json_codec::json!({"unrelated": true});
        state.operations.insert(malformed.operation_id, malformed);
        let projection = crate::state_projection::ProjectionSnapshot::new(&state);
        assert_eq!(
            crate::file_deletion::retained::events(projection.view(), "invalid-file-text")
                .unwrap_err()
                .code(),
            storage_contract::ErrorCode::PublicationInvalid,
        );
        let expected = crate::file_deletion::retained::events(projection.view(), file.as_str())
            .unwrap_err();
        let source = crate::state_projection::lifecycles::LifecycleSource::capture(
            &state,
            &file,
            || Ok(()),
        )
        .unwrap();
        let actual = crate::file_deletion::retained::events_from_source(
            source.view(),
            || Ok(()),
            Ok,
        )
        .unwrap_err();
        assert_eq!(actual.code(), storage_contract::ErrorCode::PublicationInvalid);
        assert_eq!(actual.code(), expected.code());
        let mut read = engine
            .capture_file_lifecycle(
                request_for(&engine, &file, 1),
                HistoryWindowCancellation::new(),
            )
            .unwrap();
        read.source = state;
        assert_eq!(
            read.page().unwrap_err().code(),
            storage_contract::ErrorCode::PublicationInvalid,
        );
        let mut graph_only = engine.state.operations.values().next().unwrap().clone();
        graph_only.operation_id = OperationId::generate().unwrap();
        graph_only.kind = crate::business::history_copy::KIND.to_owned();
        graph_only.result = json_codec::json!({"file_id": file.as_str()});
        read.source.operations.insert(graph_only.operation_id, graph_only);
        // New-format copies require their complete audit envelope. Neither a
        // graph-only stub nor unrelated corruption may become an empty prefix.
        assert_eq!(
            read.page().unwrap_err().code(),
            storage_contract::ErrorCode::PublicationInvalid,
        );
    }
}

#[test]
fn file_lifecycle_scoped_source_cancels_during_receipt_scan_without_a_partial_source() {
    use crate::engine::history_window::HistoryCancellationCheckpoint;

    let (_temp, mut engine, file, version) = actual();
    initialize_file_branch_owner(&mut engine, &version);
    assert!(engine.state.operations.len() >= 3);
    let cancellation = HistoryWindowCancellation::new();
    let mut checkpoints = 0;
    let result = crate::state_projection::lifecycles::LifecycleSource::capture(
        &engine.state,
        &file,
        || {
            checkpoints += 1;
            // The first checkpoint is before scanning; one actual receipt was
            // classified when cancellation arrives before the second receipt.
            if checkpoints == 3 {
                cancellation.cancel();
            }
            cancellation.checkpoint()
        },
    );
    let error = match result {
        Ok(_) => panic!("canceled ownership scan returned a partial source"),
        Err(error) => error,
    };
    assert_eq!(error.code(), storage_contract::ErrorCode::OperationAborted);
    assert_eq!(checkpoints, 3);
    assert_lifecycle_sources_agree(&engine.state, &file);
}
