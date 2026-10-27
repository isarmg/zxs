use super::*;
use crate::http_boundary::read_body;
use http_routing::{extract::FromRequestParts, response::IntoResponse};

async fn assert_error(
    response: Response,
    status: StatusCode,
    code: &str,
    retryable: bool,
) -> Value {
    assert_eq!(response.status(), status);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
    let body: Value = json_codec::from_slice(
        &http_routing::body::to_bytes(response.into_body(), 8192)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(body.as_object().unwrap().len(), 1);
    let error = &body["error"];
    assert_eq!(error.as_object().unwrap().len(), 4);
    assert_eq!(error["code"], code);
    assert_eq!(error["retryable"], retryable);
    assert!(!error["message"].as_str().unwrap().is_empty());
    storage_contract::validate_id(error["request_id"].as_str().unwrap(), "req").unwrap();
    body
}

#[tokio::test]
async fn query_and_path_rejections_use_the_api_envelope_after_authorization() {
    let root = crate::tests::private_tempdir();
    let app = application(root.path());
    let (token, _, _) = authenticated(&app).await;
    for (method, path) in [
        ("PUT", "/api/workspaces/ws/files/item/content"),
        ("PUT", "/api/workspaces/ws/files/item/content?path=a&path=b"),
        ("GET", "/api/versions/version/content?unknown=value"),
        ("GET", "/api/system/history?range=1min&range=1h"),
        ("GET", "/api/system/history?range=invalid"),
        ("GET", "/api/events?after=1&after=2"),
        (
            "GET",
            "/api/branches/branch/tree?commit_id=old&commit_id=new",
        ),
        ("GET", "/api/file-manager/disks/P01/entries?path=a&%70ath=b"),
        ("GET", "/api/repositories/%FF"),
        ("PATCH", "/api/file-manager/files/%FF/nodes/node"),
    ] {
        // Authentication must still run before request extraction.
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_error(response, StatusCode::UNAUTHORIZED, "UNAUTHORIZED", false).await;

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_error(response, StatusCode::BAD_REQUEST, "INVALID_REQUEST", false).await;
    }
}

#[tokio::test]
async fn map_query_rejects_duplicate_decoded_fields_without_overwriting() {
    for query in [
        "after=1&after=2",
        "path=a&path=b",
        "path=a&%70ath=b",
        "path=a&path=a",
        "commit_id=old&commit_id=new",
        "a+b=first&a%20b=second",
        "&after=1&&after=2&",
        "path&path=",
    ] {
        let (mut parts, _) = Request::builder()
            .uri(format!("/api/events?{query}"))
            .body(Body::empty())
            .unwrap()
            .into_parts();
        let result =
            ApiQuery::<BTreeMap<String, String>>::from_request_parts(&mut parts, &()).await;
        let error = match result {
            Ok(ApiQuery(fields)) => panic!("duplicate query accepted: {query}: {fields:?}"),
            Err(error) => error,
        };
        assert_eq!(error.status, 400);
        assert_eq!(error.code, storage_contract::ErrorCode::InvalidRequest);
    }
}

#[tokio::test]
async fn map_query_keeps_distinct_fields_and_normal_form_decoding() {
    for (query, expected) in [
        ("", BTreeMap::new()),
        (
            "&%70ath=a%26path%3Db&&commit_id=selected+commit&",
            BTreeMap::from([
                ("path".to_owned(), "a&path=b".to_owned()),
                ("commit_id".to_owned(), "selected commit".to_owned()),
            ]),
        ),
        (
            "path=&after=0&unused=0",
            BTreeMap::from([
                ("path".to_owned(), String::new()),
                ("after".to_owned(), "0".to_owned()),
                ("unused".to_owned(), "0".to_owned()),
            ]),
        ),
    ] {
        let (mut parts, _) = Request::builder()
            .uri(format!("/api/events?{query}"))
            .body(Body::empty())
            .unwrap()
            .into_parts();
        let ApiQuery(actual) =
            ApiQuery::<BTreeMap<String, String>>::from_request_parts(&mut parts, &())
                .await
                .unwrap();
        assert_eq!(actual, expected);
    }
}

#[tokio::test]
async fn routing_failures_keep_status_allow_and_error_fields() {
    let root = crate::tests::private_tempdir();
    let app = application(root.path());
    for (method, path, allowed) in [
        ("POST", "/api/capabilities", "GET,HEAD"),
        ("DELETE", "/api/repositories", "GET,HEAD,POST"),
        ("GET", "/api/auth/login", "POST"),
        ("OPTIONS", "/healthz", "GET,HEAD"),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.headers()[header::ALLOW], allowed);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_error(
            response,
            StatusCode::METHOD_NOT_ALLOWED,
            "METHOD_NOT_ALLOWED",
            false,
        )
        .await;
    }
    for path in ["/api", "/api/unknown", "/unknown"] {
        let response = app
            .clone()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        if path.starts_with("/api") {
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        }
        assert_error(response, StatusCode::NOT_FOUND, "NOT_FOUND", false).await;
    }
    let (token, _, _) = authenticated(&app).await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/repositories")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    // Backend errors retain their own code and retry policy.
    assert_error(
        response,
        StatusCode::NOT_IMPLEMENTED,
        "UNSUPPORTED_OPERATION",
        false,
    )
    .await;
}

#[tokio::test]
async fn bounded_json_routes_return_payload_too_large() {
    let root = crate::tests::private_tempdir();
    let app = application(root.path());
    let (token, _, _) = authenticated(&app).await;
    for (path, limit) in [
        ("/api/auth/login", 8 * 1024),
        ("/api/auth/password", 16 * 1024),
        ("/api/repositories", 1024 * 1024),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from(vec![b' '; limit + 1]))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_error(
            response,
            StatusCode::PAYLOAD_TOO_LARGE,
            "PAYLOAD_TOO_LARGE",
            false,
        )
        .await;
    }
}

#[tokio::test]
async fn head_errors_retain_status_and_headers_without_a_body() {
    let root = crate::tests::private_tempdir();
    let app = application(root.path());
    let (token, _, _) = authenticated(&app).await;
    for (path, status) in [
        ("/api/auth/login", StatusCode::METHOD_NOT_ALLOWED),
        ("/api/unknown", StatusCode::NOT_FOUND),
        (
            "/api/versions/version/content?unknown=value",
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("HEAD")
                    .uri(path)
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        if status == StatusCode::METHOD_NOT_ALLOWED {
            assert_eq!(response.headers()[header::ALLOW], "POST");
        }
        assert!(
            http_routing::body::to_bytes(response.into_body(), 8192)
                .await
                .unwrap()
                .is_empty()
        );
    }
}

#[tokio::test]
async fn bounded_body_counts_streamed_bytes_and_preserves_read_failures() {
    let request = Request::new(Body::from("1234"));
    assert_eq!(read_body(request, 4).await.unwrap(), "1234");
    let chunks = futures::stream::iter([
        Ok::<_, std::io::Error>(Bytes::from_static(b"12")),
        Ok(Bytes::from_static(b"345")),
    ]);
    let request = Request::builder()
        .header(header::CONTENT_LENGTH, "2")
        .body(Body::from_stream(chunks))
        .unwrap();
    assert_error(
        read_body(request, 4).await.unwrap_err().into_response(),
        StatusCode::PAYLOAD_TOO_LARGE,
        "PAYLOAD_TOO_LARGE",
        false,
    )
    .await;

    let chunks = futures::stream::iter([
        Ok(Bytes::from_static(b"12")),
        Err(std::io::Error::other("private transport detail")),
    ]);
    let request = Request::new(Body::from_stream(chunks));
    let body = assert_error(
        read_body(request, 4).await.unwrap_err().into_response(),
        StatusCode::BAD_REQUEST,
        "INVALID_REQUEST",
        false,
    )
    .await;
    assert!(!body.to_string().contains("private transport detail"));
}

// When a public route moves off call(), migrate its real typed method here too.
// An unsupported stub proves neither panic mapping nor blocking-job cleanup.
struct PanickingBackend {
    upload_admission_panics: bool,
}

impl Backend for PanickingBackend {
    fn operation(
        &self,
        _: &crate::auth::Identity,
        _: storage_contract::OperationId,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        panic!("private operation lookup panic detail")
    }
    fn commit_workspace(
        &self,
        _: &crate::auth::Identity,
        _: crate::CommitWorkspace,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        unreachable!("this test backend does not commit workspaces")
    }

    fn snapshot_at(
        &self,
        _: &crate::auth::Identity,
        _: &str,
        _: &str,
    ) -> Result<std::sync::Arc<storage_contract::Commit>, crate::ApiError> {
        unreachable!("this test backend does not serve snapshots")
    }

    fn authorize_current(
        &self,
        _identity: &crate::auth::Identity,
        _branch: &str,
    ) -> Result<(), crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }

    fn read_directory(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::ReadDirectory<'_>,
    ) -> Result<crate::DirectorySnapshot, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }

    fn authorize_workspace_selection(
        &self,
        _identity: &crate::auth::Identity,
        _workspace: &str,
        _path: &name_policy::PortablePath,
    ) -> Result<(), crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }

    fn create_workspace(
        &self,
        _identity: &crate::auth::Identity,
        _command: crate::CreateWorkspace,
    ) -> Result<storage_contract::Workspace, crate::ApiError> {
        panic!("private create_workspace panic detail")
    }
    fn abort_workspace(
        &self,
        _identity: &crate::auth::Identity,
        _workspace: &str,
    ) -> Result<(), crate::ApiError> {
        panic!("private abort_workspace panic detail")
    }

    fn workspace(
        &self,
        _identity: &crate::auth::Identity,
        _workspace: &str,
    ) -> Result<storage_contract::Workspace, crate::ApiError> {
        panic!("private workspace panic detail")
    }
    fn published_upload_status(
        &self,
        _identity: &crate::auth::Identity,
        _workspace: &str,
        _item: &str,
    ) -> Result<Option<crate::PublishedUploadStatus>, crate::ApiError> {
        panic!("private published_upload_status panic detail")
    }
    fn seal_workspace(
        &self,
        _identity: &crate::auth::Identity,
        _workspace: &str,
    ) -> Result<storage_contract::SealedWorkspace, crate::ApiError> {
        panic!("private seal_workspace panic detail")
    }
    fn commit_workspace_public(
        &self,
        _identity: &crate::auth::Identity,
        _command: storage_contract::PublicCommitEnvelope,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        panic!("private commit_workspace_public panic detail")
    }
    fn version_snapshot(
        &self,
        _identity: &crate::auth::Identity,
        _version: &str,
    ) -> Result<crate::VersionSnapshot, crate::ApiError> {
        panic!("private version_snapshot panic detail")
    }
    fn authorize_version(
        &self,
        _identity: &crate::auth::Identity,
        _version: &str,
    ) -> Result<(), crate::ApiError> {
        panic!("private authorize_version panic detail")
    }
    fn repository_snapshot(
        &self,
        _identity: &crate::auth::Identity,
        _id: &str,
    ) -> Result<storage_contract::Repository, crate::ApiError> {
        panic!("private repository_snapshot panic detail")
    }
    fn authorize_repository(
        &self,
        _identity: &crate::auth::Identity,
        _id: &str,
    ) -> Result<(), crate::ApiError> {
        panic!("private authorize_repository panic detail")
    }
    fn retry_storage(&self, _identity: &Identity) -> Result<crate::StorageStatus, ApiError> {
        panic!("private storage retry panic detail")
    }
    fn capabilities(&self) -> Result<crate::Capabilities, crate::ApiError> {
        panic!("private capabilities panic detail")
    }
    fn ready(&self) -> Result<crate::Readiness, crate::ApiError> {
        panic!("private readiness panic detail")
    }
    fn workspace_capacity(
        &self,
        _identity: &crate::auth::Identity,
    ) -> Result<storage_contract::WorkspaceCapacity, crate::ApiError> {
        panic!("private workspace capacity panic detail")
    }
    fn smb_binding_status(
        &self,
        _identity: &crate::auth::Identity,
        _id: &str,
    ) -> Result<crate::SmbBindingStatus, crate::ApiError> {
        panic!("private SMB binding panic detail")
    }
    fn update_smb_binding(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::UpdateSmbBinding<'_>,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        panic!("private SMB binding panic detail")
    }
    fn protection_policy(
        &self,
        _identity: &crate::auth::Identity,
        _id: &str,
    ) -> Result<storage_contract::ProtectionPolicy, crate::ApiError> {
        panic!("private protection policy panic detail")
    }
    fn placement_rule(
        &self,
        _identity: &crate::auth::Identity,
        _id: &str,
    ) -> Result<storage_contract::PlacementRule, crate::ApiError> {
        panic!("private protection policy panic detail")
    }
    fn update_protection_policy(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::UpdateProtectionPolicy,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        panic!("private protection policy panic detail")
    }
    fn update_placement_rule(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::UpdatePlacementRule,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        panic!("private protection policy panic detail")
    }
    fn compare_commits(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::CompareCommits<'_>,
    ) -> Result<storage_contract::CommitComparison, crate::ApiError> {
        panic!("private comparison panic detail")
    }
    fn disk_status(
        &self,
        _identity: &crate::auth::Identity,
    ) -> Result<Vec<storage_contract::DiskStatus>, crate::ApiError> {
        panic!("private disk_status panic detail")
    }
    fn integrity_check(
        &self,
        _identity: &crate::auth::Identity,
    ) -> Result<storage_contract::CheckReport, crate::ApiError> {
        panic!("private integrity_check panic detail")
    }
    fn create_merge_link(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::CreateMergeLink,
    ) -> Result<storage_contract::MergeLink, crate::ApiError> {
        panic!("private create merge link panic detail")
    }
    fn create_repository(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::CreateRepository,
    ) -> Result<storage_contract::Repository, crate::ApiError> {
        panic!("private create repository panic detail")
    }
    fn create_branch(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::CreateBranch,
    ) -> Result<crate::BranchSnapshot, crate::ApiError> {
        panic!("private create branch panic detail")
    }
    fn create_file_branch(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::CreateFileBranch,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        panic!("private directory scope panic detail")
    }
    fn update_note(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::UpdateNote,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        panic!("private directory scope panic detail")
    }
    fn merge_file_branches(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::MergeFileBranches,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        panic!("private directory scope panic detail")
    }
    fn update_file_node_tags(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::UpdateFileNodeTags,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        panic!("private directory scope panic detail")
    }
    fn create_disk_directory(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::CreateDiskDirectory,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        panic!("private directory scope panic detail")
    }
    fn create_disk_write_session(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::CreateDiskWriteSession,
    ) -> Result<crate::DiskWriteSession, crate::ApiError> {
        panic!("private directory scope panic detail")
    }
    fn backup_configuration(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::ReadBackupConfiguration,
    ) -> Result<storage_contract::BackupConfigurationSnapshot, crate::ApiError> {
        panic!("private directory scope panic detail")
    }
    fn configure_backups(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::ConfigureBackups,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        panic!("private directory scope panic detail")
    }
    fn plan(
        &self,
        _identity: &crate::auth::Identity,
        _plan: &str,
    ) -> Result<storage_contract::Plan, crate::ApiError> {
        panic!("private plan backend panic detail")
    }
    fn cancel_plan(
        &self,
        _identity: &crate::auth::Identity,
        _request: Result<crate::CancelPlan, crate::ApiError>,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        panic!("private plan backend panic detail")
    }
    fn register_disk(
        &self,
        _identity: &crate::auth::Identity,
    ) -> Result<std::convert::Infallible, crate::ApiError> {
        panic!("private register disk panic detail")
    }
    fn commit_plan(
        &self,
        _identity: &crate::auth::Identity,
        _request: Result<crate::CommitPlan, crate::ApiError>,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        panic!("private plan backend panic detail")
    }
    fn prepare_historical_save(
        &self,
        _identity: &crate::auth::Identity,
        _request: Result<crate::PrepareHistoricalSave, crate::ApiError>,
    ) -> Result<storage_contract::Plan, crate::ApiError> {
        panic!("private directory scope panic detail")
    }
    fn prepare_purge(
        &self,
        _identity: &crate::auth::Identity,
        _request: Result<crate::PreparePurge, crate::ApiError>,
    ) -> Result<storage_contract::Plan, crate::ApiError> {
        panic!("private directory scope panic detail")
    }
    fn prepare_split(
        &self,
        _identity: &crate::auth::Identity,
        _request: Result<crate::PrepareSplit, crate::ApiError>,
    ) -> Result<storage_contract::Plan, crate::ApiError> {
        panic!("private directory scope panic detail")
    }
    fn namespace_batch(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::NamespaceBatch,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        panic!("private directory scope panic detail")
    }
    fn select_file_branch(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::SelectFileBranch,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        panic!("private directory scope panic detail")
    }
    fn rename_file_branch(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::RenameFileBranch,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        panic!("private directory scope panic detail")
    }
    fn listing(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::ReadListing,
    ) -> Result<crate::ListingSnapshot, crate::ApiError> {
        panic!("private listing panic detail")
    }
    fn mutate_disk_directory(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::MutateDiskDirectory,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        panic!("private directory scope panic detail")
    }
    fn initialize_directory_scope(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::InitializeDirectoryScope,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        panic!("private directory scope panic detail")
    }
    fn archive_branch(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::ArchiveBranch,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        panic!("private archive branch panic detail")
    }
    fn rename_directory_branch(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::RenameDirectoryBranch,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        panic!("private rename directory branch panic detail")
    }
    fn select_default_directory_branch(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::SelectDefaultDirectoryBranch,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        panic!("private select default directory branch panic detail")
    }
    fn directory_selection(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::ReadDirectorySelection,
    ) -> Result<crate::DirectorySelectionSnapshot, crate::ApiError> {
        panic!("private directory selection panic detail")
    }
    fn directory_scopes(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::ReadDirectoryScopes<'_>,
    ) -> Result<crate::DirectoryScopesSnapshot, crate::ApiError> {
        panic!("private directory scopes panic detail")
    }
    fn default_directory_branch(
        &self,
        _identity: &crate::auth::Identity,
        _repository_id: &str,
    ) -> Result<crate::DefaultDirectoryBranchSnapshot, crate::ApiError> {
        panic!("private default directory branch panic detail")
    }
    fn file_history_snapshot(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::ReadFileHistory,
    ) -> Result<crate::FileHistorySnapshot, crate::ApiError> {
        panic!("private file history panic detail")
    }
    fn file_history(
        &self,
        _identity: &crate::auth::Identity,
        _file: &str,
    ) -> Result<Vec<storage_contract::FileVersion>, crate::ApiError> {
        panic!("private file history panic detail")
    }
    fn topology_snapshot(
        &self,
        _identity: &crate::auth::Identity,
    ) -> Result<crate::TopologySnapshot, crate::ApiError> {
        panic!("private storage metadata panic detail")
    }
    fn placement_snapshot(
        &self,
        _identity: &crate::auth::Identity,
        _id: &str,
    ) -> Result<storage_contract::Placement, crate::ApiError> {
        panic!("private storage metadata panic detail")
    }
    fn event_page(
        &self,
        _identity: &crate::auth::Identity,
        _after: u64,
    ) -> Result<crate::EventPage, crate::ApiError> {
        panic!("private event page panic detail")
    }
    fn activity_history(
        &self,
        _identity: &crate::auth::Identity,
    ) -> Result<crate::ActivityHistorySnapshot, crate::ApiError> {
        panic!("private activity history panic detail")
    }
    fn history_graph(
        &self,
        _identity: &crate::auth::Identity,
        _id: &str,
    ) -> Result<crate::HistoryGraphSnapshot, crate::ApiError> {
        panic!("private history graph panic detail")
    }
    fn commit_snapshot(
        &self,
        _identity: &crate::auth::Identity,
        _id: &str,
    ) -> Result<std::sync::Arc<storage_contract::Commit>, crate::ApiError> {
        panic!("private commit snapshot panic detail")
    }
    fn resolve_entry(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::ResolveEntry<'_>,
    ) -> Result<crate::ResolvedEntry, crate::ApiError> {
        panic!("private resolve entry panic detail")
    }
    fn branch_snapshot(
        &self,
        _identity: &crate::auth::Identity,
        _id: &str,
    ) -> Result<crate::BranchSnapshot, crate::ApiError> {
        panic!("private branch_snapshot panic detail")
    }
    fn branches_snapshot(
        &self,
        _identity: &crate::auth::Identity,
        _id: &str,
    ) -> Result<Vec<crate::BranchSnapshot>, crate::ApiError> {
        panic!("private branches_snapshot panic detail")
    }
    fn repositories_snapshot(
        &self,
        _identity: &crate::auth::Identity,
    ) -> Result<Vec<storage_contract::Repository>, crate::ApiError> {
        panic!("private repositories snapshot panic detail")
    }
    fn tree_snapshot(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::ReadTree<'_>,
    ) -> Result<crate::TreeSnapshot, crate::ApiError> {
        panic!("private tree snapshot panic detail")
    }
    fn create_write_session(
        &self,
        _identity: &crate::auth::Identity,
        _branch: &str,
    ) -> Result<crate::WorkspaceWriteSession, crate::ApiError> {
        panic!("private write session panic detail")
    }
    fn stage_namespace(
        &self,
        _identity: &crate::auth::Identity,
        _command: crate::StageNamespace,
    ) -> Result<storage_contract::Workspace, crate::ApiError> {
        panic!("private namespace panic detail")
    }
    fn authorize_upload(
        &self,
        _identity: &crate::auth::Identity,
        _workspace: &str,
        _logical_size: Option<u64>,
    ) -> Result<(), crate::ApiError> {
        if self.upload_admission_panics {
            panic!("private upload admission panic detail");
        }
        Ok(())
    }

    fn lookup_current(
        &self,
        _identity: &crate::auth::Identity,
        _branch: &str,
        _path: &name_policy::LookupPath,
    ) -> Result<Option<storage_contract::Entry>, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }

    fn lookup_at(
        &self,
        _: &crate::auth::Identity,
        _: &str,
        _: &str,
        _: &name_policy::LookupPath,
    ) -> Result<Option<storage_contract::Entry>, crate::ApiError> {
        unreachable!("this test backend does not serve snapshot lookups")
    }

    fn upload(
        &self,
        _identity: &Identity,
        _workspace: &str,
        _item: &str,
        _path: &name_policy::PortablePath,
        _source: &crate::UploadSource,
    ) -> Result<storage_contract::StagedFile, ApiError> {
        panic!("private upload panic detail");
    }
    fn upload_generation(
        &self,
        _identity: &crate::auth::Identity,
        _workspace: &str,
        _item: &str,
        _path: &name_policy::PortablePath,
        _source: &crate::UploadSource,
        _generation: u64,
    ) -> Result<storage_contract::StagedFile, crate::ApiError> {
        panic!("private upload_generation panic detail")
    }

    fn range_content(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::ReadContent<'_>,
    ) -> Result<crate::Content, crate::ApiError> {
        panic!("private range content panic detail")
    }
    fn content(&self, _identity: &Identity, _version: &str) -> Result<Content, ApiError> {
        panic!("private content panic detail");
    }
}

#[tokio::test]
async fn blocking_task_panics_are_internal_errors_not_client_errors() {
    let root = crate::tests::private_tempdir();
    let auth = AuthStore::initialize(root.path(), "admin", "a-long-test-password").unwrap();
    let app = router(
        Arc::new(PanickingBackend {
            upload_admission_panics: false,
        }),
        auth,
        ServerConfig::loopback(root.path()),
    )
    .unwrap();
    let (token, _, _) = authenticated(&app).await;
    for (method, path, body) in [
        ("GET", "/api/file-manager/files/file/history", None),
        ("GET", "/api/capabilities", None),
        ("GET", "/readyz", None),
        ("GET", "/api/workspaces/capacity", None),
        ("POST", "/api/system/storage/retry", Some(json!({}))),
        ("GET", "/api/repositories", None),
        ("GET", "/api/branches/branch", None),
        (
            "POST",
            "/api/history-trees/history/merge-links",
            Some(
                json!({"source_branch_id":"source","source_commit_id":"source_commit","target_branch_id":"target","target_commit_id":"target_commit","operation_id":"op_00000000000000000000000000000000"}),
            ),
        ),
        (
            "POST",
            "/api/repositories",
            Some(
                json!({"name":"Repository","primary_slot_id":"P01","target_copies":3,"operation_id":"op_00000000000000000000000000000000"}),
            ),
        ),
        (
            "POST",
            "/api/repositories/repo/branches",
            Some(
                json!({"source_commit_id":"source","name":"Side","operation_id":"op_00000000000000000000000000000000"}),
            ),
        ),
        (
            "POST",
            "/api/branches/branch/archive",
            Some(json!({"operation_id":"op_00000000000000000000000000000000"})),
        ),
        (
            "POST",
            "/api/repositories/repo/directory-scopes",
            Some(json!({"path":"","operation_id":"op_00000000000000000000000000000000"})),
        ),
        (
            "POST",
            "/api/file-manager/files/file/metadata",
            Some(json!({"note":"","operation_id":"op_00000000000000000000000000000000"})),
        ),
        (
            "POST",
            "/api/file-manager/versions/version/metadata",
            Some(json!({"note":"","operation_id":"op_00000000000000000000000000000000"})),
        ),
        (
            "PATCH",
            "/api/file-manager/files/file/nodes/node",
            Some(json!({"note":"","operation_id":"op_00000000000000000000000000000000"})),
        ),
        ("GET", "/api/repositories/repo/backup-configuration", None),
        (
            "PUT",
            "/api/repositories/repo/backup-configuration",
            Some(
                json!({"path":"","target_copies":2,"backup_prefixes":{"B01":"backup"},"expected_generation":"0","expected_scope_id":"scope","operation_id":"op_00000000000000000000000000000000"}),
            ),
        ),
        (
            "POST",
            "/api/file-manager/files/file/merge",
            Some(
                json!({"inputs":[],"chosen_node_id":"node","surviving_branch_id":"branch","confirmed":true,"operation_id":"op_00000000000000000000000000000000"}),
            ),
        ),
        (
            "POST",
            "/api/file-manager/files/file/tags",
            Some(
                json!({"targets":[],"add":[],"remove":[],"operation_id":"op_00000000000000000000000000000000"}),
            ),
        ),
        (
            "POST",
            "/api/file-manager/disks/P01/namespace",
            Some(
                json!({"kind":"mkdir","path":"folder","operation_id":"op_00000000000000000000000000000000"}),
            ),
        ),
        (
            "POST",
            "/api/file-manager/disks/P01/write-sessions",
            Some(json!({"path":"file.txt"})),
        ),
        (
            "POST",
            "/api/plans/plan/commit",
            Some(
                json!({"operation_id":"op_00000000000000000000000000000000","plan_digest":"digest"}),
            ),
        ),
        ("GET", "/api/plans/plan", None),
        (
            "POST",
            "/api/plans/plan/cancel",
            Some(json!({"operation_id":"op_00000000000000000000000000000000"})),
        ),
        (
            "POST",
            "/api/history-trees/history/historical-save-plans",
            Some(json!({"source_commit_id":"commit","name":"Saved branch"})),
        ),
        (
            "POST",
            "/api/history-trees/history/purge-plans",
            Some(json!({"version_ids":["version"]})),
        ),
        (
            "POST",
            "/api/history-trees/history/split-plans",
            Some(json!({"cut_commit_id":"cut"})),
        ),
        (
            "POST",
            "/api/file-manager/disks/P01/namespace-batch",
            Some(
                json!({"kind":"delete","items":[],"operation_id":"op_00000000000000000000000000000000"}),
            ),
        ),
        (
            "POST",
            "/api/file-manager/files/file/select",
            Some(
                json!({"branch_id":"branch","operation_id":"op_00000000000000000000000000000000"}),
            ),
        ),
        (
            "POST",
            "/api/file-manager/files/file/select",
            Some(
                json!({"branch_id":"branch","repository_id":"repo","path":"file.txt","operation_id":"op_00000000000000000000000000000000"}),
            ),
        ),
        (
            "POST",
            "/api/file-manager/files/file/branches",
            Some(
                json!({"source_version_id":"version","name":"Side","operation_id":"op_00000000000000000000000000000000"}),
            ),
        ),
        (
            "POST",
            "/api/file-manager/files/file/branches",
            Some(
                json!({"source_version_id":"version","source_node_id":"node","name":"Side","operation_id":"op_00000000000000000000000000000000"}),
            ),
        ),
        (
            "PATCH",
            "/api/file-manager/files/file/branches/branch",
            Some(json!({"name":"Side","operation_id":"op_00000000000000000000000000000000"})),
        ),
        (
            "POST",
            "/api/file-manager/disks/P01/directory-branches",
            Some(json!({"name":"Side","operation_id":"op_00000000000000000000000000000000"})),
        ),
        (
            "POST",
            "/api/file-manager/disks/P01/directory-select",
            Some(json!({"name":"Side","operation_id":"op_00000000000000000000000000000000"})),
        ),
        ("GET", "/api/commits/commit", None),
        ("GET", "/api/history-trees/history/graph", None),
        ("GET", "/api/events?after=17", None),
        ("GET", "/api/file-manager/history?ignored=true", None),
        ("GET", "/api/backup-topologies/default", None),
        ("GET", "/api/files/file/history", None),
        ("GET", "/api/repositories/repository/default-branch", None),
        (
            "PATCH",
            "/api/repositories/repository/directory-branches/directory",
            Some(json!({"name":"renamed","operation_id":"op_00000000000000000000000000000000"})),
        ),
        (
            "GET",
            "/api/repositories/repository/directory-branches",
            None,
        ),
        (
            "POST",
            "/api/repositories/repository/directory-branches",
            Some(json!({"path":"","sources":[],"entries":[]})),
        ),
        (
            "GET",
            "/api/repositories/repository/directory-branches?summary=true&path=",
            None,
        ),
        (
            "PUT",
            "/api/repositories/repository/default-branch",
            Some(
                json!({"directory_branch_id":"directory","operation_id":"op_00000000000000000000000000000000"}),
            ),
        ),
        ("GET", "/api/admin/disks", None),
        ("POST", "/api/admin/disks", Some(json!({}))),
        ("GET", "/api/file-manager/disks", None),
        ("GET", "/api/file-manager/disks/P01/entries", None),
        ("GET", "/api/admin/check", None),
        (
            "POST",
            "/api/compare",
            Some(json!({"left_commit_id":"left","right_commit_id":"right"})),
        ),
        ("GET", "/api/policies/policy", None),
        ("GET", "/api/primary-placement-rules/rule", None),
        (
            "PUT",
            "/api/policies/policy",
            Some(
                json!({"repository_id":"repository","path_prefix":"Folder","target_copies":3,"expected_generation":"0","operation_id":"op_00000000000000000000000000000000"}),
            ),
        ),
        (
            "PUT",
            "/api/primary-placement-rules/rule",
            Some(
                json!({"repository_id":"repository","path_prefix":"Folder","primary_slot_id":"P01","expected_generation":"0","operation_id":"op_00000000000000000000000000000000"}),
            ),
        ),
        ("GET", "/api/versions/version/placement", None),
        ("GET", "/api/repositories/repository/smb-binding", None),
        (
            "PUT",
            "/api/repositories/repository/smb-binding",
            Some(
                json!({"branch_id":"branch","enabled":false,"expected_history_generation":"1","operation_id":"op_00000000000000000000000000000000"}),
            ),
        ),
        (
            "POST",
            "/api/branches/branch/resolve",
            Some(json!({"path":"file.txt"})),
        ),
        ("GET", "/api/repositories/repository/branches", None),
        ("GET", "/api/branches/branch/tree?commit_id=commit", None),
        ("GET", "/api/versions/version/content", None),
        (
            "GET",
            "/api/versions/version/content?repository_id=repository&path=file.txt",
            None,
        ),
        ("GET", "/api/workspaces/ws/files/item/upload-status", None),
        ("GET", "/api/workspaces/ws", None),
        (
            "POST",
            "/api/workspaces",
            Some(json!({"branch_id":"branch"})),
        ),
        ("POST", "/api/workspaces/ws/abort", Some(json!({}))),
        ("POST", "/api/workspaces/ws/seal", Some(json!({}))),
        ("GET", "/api/versions/version", None),
        ("GET", "/api/repositories/repository", None),
        (
            "GET",
            "/api/operations/op_00000000000000000000000000000000",
            None,
        ),
        (
            "POST",
            "/api/branches/branch/write-sessions",
            Some(json!({})),
        ),
        (
            "POST",
            "/api/workspaces/ws/namespace-operations",
            Some(json!({"kind":"mkdir","path":"directory"})),
        ),
        (
            "POST",
            "/api/workspaces/ws/commit",
            Some(
                json!({"workspace_id":"ws","operation_id":"op_00000000000000000000000000000000","repository_id":"repo","branch_id":"branch","expected_head":"head","fencing_token":"1","message":"","requested_protection":"strict","file_placement":{},"expected_topology_generation":"1","sealed_manifest_hash":"manifest","request_digest":"digest"}),
            ),
        ),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(
                        body.map(|body| Body::from(json_codec::to_vec(&body).unwrap()))
                            .unwrap_or_else(Body::empty),
                    )
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = assert_error(
            response,
            StatusCode::INTERNAL_SERVER_ERROR,
            "INTERNAL_ERROR",
            true,
        )
        .await;
        assert_eq!(body["error"]["message"], "request task failed");
        assert!(!body.to_string().contains("panic"));
    }
}

#[tokio::test]
async fn cancellation_and_missing_router_state_are_internal_errors() {
    let task = tokio::spawn(futures::future::pending::<()>());
    task.abort();
    let error = task.await.unwrap_err();
    assert!(error.is_cancelled());
    assert_error(
        ApiError::join(error).into_response(),
        StatusCode::INTERNAL_SERVER_ERROR,
        "INTERNAL_ERROR",
        true,
    )
    .await;

    let (mut parts, _) = Request::new(Body::empty()).into_parts();
    let result = ApiPath::<String>::from_request_parts(&mut parts, &()).await;
    let Err(error) = result else {
        panic!("request without route state must fail");
    };
    assert_error(
        error.into_response(),
        StatusCode::INTERNAL_SERVER_ERROR,
        "INTERNAL_ERROR",
        true,
    )
    .await;
}

#[tokio::test]
async fn ordinary_body_deadline_rejects_a_stalled_sender() {
    let body = Body::from_stream(futures::stream::pending::<Result<Bytes, std::io::Error>>());
    let error = crate::http_boundary::read_body_with_timeout(
        Request::builder().body(body).unwrap(),
        4096,
        Duration::from_millis(20),
    )
    .await
    .unwrap_err();
    assert_eq!(error.status, 408);
    assert_eq!(error.code, storage_contract::ErrorCode::RequestTimeout);
    use std::error::Error as _;
    assert!(
        error
            .source()
            .unwrap()
            .source()
            .unwrap()
            .is::<tokio::time::error::Elapsed>()
    );
}

#[tokio::test]
async fn ingress_admission_bounds_slow_bodies_without_blocking_health() {
    let slots = Arc::new(tokio::sync::Semaphore::new(1));
    let app = Router::new()
        .route(
            "/api/body",
            post(|request: Request| async {
                read_body(request, 4096).await.map(|_| Json(json!({})))
            }),
        )
        .route("/healthz", get(|| async { Json(json!({})) }))
        .layer(middleware::from_fn_with_state(
            slots.clone(),
            crate::http_boundary::admit_request,
        ));
    let stalled = tokio::spawn(
        app.clone().oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/body")
                .body(Body::from_stream(futures::stream::pending::<
                    Result<Bytes, std::io::Error>,
                >()))
                .unwrap(),
        ),
    );
    tokio::time::timeout(Duration::from_secs(3), async {
        while slots.available_permits() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/body")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    stalled.abort();
    assert!(stalled.await.unwrap_err().is_cancelled());
    assert_eq!(slots.available_permits(), 1);
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/body")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}
#[tokio::test]
async fn login_requires_named_fields_before_creating_a_session() {
    let root = crate::tests::private_tempdir();
    let app = application(root.path());
    for body in [
        r#"["admin","a-long-test-password"]"#,
        r#"["admin","incorrect-password"]"#,
        r#"{"username":"admin","password":"a-long-test-password","unknown":true}"#,
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/auth/login")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(!response.headers().contains_key(header::SET_COOKIE));
        assert_error(response, StatusCode::BAD_REQUEST, "INVALID_REQUEST", false).await;
    }
    // The documented object representation still authenticates normally.
    let (_, _, _) = authenticated(&app).await;
}

#[tokio::test]
async fn password_change_rejects_positional_fields_without_revoking_the_session() {
    let root = crate::tests::private_tempdir();
    let app = application(root.path());
    let (token, _, cookie) = authenticated(&app).await;
    let malformed = r#"["a-long-test-password","replacement-test-password"]"#;
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/password")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(malformed))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_error(response, StatusCode::UNAUTHORIZED, "UNAUTHORIZED", false).await;
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/password")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, cookie)
                .body(Body::from(malformed))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_error(response, StatusCode::FORBIDDEN, "FORBIDDEN", false).await;
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/password")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(malformed))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(!response.headers().contains_key(header::SET_COOKIE));
    assert_error(response, StatusCode::BAD_REQUEST, "INVALID_REQUEST", false).await;
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/auth/me")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    // Rejection must not change the password, as well as not revoking the token.
    let (_, _, _) = authenticated(&app).await;
}

#[test]
fn api_error_codes_keep_wire_spelling_and_reject_unknown_boundary_values() {
    let error = ApiError::conflict(
        storage_contract::ErrorCode::FileWriteBusy,
        "another writer owns this file",
    );
    let mut wire = json_codec::to_value(&error).unwrap();
    assert_eq!(wire["code"], "FILE_WRITE_BUSY");
    let decoded: storage_contract::ErrorCode =
        json_codec::from_value(wire["code"].clone()).unwrap();
    assert_eq!(decoded, storage_contract::ErrorCode::FileWriteBusy);
    wire["code"] = json!("UNRECOGNIZED_EXTERNAL_CODE");
    assert!(json_codec::from_value::<storage_contract::ErrorCode>(wire["code"].clone()).is_err());
}

#[test]
fn source_survives_api_mapping_but_never_enters_the_wire_envelope() {
    use std::error::Error as _;
    let original = std::io::Error::from_raw_os_error(13);
    let error = ApiError::io(original);
    assert_eq!(error.status, 500);
    assert_eq!(error.code, storage_contract::ErrorCode::IoError);
    assert!(error.retryable);
    let source = error.source().unwrap().source().unwrap();
    assert_eq!(
        source
            .downcast_ref::<std::io::Error>()
            .unwrap()
            .raw_os_error(),
        Some(13)
    );
    let wire = json_codec::to_value(&error).unwrap();
    assert_eq!(wire.as_object().unwrap().len(), 4);
    assert_eq!(wire["message"], "local storage operation failed");
    assert!(wire.get("source").is_none());
}

#[test]
fn external_json_failure_keeps_its_concrete_cause() {
    use std::error::Error as _;
    let source = json_codec::from_slice::<Value>(b"{").unwrap_err();
    let error = ApiError::invalid_source(source);
    assert_eq!(error.status, 400);
    assert_eq!(error.code, storage_contract::ErrorCode::InvalidRequest);
    assert!(!error.retryable);
    assert!(
        error
            .source()
            .unwrap()
            .source()
            .unwrap()
            .is::<json_codec::Error>()
    );
}

#[tokio::test]
async fn failed_request_task_keeps_join_source_without_public_panic_detail() {
    use std::error::Error as _;
    let source = tokio::spawn(async { panic!("private worker diagnostic") })
        .await
        .unwrap_err();
    let error = ApiError::join(source);
    assert_eq!(error.status, 500);
    assert_eq!(error.message, "request task failed");
    assert!(
        error
            .source()
            .unwrap()
            .source()
            .unwrap()
            .downcast_ref::<tokio::task::JoinError>()
            .unwrap()
            .is_panic()
    );
    assert!(
        !json_codec::to_string(&error)
            .unwrap()
            .contains("private worker diagnostic")
    );
}

#[test]
fn io_failure_is_logged_once_at_response_not_at_mapping() {
    use std::io::{Read, Seek};
    let mut capture = tempfile::tempfile().unwrap();
    let subscriber = tracing::collector::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(Arc::new(capture.try_clone().unwrap()))
        .finish();
    tracing::subscriber::with_default(subscriber, || {
        let error = ApiError::io(std::io::Error::other("private /srv/zxs/data diagnostic"));
        assert_eq!(capture.metadata().unwrap().len(), 0);
        let wire = json_codec::to_string(&error).unwrap();
        assert!(!wire.contains("/srv/zxs"));
        let response = error.into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    });
    capture.rewind().unwrap();
    let mut logged = String::new();
    capture.read_to_string(&mut logged).unwrap();
    assert_eq!(logged.matches("request failed").count(), 1, "{logged}");
    assert!(logged.contains("/srv/zxs/data"));
}

#[test]
fn server_startup_error_preserves_its_concrete_io_cause() {
    use std::error::Error as _;
    let error = crate::ServeError::from(std::io::Error::from_raw_os_error(13));
    assert_eq!(
        error
            .source()
            .unwrap()
            .downcast_ref::<std::io::Error>()
            .unwrap()
            .raw_os_error(),
        Some(13)
    );
}

#[tokio::test]
async fn typed_upload_admission_and_publication_panics_are_internal_errors() {
    for upload_admission_panics in [true, false] {
        let root = crate::tests::private_tempdir();
        let auth = AuthStore::initialize(root.path(), "admin", "a-long-test-password").unwrap();
        let app = router(
            Arc::new(PanickingBackend {
                upload_admission_panics,
            }),
            auth,
            ServerConfig::loopback(root.path()),
        )
        .unwrap();
        let (token, _, _) = authenticated(&app).await;
        let bytes = b"abc";
        let request = Request::builder()
            .method("PUT")
            .uri("/api/workspaces/ws/files/entry_00000000000000000000000000000000/content?path=file.txt")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header("x-zxs-logical-size", bytes.len().to_string())
            .header("x-zxs-content-hash", blake3::hash(bytes).to_hex().as_str())
            .body(Body::from(bytes.to_vec()))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        let body = assert_error(
            response,
            StatusCode::INTERNAL_SERVER_ERROR,
            "INTERNAL_ERROR",
            true,
        )
        .await;
        assert_eq!(body["error"]["message"], "request task failed");
        assert!(!body.to_string().contains("panic"));
    }
}

#[tokio::test]
async fn disk_registration_keeps_build_specific_refusal_after_body_parsing() {
    let root = crate::tests::private_tempdir();
    let app = application(root.path());
    let (token, _, _) = authenticated(&app).await;
    for (body, status, code) in [
        ("null", StatusCode::NOT_IMPLEMENTED, "UNSUPPORTED_OPERATION"),
        ("{", StatusCode::BAD_REQUEST, "INVALID_REQUEST"),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/admin/disks")
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_error(response, status, code, false).await;
    }
}
