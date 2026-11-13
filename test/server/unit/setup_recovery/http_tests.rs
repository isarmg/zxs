use super::*;
use crate::{Backend, Content, ServerConfig, auth::AuthStore};
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
    fn operation_outcome(
        &self,
        _: &crate::auth::Identity,
        _: storage_contract::OperationId,
    ) -> Result<storage_contract::OperationOutcome, crate::ApiError> {
        unreachable!("this test backend does not query operation outcomes")
    }
    fn operation(
        &self,
        _: &crate::auth::Identity,
        _: storage_contract::OperationId,
    ) -> Result<crate::OperationObservation, crate::ApiError> {
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
    fn resolve_smb_path(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::ResolveSmbPath,
    ) -> Result<crate::SmbPathLocation, crate::ApiError> {
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
    fn merge_file_branches(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::MergeFileBranches,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn prepare_container_draft_commit(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::PrepareContainerDraftCommit,
    ) -> Result<storage_contract::SealedWorkspace, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn read_container_edit(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::ReadContainerEdit,
    ) -> Result<storage_contract::container_edit::ContainerEdit, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn set_container_commit_mode(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::SetContainerCommitMode,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn persist_container_draft(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::PersistContainerDraft,
        _source: &std::fs::File,
    ) -> Result<crate::DraftPersisted, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn current_container_content(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::ReadContainerEdit,
    ) -> Result<crate::CurrentContainerContent, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn replace_container_tags(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::ReplaceContainerTags,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn create_disk_directory(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::CreateDiskDirectory,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn create_disk_write_session(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::CreateDiskWriteSession,
    ) -> Result<crate::DiskWriteSession, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn backup_configuration(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::ReadBackupConfiguration,
    ) -> Result<storage_contract::BackupConfigurationSnapshot, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn configure_backups(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::ConfigureBackups,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn plan(
        &self,
        _identity: &crate::auth::Identity,
        _plan: &str,
    ) -> Result<storage_contract::Plan, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn cancel_plan(
        &self,
        _identity: &crate::auth::Identity,
        _request: Result<crate::CancelPlan, crate::ApiError>,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn register_disk(
        &self,
        _identity: &crate::auth::Identity,
    ) -> Result<std::convert::Infallible, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn commit_plan(
        &self,
        _identity: &crate::auth::Identity,
        _request: Result<crate::CommitPlan, crate::ApiError>,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn prepare_historical_save(
        &self,
        _identity: &crate::auth::Identity,
        _request: Result<crate::PrepareHistoricalSave, crate::ApiError>,
    ) -> Result<storage_contract::Plan, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn prepare_purge(
        &self,
        _identity: &crate::auth::Identity,
        _request: Result<crate::PreparePurge, crate::ApiError>,
    ) -> Result<storage_contract::Plan, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn prepare_split(
        &self,
        _identity: &crate::auth::Identity,
        _request: Result<crate::PrepareSplit, crate::ApiError>,
    ) -> Result<storage_contract::Plan, crate::ApiError> {
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
    fn history_window(
        &self,
        _identity: &crate::auth::Identity,
        _request: storage_contract::history_window::HistoryWindowRequest,
        _cancellation: storage_contract::history_window::HistoryWindowCancellation,
    ) -> Result<storage_contract::history_window::HistoryWindowPage, crate::ApiError> {
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
    fn authorize_workspace_write(
        &self,
        _identity: &crate::auth::Identity,
        _workspace: &str,
        _access: crate::WorkspaceWrite<'_>,
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
        if action == "status" {
            return Ok(json!({"state":self.service.management_status()}));
        }
        if identity.is_none() && action != "initialize" {
            self.service
                .authorize_bootstrap(bootstrap.ok_or_else(ApiError::unauthorized)?)?;
        }
        if action == "recovery_status" {
            return Ok(json_codec::to_value(self.service.status())
                .expect("invariant: setup status serialization succeeds"));
        }
        self.mutations.fetch_add(1, Ordering::SeqCst);
        Ok(json!({"accepted":true,"restart_required":false,"recovering":true}))
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
    let mut config = ServerConfig::new(&auth_root);
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
async fn recovery_http_requires_capability_csrf_origin_and_no_secret_query() {
    let (_temp, app, probe) = app();
    let missing = app
        .clone()
        .oneshot(
            request("GET", "/api/setup/recovery-status", "")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::UNAUTHORIZED);
    let query = app
        .clone()
        .oneshot(
            request("GET", "/api/setup/recovery-status?bootstrap=secret", "")
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
            request("POST", "/api/setup/restore-system", "{}")
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
                .uri("/api/setup/restore-system")
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
            request("GET", "/api/setup/recovery-status", "")
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
async fn recovery_http_bounds_body_and_returns_csrf_only_after_authorization() {
    let (_temp, app, probe) = app();
    let status = app
        .clone()
        .oneshot(
            request("GET", "/api/setup/recovery-status", "")
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
            request("POST", "/api/setup/restore-system", &large)
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
            request("POST", "/api/setup/restore-system", "{}")
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
            request("GET", "/api/setup/recovery-status", "")
                .header("x-zxs-bootstrap", TOKEN)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(!revoked.status().is_success());
}
#[tokio::test]
async fn recovery_http_rate_limits_before_backend_work() {
    let (_temp, app, probe) = app();
    for _ in 0..10 {
        let response = app
            .clone()
            .oneshot(
                request("GET", "/api/setup/recovery-status", "")
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
            request("GET", "/api/setup/recovery-status", "")
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
            let mut call = proxy_request("GET", "/api/setup/recovery-status", &source);
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
                proxy_request("GET", "/api/setup/recovery-status", &source)
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
                proxy_request("GET", "/api/setup/recovery-status", "192.0.2.100")
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
            proxy_request("GET", "/api/setup/recovery-status", "192.0.2.100")
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
                proxy_request("GET", "/api/setup/recovery-status", &ip)
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
            proxy_request("GET", "/api/setup/recovery-status", "198.51.100.1")
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
        let mut call = request("GET", "/api/setup/recovery-status", "");
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
            request("POST", "/api/setup/restore-system", "{}").header("x-zxs-bootstrap", TOKEN);
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

#[tokio::test]
async fn direct_remote_http_setup_and_login_preserve_credentials_and_csrf() {
    let (_temp, app, probe) = app();
    let remote_request = |method: &str, path: &str| {
        HttpRequest::builder()
            .method(method)
            .uri(path)
            .extension(http_routing::extract::ConnectInfo(
                "192.0.2.20:12345".parse::<std::net::SocketAddr>().unwrap(),
            ))
            .header("host", "192.0.2.10:7400")
            .header("origin", "http://192.0.2.10:7400")
            .header("content-type", "application/json")
    };
    let missing = app
        .clone()
        .oneshot(
            remote_request("GET", "/api/setup/recovery-status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::UNAUTHORIZED);
    let status = app
        .clone()
        .oneshot(
            remote_request("GET", "/api/setup/recovery-status")
                .header("x-zxs-bootstrap", TOKEN)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(status.status(), StatusCode::OK);
    let value: Value =
        json_codec::from_slice(&to_bytes(status.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(value["csrf_token"], csrf(TOKEN));
    let missing_csrf = app
        .clone()
        .oneshot(
            remote_request("POST", "/api/setup/restore-system")
                .header("x-zxs-bootstrap", TOKEN)
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing_csrf.status(), StatusCode::UNAUTHORIZED);
    let login = app
        .clone()
        .oneshot(
            remote_request("POST", "/api/auth/login")
                .body(Body::from(
                    json!({"username":"admin", "password":"long-test-password"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(login.status(), StatusCode::OK);
    let unauthenticated = app
        .oneshot(
            remote_request("GET", "/api/auth/me")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(probe.mutations.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn ordinary_http_origin_header_matrix_keeps_bearer_cookie_and_proxy_boundaries() {
    for proxy in [false, true] {
        let (_temp, app, _probe) = configured_app(false, proxy);
        let origin = if proxy {
            "https://storage.example.org"
        } else {
            "http://192.0.2.10"
        };
        let host = if proxy {
            "127.0.0.1:7400"
        } else {
            "192.0.2.10:80"
        };
        let build = |path: &str| {
            HttpRequest::builder()
                .method("POST")
                .uri(path)
                .extension(http_routing::extract::ConnectInfo(
                    "127.0.0.1:12345".parse::<std::net::SocketAddr>().unwrap(),
                ))
                .header("host", host)
                .header("x-forwarded-for", "192.0.2.20")
                .header("content-type", "application/json")
        };
        let response = app
            .clone()
            .oneshot(
                build("/api/auth/login")
                    .header("origin", origin)
                    .body(Body::from(
                        json!({"username":"admin", "password":"long-test-password"}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let session: Value =
            json_codec::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
        let token = session["token"].as_str().unwrap();
        let csrf = authentication::derive_csrf_token(token).unwrap();
        // Invalid body stops before a password change, but only after real authorization.
        for (cookie, origin_present, fetch, expected) in [
            (false, false, None, StatusCode::BAD_REQUEST),
            (false, false, Some("cross-site"), StatusCode::BAD_REQUEST),
            (true, false, None, StatusCode::FORBIDDEN),
            (true, true, None, StatusCode::BAD_REQUEST),
            (false, true, Some("cross-site"), StatusCode::FORBIDDEN),
        ] {
            let mut request = build("/api/auth/password");
            if cookie {
                request = request
                    .header("cookie", format!("zxs_session={token}"))
                    .header("x-csrf-token", &csrf);
            } else {
                request = request.header("authorization", format!("Bearer {token}"));
            }
            if origin_present {
                request = request.header("origin", origin);
            }
            if let Some(site) = fetch {
                request = request.header("sec-fetch-site", site);
            }
            assert_eq!(
                app.clone()
                    .oneshot(request.body(Body::from("{}")).unwrap())
                    .await
                    .unwrap()
                    .status(),
                expected,
                "proxy={proxy} cookie={cookie} origin={origin_present}"
            );
        }
        for (name, first, second) in [
            ("origin", origin, origin),
            ("origin", origin, "https://evil.example"),
            ("host", host, "evil.example"),
            ("sec-fetch-site", "same-origin", "cross-site"),
            ("x-csrf-token", csrf.as_str(), csrf.as_str()),
            ("x-csrf-token", csrf.as_str(), "wrong"),
        ] {
            let mut request =
                build("/api/auth/password").header("cookie", format!("zxs_session={token}"));
            if name != "origin" {
                request = request.header("origin", origin);
            }
            if name != "x-csrf-token" {
                request = request.header("x-csrf-token", &csrf);
            }
            // Host already exists in build; append the conflict once.
            if name != "host" {
                request = request.header(name, first);
            }
            request = request.header(name, second);
            assert_eq!(
                app.clone()
                    .oneshot(request.body(Body::from("{}")).unwrap())
                    .await
                    .unwrap()
                    .status(),
                StatusCode::FORBIDDEN,
                "proxy={proxy} duplicate={name}"
            );
        }
        let request = build("/api/auth/password")
            .header("authorization", format!("Bearer {token}"))
            .header("sec-fetch-site", "same-origin")
            .header("sec-fetch-site", "cross-site");
        assert_eq!(
            app.clone()
                .oneshot(request.body(Body::from("{}")).unwrap())
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
    }
}

#[tokio::test]
async fn bootstrap_fetch_and_csrf_duplicates_are_rejected_before_mutation() {
    let (_temp, app, probe) = app();
    for (method, path, duplicate, first, second) in [
        (
            "GET",
            "/api/setup/recovery-status",
            "sec-fetch-site",
            "same-origin",
            "cross-site",
        ),
        (
            "POST",
            "/api/setup/restore-system",
            "sec-fetch-site",
            "same-origin",
            "cross-site",
        ),
        (
            "POST",
            "/api/setup/restore-system",
            "x-csrf-token",
            csrf(TOKEN).as_str(),
            csrf(TOKEN).as_str(),
        ),
    ] {
        let response = app
            .clone()
            .oneshot(
                request(method, path, "{}")
                    .header("x-zxs-bootstrap", TOKEN)
                    .header(duplicate, first)
                    .header(duplicate, second)
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(matches!(
            response.status(),
            StatusCode::FORBIDDEN | StatusCode::UNAUTHORIZED
        ));
    }
    assert_eq!(probe.mutations.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn public_account_routes_need_no_key_and_disclose_no_recovery_fields() {
    let (_temp, app, probe) = app();
    std::fs::remove_file(probe.service.management_root.join("bootstrap.key")).unwrap();
    let status = app
        .clone()
        .oneshot(
            request("GET", "/api/setup/status", "")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(status.status(), StatusCode::OK);
    assert_eq!(status.headers()["cache-control"], "no-store");
    let value: Value =
        json_codec::from_slice(&to_bytes(status.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(value, json!({"state":"empty"}));
    let response = app
        .clone()
        .oneshot(
            request("POST", "/api/setup/initialize", "{}")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(probe.mutations.load(Ordering::SeqCst), 1);
    let recovery = app
        .oneshot(
            request("GET", "/api/setup/recovery-status", "")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(recovery.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn public_account_origin_fetch_json_and_body_limits_precede_backend_mutation() {
    for rejected in [
        "missing-origin",
        "cross-origin",
        "duplicate-origin",
        "cross-fetch",
        "duplicate-fetch",
        "query",
        "content-type",
        "body-limit",
    ] {
        let (_temp, app, probe) = app();
        let path = if rejected == "query" {
            "/api/setup/initialize?path=/outside"
        } else {
            "/api/setup/initialize"
        };
        let mut call = HttpRequest::builder()
            .method("POST")
            .uri(path)
            .header("host", "127.0.0.1:7400");
        if rejected != "missing-origin" {
            call = call.header(
                "origin",
                if rejected == "cross-origin" {
                    "https://evil.invalid"
                } else {
                    "http://127.0.0.1:7400"
                },
            );
        }
        if rejected == "duplicate-origin" {
            call = call.header("origin", "http://127.0.0.1:7400");
        }
        if matches!(rejected, "cross-fetch" | "duplicate-fetch") {
            call = call.header("sec-fetch-site", "cross-site");
        }
        if rejected == "duplicate-fetch" {
            call = call.header("sec-fetch-site", "same-origin");
        }
        call = call.header(
            "content-type",
            if rejected == "content-type" {
                "text/plain"
            } else {
                "application/json"
            },
        );
        let body = if rejected == "body-limit" {
            "x".repeat(super::super::MAX_SETUP_BODY_BYTES + 1)
        } else {
            "{}".into()
        };
        let response = app
            .oneshot(call.body(Body::from(body)).unwrap())
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            match rejected {
                "query" => StatusCode::BAD_REQUEST,
                "content-type" => StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "body-limit" => StatusCode::PAYLOAD_TOO_LARGE,
                _ => StatusCode::FORBIDDEN,
            },
            "{rejected}"
        );
        assert_eq!(probe.mutations.load(Ordering::SeqCst), 0, "{rejected}");
    }
}

#[tokio::test]
async fn public_account_queries_share_bounded_source_admission_before_mutation() {
    let (_temp, app, probe) = app();
    for _ in 0..10 {
        assert_eq!(
            app.clone()
                .oneshot(
                    request("GET", "/api/setup/status", "")
                        .body(Body::empty())
                        .unwrap()
                )
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
    }
    let response = app
        .oneshot(
            request("POST", "/api/setup/initialize", "{}")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(probe.mutations.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn authenticated_storage_request_budgets_accept_full_candidates_and_reject_overflow() {
    let (_temp, app, probe) = configured_app(false, true);
    let token = proxy_admin_token(&app).await;
    let before = probe.mutations.load(Ordering::SeqCst);
    let vectors: Value =
        json_codec::from_str(include_str!("../../../../docs/contracts/vectors.json")).unwrap();
    let candidate = vectors["positive"]
        .as_array()
        .unwrap()
        .iter()
        .find(|vector| vector["schema"] == "StorageCandidateSelection")
        .unwrap()["value"]
        .clone();
    let ordinary = json!({"selections":vec![candidate.clone();32],"mode":"production"});
    let ordinary_bytes = json_codec::to_vec(&ordinary).unwrap();
    assert!(ordinary_bytes.len() > super::super::MAX_SETUP_BODY_BYTES);
    assert!(ordinary_bytes.len() < MAX_ACTIVATION_BODY_BYTES);
    let _: super::super::StorageActivationRequest = json_codec::from_value(ordinary).unwrap();
    // Exercise maximum JSON escaping, not only ordinary short fixture paths.
    let path = format!("/{}", "\u{1}".repeat(4095));
    let mut long = candidate;
    for key in ["discovery_root", "mount_path", "root_path"] {
        long[key] = json!(path);
    }
    long["mount_identity"]["filesystem_root"] = json!(path);
    let long_body =
        json_codec::to_vec(&json!({"selections":vec![long;32],"mode":"production"})).unwrap();
    assert!(long_body.len() < MAX_ACTIVATION_BODY_BYTES);
    for (method, path, source, mut body, limit) in [
        (
            "POST",
            "/api/admin/storage/activate",
            "192.0.2.1",
            long_body,
            MAX_ACTIVATION_BODY_BYTES,
        ),
        (
            "POST",
            "/api/admin/storage/directories/inspect",
            "192.0.2.2",
            json_codec::to_vec(&json!({"mount_path":path,"root_path":path})).unwrap(),
            MAX_DIRECTORY_BODY_BYTES,
        ),
    ] {
        assert!(body.len() <= limit);
        body.resize(limit, b' ');
        let accepted = app
            .clone()
            .oneshot(
                proxy_request(method, path, source)
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::from(body.clone()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(accepted.status(), StatusCode::OK, "{path}");
        body.push(b' ');
        let rejected = app
            .clone()
            .oneshot(
                proxy_request(method, path, source)
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(rejected.status(), StatusCode::PAYLOAD_TOO_LARGE, "{path}");
    }
    assert_eq!(probe.mutations.load(Ordering::SeqCst), before + 2);
}

#[tokio::test]
async fn existing_pool_addition_routes_keep_session_origin_and_csrf_boundaries() {
    let (_temp, app, probe) = configured_app(false, true);
    let token = proxy_admin_token(&app).await;
    for (method, path) in [
        ("POST", "/api/admin/storage/foreign/preview"),
        ("POST", "/api/admin/storage/independent/preview"),
        ("POST", "/api/admin/storage/foreign/plans/preview"),
        ("POST", "/api/admin/storage/foreign/plans/save"),
        ("POST", "/api/admin/storage/foreign/plans/revalidate"),
        ("POST", "/api/admin/storage/foreign/plans/execute"),
        ("GET", "/api/admin/storage/foreign/plans/status"),
        ("POST", "/api/admin/storage/conversion/stop"),
        ("POST", "/api/admin/disks/preview"),
        ("POST", "/api/admin/disks"),
        ("POST", "/api/admin/disks/addition/resume"),
        ("POST", "/api/admin/disks/addition/cancel"),
        ("GET", "/api/admin/disks/addition"),
        ("POST", "/api/admin/disks/exit/preview"),
        ("POST", "/api/admin/disks/exit"),
        ("POST", "/api/admin/disks/exit/resume"),
        ("GET", "/api/admin/disks/exit"),
        ("GET", "/api/admin/disks/registrations"),
        ("POST", "/api/admin/disks/readmission/preview"),
        ("POST", "/api/admin/disks/readmission"),
        ("POST", "/api/admin/disks/readmission/resume"),
        ("GET", "/api/admin/disks/readmission"),
    ] {
        let response = app
            .clone()
            .oneshot(
                proxy_request(method, path, "192.0.2.20")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{path}");
        if method == "POST" {
            let response = app
                .clone()
                .oneshot(
                    proxy_request(method, path, "192.0.2.20")
                        .header("cookie", format!("zxs_session={token}"))
                        .body(Body::from("{}"))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN, "{path}");
        }
    }
    for (path, body, expected) in [
        (
            "/api/admin/storage/foreign/preview?execute=true",
            "{}".to_owned(),
            StatusCode::BAD_REQUEST,
        ),
        (
            "/api/admin/storage/independent/preview?execute=true",
            "{}".to_owned(),
            StatusCode::BAD_REQUEST,
        ),
        (
            "/api/admin/storage/independent/preview",
            " ".repeat(MAX_DIRECTORY_BODY_BYTES + 1),
            StatusCode::PAYLOAD_TOO_LARGE,
        ),
        (
            "/api/admin/storage/foreign/preview",
            " ".repeat(MAX_DIRECTORY_BODY_BYTES + 1),
            StatusCode::PAYLOAD_TOO_LARGE,
        ),
    ] {
        let response = app
            .clone()
            .oneshot(
                proxy_request("POST", path, "192.0.2.20")
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), expected, "{path}");
    }
    assert_eq!(probe.mutations.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn foreign_intent_routes_enforce_http_admission_and_exact_body_limits() {
    let (_temp, app, probe) = configured_app(false, true);
    let token = proxy_admin_token(&app).await;
    for (action, limit) in [
        ("preview", super::super::MAX_SETUP_BODY_BYTES),
        ("save", 2 * crate::JSON_BODY_LIMIT),
        ("status", super::super::MAX_SETUP_BODY_BYTES),
    ] {
        let path = format!("/api/admin/storage/foreign/execution-intents/{action}");
        for rejection in [
            "session",
            "csrf",
            "origin",
            "duplicate-json",
            "query",
            "overflow",
        ] {
            let url = if rejection == "query" {
                format!("{path}?execute=true")
            } else {
                path.clone()
            };
            let mut request = proxy_request("POST", &url, "192.0.2.21");
            if rejection == "csrf" {
                request = request.header("cookie", format!("zxs_session={token}"));
            } else if rejection != "session" {
                request = request.header("authorization", format!("Bearer {token}"));
            }
            if rejection == "origin" {
                // A duplicate conflicting Origin must not be collapsed to the
                // expected one by any proxy/header normalization shortcut.
                request = request.header("origin", "https://elsewhere.invalid");
            }
            let body = match rejection {
                "duplicate-json" => {
                    "{\"operation_id\":\"one\",\"operation_id\":\"two\"}".to_owned()
                }
                "overflow" => " ".repeat(limit + 1),
                _ => "{}".to_owned(),
            };
            let response = app
                .clone()
                .oneshot(request.body(Body::from(body)).unwrap())
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                match rejection {
                    "session" => StatusCode::UNAUTHORIZED,
                    "csrf" | "origin" => StatusCode::FORBIDDEN,
                    "overflow" => StatusCode::PAYLOAD_TOO_LARGE,
                    _ => StatusCode::BAD_REQUEST,
                },
                "{path}: {rejection}"
            );
        }
    }
    assert_eq!(probe.mutations.load(Ordering::SeqCst), 0);
    // This probe tests only HTTP admission, not intent or namespace completion.
    // The real manager/Engine integration is exercised by foreign_intent tests.
    for (index, action, limit) in [
        (1, "preview", super::super::MAX_SETUP_BODY_BYTES),
        (2, "save", 2 * crate::JSON_BODY_LIMIT),
        (3, "status", super::super::MAX_SETUP_BODY_BYTES),
    ] {
        let path = format!("/api/admin/storage/foreign/execution-intents/{action}");
        let mut bytes = b"{}".to_vec();
        bytes.resize(limit, b' ');
        let response = app
            .clone()
            .oneshot(
                proxy_request("POST", &path, &format!("192.0.2.{}", 30 + index))
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::from(bytes))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{path}");
    }
    assert_eq!(probe.mutations.load(Ordering::SeqCst), 3);
}
