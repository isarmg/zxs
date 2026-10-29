use super::*;
use crate::{
    Backend, Content,
    auth::{AuthStore, Identity},
};
use http_routing::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use json_codec::Value;
use service_stack::ServiceExt;
use std::{
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

const DEADLINE: Duration = Duration::from_secs(10);

#[derive(Default)]
struct Probe(AtomicUsize, Option<Activation>);
enum Notifications {
    Live,
    Absent,
    Closed,
}
struct Activation {
    notifications: Notifications,
    corrupt: std::sync::atomic::AtomicBool,
    ready: std::sync::atomic::AtomicBool,
    changed: tokio::sync::watch::Sender<()>,
    attempted: tokio::sync::Semaphore,
}
impl Backend for Probe {
    fn qualify_http_spool_copy(
        &self,
        _: &crate::InternalHttpSpoolRetirement,
        _: &crate::HttpSpoolCandidate,
    ) -> Result<crate::HttpSpoolQualification, ApiError> {
        let Some(activation) = &self.1 else {
            return Ok(crate::HttpSpoolQualification::Retain);
        };
        activation.attempted.add_permits(1);
        if activation.corrupt.load(Ordering::SeqCst) {
            return Err(ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                storage_contract::ErrorCode::RecoveryRequired,
                "corrupt retirement proof",
            ));
        }
        if activation.ready.load(Ordering::SeqCst) {
            Ok(crate::HttpSpoolQualification::Retain)
        } else {
            Ok(crate::HttpSpoolQualification::Unavailable)
        }
    }
    fn http_spool_activation(
        &self,
        _: &crate::InternalHttpSpoolRetirement,
    ) -> Option<tokio::sync::watch::Receiver<()>> {
        self.1
            .as_ref()
            .and_then(|activation| match activation.notifications {
                Notifications::Live => Some(activation.changed.subscribe()),
                Notifications::Absent => None,
                Notifications::Closed => {
                    let (sender, receiver) = tokio::sync::watch::channel(());
                    drop(sender);
                    Some(receiver)
                }
            })
    }

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
    fn merge_file_branches(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::MergeFileBranches,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn update_file_node_tags(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::UpdateFileNodeTags,
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
    fn file_history_snapshot(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::ReadFileHistory,
    ) -> Result<crate::FileHistoryPage, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn file_history_node_parents(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::ReadNodeParents,
    ) -> Result<crate::NodeParentsPage, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn file_history_branches(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::ReadHistoryBranches,
    ) -> Result<crate::HistoryBranchesPage, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn file_history_merges(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::ReadHistoryMerges,
    ) -> Result<crate::HistoryMergesPage, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn file_history_merge_inputs(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::ReadMergeInputs,
    ) -> Result<crate::MergeInputsPage, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn file_history_merge_parents(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::ReadMergeParents,
    ) -> Result<crate::MergeParentsPage, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn file_history_node_field(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::ReadNodeField,
    ) -> Result<crate::NodeFieldSlice, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn file_version_page(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::ReadVersionPage,
    ) -> Result<crate::VersionPage, crate::ApiError> {
        Err(crate::ApiError::unsupported())
    }
    fn file_version_sources(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::ReadVersionSources,
    ) -> Result<crate::VersionSourcesPage, crate::ApiError> {
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

    fn observe_disks(
        &self,
        _: &crate::InternalDiskObservation,
    ) -> Result<Vec<crate::DiskSample>, ApiError> {
        Ok(vec![])
    }
    fn upload(
        &self,
        _: &Identity,
        _: &str,
        _: &str,
        _: &name_policy::PortablePath,
        _: &crate::UploadSource,
    ) -> Result<storage_contract::StagedFile, ApiError> {
        self.0.fetch_add(1, Ordering::SeqCst);
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

async fn response(
    app: &Router,
    method: &str,
    path: &str,
    token: Option<&str>,
) -> http_routing::response::Response {
    let mut request = Request::builder().method(method).uri(path);
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    app.clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
}
async fn json_body(response: http_routing::response::Response) -> Value {
    json_codec::from_slice(
        &http_routing::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap()
}

#[tokio::test(flavor = "current_thread")]
async fn stalled_real_inventory_keeps_management_responsive_and_uploads_pending() {
    let root = crate::tests::private_tempdir();
    let mut config = ServerConfig::loopback(root.path());
    config.http_spool_limits.bytes_total = 65536 + 7;
    config.http_spool_limits.bytes_per_account = 65536 + 7;
    config.prepare_spool().unwrap();
    let unknown = config.spool_dir.join("retained-unknown");
    std::fs::write(&unknown, b"payload").unwrap();
    let web = root.path().join("web");
    std::fs::create_dir(&web).unwrap();
    std::fs::write(web.join("index.html"), b"management").unwrap();
    config.web_dir = Some(web);
    let auth =
        AuthStore::initialize(&root.path().join("auth"), "admin", "a-long-test-password").unwrap();
    let probe = Arc::new(Probe::default());
    let runtime = crate::ManagementRuntime::start(probe.clone(), auth, &config).unwrap();
    let (spool, startup) = HttpSpool::pending();
    let app = crate::router_shared(
        runtime.backend,
        runtime.auth,
        config.clone().admit().unwrap(),
        runtime.telemetry,
        spool.clone(),
    )
    .unwrap();
    let (started, waiting) = tokio::sync::oneshot::channel();
    let (release, blocked) = std::sync::mpsc::channel();
    let worker_config = config.clone();
    let job = tokio::spawn(startup.run_with(move || {
        // Finish real inventory and retain its exclusive lock, but withhold
        // its result to model a slow startup job completion boundary.
        let budget = worker_config.open_http_spool().unwrap();
        started.send(()).unwrap();
        blocked.recv_timeout(DEADLINE).unwrap();
        Some(budget)
    }));
    tokio::time::timeout(DEADLINE, waiting)
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(DEADLINE, tokio::time::sleep(Duration::from_millis(10)))
        .await
        .unwrap();
    assert!(!job.is_finished());
    assert!(SpoolBudget::recover(&config.spool_dir, config.http_spool_limits.clone()).is_err());
    for path in ["/", "/healthz", "/api/management/status"] {
        assert_eq!(
            response(&app, "GET", path, None).await.status(),
            StatusCode::OK
        );
    }
    let status = json_body(response(&app, "GET", "/api/management/status", None).await).await;
    assert_eq!(status["auxiliary"]["http_uploads"], "pending");
    assert!(status["auxiliary"]["http_upload_maintenance"]["unresolved"].is_null());
    assert!(!status.to_string().contains(root.path().to_str().unwrap()));
    let login = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/login")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"username":"admin","password":"a-long-test-password"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(login.status(), StatusCode::OK);
    let cookie = login.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let login = json_body(login).await;
    let token = login["token"].as_str().unwrap();
    for path in [
        "/api/auth/me",
        "/api/system/status",
        "/api/system/history?range=1min",
    ] {
        assert_eq!(
            response(&app, "GET", path, Some(token)).await.status(),
            StatusCode::OK
        );
    }
    for (method, path) in [
        ("PUT", "/api/workspaces/ws/files/item/content?path=file"),
        ("GET", "/api/workspaces/ws/files/item/upload-status"),
    ] {
        assert_eq!(
            response(&app, method, path, None).await.status(),
            StatusCode::UNAUTHORIZED
        );
        let pending = response(&app, method, path, Some(token)).await;
        assert_eq!(pending.status(), StatusCode::SERVICE_UNAVAILABLE);
        let error = json_body(pending).await;
        assert_eq!(error["error"]["code"], "HTTP_UPLOADS_PENDING");
        assert_eq!(error["error"]["retryable"], true);
    }
    let csrf = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/workspaces/ws/files/item/content?path=file")
                .header("cookie", cookie)
                .header("host", "127.0.0.1:7400")
                .header("origin", "http://127.0.0.1:7400")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(csrf.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        probe.0.load(Ordering::SeqCst),
        0,
        "pending staging must not dispatch backend work"
    );
    assert_eq!(std::fs::read_dir(&config.spool_dir).unwrap().count(), 2);
    release.send(()).unwrap();
    tokio::time::timeout(DEADLINE, job).await.unwrap().unwrap();
    assert_eq!(spool.status(), "ready");
    assert_eq!(
        json_body(response(&app, "GET", "/api/management/status", None).await).await["auxiliary"]["http_uploads"],
        "ready"
    );
    assert_eq!(
        json_body(response(&app, "GET", "/api/management/status", None).await).await["auxiliary"]["http_upload_maintenance"]
            ["unresolved"],
        0
    );
    // Unknown retained bytes remain charged; readiness never creates capacity.
    let error = spool
        .budget()
        .unwrap()
        .reserve("admin", "new", 0)
        .err()
        .unwrap();
    assert_eq!(
        error.code,
        storage_contract::ErrorCode::ResourceLimitExceeded
    );
    assert_eq!(std::fs::read(&unknown).unwrap(), b"payload");
}

#[tokio::test]
async fn failed_inventory_and_panicked_job_become_terminal_unavailable() {
    for failure in ["occupied", "unsafe_inventory", "locked", "panic"] {
        let root = crate::tests::private_tempdir();
        let config = ServerConfig::loopback(root.path());
        let owner = match failure {
            "occupied" => {
                std::fs::write(&config.spool_dir, b"evidence").unwrap();
                None
            }
            "unsafe_inventory" => {
                config.prepare_spool().unwrap();
                std::fs::create_dir(config.spool_dir.join("unexpected")).unwrap();
                None
            }
            "locked" => config.open_http_spool(),
            _ => None,
        };
        let (state, startup) = HttpSpool::pending();
        assert_eq!(state.status(), "pending");
        if failure == "panic" {
            startup.run_with(|| panic!("injected startup panic")).await;
        } else {
            startup
                .run(config.clone().admit().unwrap(), Arc::new(Probe::default()))
                .await;
        }
        assert_eq!(state.status(), "unavailable", "{failure}");
        assert_eq!(
            state.budget().err().unwrap().code,
            storage_contract::ErrorCode::HttpUploadsUnavailable
        );
        if failure == "occupied" {
            assert_eq!(std::fs::read(&config.spool_dir).unwrap(), b"evidence");
        }
        drop(owner);
        // No automatic retry after the cause changes or another lock releases.
        tokio::task::yield_now().await;
        assert_eq!(state.status(), "unavailable");
    }
}

#[test]
fn dropped_publisher_never_becomes_ready_and_runtime_joins_inventory() {
    let root = crate::tests::private_tempdir();
    let config = ServerConfig::loopback(root.path());
    let (state, startup) = HttpSpool::pending();
    let observed = state.clone();
    let (release, blocked) = std::sync::mpsc::channel();
    let (cancelled, cancellation) = std::sync::mpsc::channel();
    let (finished, completion) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async move {
            let (started, waiting) = tokio::sync::oneshot::channel();
            let job = tokio::spawn(startup.run_with(move || {
                let budget = config.open_http_spool().unwrap();
                started.send(()).unwrap();
                blocked.recv_timeout(DEADLINE).unwrap();
                Some(budget)
            }));
            waiting.await.unwrap();
            job.abort();
            assert!(job.await.unwrap_err().is_cancelled());
            cancelled.send(()).unwrap();
        });
        drop(runtime); // Must join the already running blocking inventory.
        finished.send(()).unwrap();
    });
    cancellation.recv_timeout(DEADLINE).unwrap();
    assert!(matches!(
        completion.recv_timeout(Duration::from_millis(50)),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout)
    ));
    assert_eq!(observed.status(), "pending");
    assert_eq!(
        observed.budget().err().unwrap().code,
        storage_contract::ErrorCode::HttpUploadsPending
    );
    release.send(()).unwrap();
    completion.recv_timeout(DEADLINE).unwrap();
    worker.join().unwrap();
    assert_eq!(
        state.status(),
        "pending",
        "dropped publisher must never install a late budget"
    );
    let config = ServerConfig::loopback(root.path());
    assert!(
        config.open_http_spool().is_some(),
        "unpublished completed inventory must release its lock"
    );
}

#[tokio::test]
async fn dropping_unstarted_token_performs_no_inventory() {
    let root = crate::tests::private_tempdir();
    let config = ServerConfig::loopback(root.path());
    let (state, startup) = HttpSpool::pending();
    let future = startup.run(config.clone().admit().unwrap(), Arc::new(Probe::default()));
    drop(future);
    assert_eq!(state.status(), "pending");
    assert!(!config.spool_dir.exists());
}

#[test]
fn poisoned_spool_state_is_an_invariant_failure_not_unavailable() {
    let (spool, _startup) = HttpSpool::pending();
    let worker = spool.clone();
    assert!(
        std::thread::spawn(move || {
            let _state = worker.state.lock().unwrap();
            panic!("poison fixture");
        })
        .join()
        .is_err()
    );
    assert!(std::panic::catch_unwind(|| spool.status()).is_err());
    assert!(std::panic::catch_unwind(|| spool.budget()).is_err());
}

#[tokio::test]
async fn retirement_waits_for_activation_and_cancelled_owner_never_publishes() {
    enum Outcome {
        Activate,
        Cancel,
        Corrupt,
        Embedded,
        NoActivation,
        ClosedActivation,
    }
    for outcome in [
        Outcome::Activate,
        Outcome::Cancel,
        Outcome::Corrupt,
        Outcome::Embedded,
        Outcome::NoActivation,
        Outcome::ClosedActivation,
    ] {
        let root = crate::tests::private_tempdir();
        let config = ServerConfig::loopback(root.path());
        config.prepare_spool().unwrap();
        let hash = blake3::hash(b"abc").to_hex().to_string();
        let record = crate::uploads::UploadRecord {
            workspace_id: "ws".into(),
            item_id: "item".into(),
            principal: "admin".into(),
            path: "file".into(),
            generation: 1,
            logical_size: 3,
            content_hash: Some(hash.parse().unwrap()),
            persisted_bytes: 3,
            persisted_hash: hash,
            state: crate::uploads::UploadState::Complete,
            data_file: "copy.data".into(),
        };
        {
            use platform_io::fs_safety::{EntryName, PayloadOpenMode, PrivateDirectory};
            use std::io::Write;
            let directory = PrivateDirectory::open_trusted(&config.spool_dir).unwrap();
            let mut payload = directory
                .open_payload(
                    &EntryName::new(record.data_file.clone()).unwrap(),
                    PayloadOpenMode::CreateNew,
                )
                .unwrap();
            payload.write_all(b"abc").unwrap();
            payload.sync_all().unwrap();
        }
        crate::uploads::persist(&config.spool_dir, &record).unwrap();
        let probe = Arc::new(Probe(
            AtomicUsize::new(0),
            Some(Activation {
                notifications: match outcome {
                    Outcome::NoActivation => Notifications::Absent,
                    Outcome::ClosedActivation => Notifications::Closed,
                    Outcome::Activate | Outcome::Cancel | Outcome::Corrupt | Outcome::Embedded => {
                        Notifications::Live
                    }
                },
                corrupt: std::sync::atomic::AtomicBool::new(matches!(outcome, Outcome::Corrupt)),
                ready: std::sync::atomic::AtomicBool::new(false),
                changed: tokio::sync::watch::channel(()).0,
                attempted: tokio::sync::Semaphore::new(0),
            }),
        ));
        if matches!(outcome, Outcome::Embedded) {
            let spool = HttpSpool::open_blocking(&config, probe.as_ref());
            assert_eq!(spool.status(), "ready");
            assert!(spool.retirement_budget().is_some());
            assert_eq!(
                std::fs::read(config.spool_dir.join("copy.data")).unwrap(),
                b"abc"
            );
            assert!(
                crate::uploads::load(&config.spool_dir, "ws", "item")
                    .unwrap()
                    .is_some()
            );
            continue;
        }
        let (spool, startup) = HttpSpool::pending();
        let job = tokio::spawn(startup.run(config.clone().admit().unwrap(), probe.clone()));
        let activation = probe.1.as_ref().unwrap();
        tokio::time::timeout(DEADLINE, activation.attempted.acquire())
            .await
            .unwrap()
            .unwrap()
            .forget();
        if matches!(
            outcome,
            Outcome::Corrupt | Outcome::NoActivation | Outcome::ClosedActivation
        ) {
            tokio::time::timeout(DEADLINE, job).await.unwrap().unwrap();
            assert_eq!(spool.status(), "ready");
            assert!(spool.retirement_budget().is_some());
            if matches!(outcome, Outcome::Corrupt) {
                let budget = spool.budget().unwrap();
                assert_eq!(spool.maintenance_issues(), Some(1));
                assert!(budget.retire_startup(probe.as_ref()).unwrap());
                assert_eq!(spool.maintenance_issues(), Some(1));
                let auth = AuthStore::initialize(
                    &root.path().join("auth"),
                    "admin",
                    "a-long-test-password",
                )
                .unwrap();
                let runtime =
                    crate::ManagementRuntime::start(probe.clone(), auth, &config).unwrap();
                let app = crate::router_shared(
                    runtime.backend,
                    runtime.auth,
                    config.clone().admit().unwrap(),
                    runtime.telemetry,
                    spool.clone(),
                )
                .unwrap();
                let status =
                    json_body(response(&app, "GET", "/api/management/status", None).await).await;
                assert_eq!(status["auxiliary"]["http_uploads"], "ready");
                assert_eq!(
                    status["auxiliary"]["http_upload_maintenance"]["unresolved"],
                    1
                );
                assert!(!status.to_string().contains("copy.data"));
                activation.corrupt.store(false, Ordering::SeqCst);
                activation.ready.store(true, Ordering::SeqCst);
                assert!(budget.retire_startup(probe.as_ref()).unwrap());
                assert_eq!(spool.maintenance_issues(), Some(0));
                let status =
                    json_body(response(&app, "GET", "/api/management/status", None).await).await;
                assert_eq!(
                    status["auxiliary"]["http_upload_maintenance"]["unresolved"],
                    0
                );
            }
            assert_eq!(
                std::fs::read(config.spool_dir.join("copy.data")).unwrap(),
                b"abc"
            );
            assert!(
                crate::uploads::load(&config.spool_dir, "ws", "item")
                    .unwrap()
                    .is_some()
            );
            continue;
        }
        assert_eq!(spool.status(), "pending");
        assert!(!job.is_finished());
        if matches!(outcome, Outcome::Cancel) {
            job.abort();
            assert!(job.await.unwrap_err().is_cancelled());
            activation.ready.store(true, Ordering::SeqCst);
            activation.changed.send_replace(());
            assert_eq!(spool.status(), "pending");
        } else {
            activation.ready.store(true, Ordering::SeqCst);
            activation.changed.send_replace(());
            tokio::time::timeout(DEADLINE, job).await.unwrap().unwrap();
            assert_eq!(spool.status(), "ready");
            assert!(activation.attempted.try_acquire().is_ok());
        }
        assert!(
            config
                .spool_dir
                .join(format!("{}.json", crate::uploads::key("ws", "item")))
                .is_file()
        );
    }
}

#[test]
fn candidate_requires_exact_complete_committed_tuple_without_reading_full_body() {
    use storage_contract::{StagedFile, Workspace, WorkspacePurpose, WorkspaceState};
    let hash = blake3::hash(b"abc").to_hex().to_string();
    let candidate = crate::HttpSpoolCandidate {
        record: crate::uploads::UploadRecord {
            workspace_id: "ws".into(),
            item_id: "item".into(),
            principal: "admin".into(),
            path: "file".into(),
            generation: 1,
            logical_size: 3,
            content_hash: Some(hash.parse().unwrap()),
            persisted_bytes: 3,
            persisted_hash: hash,
            state: crate::uploads::UploadState::Complete,
            data_file: "copy.data".into(),
        },
    };
    let workspace = Workspace {
        workspace_id: "ws".into(),
        purpose: WorkspacePurpose::Normal,
        branch_id: "branch".into(),
        principal: "admin".into(),
        base_head: "head".into(),
        fencing_token: 1,
        files: std::collections::BTreeMap::from([(
            "item".into(),
            StagedFile {
                item_id: "item".into(),
                path: name_policy::PortablePath::parse("file").unwrap(),
                source_path: "/not-opened-by-tuple-qualification".into(),
                size: 3,
                content_hash: candidate.record.content_hash.unwrap(),
                dirty_generation: 1,
                base_version_id: None,
            },
        )]),
        selected_file_branches: Default::default(),
        selected_file_contexts: Default::default(),
        selected_directory_branches: Default::default(),
        directory_selection_generations: Default::default(),
        file_selection_generations: Default::default(),
        namespace_operations: Vec::new(),
        created_at: 1,
        state: WorkspaceState::Committed,
    };
    assert_eq!(candidate.workspace_id(), "ws");
    assert!(candidate.matches(&workspace));
    enum Mismatch {
        Open,
        Aborted,
        Hash,
        Generation,
        ZeroGeneration,
        Item,
        Principal,
        Incomplete,
    }
    for mismatch in [
        Mismatch::Open,
        Mismatch::Aborted,
        Mismatch::Hash,
        Mismatch::Generation,
        Mismatch::ZeroGeneration,
        Mismatch::Item,
        Mismatch::Principal,
        Mismatch::Incomplete,
    ] {
        let mut workspace = workspace.clone();
        let mut candidate = crate::HttpSpoolCandidate {
            record: candidate.record.clone(),
        };
        match mismatch {
            Mismatch::Open => workspace.state = WorkspaceState::Open,
            Mismatch::Aborted => workspace.state = WorkspaceState::Aborted,
            Mismatch::Hash => {
                workspace.files.get_mut("item").unwrap().content_hash =
                    "0".repeat(64).parse().unwrap()
            }
            Mismatch::Generation => workspace.files.get_mut("item").unwrap().dirty_generation = 2,
            Mismatch::ZeroGeneration => {
                candidate.record.generation = 0;
                workspace.files.get_mut("item").unwrap().dirty_generation = 0;
            }
            Mismatch::Item => workspace.files.get_mut("item").unwrap().item_id = "other".into(),
            Mismatch::Principal => workspace.principal = "other".into(),
            Mismatch::Incomplete => {
                candidate.record.state = crate::uploads::UploadState::Incomplete
            }
        }
        assert!(!candidate.matches(&workspace));
    }
}

#[tokio::test]
async fn management_status_reports_uncertain_inventory_as_recovery_required() {
    let root = crate::tests::private_tempdir();
    let mut config = ServerConfig::loopback(root.path());
    config.http_spool_limits.recovery_reserve_bytes = 1;
    let backend = Arc::new(Probe::default());
    let spool = HttpSpool::open_blocking(&config, backend.as_ref());
    let budget = spool.budget().unwrap();
    let reservation = budget.reserve("admin", "retained.data", 3).unwrap();
    reservation.persist_owner().unwrap();
    reservation.block_admission();
    drop(reservation);
    let auth =
        AuthStore::initialize(&root.path().join("auth"), "admin", "a-long-test-password").unwrap();
    let runtime = crate::ManagementRuntime::start(backend, auth, &config).unwrap();
    let app = crate::router_shared(
        runtime.backend,
        runtime.auth,
        config.admit().unwrap(),
        runtime.telemetry,
        spool.clone(),
    )
    .unwrap();
    assert_eq!(spool.status(), "recovery_required");
    assert_eq!(
        json_body(response(&app, "GET", "/api/management/status", None).await).await["auxiliary"]["http_uploads"],
        "recovery_required"
    );
    assert!(budget.reserve("admin", "another.data", 0).is_err());
}
