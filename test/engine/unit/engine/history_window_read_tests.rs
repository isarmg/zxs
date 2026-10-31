use super::*;
use crate::engine::history_window::oracle_tests::{context, fixture, request};

fn actual() -> (tempfile::TempDir, Engine, HistoryFileId, HistoryVersionId) {
    let temp = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &temp.path().join("state"),
        crate::Config::development([temp.path().join("disk")]),
    )
    .unwrap();
    let repository = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("observed").unwrap(),
            "P01",
            1,
            "admin",
            crate::OperationId::generate(),
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
            operation_id: crate::OperationId::generate(),
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
        let reference = read
            .view
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
    let reference = read
        .view
        .history_window(&read.request, &read.context, &read.cancellation)
        .unwrap();
    assert_eq!(
        json_codec::to_value(actual).unwrap(),
        json_codec::to_value(reference).unwrap()
    );
    engine.recheck_history_window(&read).unwrap();
}
