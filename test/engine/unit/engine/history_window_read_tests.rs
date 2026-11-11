use super::*;
use crate::engine::history_window::oracle_tests::{context, fixture, request};

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
    let mut request = request(HistoryCollection::Nodes, 64);
    request.file_id = file.clone();
    request.history_root = None;
    request.location = None;
    request
}

fn assert_hidden_history(
    engine: &Engine,
    file: &HistoryFileId,
    version: &HistoryVersionId,
) -> HistoryWindowRead {
    let read = engine
        .capture_history_window(visible_request(file), HistoryWindowCancellation::new())
        .unwrap();
    let HistoryWindowPage::Nodes {
        identity, context, nodes, ..
    } = read.page().unwrap() else {
        panic!("node request returns a node page");
    };
    assert!(identity.context.path.is_none());
    assert!(identity.context.current_node_id.is_none());
    assert!(context.content.is_none());
    assert_eq!(nodes.len(), 1);
    assert_eq!(&nodes[0].version_id, version);
    assert!(nodes.iter().all(|node| !node.current));
    assert!(nodes.iter().all(|node| node.download_name == "file.txt"));
    engine.recheck_history_window(&read).unwrap();
    read
}

fn delete_observed_file(
    engine: &mut Engine,
    file: &HistoryFileId,
    version: &HistoryVersionId,
) -> (HistoryResolvedContext, crate::FileDeletionNode, HistoryWindowRequest) {
    use crate::file_deletion::{DeleteFileRequest, FileDeletionTarget};
    let before = engine
        .capture_history_window(visible_request(file), HistoryWindowCancellation::new())
        .unwrap();
    let original = before.context().clone();
    assert_eq!(original.path.as_ref().unwrap().as_str(), "file.txt");
    assert_eq!(original.current_node_id.as_ref().unwrap().as_str(), version.as_str());
    let path = name_policy::LookupPath::parse("file.txt").unwrap();
    let mut scoped = visible_request(file);
    scoped.location = Some(HistoryLocation {
        repository_id: original.repository_id.clone(),
        path: path.clone(),
    });
    let scoped_before = engine
        .capture_history_window(scoped.clone(), HistoryWindowCancellation::new())
        .unwrap();
    engine
        .soft_delete_file(DeleteFileRequest {
            target: FileDeletionTarget {
                repository_id: &original.repository_id,
                file_id: file,
                path: &path,
            },
            principal: "admin",
            operation_id: crate::OperationId::generate().unwrap(),
        })
        .unwrap();
    let deleted = engine.file_deletion(file.as_str()).unwrap().clone();
    assert_eq!(deleted.path.as_str(), "file.txt");
    let repository = &engine.state.repositories[original.repository_id.as_str()];
    let tree = engine.namespace_tree(&repository.main_branch_id).unwrap();
    assert!(tree.entries().iter().any(|entry| entry.file_id == file.as_str()));
    assert!(tree.get("file.txt").is_none());
    assert_eq!(
        engine.recheck_history_window(&before).unwrap_err().code(),
        storage_contract::ErrorCode::HistoryChanged
    );
    assert_eq!(
        engine.recheck_history_window(&scoped_before).unwrap_err().code(),
        storage_contract::ErrorCode::FileNotFound
    );
    drop(before);
    drop(scoped_before);
    (original, deleted, scoped)
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

#[test]
fn soft_deleted_history_keeps_retained_nodes_without_current_context_across_reopen_and_undo() {
    use crate::file_deletion::UndoFileDeletionRequest;
    let (temp, mut engine, file, version) = actual();
    initialize_file_branch_owner(&mut engine, &version);
    let (original, deleted, scoped) = delete_observed_file(&mut engine, &file, &version);
    assert_hidden_history(&engine, &file, &version);
    drop(engine);
    let mut engine = Engine::open(&temp.path().join("state")).unwrap();
    assert_eq!(engine.file_deletion(file.as_str()), Some(&deleted));
    let hidden = assert_hidden_history(&engine, &file, &version);
    assert_eq!(hidden.context().selected_branch_id, original.selected_branch_id);
    assert_eq!(
        engine
            .capture_history_window(scoped.clone(), HistoryWindowCancellation::new())
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::FileNotFound
    );
    let exported = temp.path().join("retained.bin");
    engine.export_version(version.as_str(), &exported).unwrap();
    assert_eq!(std::fs::read(exported).unwrap(), b"retained source");
    engine
        .undo_file_deletion(UndoFileDeletionRequest {
            file_id: &file,
            deletion_node_id: &deleted.node_id,
            principal: "admin",
            operation_id: crate::OperationId::generate().unwrap(),
        })
        .unwrap();
    assert_eq!(
        engine.recheck_history_window(&hidden).unwrap_err().code(),
        storage_contract::ErrorCode::HistoryChanged
    );
    drop(hidden);
    drop(engine);
    let engine = Engine::open(&temp.path().join("state")).unwrap();
    let restored = engine
        .capture_history_window(scoped, HistoryWindowCancellation::new())
        .unwrap();
    assert_eq!(restored.context(), &original);
    let HistoryWindowPage::Nodes { context, nodes, .. } = restored.page().unwrap() else {
        panic!("node request returns a node page");
    };
    assert_eq!(context.content.unwrap().version_id.as_ref(), Some(&version));
    assert!(nodes.iter().any(|node| node.current && node.version_id == version));
    engine.recheck_history_window(&restored).unwrap();
}

#[test]
fn captured_direct_pages_reuse_scope_witness_without_opening_membership_streams() {
    let (_temp, engine, file, version) = actual();
    for collection in [
        HistoryCollection::VersionSources {
            version_id: version.clone(),
        },
        HistoryCollection::NodeField {
            node_id: HistoryNodeId::parse(version.as_str()).unwrap(),
            field: HistoryField::CreatedAt,
        },
    ] {
        let mut request = request(collection, 64);
        request.file_id = file.clone();
        request.history_root = Some(
            HistoryRoot::parse(
                engine
                    .metadata_view()
                    .history_root(
                        &engine
                            .state
                            .versions
                            .values()
                            .find(|v| v.file_id == request.file_id.as_str())
                            .unwrap()
                            .history_tree_id,
                    )
                    .unwrap(),
            )
            .unwrap(),
        );
        request.location = None;
        let read = engine
            .capture_history_window(request, HistoryWindowCancellation::new())
            .unwrap();
        assert!(read.scope.is_none());
        crate::engine::index::reset_scope_read_metrics();
        query::take_scope_version_headers();
        let actual = read.page().unwrap();
        assert_eq!(query::take_scope_version_headers(), 0);
        assert_eq!(crate::engine::index::scope_read_metrics(), ([0; 6], 0));
        let reference = engine
            .metadata_view()
            .history_window(&read.request, &read.context, &read.cancellation)
            .unwrap();
        assert!(query::take_scope_version_headers() > 0);
        assert_eq!(
            json_codec::to_value(actual).unwrap(),
            json_codec::to_value(reference).unwrap()
        );
        engine.recheck_history_window(&read).unwrap();
    }
}

#[test]
fn owner_witness_never_admits_missing_or_foreign_scope() {
    let view = fixture();
    let request = request(HistoryCollection::Branches, 3);
    let context = context();
    for owner in [
        "ver_00000000000000000000000000099999",
        "ver_00000000000000000000000000000064",
        "ver_00000000000000000000000000000065",
    ] {
        let error = view
            .history_window_observed(
                &request,
                &context,
                &HistoryWindowCancellation::new(),
                None,
                Some(owner),
            )
            .unwrap_err();
        assert_eq!(error.code(), storage_contract::ErrorCode::FileNotFound);
    }
    let mut absent = request.clone();
    absent.file_id = HistoryFileId::parse("file_00000000000000000000000000099999").unwrap();
    assert!(
        view.history_window_observed(
            &absent,
            &context,
            &HistoryWindowCancellation::new(),
            None,
            Some("ver_00000000000000000000000000000000")
        )
        .is_err()
    );
}

#[test]
fn unavailable_index_capture_still_checks_and_reuses_authoritative_owner() {
    use sqlite_access::{Connection, Executor};
    let (_temp, engine, file, version) = actual();
    platform_io::sqlite::block_on_sqlite_connection(async {
        let mut c = platform_io::sqlite::open_index_connection(
            &engine.root.join("index.sqlite"),
            false,
            None,
        )
        .await
        .unwrap();
        c.execute("DELETE FROM history_scope_members WHERE kind=0")
            .await
            .unwrap();
        c.close().await.unwrap();
    });
    let mut request = request(
        HistoryCollection::VersionSources {
            version_id: version,
        },
        64,
    );
    request.file_id = file;
    request.history_root = Some(
        HistoryRoot::parse(
            engine
                .metadata_view()
                .history_root(
                    &engine
                        .state
                        .versions
                        .values()
                        .find(|v| v.file_id == request.file_id.as_str())
                        .unwrap()
                        .history_tree_id,
                )
                .unwrap(),
        )
        .unwrap(),
    );
    request.location = None;
    let read = engine
        .capture_history_window(request, HistoryWindowCancellation::new())
        .unwrap();
    assert!(matches!(
        engine.index_health(),
        crate::engine::index::IndexHealth::Unavailable(_)
    ));
    query::take_scope_version_headers();
    let actual = read.page().unwrap();
    assert_eq!(query::take_scope_version_headers(), 0);
    let reference = engine
        .metadata_view()
        .history_window(&read.request, &read.context, &read.cancellation)
        .unwrap();
    assert_eq!(
        json_codec::to_value(actual).unwrap(),
        json_codec::to_value(reference).unwrap()
    );
    engine.recheck_history_window(&read).unwrap();
}

#[test]
fn captured_window_does_not_retain_namespace_placement_or_policy_tables() {
    let (_temp, mut engine, file, _) = actual();
    let repository = engine.state.repositories.keys().next().unwrap().clone();
    engine
        .initialize_directory_scope(
            &repository,
            None,
            "admin",
            crate::OperationId::generate().unwrap(),
        )
        .unwrap();
    assert!(!engine.state.directory_trees.is_empty());
    assert!(!engine.state.placements.is_empty());
    let mut request = request(HistoryCollection::Nodes, 2);
    request.file_id = file;
    request.history_root = None;
    request.location = None;
    let read = engine
        .capture_history_window(request, HistoryWindowCancellation::new())
        .unwrap();
    let preparation = read.preparation.lock().unwrap();
    let PagePreparation::Pending { view, .. } = &*preparation else {
        panic!("new read is pending");
    };
    let state = view.business_projection();
    assert!(state.directory_branches.is_empty());
    assert!(state.directory_trees.is_empty());
    assert!(state.directory_tree_selections.is_empty());
    assert!(state.file_locations.is_empty());
    assert!(state.file_selections.is_empty());
    assert!(state.file_backup_paths.is_empty());
    assert!(state.file_backup_policies.is_empty());
    assert!(state.directory_selections.is_empty());
    assert!(state.directory_selection_generations.is_empty());
    assert!(state.file_selection_generations.is_empty());
    assert!(state.placements.is_empty());
    assert!(state.merge_links.is_empty());
    assert!(state.history_boundaries.is_empty());
    assert!(state.placement_rules.is_empty());
    assert!(state.protection_policies.is_empty());
    assert!(state.versions.shares_storage_with(&engine.state.versions));
    assert!(state.commits.shares_storage_with(&engine.state.commits));
    drop(preparation);
    let page = read.page().unwrap();
    let reference = engine
        .metadata_view()
        .history_window_observed(
            &read.request,
            &read.context,
            &read.cancellation,
            None,
            Some(&read.owner_version),
        )
        .unwrap();
    assert_eq!(page, reference);
    engine.recheck_history_window(&read).unwrap();
}

#[test]
fn narrow_window_fields_preserve_all_collection_projections() {
    let broad = fixture();
    let state = super::super::super::BusinessMetadataProjection::capture_history_window(
        broad.business_projection(),
    );
    let history_window_roots = broad
        .history_window_roots
        .as_ref()
        .unwrap()
        .projected_for(&broad.state.0, &state.0);
    let narrow = MetadataView {
        state,
        current_namespaces: crate::engine::NamespaceReadView::Initial,
        history_window_roots,
        history_action_max_entries: broad.history_action_max_entries,
    };
    for collection in [
        HistoryCollection::Nodes,
        HistoryCollection::Versions,
        HistoryCollection::Branches,
        HistoryCollection::Merges,
        HistoryCollection::NodeParents {
            node_id: "ver_00000000000000000000000000000010".parse().unwrap(),
        },
        HistoryCollection::VersionSources {
            version_id: "ver_00000000000000000000000000000010".parse().unwrap(),
        },
        HistoryCollection::MergeInputs {
            merge_id: "fmerge_00000000000000000000000000000000".parse().unwrap(),
        },
        HistoryCollection::MergeParents {
            merge_id: "fmerge_00000000000000000000000000000001".parse().unwrap(),
        },
        HistoryCollection::NodeField {
            node_id: "ver_00000000000000000000000000000000".parse().unwrap(),
            field: HistoryField::Message,
        },
    ] {
        let request = request(collection, 2);
        let cancel = HistoryWindowCancellation::new();
        assert_eq!(
            narrow
                .history_window(&request, &context(), &cancel)
                .unwrap(),
            broad.history_window(&request, &context(), &cancel).unwrap(),
        );
    }
}

#[test]
fn prepared_page_releases_business_snapshot_and_repeats_without_retaining_history() {
    let (_temp, engine, file, version) = actual();
    let mut request = request(HistoryCollection::Nodes, 2);
    request.file_id = file;
    request.history_root = None;
    request.location = None;
    let read = engine
        .capture_history_window(request, HistoryWindowCancellation::new())
        .unwrap();
    let snapshot = {
        let preparation = read.preparation.lock().unwrap();
        let PagePreparation::Pending { view, .. } = &*preparation else {
            panic!("new read is pending");
        };
        std::sync::Arc::downgrade(&view.state.0)
    };
    let version_record =
        std::sync::Arc::downgrade(&engine.state.versions.get_shared(version.as_str()).unwrap());
    assert!(snapshot.upgrade().is_some());
    let page = read.page().unwrap();
    assert!(
        snapshot.upgrade().is_none(),
        "prepared read releases every captured table handle"
    );
    assert!(matches!(
        *read.preparation.lock().unwrap(),
        PagePreparation::Prepared(_)
    ));
    engine.recheck_history_window(&read).unwrap();
    crate::engine::index::reset_scope_read_metrics();
    assert_eq!(read.page().unwrap(), page);
    assert_eq!(crate::engine::index::scope_read_metrics(), ([0; 6], 0));
    drop(engine);
    assert!(
        version_record.upgrade().is_none(),
        "retained prepared API handle holds no historical record"
    );
    assert_eq!(read.page().unwrap(), page);
    read.cancellation().cancel();
    assert_eq!(
        read.page().unwrap_err().code(),
        storage_contract::ErrorCode::OperationAborted
    );
}

#[test]
fn failed_preparation_does_not_publish_a_cached_page() {
    let (_temp, engine, file, _) = actual();
    let mut request = request(HistoryCollection::Nodes, 2);
    request.file_id = file;
    request.history_root = None;
    request.location = None;
    let read = engine
        .capture_history_window(request, HistoryWindowCancellation::new())
        .unwrap();
    let snapshot = {
        let preparation = read.preparation.lock().unwrap();
        let PagePreparation::Pending { view, .. } = &*preparation else {
            panic!("new read is pending");
        };
        std::sync::Arc::downgrade(&view.state.0)
    };
    read.cancellation().cancel();
    for _ in 0..2 {
        assert_eq!(
            read.page().unwrap_err().code(),
            storage_contract::ErrorCode::OperationAborted
        );
        assert!(matches!(
            *read.preparation.lock().unwrap(),
            PagePreparation::Pending { .. }
        ));
    }
    assert!(snapshot.upgrade().is_some());
    drop(read);
    assert!(snapshot.upgrade().is_none());
}
