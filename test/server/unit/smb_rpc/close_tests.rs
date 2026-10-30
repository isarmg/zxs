//! VFS close policy exercised against the actual socket bridge and core backend.
use super::*;
use smb_protocol::client::{Client, RemoteError, close_descriptor};
use std::{
    os::{
        fd::{AsRawFd, OwnedFd},
        unix::net::UnixListener,
    },
    thread::JoinHandle,
};

#[derive(Clone, Copy)]
enum Failure {
    Upload,
    PendingOperation,
    UnknownOperation,
    MissingHistory,
    Abort,
}

struct CloseFailure {
    inner: Arc<dyn Backend>,
    failure: Failure,
    enabled: AtomicBool,
    uploads: AtomicUsize,
    commits: AtomicUsize,
    aborts: AtomicUsize,
}

impl Backend for CloseFailure {
    fn operation(
        &self,
        _identity: &Identity,
        _operation_id: storage_contract::OperationId,
    ) -> Result<crate::OperationObservation, ApiError> {
        Err(ApiError::unsupported())
    }
    fn operation_outcome(
        &self,
        identity: &Identity,
        operation_id: storage_contract::OperationId,
    ) -> Result<Option<storage_contract::OperationOutcome>, ApiError> {
        if self.enabled.load(Ordering::SeqCst) {
            if matches!(self.failure, Failure::MissingHistory) {
                return Err(ApiError::new(
                    ::http::StatusCode::NOT_FOUND,
                    storage_contract::ErrorCode::NotFound,
                    "history not found",
                ));
            }
            if matches!(self.failure, Failure::PendingOperation) {
                return Ok(Some(storage_contract::OperationOutcome {
                    operation_id,
                    kind: "workspace.commit".into(),
                    commit_id: None,
                    history_tree_id: None,
                    branch_id: None,
                    commit_state: storage_contract::CommitState::Prepared,
                    protection_at_publish: storage_contract::ProtectionAtPublish::NotApplicable,
                    request_digest: "0".repeat(64),
                    sequence: 0,
                    result: json!({}),
                }));
            }
            if matches!(self.failure, Failure::UnknownOperation) {
                return Err(ApiError::new(
                    ::http::StatusCode::SERVICE_UNAVAILABLE,
                    storage_contract::ErrorCode::RecoveryRequired,
                    "original operation outcome is unavailable",
                ));
            }
        }
        self.inner.operation_outcome(identity, operation_id)
    }
    fn require_native_workspace_commit_completion(
        &self,
        identity: &Identity,
        command: crate::CommitWorkspace,
    ) -> Result<(), ApiError> {
        self.inner
            .require_native_workspace_commit_completion(identity, command)
    }
    fn commit_workspace(
        &self,
        identity: &Identity,
        command: crate::CommitWorkspace,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        self.commits.fetch_add(1, Ordering::SeqCst);
        self.inner.commit_workspace(identity, command)
    }

    fn snapshot_at(
        &self,
        identity: &crate::auth::Identity,
        branch: &str,
        commit: &str,
    ) -> Result<std::sync::Arc<storage_contract::Commit>, crate::ApiError> {
        self.inner.snapshot_at(identity, branch, commit)
    }

    fn snapshot_entry_by_identity(
        &self,
        identity: &crate::auth::Identity,
        branch: &str,
        commit: &str,
        key: &str,
        entry_id: &str,
    ) -> Result<Option<storage_contract::Entry>, crate::ApiError> {
        self.inner
            .snapshot_entry_by_identity(identity, branch, commit, key, entry_id)
    }

    fn snapshot_entry(
        &self,
        identity: &crate::auth::Identity,
        branch: &str,
        commit: &str,
        key: &str,
    ) -> Result<Option<storage_contract::Entry>, crate::ApiError> {
        self.inner.snapshot_entry(identity, branch, commit, key)
    }

    fn snapshot_directory_has_children(
        &self,
        identity: &crate::auth::Identity,
        branch: &str,
        commit: &str,
        key: &str,
    ) -> Result<bool, crate::ApiError> {
        self.inner
            .snapshot_directory_has_children(identity, branch, commit, key)
    }

    fn authorize_current(
        &self,
        identity: &crate::auth::Identity,
        branch: &str,
    ) -> Result<(), crate::ApiError> {
        self.inner.authorize_current(identity, branch)
    }

    fn read_directory(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadDirectory<'_>,
    ) -> Result<crate::DirectorySnapshot, crate::ApiError> {
        self.inner.read_directory(identity, request)
    }

    fn authorize_workspace_selection(
        &self,
        identity: &crate::auth::Identity,
        workspace: &str,
        path: &name_policy::PortablePath,
    ) -> Result<(), crate::ApiError> {
        self.inner
            .authorize_workspace_selection(identity, workspace, path)
    }

    fn create_workspace(
        &self,
        identity: &crate::auth::Identity,
        command: crate::CreateWorkspace,
    ) -> Result<storage_contract::Workspace, crate::ApiError> {
        self.inner.create_workspace(identity, command)
    }
    fn abort_workspace(
        &self,
        identity: &crate::auth::Identity,
        workspace: &str,
    ) -> Result<(), crate::ApiError> {
        self.aborts.fetch_add(1, Ordering::SeqCst);
        if self.enabled.load(Ordering::SeqCst) && matches!(self.failure, Failure::Abort) {
            return Err(ApiError::new(
                ::http::StatusCode::SERVICE_UNAVAILABLE,
                storage_contract::ErrorCode::WorkspaceBusy,
                "injected abort failure",
            ));
        }
        self.inner.abort_workspace(identity, workspace)
    }

    fn workspace(
        &self,
        identity: &crate::auth::Identity,
        workspace: &str,
    ) -> Result<storage_contract::Workspace, crate::ApiError> {
        self.inner.workspace(identity, workspace)
    }
    fn published_upload_status(
        &self,
        identity: &crate::auth::Identity,
        workspace: &str,
        item: &str,
    ) -> Result<Option<crate::PublishedUploadStatus>, crate::ApiError> {
        self.inner
            .published_upload_status(identity, workspace, item)
    }
    fn seal_workspace(
        &self,
        identity: &crate::auth::Identity,
        workspace: &str,
    ) -> Result<storage_contract::SealedWorkspace, crate::ApiError> {
        self.inner.seal_workspace(identity, workspace)
    }
    fn commit_workspace_public(
        &self,
        identity: &crate::auth::Identity,
        command: storage_contract::PublicCommitEnvelope,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.inner.commit_workspace_public(identity, command)
    }
    fn version_snapshot(
        &self,
        identity: &crate::auth::Identity,
        version: &str,
    ) -> Result<crate::VersionSnapshot, crate::ApiError> {
        self.inner.version_snapshot(identity, version)
    }
    fn authorize_version(
        &self,
        identity: &crate::auth::Identity,
        version: &str,
    ) -> Result<(), crate::ApiError> {
        self.inner.authorize_version(identity, version)
    }
    fn repository_snapshot(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<storage_contract::Repository, crate::ApiError> {
        self.inner.repository_snapshot(identity, id)
    }
    fn authorize_repository(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<(), crate::ApiError> {
        self.inner.authorize_repository(identity, id)
    }
    fn capabilities(&self) -> Result<crate::Capabilities, crate::ApiError> {
        self.inner.capabilities()
    }
    fn ready(&self) -> Result<crate::Readiness, crate::ApiError> {
        self.inner.ready()
    }
    fn workspace_capacity(
        &self,
        identity: &crate::auth::Identity,
    ) -> Result<storage_contract::WorkspaceCapacity, crate::ApiError> {
        self.inner.workspace_capacity(identity)
    }
    fn smb_binding_status(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<crate::SmbBindingStatus, crate::ApiError> {
        self.inner.smb_binding_status(identity, id)
    }
    fn update_smb_binding(
        &self,
        identity: &crate::auth::Identity,
        request: crate::UpdateSmbBinding<'_>,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.inner.update_smb_binding(identity, request)
    }
    fn protection_policy(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<storage_contract::ProtectionPolicy, crate::ApiError> {
        self.inner.protection_policy(identity, id)
    }
    fn placement_rule(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<storage_contract::PlacementRule, crate::ApiError> {
        self.inner.placement_rule(identity, id)
    }
    fn update_protection_policy(
        &self,
        identity: &crate::auth::Identity,
        request: crate::UpdateProtectionPolicy,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.inner.update_protection_policy(identity, request)
    }
    fn update_placement_rule(
        &self,
        identity: &crate::auth::Identity,
        request: crate::UpdatePlacementRule,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.inner.update_placement_rule(identity, request)
    }
    fn compare_commits(
        &self,
        identity: &crate::auth::Identity,
        request: crate::CompareCommits<'_>,
    ) -> Result<storage_contract::CommitComparison, crate::ApiError> {
        self.inner.compare_commits(identity, request)
    }
    fn disk_status(
        &self,
        identity: &crate::auth::Identity,
    ) -> Result<Vec<storage_contract::DiskStatus>, crate::ApiError> {
        self.inner.disk_status(identity)
    }
    fn integrity_check(
        &self,
        identity: &crate::auth::Identity,
    ) -> Result<storage_contract::CheckReport, crate::ApiError> {
        self.inner.integrity_check(identity)
    }
    fn create_merge_link(
        &self,
        identity: &crate::auth::Identity,
        request: crate::CreateMergeLink,
    ) -> Result<storage_contract::MergeLink, crate::ApiError> {
        self.inner.create_merge_link(identity, request)
    }
    fn create_repository(
        &self,
        identity: &crate::auth::Identity,
        request: crate::CreateRepository,
    ) -> Result<storage_contract::Repository, crate::ApiError> {
        self.inner.create_repository(identity, request)
    }
    fn create_branch(
        &self,
        identity: &crate::auth::Identity,
        request: crate::CreateBranch,
    ) -> Result<crate::BranchSnapshot, crate::ApiError> {
        self.inner.create_branch(identity, request)
    }
    fn create_file_branch(
        &self,
        identity: &crate::auth::Identity,
        request: crate::CreateFileBranch,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.inner.create_file_branch(identity, request)
    }
    fn update_note(
        &self,
        identity: &crate::auth::Identity,
        request: crate::UpdateNote,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.inner.update_note(identity, request)
    }
    fn merge_file_branches(
        &self,
        identity: &crate::auth::Identity,
        request: crate::MergeFileBranches,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.inner.merge_file_branches(identity, request)
    }
    fn update_file_node_tags(
        &self,
        identity: &crate::auth::Identity,
        request: crate::UpdateFileNodeTags,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.inner.update_file_node_tags(identity, request)
    }
    fn create_disk_directory(
        &self,
        identity: &crate::auth::Identity,
        request: crate::CreateDiskDirectory,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.inner.create_disk_directory(identity, request)
    }
    fn create_disk_write_session(
        &self,
        identity: &crate::auth::Identity,
        request: crate::CreateDiskWriteSession,
    ) -> Result<crate::DiskWriteSession, crate::ApiError> {
        self.inner.create_disk_write_session(identity, request)
    }
    fn backup_configuration(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadBackupConfiguration,
    ) -> Result<storage_contract::BackupConfigurationSnapshot, crate::ApiError> {
        self.inner.backup_configuration(identity, request)
    }
    fn configure_backups(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ConfigureBackups,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.inner.configure_backups(identity, request)
    }
    fn plan(
        &self,
        identity: &crate::auth::Identity,
        plan: &str,
    ) -> Result<storage_contract::Plan, crate::ApiError> {
        self.inner.plan(identity, plan)
    }
    fn cancel_plan(
        &self,
        identity: &crate::auth::Identity,
        request: Result<crate::CancelPlan, crate::ApiError>,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.inner.cancel_plan(identity, request)
    }
    fn register_disk(
        &self,
        identity: &crate::auth::Identity,
    ) -> Result<std::convert::Infallible, crate::ApiError> {
        self.inner.register_disk(identity)
    }
    fn commit_plan(
        &self,
        identity: &crate::auth::Identity,
        request: Result<crate::CommitPlan, crate::ApiError>,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.inner.commit_plan(identity, request)
    }
    fn prepare_historical_save(
        &self,
        identity: &crate::auth::Identity,
        request: Result<crate::PrepareHistoricalSave, crate::ApiError>,
    ) -> Result<storage_contract::Plan, crate::ApiError> {
        self.inner.prepare_historical_save(identity, request)
    }
    fn prepare_purge(
        &self,
        identity: &crate::auth::Identity,
        request: Result<crate::PreparePurge, crate::ApiError>,
    ) -> Result<storage_contract::Plan, crate::ApiError> {
        self.inner.prepare_purge(identity, request)
    }
    fn prepare_split(
        &self,
        identity: &crate::auth::Identity,
        request: Result<crate::PrepareSplit, crate::ApiError>,
    ) -> Result<storage_contract::Plan, crate::ApiError> {
        self.inner.prepare_split(identity, request)
    }
    fn namespace_batch(
        &self,
        identity: &crate::auth::Identity,
        request: crate::NamespaceBatch,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.inner.namespace_batch(identity, request)
    }
    fn select_file_branch(
        &self,
        identity: &crate::auth::Identity,
        request: crate::SelectFileBranch,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.inner.select_file_branch(identity, request)
    }
    fn rename_file_branch(
        &self,
        identity: &crate::auth::Identity,
        request: crate::RenameFileBranch,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.inner.rename_file_branch(identity, request)
    }
    fn listing(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadListing,
        control: &crate::HistoryReadControl,
    ) -> Result<crate::ListingSnapshot, crate::ApiError> {
        self.inner.listing(identity, request, control)
    }
    fn mutate_disk_directory(
        &self,
        identity: &crate::auth::Identity,
        request: crate::MutateDiskDirectory,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.inner.mutate_disk_directory(identity, request)
    }
    fn initialize_directory_scope(
        &self,
        identity: &crate::auth::Identity,
        request: crate::InitializeDirectoryScope,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.inner.initialize_directory_scope(identity, request)
    }
    fn archive_branch(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ArchiveBranch,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.inner.archive_branch(identity, request)
    }
    fn rename_directory_branch(
        &self,
        identity: &crate::auth::Identity,
        request: crate::RenameDirectoryBranch,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.inner.rename_directory_branch(identity, request)
    }
    fn select_default_directory_branch(
        &self,
        identity: &crate::auth::Identity,
        request: crate::SelectDefaultDirectoryBranch,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.inner
            .select_default_directory_branch(identity, request)
    }
    fn directory_selection(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadDirectorySelection,
    ) -> Result<crate::DirectorySelectionSnapshot, crate::ApiError> {
        self.inner.directory_selection(identity, request)
    }
    fn directory_scopes(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadDirectoryScopes<'_>,
    ) -> Result<crate::DirectoryScopesSnapshot, crate::ApiError> {
        self.inner.directory_scopes(identity, request)
    }
    fn default_directory_branch(
        &self,
        identity: &crate::auth::Identity,
        repository_id: &str,
    ) -> Result<crate::DefaultDirectoryBranchSnapshot, crate::ApiError> {
        self.inner.default_directory_branch(identity, repository_id)
    }
    fn file_history_snapshot(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadFileHistory,
    ) -> Result<crate::FileHistoryPage, crate::ApiError> {
        self.inner.file_history_snapshot(identity, request)
    }
    fn file_history_node_parents(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadNodeParents,
    ) -> Result<crate::NodeParentsPage, crate::ApiError> {
        self.inner.file_history_node_parents(identity, request)
    }
    fn file_history_branches(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadHistoryBranches,
    ) -> Result<crate::HistoryBranchesPage, crate::ApiError> {
        self.inner.file_history_branches(identity, request)
    }
    fn file_history_merges(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadHistoryMerges,
    ) -> Result<crate::HistoryMergesPage, crate::ApiError> {
        self.inner.file_history_merges(identity, request)
    }
    fn file_history_merge_inputs(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadMergeInputs,
    ) -> Result<crate::MergeInputsPage, crate::ApiError> {
        self.inner.file_history_merge_inputs(identity, request)
    }
    fn file_history_merge_parents(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadMergeParents,
    ) -> Result<crate::MergeParentsPage, crate::ApiError> {
        self.inner.file_history_merge_parents(identity, request)
    }
    fn file_history_node_field(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadNodeField,
    ) -> Result<crate::NodeFieldSlice, crate::ApiError> {
        self.inner.file_history_node_field(identity, request)
    }
    fn file_version_page(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadVersionPage,
    ) -> Result<crate::VersionPage, crate::ApiError> {
        self.inner.file_version_page(identity, request)
    }
    fn file_version_sources(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadVersionSources,
    ) -> Result<crate::VersionSourcesPage, crate::ApiError> {
        self.inner.file_version_sources(identity, request)
    }
    fn topology_snapshot(
        &self,
        identity: &crate::auth::Identity,
    ) -> Result<crate::TopologySnapshot, crate::ApiError> {
        self.inner.topology_snapshot(identity)
    }
    fn placement_snapshot(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<storage_contract::Placement, crate::ApiError> {
        self.inner.placement_snapshot(identity, id)
    }
    fn event_page(
        &self,
        identity: &crate::auth::Identity,
        after: u64,
    ) -> Result<crate::EventPage, crate::ApiError> {
        self.inner.event_page(identity, after)
    }
    fn activity_history(
        &self,
        identity: &crate::auth::Identity,
        control: &crate::HistoryReadControl,
    ) -> Result<crate::ActivityHistorySnapshot, crate::ApiError> {
        self.inner.activity_history(identity, control)
    }
    fn history_graph(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadHistoryGraph,
        control: &crate::HistoryReadControl,
    ) -> Result<crate::HistoryGraphResponse, crate::ApiError> {
        self.inner.history_graph(identity, request, control)
    }
    fn commit_snapshot(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<std::sync::Arc<storage_contract::Commit>, crate::ApiError> {
        self.inner.commit_snapshot(identity, id)
    }
    fn resolve_entry(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ResolveEntry<'_>,
    ) -> Result<crate::ResolvedEntry, crate::ApiError> {
        self.inner.resolve_entry(identity, request)
    }
    fn branch_snapshot(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<crate::BranchSnapshot, crate::ApiError> {
        self.inner.branch_snapshot(identity, id)
    }
    fn branches_snapshot(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<Vec<crate::BranchSnapshot>, crate::ApiError> {
        self.inner.branches_snapshot(identity, id)
    }
    fn repositories_snapshot(
        &self,
        identity: &crate::auth::Identity,
    ) -> Result<Vec<storage_contract::Repository>, crate::ApiError> {
        self.inner.repositories_snapshot(identity)
    }
    fn tree_snapshot(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadTree<'_>,
        control: &crate::HistoryReadControl,
    ) -> Result<crate::TreeSnapshot, crate::ApiError> {
        self.inner.tree_snapshot(identity, request, control)
    }
    fn create_write_session(
        &self,
        identity: &crate::auth::Identity,
        branch: &str,
    ) -> Result<crate::WorkspaceWriteSession, crate::ApiError> {
        self.inner.create_write_session(identity, branch)
    }
    fn stage_namespace(
        &self,
        identity: &crate::auth::Identity,
        command: crate::StageNamespace,
    ) -> Result<storage_contract::Workspace, crate::ApiError> {
        self.inner.stage_namespace(identity, command)
    }
    fn authorize_upload(
        &self,
        identity: &crate::auth::Identity,
        workspace: &str,
        logical_size: Option<u64>,
    ) -> Result<(), crate::ApiError> {
        self.inner
            .authorize_upload(identity, workspace, logical_size)
    }

    fn lookup_current(
        &self,
        identity: &crate::auth::Identity,
        branch: &str,
        path: &name_policy::LookupPath,
    ) -> Result<Option<storage_contract::Entry>, crate::ApiError> {
        self.inner.lookup_current(identity, branch, path)
    }

    fn lookup_at(
        &self,
        identity: &crate::auth::Identity,
        branch: &str,
        commit: &str,
        path: &name_policy::LookupPath,
    ) -> Result<Option<storage_contract::Entry>, crate::ApiError> {
        self.inner.lookup_at(identity, branch, commit, path)
    }

    fn upload(
        &self,
        identity: &Identity,
        workspace: &str,
        item: &str,
        path: &name_policy::PortablePath,
        source: &crate::UploadSource,
    ) -> Result<storage_contract::StagedFile, ApiError> {
        self.uploads.fetch_add(1, Ordering::SeqCst);
        if self.enabled.load(Ordering::SeqCst) && matches!(self.failure, Failure::Upload) {
            return Err(ApiError::forbidden());
        }
        self.inner.upload(identity, workspace, item, path, source)
    }
    fn upload_generation(
        &self,
        identity: &crate::auth::Identity,
        workspace: &str,
        item: &str,
        path: &name_policy::PortablePath,
        source: &crate::UploadSource,
        generation: u64,
    ) -> Result<storage_contract::StagedFile, crate::ApiError> {
        self.inner
            .upload_generation(identity, workspace, item, path, source, generation)
    }

    fn range_content(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadContent<'_>,
    ) -> Result<crate::Content, crate::ApiError> {
        self.inner.range_content(identity, request)
    }
    fn content(&self, identity: &Identity, version: &str) -> Result<Content, ApiError> {
        self.inner.content(identity, version)
    }
}

fn connect(shared: Arc<Shared>) -> (Client, JoinHandle<()>) {
    let repository = shared
        .backend
        .repositories_snapshot(&identity(&shared))
        .unwrap()[0]
        .repository_id
        .clone();
    let socket = shared.spool.join(format!("{}.sock", new_id("rpc")));
    let listener = UnixListener::bind(&socket).unwrap();
    let worker = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        run_connection(FrameStream::new(stream).unwrap(), shared).unwrap();
    });
    let client = Client::connect(&socket, "alice", &repository).unwrap();
    // The connected sockets stay live; the listener is used only once.
    fs::remove_file(socket).unwrap();
    (client, worker)
}

fn open(client: &Client, path: &str, create: bool) -> (OpenResult, OwnedFd) {
    client
        .open(Request::Open {
            path: path.into(),
            writable: true,
            create,
            truncate: false,
            exclusive: false,
            write_through: false,
        })
        .unwrap()
}

fn source(shared: &Shared, opened: &OpenResult) -> PathBuf {
    fs::read_dir(&shared.spool)
        .unwrap()
        .filter_map(|entry| {
            let path = entry.unwrap().path();
            if !path.to_string_lossy().ends_with(".work.json") {
                return None;
            }
            let record: Value = json_codec::from_slice(&fs::read(path).unwrap()).unwrap();
            (record["workspace_id"].as_str() == opened.workspace_id.as_deref())
                .then(|| PathBuf::from(record["source_path"].as_str().unwrap()))
        })
        .next()
        .expect("writer recovery record missing")
}

fn record(source: &Path) -> Value {
    json_codec::from_slice(&fs::read(source.with_extension("work.json")).unwrap()).unwrap()
}

fn remote(error: &io::Error) -> &RemoteError {
    error
        .get_ref()
        .unwrap()
        .downcast_ref::<RemoteError>()
        .unwrap()
}

fn close_vfs(client: &Client, opened: &OpenResult, fd: OwnedFd) -> io::Result<()> {
    // This is the same descriptor/lock/request policy called by close_opened in
    // the Samba VFS module, without replacing the RPC or core backend with mocks.
    close_descriptor(opened.handle, fd, &Mutex::new(()), |request| {
        client.call(request).map(|_| ())
    })
}

#[test]
fn namespace_history_lookup_failure_does_not_start_staging() {
    let (_temp, mut shared, backend, branch) = fixture();
    let injected = Arc::new(CloseFailure {
        inner: shared.backend.clone(),
        failure: Failure::MissingHistory,
        enabled: AtomicBool::new(true),
        uploads: AtomicUsize::new(0),
        commits: AtomicUsize::new(0),
        aborts: AtomicUsize::new(0),
    });
    Arc::get_mut(&mut shared).unwrap().backend = injected.clone();
    let mut session = session(shared.clone(), &branch);
    let operation = storage_contract::OperationId::generate();
    let error = session
        .namespace(
            smb_protocol::protocol::NamespaceOperation::Mkdir,
            "not-created".into(),
            None,
            false,
            operation,
        )
        .unwrap_err();
    assert_eq!(error.code, storage_contract::ErrorCode::NotFound);
    assert_eq!(error.message, "history not found");
    assert_eq!(injected.uploads.load(Ordering::SeqCst), 0);
    assert_eq!(injected.commits.load(Ordering::SeqCst), 0);
    assert_eq!(injected.aborts.load(Ordering::SeqCst), 0);
    assert!(
        !shared
            .spool
            .join(format!("{operation}.namespace.json"))
            .exists()
    );
    assert!(
        backend
            .operation_outcome(&session.identity, operation)
            .unwrap()
            .is_none()
    );
    assert!(session.writer_locks.is_empty());
    assert!(shared.namespace_operations.lock().unwrap().is_empty());
}

#[test]
fn failed_vfs_close_detaches_only_its_writer_and_keeps_both_sessions_usable() {
    for failure in [
        Failure::Upload,
        Failure::PendingOperation,
        Failure::UnknownOperation,
        Failure::MissingHistory,
        Failure::Abort,
    ] {
        let (_temp, mut shared, _backend, _branch) = fixture();
        let injected = Arc::new(CloseFailure {
            inner: shared.backend.clone(),
            failure,
            enabled: AtomicBool::new(false),
            uploads: AtomicUsize::new(0),
            commits: AtomicUsize::new(0),
            aborts: AtomicUsize::new(0),
        });
        Arc::get_mut(&mut shared).unwrap().backend = injected.clone();
        let (client, worker) = connect(shared.clone());
        let (other, other_worker) = connect(shared.clone());
        let (opened, fd) = open(&client, "report.txt", false);
        let (unrelated, unrelated_fd) = open(&client, "other.txt", true);
        let retained = source(&shared, &opened);
        let operation = record(&retained)["operation_id"]
            .as_str()
            .unwrap()
            .to_owned();
        if !matches!(failure, Failure::Abort) {
            File::from(fd.try_clone().unwrap())
                .write_all_at(b"pending", 0)
                .unwrap();
            client
                .call(Request::Dirty {
                    handle: opened.handle,
                })
                .unwrap();
        }
        injected.enabled.store(true, Ordering::SeqCst);
        let before = (
            injected.uploads.load(Ordering::SeqCst),
            injected.commits.load(Ordering::SeqCst),
            injected.aborts.load(Ordering::SeqCst),
        );
        let raw_fd = fd.as_raw_fd();
        let error = close_vfs(&client, &opened, fd).unwrap_err();
        let expected_code = match failure {
            Failure::Upload => "FORBIDDEN",
            Failure::PendingOperation | Failure::UnknownOperation => "RECOVERY_REQUIRED",
            Failure::MissingHistory => "NOT_FOUND",
            Failure::Abort => "WORKSPACE_BUSY",
        };
        assert_eq!(remote(&error).code.to_string(), expected_code);
        assert_eq!(
            remote(&error).operation_id.as_deref(),
            Some(operation.as_str())
        );
        assert!(
            !fs::read_link(format!("/proc/self/fd/{raw_fd}"))
                .is_ok_and(|target| target == retained),
            "VFS descriptor survived its terminal close"
        );
        assert!(retained.with_extension("budget.json").is_file());
        let metadata = record(&retained);
        assert_eq!(metadata["operation_id"], operation);
        assert_eq!(metadata["handle_state"], "detached");
        assert_eq!(metadata["detach_error"]["code"], expected_code);
        assert_eq!(metadata["retained_bytes_synced"], true);
        assert_eq!(
            fs::read(&retained).unwrap(),
            if matches!(failure, Failure::Abort) {
                b"old".as_slice()
            } else {
                b"pending".as_slice()
            }
        );
        assert_eq!(injected.commits.load(Ordering::SeqCst), before.1);
        assert_eq!(
            injected.aborts.load(Ordering::SeqCst),
            before.2 + usize::from(matches!(failure, Failure::Abort)),
            "a failed save must not be replaced with an abort"
        );
        if matches!(
            failure,
            Failure::PendingOperation | Failure::UnknownOperation | Failure::MissingHistory
        ) {
            assert_eq!(injected.uploads.load(Ordering::SeqCst), before.0);
        }
        let repeated = client
            .call(Request::Close {
                handle: opened.handle,
            })
            .unwrap_err();
        assert_eq!(
            remote(&repeated).code,
            smb_protocol::client::RemoteErrorCode::Server(storage_contract::ErrorCode::NotFound)
        );
        assert_eq!(injected.commits.load(Ordering::SeqCst), before.1);
        injected.enabled.store(false, Ordering::SeqCst);
        assert!(
            shared
                .file_writers
                .lock()
                .unwrap()
                .iter()
                .all(|(_, path)| path != "report.txt")
        );
        assert!(
            shared
                .file_writers
                .lock()
                .unwrap()
                .iter()
                .any(|(_, path)| path == "other.txt")
        );

        // Traffic keeps the original connection active. Reopen succeeds now,
        // rather than waiting for the 120-second idle timeout to release locks.
        client
            .call(Request::Stat {
                path: "report.txt".into(),
            })
            .unwrap();
        let (reopened, reopened_fd) = open(&client, "report.txt", false);
        close_vfs(&client, &reopened, reopened_fd).unwrap();
        let (reopened, reopened_fd) = open(&other, "report.txt", false);
        close_vfs(&other, &reopened, reopened_fd).unwrap();
        File::from(unrelated_fd.try_clone().unwrap())
            .write_all_at(b"healthy", 0)
            .unwrap();
        client
            .call(Request::Dirty {
                handle: unrelated.handle,
            })
            .unwrap();
        close_vfs(&client, &unrelated, unrelated_fd).unwrap();
        assert!(retained.is_file());
        assert_eq!(record(&retained)["operation_id"], operation);
        drop(client);
        drop(other);
        worker.join().unwrap();
        other_worker.join().unwrap();
    }
}

#[test]
fn lost_close_reply_retains_original_published_operation_without_resubmitting() {
    let (_temp, shared, backend, _branch) = fixture();
    let (client, worker) = connect(shared.clone());
    let (opened, fd) = open(&client, "report.txt", false);
    File::from(fd.try_clone().unwrap())
        .write_all_at(b"new", 0)
        .unwrap();
    client
        .call(Request::Dirty {
            handle: opened.handle,
        })
        .unwrap();
    let retained = source(&shared, &opened);
    let operation = record(&retained)["operation_id"]
        .as_str()
        .unwrap()
        .to_owned();
    backend.fail.store(true, Ordering::SeqCst);
    let error = close_vfs(&client, &opened, fd).unwrap_err();
    assert_eq!(
        remote(&error).code,
        smb_protocol::client::RemoteErrorCode::Server(storage_contract::ErrorCode::InjectedFailure)
    );
    assert_eq!(
        remote(&error).operation_id.as_deref(),
        Some(operation.as_str())
    );
    let uploads = backend.uploads.load(Ordering::SeqCst);
    let result = backend
        .operation_outcome(&identity(&shared), operation.parse().unwrap())
        .and_then(|outcome| crate::operation_wire::operation_wire(outcome.unwrap()))
        .unwrap();
    assert_eq!(result["commit_state"], "PUBLISHED");
    assert_eq!(fs::read(&retained).unwrap(), b"new");
    assert_eq!(record(&retained)["handle_state"], "detached");
    let (next, next_fd) = open(&client, "report.txt", false);
    close_vfs(&client, &next, next_fd).unwrap();
    assert_eq!(backend.uploads.load(Ordering::SeqCst), uploads);
    assert!(retained.is_file());
    drop(client);
    worker.join().unwrap();
}

#[test]
fn poisoned_vfs_lock_detaches_without_publishing_or_discarding_unannounced_bytes() {
    let (_temp, shared, backend, _branch) = fixture();
    let (client, worker) = connect(shared.clone());
    let (opened, fd) = open(&client, "report.txt", false);
    let retained = source(&shared, &opened);
    File::from(fd.try_clone().unwrap())
        .write_all_at(b"unsent", 0)
        .unwrap();
    let lock = Mutex::new(());
    let _ = std::panic::catch_unwind(|| {
        let _guard = lock.lock().unwrap();
        panic!("injected VFS worker failure");
    });
    let uploads = backend.uploads.load(Ordering::SeqCst);
    assert!(
        close_descriptor(opened.handle, fd, &lock, |request| {
            assert!(matches!(request, Request::Detach { .. }));
            client.call(request).map(|_| ())
        })
        .is_err()
    );
    assert_eq!(backend.uploads.load(Ordering::SeqCst), uploads);
    assert_eq!(fs::read(&retained).unwrap(), b"unsent");
    let metadata = record(&retained);
    assert_eq!(metadata["handle_state"], "detached");
    assert_eq!(metadata["dirty"], false);
    assert_eq!(metadata["descriptor_changes_unconfirmed"], true);
    let (next, next_fd) = open(&client, "report.txt", false);
    close_vfs(&client, &next, next_fd).unwrap();
    assert!(retained.is_file());
    drop(client);
    worker.join().unwrap();
}
