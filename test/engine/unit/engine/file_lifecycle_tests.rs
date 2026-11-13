use crate::{Engine, OperationId};
use storage_contract::file_lifecycle::{Detail, Request, Timestamp};
use storage_contract::history_window::{
    HistoryCollection, HistoryFileId, HistoryPageLimit, HistoryVersionId, HistoryWindowCancellation,
    HistoryWindowRequest,
};

fn actual() -> (tempfile::TempDir, Engine, HistoryFileId, HistoryVersionId) {
    actual_with_disks(1)
}

fn actual_with_disks(disks: usize) -> (tempfile::TempDir, Engine, HistoryFileId, HistoryVersionId) {
    actual_with_path(disks, "file.txt")
}

fn actual_with_path(
    disks: usize,
    path: &str,
) -> (tempfile::TempDir, Engine, HistoryFileId, HistoryVersionId) {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &temp.path().join("state"),
        crate::Config::development(
            (0..disks).map(|index| temp.path().join(format!("disk-{index}"))),
        )
        .unwrap(),
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
        .stage_bytes(&workspace.workspace_id, path, b"retained source")
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
        .find(|entry| entry.path() == path)
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
        hidden.page().unwrap_err()
                .code(),
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
        engine.recheck_file_lifecycle(&read).unwrap_err()
                .code(),
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
        read.page().unwrap_err()
                .code(),
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
fn file_lifecycle_rename_uses_permanent_fact_without_changing_content_graph() {
    let (temporary, mut engine, file, version) = actual();
    initialize_file_branch_owner(&mut engine, &version);
    let history = engine.state.versions[version.as_str()].history_tree_id.clone();
    let repository = engine.state.histories[&history].repository_id.clone();
    let context = engine.file_context(&repository, "file.txt").unwrap();
    let branch = engine.state.branches[&context.branch_id].clone();
    let versions = engine.state.versions.clone();
    let metadata = engine.state.file_metadata_nodes.clone();
    let operation = OperationId::generate().unwrap();
    let receipt = engine.file_namespace(
        &repository,
        crate::NamespaceOperation::parse(
            crate::NamespaceOperationKind::Rename,
            "file.txt",
            Some("renamed.txt"),
        )
        .unwrap(),
        "admin",
        operation,
    )
    .unwrap();
    let facts = crate::persist::path_changes::read(&receipt).unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(engine.state.versions, versions);
    assert_eq!(engine.state.file_metadata_nodes, metadata);
    assert!(engine.state.file_path_nodes.is_empty());
    assert_eq!(engine.state.branches[&context.branch_id], branch);
    let request = request_for(&engine, &file, 64);
    let page = engine.capture_file_lifecycle(request.clone(), HistoryWindowCancellation::new())
        .unwrap().page().unwrap();
    page.validate(&request).unwrap();
    assert_eq!(page.events.len(), 1);
    assert_eq!(page.events[0].operation_id, operation);
    assert_eq!(page.events[0].sequence, receipt.sequence);
    assert_eq!(page.events[0].occurred_at, Some(facts[0].occurred_at));
    assert_eq!(page.events[0].detail, Detail::PathChanged {
        event_id: facts[0].event_id,
        before: facts[0].before.clone(),
        after: facts[0].after.clone(),
    });
    crate::history_package::validate_prepared_state(engine.state()).unwrap();
    for absent_branch in [false, true] {
        let mut corrupted = engine.state().clone();
        let mut forged = facts.clone();
        if absent_branch {
            forged[0].branch_id = storage_contract::BranchId::generate().unwrap();
        } else {
            forged[0].after_version_id = storage_contract::VersionId::generate().unwrap();
        }
        corrupted.operations.get_mut(&operation).unwrap().result["path_changes"] =
            json_codec::to_value(forged).unwrap();
        let bytes = storage_contract::canonical_json(&corrupted).unwrap();
        assert_eq!(
            crate::history_package::validate_prepared_state(&corrupted)
                .unwrap_err()
                .code(),
            storage_contract::ErrorCode::PublicationInvalid,
        );
        let decoded: crate::State = json_codec::from_slice(&bytes).unwrap();
        assert_eq!(
            crate::history_package::validate_prepared_state(&decoded)
                .unwrap_err()
                .code(),
            storage_contract::ErrorCode::PublicationInvalid,
        );
        assert_eq!(storage_contract::canonical_json(&corrupted).unwrap(), bytes);
    }
    for missing in [false, true] {
        let mut corrupted = engine.state().clone();
        let result = &mut corrupted.operations.get_mut(&operation).unwrap().result;
        if missing {
            result.as_object_mut().unwrap().remove("path_changes");
        } else {
            let mut forged = facts.clone();
            forged[0].author = storage_contract::Principal::new("other".into()).unwrap();
            result["path_changes"] = json_codec::to_value(forged).unwrap();
        }
        let bytes = storage_contract::canonical_json(&corrupted).unwrap();
        assert_eq!(
            crate::history_package::validate_prepared_state(&corrupted)
                .unwrap_err()
                .code(),
            storage_contract::ErrorCode::PublicationInvalid,
        );
        assert_eq!(storage_contract::canonical_json(&corrupted).unwrap(), bytes);
    }
    drop(engine);
    let engine = Engine::open(&temporary.path().join("state")).unwrap();
    let reopened = engine.capture_file_lifecycle(
        request_for(&engine, &file, 64), HistoryWindowCancellation::new(),
    ).unwrap().page().unwrap();
    assert_eq!(reopened.events, page.events);
}

#[test]
fn file_lifecycle_new_graph_copy_rejects_missing_audit_without_repairing_source() {
    let (_temporary, mut engine, file, version) = actual_with_disks(4);
    initialize_file_branch_owner(&mut engine, &version);
    let history = engine.state.versions[version.as_str()].history_tree_id.clone();
    let source = engine.state.repositories[
        &engine.state.histories[&history].repository_id
    ].clone();
    engine
        .file_namespace(
            &source.repository_id,
            crate::NamespaceOperation::parse(
                crate::NamespaceOperationKind::Rename,
                "file.txt",
                Some("renamed.txt"),
            )
            .unwrap(),
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    let target = engine.create_disk_root("D04", "admin", OperationId::generate().unwrap())
        .unwrap();
    let operation = OperationId::generate().unwrap();
    let receipt = engine.copy_file_history(
        &crate::FileHistoryCopyRequest {
            source_repository_id: source.repository_id.clone(),
            source_path: "renamed.txt".into(),
            expected_source_head: engine.state.branches[&source.main_branch_id]
                .head_commit_id.clone(),
            target_repository_id: target.repository_id.clone(),
            target_name: "copied.txt".into(),
            expected_target_head: engine.state.branches[&target.main_branch_id]
                .head_commit_id.clone(),
        },
        "admin",
        operation,
    ).unwrap();
    assert!(receipt.result.get("retained_lifecycle").is_some());
    let copied = engine.list_tree(&target.main_branch_id).unwrap().into_iter()
        .find(|entry| entry.path() == "copied.txt").unwrap();
    let copied_file = HistoryFileId::parse(&copied.file_id).unwrap();
    let retained = crate::business::history_copy::retained::read(&receipt, &copied.file_id)
        .unwrap();
    assert_eq!(retained.path_changes.len(), 1);
    enum Damage {
        MissingBranch,
        MissingVersion,
        SourceOwner,
    }
    for damage in [Damage::MissingBranch, Damage::MissingVersion, Damage::SourceOwner] {
        let mut corrupted = engine.state().clone();
        let mut forged = retained.clone();
        match damage {
            Damage::MissingBranch => {
                forged.path_changes[0].branch_id = storage_contract::BranchId::generate().unwrap();
            }
            Damage::MissingVersion => {
                forged.path_changes[0].after_version_id =
                    storage_contract::VersionId::generate().unwrap();
            }
            Damage::SourceOwner => {
                forged.path_changes[0].after_version_id = version.as_str().parse().unwrap();
            }
        }
        corrupted.operations.get_mut(&operation).unwrap().result["retained_lifecycle"] =
            json_codec::to_value(forged).unwrap();
        let bytes = storage_contract::canonical_json(&corrupted).unwrap();
        assert_eq!(
            crate::history_package::validate_prepared_state(&corrupted)
                .unwrap_err()
                .code(),
            storage_contract::ErrorCode::PublicationInvalid,
        );
        let decoded: crate::State = json_codec::from_slice(&bytes).unwrap();
        assert_eq!(
            crate::history_package::validate_prepared_state(&decoded)
                .unwrap_err()
                .code(),
            storage_contract::ErrorCode::PublicationInvalid,
        );
        assert_eq!(storage_contract::canonical_json(&corrupted).unwrap(), bytes);
    }
    let mut read = engine.capture_file_lifecycle(
        request_for(&engine, &copied_file, 64), HistoryWindowCancellation::new(),
    ).unwrap();
    assert!(read.page().unwrap().events.is_empty());
    crate::history_package::validate_prepared_state(&read.source).unwrap();
    read.source.operations.get_mut(&operation).unwrap().result.as_object_mut().unwrap()
        .remove("retained_lifecycle");
    let bytes = storage_contract::canonical_json(&read.source).unwrap();
    assert_eq!(
        crate::history_package::validate_prepared_state(&read.source)
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::PublicationInvalid,
    );
    assert_eq!(read.page().unwrap_err()
                .code(), storage_contract::ErrorCode::PublicationInvalid);
    let mut unrelated = engine
        .capture_file_lifecycle(
            request_for(&engine, &file, 64),
            HistoryWindowCancellation::new(),
        )
        .unwrap();
    unrelated.source = read.source.clone();
    assert_eq!(
        unrelated.page().unwrap_err()
                .code(),
        storage_contract::ErrorCode::PublicationInvalid,
    );
    assert_eq!(storage_contract::canonical_json(&read.source).unwrap(), bytes);
    assert_eq!(engine.state.operations[&operation], receipt);
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
            read.page().unwrap_err()
                .code(),
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
            read.page().unwrap_err()
                .code(),
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

#[test]
fn lifecycle_summary_budget_counts_mixed_events_and_rejects_overflow_without_admission() {
    use crate::engine::history_window::read::lifecycle::EventBudget;
    use storage_contract::file_lifecycle::{Event, MAX_AUDIT_BYTES, PathLocation};

    let deletion = Event {
        operation_id: OperationId::generate().unwrap(),
        sequence: 1,
        occurred_at: None,
        detail: Detail::Undo {
            deletion_id: storage_contract::FileDeletionId::generate().unwrap(),
        },
    };
    let path = Event {
        operation_id: OperationId::generate().unwrap(),
        sequence: 2,
        occurred_at: Some(Timestamp::from_unix_seconds(1)),
        detail: Detail::PathChanged {
            event_id: storage_contract::FilePathEventId::generate().unwrap(),
            before: PathLocation {
                repository_id: storage_contract::RepositoryId::generate().unwrap(),
                path: name_policy::PortablePath::parse(&"a".repeat(180)).unwrap(),
            },
            after: PathLocation {
                repository_id: storage_contract::RepositoryId::generate().unwrap(),
                path: name_policy::PortablePath::parse(&"b".repeat(180)).unwrap(),
            },
        },
    };
    let mut budget = EventBudget::new();
    let mut encoded_bytes = 2;
    let mut count = 0;
    loop {
        let event = if count % 2 == 0 { &deletion } else { &path };
        let next_bytes = json_codec::to_vec(event).unwrap().len() + usize::from(count != 0);
        if encoded_bytes + next_bytes > MAX_AUDIT_BYTES {
            assert_eq!(
                budget.admit(event)
                    .unwrap_err()
                    .code(),
                storage_contract::ErrorCode::ResourceLimit,
            );
            assert_eq!(budget.bytes, encoded_bytes);
            assert_eq!(budget.count, count);
            break;
        }
        budget.admit(event).unwrap();
        encoded_bytes += next_bytes;
        count += 1;
        assert_eq!(budget.bytes, encoded_bytes);
    }
    assert!(count < storage_contract::file_lifecycle::MAX_AUDIT_EVENTS);
}

#[test]
fn lifecycle_summary_count_boundary_preserves_admitted_budget_on_rejection() {
    use crate::engine::history_window::read::lifecycle::EventBudget;
    use storage_contract::file_lifecycle::{Event, MAX_AUDIT_BYTES, MAX_AUDIT_EVENTS};

    let event = Event {
        operation_id: OperationId::generate().unwrap(),
        sequence: 1,
        occurred_at: None,
        detail: Detail::Undo {
            deletion_id: storage_contract::FileDeletionId::generate().unwrap(),
        },
    };
    let size = json_codec::to_vec(&event).unwrap().len();
    assert!(2 + (size + 1) * MAX_AUDIT_EVENTS < MAX_AUDIT_BYTES);
    let mut budget = EventBudget::new();
    for _ in 0..MAX_AUDIT_EVENTS {
        budget.admit(&event).unwrap();
    }
    assert_eq!(budget.count, MAX_AUDIT_EVENTS);
    assert_eq!(budget.bytes, 2 + size * MAX_AUDIT_EVENTS + MAX_AUDIT_EVENTS - 1);
    let bytes = budget.bytes;
    assert_eq!(
        budget.admit(&event).unwrap_err().code(),
        storage_contract::ErrorCode::ResourceLimit,
    );
    assert_eq!(budget.count, MAX_AUDIT_EVENTS);
    assert_eq!(budget.bytes, bytes);
}

#[test]
fn file_lifecycle_real_long_paths_form_short_bounded_pages_without_losing_events() {
    use storage_contract::file_lifecycle::MAX_RESPONSE_BYTES;

    let parent = ["界".repeat(36), "路".repeat(36), "径".repeat(36), "名".repeat(36)]
        .join("/");
    let first = format!("{parent}/{}", "甲".repeat(20));
    let second = format!("{parent}/{}", "乙".repeat(20));
    let (temporary, mut engine, file, version) = actual_with_path(1, &first);
    initialize_file_branch_owner(&mut engine, &version);
    let history = engine.state.versions[version.as_str()].history_tree_id.clone();
    let repository = engine.state.histories[&history].repository_id.clone();
    let context = engine.file_context(&repository, &first).unwrap();
    let branch = engine.state.branches[&context.branch_id].clone();
    let versions = engine.state.versions.clone();
    let mut expected = Vec::new();
    for index in 0..64 {
        let (source, target) = if index % 2 == 0 {
            (&first, &second)
        } else {
            (&second, &first)
        };
        let receipt = engine
            .file_namespace(
                &repository,
                crate::NamespaceOperation::parse(
                    crate::NamespaceOperationKind::Rename,
                    source,
                    Some(target),
                )
                .unwrap(),
                "admin",
                OperationId::generate().unwrap(),
            )
            .unwrap();
        let facts = crate::persist::path_changes::read(&receipt).unwrap();
        assert_eq!(facts.len(), 1);
        expected.push((receipt.sequence, receipt.operation_id, facts[0].event_id));
    }
    assert_eq!(engine.state.versions, versions);
    assert_eq!(engine.state.branches[&context.branch_id], branch);
    assert!(engine.state.file_path_nodes.is_empty());
    drop(engine);
    let engine = Engine::open(&temporary.path().join("state")).unwrap();
    let before = storage_contract::canonical_json(engine.state()).unwrap();
    let mut request = request_for(&engine, &file, 64);
    let mut actual = Vec::new();
    let mut pages = 0;
    loop {
        let read = engine
            .capture_file_lifecycle(request.clone(), HistoryWindowCancellation::new())
            .unwrap();
        let page = read.page().unwrap();
        page.validate(&request).unwrap();
        engine.recheck_file_lifecycle(&read).unwrap();
        assert!(!page.events.is_empty());
        assert!(json_codec::to_vec(&page).unwrap().len() <= MAX_RESPONSE_BYTES);
        for event in &page.events {
            let Detail::PathChanged { event_id, .. } = &event.detail else {
                panic!("real rename must return a path event");
            };
            actual.push((event.sequence, event.operation_id, *event_id));
        }
        pages += 1;
        let Some(cursor) = page.next_cursor else {
            break;
        };
        assert!(page.events.len() < request.limit());
        assert_eq!(cursor.after_key(), page.events.last().unwrap().order_key());
        request.cursor = Some(cursor);
    }
    assert!(pages > 1);
    assert_eq!(actual, expected);
    assert_eq!(storage_contract::canonical_json(engine.state()).unwrap(), before);
}
