use super::*;
use crate::{Backend, BackendAction, Content, ServerConfig, auth::AuthStore};
use http_routing::{
    body::{Body, to_bytes},
    http::{Request as HttpRequest, StatusCode},
};
use service_stack::ServiceExt;
use std::os::unix::fs::PermissionsExt;
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
struct Probe {
    service: super::super::SetupRecovery,
    mutations: AtomicUsize,
}
impl Backend for Probe {
    fn operation(
        &self,
        _: &crate::auth::Identity,
        _: storage_contract::OperationId,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        unreachable!("this test backend does not query operations")
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
        Err(crate::ApiError::unsupported())
    }
    fn abort_workspace(
        &self,
        _identity: &crate::auth::Identity,
        _workspace: &str,
    ) -> Result<(), crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }

    fn workspace(
        &self,
        _identity: &crate::auth::Identity,
        _workspace: &str,
    ) -> Result<storage_contract::Workspace, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn published_upload_status(
        &self,
        _identity: &crate::auth::Identity,
        _workspace: &str,
        _item: &str,
    ) -> Result<Option<crate::PublishedUploadStatus>, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn seal_workspace(
        &self,
        _identity: &crate::auth::Identity,
        _workspace: &str,
    ) -> Result<storage_contract::SealedWorkspace, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn commit_workspace_public(
        &self,
        _identity: &crate::auth::Identity,
        _command: storage_contract::PublicCommitEnvelope,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn version_snapshot(
        &self,
        _identity: &crate::auth::Identity,
        _version: &str,
    ) -> Result<crate::VersionSnapshot, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn authorize_version(
        &self,
        _identity: &crate::auth::Identity,
        _version: &str,
    ) -> Result<(), crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn repository_snapshot(
        &self,
        _identity: &crate::auth::Identity,
        _id: &str,
    ) -> Result<storage_contract::Repository, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn authorize_repository(
        &self,
        _identity: &crate::auth::Identity,
        _id: &str,
    ) -> Result<(), crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn capabilities(&self) -> Result<crate::Capabilities, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn ready(&self) -> Result<crate::Readiness, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn workspace_capacity(
        &self,
        _identity: &crate::auth::Identity,
    ) -> Result<storage_contract::WorkspaceCapacity, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn smb_binding_status(
        &self,
        _identity: &crate::auth::Identity,
        _id: &str,
    ) -> Result<crate::SmbBindingStatus, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn update_smb_binding(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::UpdateSmbBinding<'_>,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn protection_policy(
        &self,
        _identity: &crate::auth::Identity,
        _id: &str,
    ) -> Result<storage_contract::ProtectionPolicy, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn placement_rule(
        &self,
        _identity: &crate::auth::Identity,
        _id: &str,
    ) -> Result<storage_contract::PlacementRule, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn update_protection_policy(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::UpdateProtectionPolicy,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn update_placement_rule(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::UpdatePlacementRule,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn compare_commits(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::CompareCommits<'_>,
    ) -> Result<storage_contract::CommitComparison, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn disk_status(
        &self,
        _identity: &crate::auth::Identity,
    ) -> Result<Vec<storage_contract::DiskStatus>, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn integrity_check(
        &self,
        _identity: &crate::auth::Identity,
    ) -> Result<storage_contract::CheckReport, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn create_merge_link(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::CreateMergeLink,
    ) -> Result<storage_contract::MergeLink, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn create_repository(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::CreateRepository,
    ) -> Result<storage_contract::Repository, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn create_branch(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::CreateBranch,
    ) -> Result<crate::BranchSnapshot, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn create_file_branch(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::CreateFileBranch,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn update_note(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::UpdateNote,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn namespace_batch(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::NamespaceBatch,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn select_file_branch(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::SelectFileBranch,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn rename_file_branch(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::RenameFileBranch,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn listing(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::ReadListing,
    ) -> Result<crate::ListingSnapshot, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn mutate_disk_directory(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::MutateDiskDirectory,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn initialize_directory_scope(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::InitializeDirectoryScope,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn archive_branch(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::ArchiveBranch,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn rename_directory_branch(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::RenameDirectoryBranch,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn select_default_directory_branch(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::SelectDefaultDirectoryBranch,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn directory_selection(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::ReadDirectorySelection,
    ) -> Result<crate::DirectorySelectionSnapshot, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn directory_scopes(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::ReadDirectoryScopes<'_>,
    ) -> Result<crate::DirectoryScopesSnapshot, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn default_directory_branch(
        &self,
        _identity: &crate::auth::Identity,
        _repository_id: &str,
    ) -> Result<crate::DefaultDirectoryBranchSnapshot, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn file_history_snapshot(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::ReadFileHistory,
    ) -> Result<crate::FileHistorySnapshot, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn file_history(
        &self,
        _identity: &crate::auth::Identity,
        _file: &str,
    ) -> Result<Vec<storage_contract::FileVersion>, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn topology_snapshot(
        &self,
        _identity: &crate::auth::Identity,
    ) -> Result<crate::TopologySnapshot, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn placement_snapshot(
        &self,
        _identity: &crate::auth::Identity,
        _id: &str,
    ) -> Result<storage_contract::Placement, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn event_page(
        &self,
        _identity: &crate::auth::Identity,
        _after: u64,
    ) -> Result<crate::EventPage, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn activity_history(
        &self,
        _identity: &crate::auth::Identity,
    ) -> Result<crate::ActivityHistorySnapshot, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn history_graph(
        &self,
        _identity: &crate::auth::Identity,
        _id: &str,
    ) -> Result<crate::HistoryGraphSnapshot, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn commit_snapshot(
        &self,
        _identity: &crate::auth::Identity,
        _id: &str,
    ) -> Result<std::sync::Arc<storage_contract::Commit>, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn resolve_entry(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::ResolveEntry<'_>,
    ) -> Result<crate::ResolvedEntry, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn branch_snapshot(
        &self,
        _identity: &crate::auth::Identity,
        _id: &str,
    ) -> Result<crate::BranchSnapshot, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn branches_snapshot(
        &self,
        _identity: &crate::auth::Identity,
        _id: &str,
    ) -> Result<Vec<crate::BranchSnapshot>, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn repositories_snapshot(
        &self,
        _identity: &crate::auth::Identity,
    ) -> Result<Vec<storage_contract::Repository>, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn tree_snapshot(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::ReadTree<'_>,
    ) -> Result<crate::TreeSnapshot, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn create_write_session(
        &self,
        _identity: &crate::auth::Identity,
        _branch: &str,
    ) -> Result<crate::WorkspaceWriteSession, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn stage_namespace(
        &self,
        _identity: &crate::auth::Identity,
        _command: crate::StageNamespace,
    ) -> Result<storage_contract::Workspace, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn authorize_upload(
        &self,
        _identity: &crate::auth::Identity,
        _workspace: &str,
        _logical_size: Option<u64>,
    ) -> Result<(), crate::ApiError> {
        Err(crate::ApiError::unsupported())
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

    fn setup_operation(
        &self,
        action: &'static str,
        _: Value,
        bootstrap: Option<&str>,
        identity: Option<&Identity>,
    ) -> Result<Value, ApiError> {
        if identity.is_none() {
            self.service
                .authorize_bootstrap(bootstrap.ok_or_else(ApiError::unauthorized)?)?;
        }
        if action == "status" {
            return Ok(json_codec::to_value(self.service.status())
                .expect("invariant: setup status serialization succeeds"));
        }
        self.mutations.fetch_add(1, Ordering::SeqCst);
        Ok(json!({"accepted":true,"restart_required":false,"recovering":true}))
    }
    fn call(&self, _: BackendAction, _: &Identity, _: Value) -> Result<Value, ApiError> {
        Err(ApiError::unsupported())
    }
    fn upload(
        &self,
        _: &Identity,
        _: &str,
        _: &str,
        _: &name_policy::PortablePath,
        _: &crate::UploadSource,
    ) -> Result<storage_contract::StagedFile, ApiError> {
        Err(ApiError::unsupported())
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
        Err(crate::ApiError::unsupported())
    }

    fn range_content(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadContent<'_>,
    ) -> Result<crate::Content, crate::ApiError> {
        match request.context {
            Some(_) => Err(crate::ApiError::unsupported()),
            None => self.content(identity, request.version),
        }
    }
    fn content(&self, _: &Identity, _: &str) -> Result<Content, ApiError> {
        Err(ApiError::unsupported())
    }
}
fn app() -> (tempfile::TempDir, Router, Arc<Probe>) {
    app_with_auxiliary_failure(false)
}
fn app_with_auxiliary_failure(failed: bool) -> (tempfile::TempDir, Router, Arc<Probe>) {
    configured_app(failed, false)
}
fn configured_app(failed: bool, proxy: bool) -> (tempfile::TempDir, Router, Arc<Probe>) {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("system");
    let management = temp.path().join("management");
    std::fs::create_dir(&data).unwrap();
    std::fs::create_dir(&management).unwrap();
    let mut disks = BTreeMap::new();
    for id in ["one", "two", "three"] {
        let path = temp.path().join(id);
        std::fs::create_dir(&path).unwrap();
        disks.insert(id.into(), path);
    }
    let key = management.join("bootstrap.key");
    std::fs::write(&key, TOKEN).unwrap();
    std::fs::set_permissions(key, std::fs::Permissions::from_mode(0o600)).unwrap();
    let probe = Arc::new(Probe {
        service: super::super::SetupRecovery::new(data, management, disks).unwrap(),
        mutations: AtomicUsize::new(0),
    });
    let auth_root = temp.path().join("http-auth");
    let auth = AuthStore::initialize(&auth_root, "admin", "long-test-password").unwrap();
    let mut config = ServerConfig::loopback(&auth_root);
    if proxy {
        config.external_origin = Some("https://storage.example.org".into());
        config.trusted_proxy_ips = vec!["127.0.0.1".parse().unwrap()];
    }
    if failed {
        std::fs::write(&config.spool_dir, "keep this evidence").unwrap();
        config.web_dir = Some(temp.path().join("missing-web"));
    }
    let router = crate::router(probe.clone(), auth, config).unwrap();
    (temp, router, probe)
}
fn request(method: &str, path: &str, body: &str) -> http_routing::http::request::Builder {
    let _ = body;
    HttpRequest::builder()
        .method(method)
        .uri(path)
        .header("host", "127.0.0.1:7400")
        .header("origin", "http://127.0.0.1:7400")
        .header("content-type", "application/json")
}
fn proxy_request(method: &str, path: &str, ip: &str) -> http_routing::http::request::Builder {
    HttpRequest::builder()
        .method(method)
        .uri(path)
        .extension(http_routing::extract::ConnectInfo(
            "127.0.0.1:12345".parse::<std::net::SocketAddr>().unwrap(),
        ))
        .header("host", "storage.example.org")
        .header("origin", "https://storage.example.org")
        .header("x-forwarded-for", ip)
        .header("content-type", "application/json")
}
async fn proxy_admin_token(app: &Router) -> String {
    let response = app
        .clone()
        .oneshot(
            proxy_request("POST", "/api/auth/login", "192.0.2.200")
                .body(Body::from(
                    json!({"username":"admin", "password":"long-test-password"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let value: Value =
        json_codec::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
    value["token"].as_str().unwrap().to_owned()
}
#[tokio::test]
async fn setup_http_requires_capability_csrf_origin_and_no_secret_query() {
    let (_temp, app, probe) = app();
    let missing = app
        .clone()
        .oneshot(
            request("GET", "/api/setup/status", "")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::UNAUTHORIZED);
    let query = app
        .clone()
        .oneshot(
            request("GET", "/api/setup/status?bootstrap=secret", "")
                .header("x-zxs-bootstrap", TOKEN)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(query.status(), StatusCode::BAD_REQUEST);
    let missing_csrf = app
        .clone()
        .oneshot(
            request("POST", "/api/setup/initialize", "{}")
                .header("x-zxs-bootstrap", TOKEN)
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing_csrf.status(), StatusCode::UNAUTHORIZED);
    let cross = app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri("/api/setup/initialize")
                .header("host", "127.0.0.1:7400")
                .header("origin", "https://evil.invalid")
                .header("x-zxs-bootstrap", TOKEN)
                .header("x-csrf-token", csrf(TOKEN))
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(cross.status(), StatusCode::FORBIDDEN);
    let duplicate = app
        .clone()
        .oneshot(
            request("GET", "/api/setup/status", "")
                .header("x-zxs-bootstrap", TOKEN)
                .header("x-zxs-bootstrap", TOKEN)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(duplicate.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(probe.mutations.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn setup_http_bounds_body_and_returns_csrf_only_after_authorization() {
    let (_temp, app, probe) = app();
    let status = app
        .clone()
        .oneshot(
            request("GET", "/api/setup/status", "")
                .header("x-zxs-bootstrap", TOKEN)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(status.status(), StatusCode::OK);
    assert_eq!(status.headers()["cache-control"], "no-store");
    let bytes = to_bytes(status.into_body(), 65536).await.unwrap();
    let value: Value = json_codec::from_slice(&bytes).unwrap();
    assert_eq!(value["csrf_token"], csrf(TOKEN));
    assert!(!String::from_utf8_lossy(&bytes).contains(TOKEN));
    let large = "x".repeat(super::super::MAX_SETUP_BODY_BYTES + 1);
    let overflow = app
        .clone()
        .oneshot(
            request("POST", "/api/setup/initialize", &large)
                .header("x-zxs-bootstrap", TOKEN)
                .header("x-csrf-token", csrf(TOKEN))
                .body(Body::from(large))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(overflow.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let success = app
        .clone()
        .oneshot(
            request("POST", "/api/setup/initialize", "{}")
                .header("x-zxs-bootstrap", TOKEN)
                .header("x-csrf-token", csrf(TOKEN))
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(success.status(), StatusCode::OK);
    assert_eq!(probe.mutations.load(Ordering::SeqCst), 1);
    std::fs::remove_file(probe.service.management_root.join("bootstrap.key")).unwrap();
    let revoked = app
        .clone()
        .oneshot(
            request("GET", "/api/setup/status", "")
                .header("x-zxs-bootstrap", TOKEN)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(!revoked.status().is_success());
}
#[tokio::test]
async fn setup_http_rate_limits_before_backend_work() {
    let (_temp, app, probe) = app();
    for _ in 0..10 {
        let response = app
            .clone()
            .oneshot(
                request("GET", "/api/setup/status", "")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    let limited = app
        .clone()
        .oneshot(
            request("GET", "/api/setup/status", "")
                .header("x-zxs-bootstrap", TOKEN)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(probe.mutations.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn invalid_bootstrap_attempts_do_not_poison_other_sources_or_protected_recovery() {
    let (_temp, app, probe) = configured_app(false, true);
    let token = proxy_admin_token(&app).await;
    for (index, invalid) in ["missing", "malformed", "wrong", "origin"]
        .into_iter()
        .enumerate()
    {
        let source = format!("192.0.2.{}", index + 1);
        for _ in 0..10 {
            let mut call = proxy_request("GET", "/api/setup/status", &source);
            match invalid {
                "missing" => {}
                "malformed" => {
                    call = call.header("x-zxs-bootstrap", "not-a-capability");
                }
                "wrong" => {
                    call = call.header("x-zxs-bootstrap", "f".repeat(64));
                }
                "origin" => {
                    call = call.header("x-zxs-bootstrap", TOKEN);
                    call.headers_mut()
                        .unwrap()
                        .insert("origin", "https://evil.invalid".parse().unwrap());
                }
                _ => unreachable!(),
            }
            let response = app
                .clone()
                .oneshot(call.body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                if invalid == "origin" {
                    StatusCode::FORBIDDEN
                } else {
                    StatusCode::UNAUTHORIZED
                }
            );
        }
        let same_source = app
            .clone()
            .oneshot(
                proxy_request("GET", "/api/setup/status", &source)
                    .header("x-zxs-bootstrap", TOKEN)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(same_source.status(), StatusCode::TOO_MANY_REQUESTS);

        let other_source = app
            .clone()
            .oneshot(
                proxy_request("GET", "/api/setup/status", "192.0.2.100")
                    .header("x-zxs-bootstrap", TOKEN)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(other_source.status(), StatusCode::OK);
        // Authenticated recovery is independent even on the rejected public source.
        let recovery = app
            .clone()
            .oneshot(
                proxy_request("POST", "/api/management/offline-check", &source)
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(recovery.status(), StatusCode::OK);
    }
    assert_eq!(probe.mutations.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn protected_recovery_keeps_its_own_global_rate_budget() {
    let (_temp, app, probe) = configured_app(false, true);
    let token = proxy_admin_token(&app).await;
    for index in 1..=11 {
        let response = app
            .clone()
            .oneshot(
                proxy_request(
                    "POST",
                    "/api/management/offline-check",
                    &format!("192.0.2.{index}"),
                )
                .header("authorization", format!("Bearer {token}"))
                .body(Body::from("{}"))
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            if index <= 10 {
                StatusCode::OK
            } else {
                StatusCode::TOO_MANY_REQUESTS
            }
        );
    }
    assert_eq!(probe.mutations.load(Ordering::SeqCst), 10);
    let bootstrap = app
        .oneshot(
            proxy_request("GET", "/api/setup/status", "192.0.2.100")
                .header("x-zxs-bootstrap", TOKEN)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(bootstrap.status(), StatusCode::OK);
}

#[tokio::test]
async fn public_setup_key_capacity_cannot_exhaust_login_or_recovery_admission() {
    let (_temp, app, probe) = configured_app(false, true);
    for index in 0..4096_u32 {
        let ip = std::net::Ipv4Addr::from(0xc000_0000 + index).to_string();
        let response = app
            .clone()
            .oneshot(
                proxy_request("GET", "/api/setup/status", &ip)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    let full = app
        .clone()
        .oneshot(
            proxy_request("GET", "/api/setup/status", "198.51.100.1")
                .header("x-zxs-bootstrap", TOKEN)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(full.status(), StatusCode::TOO_MANY_REQUESTS);
    let token = proxy_admin_token(&app).await;
    let recovery = app
        .oneshot(
            proxy_request("POST", "/api/management/offline-check", "198.51.100.1")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(recovery.status(), StatusCode::OK);
    assert_eq!(probe.mutations.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn auxiliary_failure_preserves_bootstrap_authorization_and_setup_routes() {
    let (_temp, app, probe) = app_with_auxiliary_failure(true);
    for authorized in [false, true] {
        let mut call = request("GET", "/api/setup/status", "");
        if authorized {
            call = call.header("x-zxs-bootstrap", TOKEN);
        }
        let response = app
            .clone()
            .oneshot(call.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            if authorized {
                StatusCode::OK
            } else {
                StatusCode::UNAUTHORIZED
            }
        );
    }
    for csrf_provided in [false, true] {
        let mut call =
            request("POST", "/api/setup/initialize", "{}").header("x-zxs-bootstrap", TOKEN);
        if csrf_provided {
            call = call.header("x-csrf-token", csrf(TOKEN));
        }
        let response = app
            .clone()
            .oneshot(call.body(Body::from("{}")).unwrap())
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            if csrf_provided {
                StatusCode::OK
            } else {
                StatusCode::UNAUTHORIZED
            }
        );
    }
    assert_eq!(probe.mutations.load(Ordering::SeqCst), 1);
}
