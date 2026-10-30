//! Deterministic account-publication races at the auth -> engine boundary.
use super::*;
use crate::{AuthorizedBackend, auth::AuthStore};
use std::{
    path::PathBuf,
    sync::{Arc, Barrier},
};
use storage_engine::{CommitRequest, Config, Workspace, new_id};

/// Pause only after AuthorizedBackend has released its account-store lock.
/// Publishing a revocation here must finish before EngineBackend captures a
/// read plan or admits a mutation; neither side may reverse the lock order.
struct PausedBackend {
    inner: Arc<EngineBackend>,
    captured: Arc<Barrier>,
    released: Arc<Barrier>,
}
impl PausedBackend {
    fn enter(&self) {
        self.captured.wait();
        self.released.wait();
    }
}
impl Backend for PausedBackend {
    fn operation_outcome(
        &self,
        identity: &Identity,
        operation_id: storage_contract::OperationId,
    ) -> Result<Option<storage_contract::OperationOutcome>, ApiError> {
        self.enter();
        self.inner.operation_outcome(identity, operation_id)
    }
    fn operation(
        &self,
        identity: &Identity,
        operation_id: storage_contract::OperationId,
    ) -> Result<crate::OperationObservation, ApiError> {
        self.enter();
        self.inner.operation(identity, operation_id)
    }
    fn commit_workspace(
        &self,
        identity: &Identity,
        command: crate::CommitWorkspace,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        self.enter();
        self.inner.commit_workspace(identity, command)
    }

    fn snapshot_at(
        &self,
        identity: &crate::auth::Identity,
        branch: &str,
        commit: &str,
    ) -> Result<std::sync::Arc<storage_contract::Commit>, crate::ApiError> {
        self.enter();
        self.inner.snapshot_at(identity, branch, commit)
    }

    fn authorize_current(
        &self,
        identity: &crate::auth::Identity,
        branch: &str,
    ) -> Result<(), crate::ApiError> {
        self.enter();
        self.inner.authorize_current(identity, branch)
    }

    fn read_directory(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadDirectory<'_>,
    ) -> Result<crate::DirectorySnapshot, crate::ApiError> {
        self.enter();
        self.inner.read_directory(identity, request)
    }

    fn authorize_workspace_selection(
        &self,
        identity: &crate::auth::Identity,
        workspace: &str,
        path: &name_policy::PortablePath,
    ) -> Result<(), crate::ApiError> {
        self.enter();
        self.inner
            .authorize_workspace_selection(identity, workspace, path)
    }

    fn create_workspace(
        &self,
        identity: &crate::auth::Identity,
        command: crate::CreateWorkspace,
    ) -> Result<storage_contract::Workspace, crate::ApiError> {
        self.enter();
        self.inner.create_workspace(identity, command)
    }
    fn abort_workspace(
        &self,
        identity: &crate::auth::Identity,
        workspace: &str,
    ) -> Result<(), crate::ApiError> {
        self.enter();
        self.inner.abort_workspace(identity, workspace)
    }

    fn workspace(
        &self,
        identity: &crate::auth::Identity,
        workspace: &str,
    ) -> Result<storage_contract::Workspace, crate::ApiError> {
        self.enter();
        self.inner.workspace(identity, workspace)
    }
    fn published_upload_status(
        &self,
        identity: &crate::auth::Identity,
        workspace: &str,
        item: &str,
    ) -> Result<Option<crate::PublishedUploadStatus>, crate::ApiError> {
        self.enter();
        self.inner
            .published_upload_status(identity, workspace, item)
    }
    fn seal_workspace(
        &self,
        identity: &crate::auth::Identity,
        workspace: &str,
    ) -> Result<storage_contract::SealedWorkspace, crate::ApiError> {
        self.enter();
        self.inner.seal_workspace(identity, workspace)
    }
    fn commit_workspace_public(
        &self,
        identity: &crate::auth::Identity,
        command: storage_contract::PublicCommitEnvelope,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.enter();
        self.inner.commit_workspace_public(identity, command)
    }
    fn version_snapshot(
        &self,
        identity: &crate::auth::Identity,
        version: &str,
    ) -> Result<crate::VersionSnapshot, crate::ApiError> {
        self.enter();
        self.inner.version_snapshot(identity, version)
    }
    fn authorize_version(
        &self,
        identity: &crate::auth::Identity,
        version: &str,
    ) -> Result<(), crate::ApiError> {
        self.enter();
        self.inner.authorize_version(identity, version)
    }
    fn repository_snapshot(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<storage_contract::Repository, crate::ApiError> {
        self.enter();
        self.inner.repository_snapshot(identity, id)
    }
    fn authorize_repository(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<(), crate::ApiError> {
        self.enter();
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
        self.enter();
        self.inner.workspace_capacity(identity)
    }
    fn smb_binding_status(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<crate::SmbBindingStatus, crate::ApiError> {
        self.enter();
        self.inner.smb_binding_status(identity, id)
    }
    fn update_smb_binding(
        &self,
        identity: &crate::auth::Identity,
        request: crate::UpdateSmbBinding<'_>,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.enter();
        self.inner.update_smb_binding(identity, request)
    }
    fn protection_policy(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<storage_contract::ProtectionPolicy, crate::ApiError> {
        self.enter();
        self.inner.protection_policy(identity, id)
    }
    fn placement_rule(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<storage_contract::PlacementRule, crate::ApiError> {
        self.enter();
        self.inner.placement_rule(identity, id)
    }
    fn update_protection_policy(
        &self,
        identity: &crate::auth::Identity,
        request: crate::UpdateProtectionPolicy,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.enter();
        self.inner.update_protection_policy(identity, request)
    }
    fn update_placement_rule(
        &self,
        identity: &crate::auth::Identity,
        request: crate::UpdatePlacementRule,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.enter();
        self.inner.update_placement_rule(identity, request)
    }
    fn compare_commits(
        &self,
        identity: &crate::auth::Identity,
        request: crate::CompareCommits<'_>,
        control: &crate::HistoryReadControl,
    ) -> Result<crate::ComparisonSnapshot, crate::ApiError> {
        self.enter();
        self.inner.compare_commits(identity, request, control)
    }
    fn disk_status(
        &self,
        identity: &crate::auth::Identity,
    ) -> Result<Vec<storage_contract::DiskStatus>, crate::ApiError> {
        self.enter();
        self.inner.disk_status(identity)
    }
    fn integrity_check(
        &self,
        identity: &crate::auth::Identity,
    ) -> Result<storage_contract::CheckReport, crate::ApiError> {
        self.enter();
        self.inner.integrity_check(identity)
    }
    fn create_merge_link(
        &self,
        identity: &crate::auth::Identity,
        request: crate::CreateMergeLink,
    ) -> Result<storage_contract::MergeLink, crate::ApiError> {
        self.enter();
        self.inner.create_merge_link(identity, request)
    }
    fn create_repository(
        &self,
        identity: &crate::auth::Identity,
        request: crate::CreateRepository,
    ) -> Result<storage_contract::Repository, crate::ApiError> {
        self.enter();
        self.inner.create_repository(identity, request)
    }
    fn create_branch(
        &self,
        identity: &crate::auth::Identity,
        request: crate::CreateBranch,
    ) -> Result<crate::BranchSnapshot, crate::ApiError> {
        self.enter();
        self.inner.create_branch(identity, request)
    }
    fn create_file_branch(
        &self,
        identity: &crate::auth::Identity,
        request: crate::CreateFileBranch,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.enter();
        self.inner.create_file_branch(identity, request)
    }
    fn update_note(
        &self,
        identity: &crate::auth::Identity,
        request: crate::UpdateNote,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.enter();
        self.inner.update_note(identity, request)
    }
    fn merge_file_branches(
        &self,
        identity: &crate::auth::Identity,
        request: crate::MergeFileBranches,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.enter();
        self.inner.merge_file_branches(identity, request)
    }
    fn update_file_node_tags(
        &self,
        identity: &crate::auth::Identity,
        request: crate::UpdateFileNodeTags,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.enter();
        self.inner.update_file_node_tags(identity, request)
    }
    fn create_disk_directory(
        &self,
        identity: &crate::auth::Identity,
        request: crate::CreateDiskDirectory,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.enter();
        self.inner.create_disk_directory(identity, request)
    }
    fn create_disk_write_session(
        &self,
        identity: &crate::auth::Identity,
        request: crate::CreateDiskWriteSession,
    ) -> Result<crate::DiskWriteSession, crate::ApiError> {
        self.enter();
        self.inner.create_disk_write_session(identity, request)
    }
    fn backup_configuration(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadBackupConfiguration,
        control: &crate::HistoryReadControl,
    ) -> Result<storage_contract::BackupConfigurationSnapshot, crate::ApiError> {
        self.enter();
        self.inner.backup_configuration(identity, request, control)
    }
    fn configure_backups(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ConfigureBackups,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.enter();
        self.inner.configure_backups(identity, request)
    }
    fn plan(
        &self,
        identity: &crate::auth::Identity,
        plan: &str,
    ) -> Result<storage_contract::Plan, crate::ApiError> {
        self.enter();
        self.inner.plan(identity, plan)
    }
    fn cancel_plan(
        &self,
        identity: &crate::auth::Identity,
        request: Result<crate::CancelPlan, crate::ApiError>,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.enter();
        self.inner.cancel_plan(identity, request)
    }
    fn register_disk(
        &self,
        identity: &crate::auth::Identity,
    ) -> Result<std::convert::Infallible, crate::ApiError> {
        self.enter();
        self.inner.register_disk(identity)
    }
    fn commit_plan(
        &self,
        identity: &crate::auth::Identity,
        request: Result<crate::CommitPlan, crate::ApiError>,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.enter();
        self.inner.commit_plan(identity, request)
    }
    fn prepare_historical_save(
        &self,
        identity: &crate::auth::Identity,
        request: Result<crate::PrepareHistoricalSave, crate::ApiError>,
    ) -> Result<storage_contract::Plan, crate::ApiError> {
        self.enter();
        self.inner.prepare_historical_save(identity, request)
    }
    fn prepare_purge(
        &self,
        identity: &crate::auth::Identity,
        request: Result<crate::PreparePurge, crate::ApiError>,
    ) -> Result<storage_contract::Plan, crate::ApiError> {
        self.enter();
        self.inner.prepare_purge(identity, request)
    }
    fn prepare_split(
        &self,
        identity: &crate::auth::Identity,
        request: Result<crate::PrepareSplit, crate::ApiError>,
    ) -> Result<storage_contract::Plan, crate::ApiError> {
        self.enter();
        self.inner.prepare_split(identity, request)
    }
    fn namespace_batch(
        &self,
        identity: &crate::auth::Identity,
        request: crate::NamespaceBatch,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.enter();
        self.inner.namespace_batch(identity, request)
    }
    fn select_file_branch(
        &self,
        identity: &crate::auth::Identity,
        request: crate::SelectFileBranch,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.enter();
        self.inner.select_file_branch(identity, request)
    }
    fn rename_file_branch(
        &self,
        identity: &crate::auth::Identity,
        request: crate::RenameFileBranch,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.enter();
        self.inner.rename_file_branch(identity, request)
    }
    fn listing(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadListing,
        control: &crate::HistoryReadControl,
    ) -> Result<crate::ListingSnapshot, crate::ApiError> {
        self.enter();
        self.inner.listing(identity, request, control)
    }
    fn mutate_disk_directory(
        &self,
        identity: &crate::auth::Identity,
        request: crate::MutateDiskDirectory,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.enter();
        self.inner.mutate_disk_directory(identity, request)
    }
    fn initialize_directory_scope(
        &self,
        identity: &crate::auth::Identity,
        request: crate::InitializeDirectoryScope,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.enter();
        self.inner.initialize_directory_scope(identity, request)
    }
    fn archive_branch(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ArchiveBranch,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.enter();
        self.inner.archive_branch(identity, request)
    }
    fn rename_directory_branch(
        &self,
        identity: &crate::auth::Identity,
        request: crate::RenameDirectoryBranch,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.enter();
        self.inner.rename_directory_branch(identity, request)
    }
    fn select_default_directory_branch(
        &self,
        identity: &crate::auth::Identity,
        request: crate::SelectDefaultDirectoryBranch,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.enter();
        self.inner
            .select_default_directory_branch(identity, request)
    }
    fn directory_selection(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadDirectorySelection,
        control: &crate::HistoryReadControl,
    ) -> Result<crate::DirectorySelectionSnapshot, crate::ApiError> {
        self.enter();
        self.inner.directory_selection(identity, request, control)
    }
    fn directory_scopes(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadDirectoryScopes<'_>,
        control: &crate::HistoryReadControl,
    ) -> Result<crate::DirectoryScopesSnapshot, crate::ApiError> {
        self.enter();
        self.inner.directory_scopes(identity, request, control)
    }
    fn default_directory_branch(
        &self,
        identity: &crate::auth::Identity,
        repository_id: &str,
        control: &crate::HistoryReadControl,
    ) -> Result<crate::DefaultDirectoryBranchSnapshot, crate::ApiError> {
        self.enter();
        self.inner
            .default_directory_branch(identity, repository_id, control)
    }
    fn file_history_snapshot(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadFileHistory,
        control: &crate::HistoryReadControl,
    ) -> Result<crate::FileHistoryPage, crate::ApiError> {
        self.enter();
        self.inner.file_history_snapshot(identity, request, control)
    }
    fn file_history_node_parents(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadNodeParents,
        control: &crate::HistoryReadControl,
    ) -> Result<crate::NodeParentsPage, crate::ApiError> {
        self.enter();
        self.inner
            .file_history_node_parents(identity, request, control)
    }
    fn file_history_branches(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadHistoryBranches,
        control: &crate::HistoryReadControl,
    ) -> Result<crate::HistoryBranchesPage, crate::ApiError> {
        self.enter();
        self.inner.file_history_branches(identity, request, control)
    }
    fn file_history_merges(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadHistoryMerges,
    ) -> Result<crate::HistoryMergesPage, crate::ApiError> {
        self.enter();
        self.inner.file_history_merges(identity, request)
    }
    fn file_history_merge_inputs(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadMergeInputs,
    ) -> Result<crate::MergeInputsPage, crate::ApiError> {
        self.enter();
        self.inner.file_history_merge_inputs(identity, request)
    }
    fn file_history_merge_parents(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadMergeParents,
    ) -> Result<crate::MergeParentsPage, crate::ApiError> {
        self.enter();
        self.inner.file_history_merge_parents(identity, request)
    }
    fn file_history_node_field(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadNodeField,
        control: &crate::HistoryReadControl,
    ) -> Result<crate::NodeFieldSlice, crate::ApiError> {
        self.enter();
        self.inner
            .file_history_node_field(identity, request, control)
    }
    fn file_version_page(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadVersionPage,
    ) -> Result<crate::VersionPage, crate::ApiError> {
        self.enter();
        self.inner.file_version_page(identity, request)
    }
    fn file_version_sources(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadVersionSources,
    ) -> Result<crate::VersionSourcesPage, crate::ApiError> {
        self.enter();
        self.inner.file_version_sources(identity, request)
    }
    fn topology_snapshot(
        &self,
        identity: &crate::auth::Identity,
    ) -> Result<crate::TopologySnapshot, crate::ApiError> {
        self.enter();
        self.inner.topology_snapshot(identity)
    }
    fn placement_snapshot(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<storage_contract::Placement, crate::ApiError> {
        self.enter();
        self.inner.placement_snapshot(identity, id)
    }
    fn event_page(
        &self,
        identity: &crate::auth::Identity,
        after: u64,
    ) -> Result<crate::EventPage, crate::ApiError> {
        self.enter();
        self.inner.event_page(identity, after)
    }
    fn activity_history(
        &self,
        identity: &crate::auth::Identity,
        control: &crate::HistoryReadControl,
    ) -> Result<crate::ActivityHistorySnapshot, crate::ApiError> {
        self.enter();
        self.inner.activity_history(identity, control)
    }
    fn history_graph(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadHistoryGraph,
        control: &crate::HistoryReadControl,
    ) -> Result<crate::HistoryGraphResponse, crate::ApiError> {
        self.enter();
        self.inner.history_graph(identity, request, control)
    }
    fn commit_snapshot(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<std::sync::Arc<storage_contract::Commit>, crate::ApiError> {
        self.enter();
        self.inner.commit_snapshot(identity, id)
    }
    fn resolve_entry(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ResolveEntry<'_>,
    ) -> Result<crate::ResolvedEntry, crate::ApiError> {
        self.enter();
        self.inner.resolve_entry(identity, request)
    }
    fn branch_snapshot(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<crate::BranchSnapshot, crate::ApiError> {
        self.enter();
        self.inner.branch_snapshot(identity, id)
    }
    fn branches_snapshot(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<Vec<crate::BranchSnapshot>, crate::ApiError> {
        self.enter();
        self.inner.branches_snapshot(identity, id)
    }
    fn repositories_snapshot(
        &self,
        identity: &crate::auth::Identity,
    ) -> Result<Vec<storage_contract::Repository>, crate::ApiError> {
        self.enter();
        self.inner.repositories_snapshot(identity)
    }
    fn tree_snapshot(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadTree<'_>,
        control: &crate::HistoryReadControl,
    ) -> Result<crate::TreeSnapshot, crate::ApiError> {
        self.enter();
        self.inner.tree_snapshot(identity, request, control)
    }
    fn create_write_session(
        &self,
        identity: &crate::auth::Identity,
        branch: &str,
    ) -> Result<crate::WorkspaceWriteSession, crate::ApiError> {
        self.enter();
        self.inner.create_write_session(identity, branch)
    }
    fn stage_namespace(
        &self,
        identity: &crate::auth::Identity,
        command: crate::StageNamespace,
    ) -> Result<storage_contract::Workspace, crate::ApiError> {
        self.enter();
        self.inner.stage_namespace(identity, command)
    }
    fn authorize_upload(
        &self,
        identity: &crate::auth::Identity,
        workspace: &str,
        logical_size: Option<u64>,
    ) -> Result<(), crate::ApiError> {
        self.enter();
        self.inner
            .authorize_upload(identity, workspace, logical_size)
    }

    fn lookup_current(
        &self,
        identity: &crate::auth::Identity,
        branch: &str,
        path: &name_policy::LookupPath,
    ) -> Result<Option<storage_contract::Entry>, crate::ApiError> {
        self.enter();
        self.inner.lookup_current(identity, branch, path)
    }

    fn lookup_at(
        &self,
        identity: &crate::auth::Identity,
        branch: &str,
        commit: &str,
        path: &name_policy::LookupPath,
    ) -> Result<Option<storage_contract::Entry>, crate::ApiError> {
        self.enter();
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
        self.enter();
        self.inner.upload(identity, workspace, item, path, source)
    }
    fn upload_generation(
        &self,
        identity: &Identity,
        workspace: &str,
        item: &str,
        path: &name_policy::PortablePath,
        source: &crate::UploadSource,
        generation: u64,
    ) -> Result<storage_contract::StagedFile, ApiError> {
        self.enter();
        self.inner
            .upload_generation(identity, workspace, item, path, source, generation)
    }
    fn content(&self, identity: &Identity, version: &str) -> Result<Content, ApiError> {
        self.enter();
        self.inner.content(identity, version)
    }
    fn range_content(
        &self,
        identity: &Identity,
        request: crate::ReadContent<'_>,
    ) -> Result<Content, ApiError> {
        self.enter();
        self.inner.range_content(identity, request)
    }
    fn workspace_content(
        &self,
        identity: &Identity,
        workspace: &str,
        item: &str,
    ) -> Result<Content, ApiError> {
        self.enter();
        self.inner.workspace_content(identity, workspace, item)
    }
    fn checksums(&self, identity: &Identity, version: &str) -> Result<Vec<u8>, ApiError> {
        self.enter();
        self.inner.checksums(identity, version)
    }
}

struct Fixture {
    _root: tempfile::TempDir,
    backend: Arc<EngineBackend>,
    auth: Arc<Mutex<AuthStore>>,
    identity: Identity,
    token: String,
    repo: Repository,
    version: String,
    operation: storage_contract::OperationId,
    workspace: Workspace,
    item: String,
    source: PathBuf,
}
impl Fixture {
    fn new(protected: bool) -> Self {
        let root = crate::tests::private_tempdir();
        let config = Config::development([
            root.path().join("primary"),
            root.path().join("secondary"),
            root.path().join("tertiary"),
        ]);
        let mut engine = Engine::initialize(&root.path().join("system"), config).unwrap();
        let repo = engine
            .create_repository(
                &storage_contract::RepositoryName::parse("Authorization").unwrap(),
                "P01",
                3,
                "owner",
                storage_contract::OperationId::generate(),
            )
            .unwrap();
        let workspace = engine
            .create_workspace(&repo.main_branch_id, "owner")
            .unwrap();
        engine
            .stage_bytes(&workspace.workspace_id, "file.txt", b"published")
            .unwrap();
        let operation = storage_contract::OperationId::generate();
        engine
            .commit_workspace(CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: operation,
                principal: "owner".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "authorization fixture".into(),
                request_digest: None,
            })
            .unwrap();
        let version = engine.list_tree(&repo.main_branch_id).unwrap()[0]
            .version_id
            .clone();
        let workspace = engine
            .create_workspace(&repo.main_branch_id, "owner")
            .unwrap();
        let staged = engine
            .stage_bytes(&workspace.workspace_id, "private.txt", b"private")
            .unwrap();
        let source = root.path().join("source");
        std::fs::write(&source, b"new upload").unwrap();
        let backend = Arc::new(EngineBackend::new(engine));
        let auth =
            AuthStore::initialize(root.path(), "owner", "integration-test-password").unwrap();
        let mut auth = if protected {
            backend.protect_auth(auth).unwrap()
        } else {
            auth
        };
        let token = auth
            .login("owner", "integration-test-password")
            .unwrap()
            .token;
        let identity = auth.authenticate(&token).unwrap();
        Self {
            _root: root,
            backend,
            auth: Arc::new(Mutex::new(auth)),
            identity,
            token,
            repo,
            version,
            operation,
            workspace,
            item: staged.item_id,
            source,
        }
    }
    fn snapshot(&self) -> (Value, Value, Vec<Vec<u8>>) {
        let engine = self.backend.engine.lock().unwrap();
        (
            json_codec::to_value(engine.state()).unwrap(),
            json_codec::to_value(engine.workspace(&self.workspace.workspace_id).unwrap()).unwrap(),
            engine
                .config()
                .disks
                .iter()
                .filter(|disk| disk.metadata_member)
                .map(|disk| {
                    std::fs::read(disk.root.join("metadata/application-log/journal.bin")).unwrap()
                })
                .collect(),
        )
    }
}

fn rooted_query() -> std::collections::BTreeMap<String, String> {
    std::collections::BTreeMap::from([("history_root".to_owned(), "0".repeat(64))])
}

#[derive(Clone, Copy, Debug)]
enum Request {
    Operation,
    OperationOutcome,
    CommitWorkspace,
    Metadata,
    Administrator,
    Tree,
    SmbAuthorizeRead,
    SnapshotLookup,
    CurrentLookup,
    DirectoryRead,
    WorkspaceSelection,
    WorkspaceCreate,
    WriteSession,
    WorkspaceAbort,
    UploadAdmission,
    SealWorkspace,
    PublicCommit,
    WorkspaceRead,
    WorkspaceCapacity,
    UploadStatus,
    Snapshot,
    Version,
    VersionAuthorization,
    RepositorySnapshot,
    RepositoriesSnapshot,
    BranchSnapshot,
    Resolve,
    CommitSnapshot,
    HistoryGraph,
    EventPage,
    ActivityHistorySnapshot,
    TopologySnapshot,
    FileHistory,
    FileHistorySnapshot,
    FileHistoryNodeParents,
    FileHistoryBranches,
    FileHistoryMerges,
    FileHistoryMergeInputs,
    FileHistoryMergeParents,
    FileHistoryNodeField,
    FileVersionSources,
    DefaultDirectoryBranch,
    Listing,
    DirectoryScopes,
    DirectorySelection,
    SelectDefaultDirectoryBranch,
    RenameDirectoryBranch,
    CreateFileBranch,
    CreateFileBranchFromNode,
    FileNote,
    VersionNote,
    NodeNote,
    MergeFileBranches,
    MergeFileBranchesAt,
    FileNodeTags,
    FileNodeTagsAt,
    CreateDiskDirectory,
    DiskWriteSession,
    DiskWriteSessionOnBranch,
    BackupConfiguration,
    ConfigureBackups,
    Plan,
    PlanCancel,
    PlanCommit,
    PrepareHistoricalSave,
    PreparePurge,
    PrepareSplit,
    RegisterDisk,
    NamespaceBatch,
    SelectFileBranch,
    SelectFileBranchAt,
    RenameFileBranch,
    DiskDirectoryCreate,
    DiskDirectorySelect,
    InitializeDirectoryScope,
    ArchiveBranch,
    CreateBranch,
    CreateMergeLink,
    DiskStatus,
    IntegrityCheck,
    CompareCommits,
    ProtectionPolicy,
    PlacementRule,
    UpdateProtectionPolicy,
    PlacementSnapshot,
    SmbBindingStatus,
    UpdateSmbBinding,
    BranchesSnapshot,
    RepositoryAuthorization,
    Namespace,
    Content,
    RangeContent,
    ContextContent,
    WorkspaceContent,
    Checksums,
    Upload,
    UploadGeneration,
}
impl Request {
    fn execute(self, fixture: &Fixture, backend: &dyn Backend) -> Result<(), ApiError> {
        let identity = &fixture.identity;
        let workspace = &fixture.workspace.workspace_id;
        match self {
            Self::BackupConfiguration => backend
                .backup_configuration(
                    identity,
                    crate::ReadBackupConfiguration {
                        repository_id: "missing".into(),
                        path: String::new(),
                    },
                    &crate::tests::history_read_control(),
                )
                .map(|_| ()),
            Self::ConfigureBackups => backend
                .configure_backups(
                    identity,
                    crate::ConfigureBackups {
                        repository_id: "missing".into(),
                        path: String::new(),
                        target_copies: 2,
                        backup_prefixes: std::collections::BTreeMap::new(),
                        expected_generation: 0,
                        expected_scope_id: "missing".into(),
                        operation_id: storage_contract::OperationId::generate(),
                    },
                )
                .map(|_| ()),
            Self::RegisterDisk => backend.register_disk(identity).map(|never| match never {}),
            Self::Plan => backend.plan(identity, "missing").map(|_| ()),
            Self::PlanCancel => backend
                .cancel_plan(
                    identity,
                    Ok(crate::CancelPlan {
                        plan_id: "missing".into(),
                        operation_id: Ok(storage_contract::OperationId::generate()),
                    }),
                )
                .map(|_| ()),
            Self::PlanCommit => backend
                .commit_plan(
                    identity,
                    Ok(crate::CommitPlan {
                        plan_id: "missing".into(),
                        plan_digest: "digest".into(),
                        operation_id: Ok(storage_contract::OperationId::generate()),
                    }),
                )
                .map(|_| ()),
            Self::NamespaceBatch => backend
                .namespace_batch(
                    identity,
                    crate::NamespaceBatch {
                        slot_id: "missing".into(),
                        request: storage_contract::DirectoryNamespaceBatch {
                            kind: storage_contract::DirectoryBatchKind::Delete,
                            items: vec![],
                        },
                        operation_id: storage_contract::OperationId::generate(),
                    },
                )
                .map(|_| ()),
            Self::SelectFileBranch => backend
                .select_file_branch(
                    identity,
                    crate::SelectFileBranch {
                        file_id: "missing".into(),
                        branch_id: "missing".into(),
                        location: None,
                        operation_id: storage_contract::OperationId::generate(),
                    },
                )
                .map(|_| ()),
            Self::SelectFileBranchAt => backend
                .select_file_branch(
                    identity,
                    crate::SelectFileBranch {
                        file_id: "missing".into(),
                        branch_id: "missing".into(),
                        location: Some(crate::FileSelectionLocation {
                            repository_id: "repository".into(),
                            path: name_policy::PortablePath::parse("Café/file.txt").unwrap(),
                        }),
                        operation_id: storage_contract::OperationId::generate(),
                    },
                )
                .map(|_| ()),
            Self::FileNote => backend
                .update_note(
                    identity,
                    crate::UpdateNote {
                        target: crate::NoteTarget::File("file".into()),
                        note: storage_contract::Note::parse("".into()).unwrap(),
                        operation_id: storage_contract::OperationId::generate(),
                    },
                )
                .map(|_| ()),
            Self::VersionNote => backend
                .update_note(
                    identity,
                    crate::UpdateNote {
                        target: crate::NoteTarget::Version("version".into()),
                        note: storage_contract::Note::parse("".into()).unwrap(),
                        operation_id: storage_contract::OperationId::generate(),
                    },
                )
                .map(|_| ()),
            Self::NodeNote => backend
                .update_note(
                    identity,
                    crate::UpdateNote {
                        target: crate::NoteTarget::Node {
                            file: "file".into(),
                            node: "node".into(),
                        },
                        note: storage_contract::Note::parse("".into()).unwrap(),
                        operation_id: storage_contract::OperationId::generate(),
                    },
                )
                .map(|_| ()),
            Self::RenameFileBranch => backend
                .rename_file_branch(
                    identity,
                    crate::RenameFileBranch {
                        file_id: "missing".into(),
                        branch_id: "missing".into(),
                        name: storage_contract::BranchName::parse("Side").unwrap(),
                        operation_id: storage_contract::OperationId::generate(),
                    },
                )
                .map(|_| ()),
            Self::CreateFileBranch => backend
                .create_file_branch(
                    identity,
                    crate::CreateFileBranch {
                        file_id: "missing".into(),
                        source_version_id: "missing".into(),
                        source_node_id: None,
                        name: storage_contract::BranchName::parse("Side").unwrap(),
                        operation_id: storage_contract::OperationId::generate(),
                    },
                )
                .map(|_| ()),
            Self::CreateFileBranchFromNode => backend
                .create_file_branch(
                    identity,
                    crate::CreateFileBranch {
                        file_id: "missing".into(),
                        source_version_id: "missing".into(),
                        source_node_id: Some("missing".into()),
                        name: storage_contract::BranchName::parse("Side").unwrap(),
                        operation_id: storage_contract::OperationId::generate(),
                    },
                )
                .map(|_| ()),
            Self::MergeFileBranches => backend
                .merge_file_branches(
                    identity,
                    crate::MergeFileBranches {
                        file_id: "file".into(),
                        inputs: vec![storage_contract::FileMergeInput {
                            branch_id: "branch".into(),
                            expected_node_id: "head".into(),
                        }],
                        chosen_node_id: "chosen".into(),
                        surviving_branch_id: "branch".into(),
                        confirmed: true,
                        location: None,
                        operation_id: storage_contract::OperationId::generate(),
                    },
                )
                .map(|_| ()),
            Self::MergeFileBranchesAt => backend
                .merge_file_branches(
                    identity,
                    crate::MergeFileBranches {
                        file_id: "file".into(),
                        inputs: vec![storage_contract::FileMergeInput {
                            branch_id: "branch".into(),
                            expected_node_id: "head".into(),
                        }],
                        chosen_node_id: "chosen".into(),
                        surviving_branch_id: "branch".into(),
                        confirmed: true,
                        location: Some(storage_contract::FileLocation {
                            repository_id: "repo".into(),
                            path: "file.txt".into(),
                        }),
                        operation_id: storage_contract::OperationId::generate(),
                    },
                )
                .map(|_| ()),
            Self::FileNodeTags => backend
                .update_file_node_tags(
                    identity,
                    crate::UpdateFileNodeTags {
                        file_id: "file".into(),
                        targets: vec![storage_contract::FileTagTarget {
                            branch_id: "branch".into(),
                            expected_node_id: "head".into(),
                            source_node_id: "source".into(),
                        }],
                        add: vec!["added".into()],
                        remove: vec!["removed".into()],
                        location: None,
                        operation_id: storage_contract::OperationId::generate(),
                    },
                )
                .map(|_| ()),
            Self::FileNodeTagsAt => backend
                .update_file_node_tags(
                    identity,
                    crate::UpdateFileNodeTags {
                        file_id: "file".into(),
                        targets: vec![storage_contract::FileTagTarget {
                            branch_id: "branch".into(),
                            expected_node_id: "head".into(),
                            source_node_id: "source".into(),
                        }],
                        add: vec!["added".into()],
                        remove: vec!["removed".into()],
                        location: Some(storage_contract::FileLocation {
                            repository_id: "repo".into(),
                            path: "file.txt".into(),
                        }),
                        operation_id: storage_contract::OperationId::generate(),
                    },
                )
                .map(|_| ()),
            Self::CreateDiskDirectory => backend
                .create_disk_directory(
                    identity,
                    crate::CreateDiskDirectory {
                        slot_id: "P01".into(),
                        path: name_policy::PortablePath::parse("Café/folder").unwrap(),
                        operation_id: storage_contract::OperationId::generate(),
                    },
                )
                .map(|_| ()),
            Self::DiskWriteSession => backend
                .create_disk_write_session(
                    identity,
                    crate::CreateDiskWriteSession {
                        slot_id: "P01".into(),
                        path: name_policy::PortablePath::parse("Café/file.txt").unwrap(),
                        branch_id: None,
                    },
                )
                .map(|_| ()),
            Self::DiskWriteSessionOnBranch => backend
                .create_disk_write_session(
                    identity,
                    crate::CreateDiskWriteSession {
                        slot_id: "P01".into(),
                        path: name_policy::PortablePath::parse("Café/file.txt").unwrap(),
                        branch_id: Some(
                            "br_00000000000000000000000000000000"
                                .to_owned()
                                .try_into()
                                .unwrap(),
                        ),
                    },
                )
                .map(|_| ()),
            Self::PrepareHistoricalSave => backend
                .prepare_historical_save(
                    identity,
                    Ok(crate::PrepareHistoricalSave {
                        history_tree_id: "history".into(),
                        options: Ok(storage_contract::HistoricalSaveOptions {
                            source_commit_id: "commit".into(),
                            name: "Saved branch".into(),
                            workspace_id: None,
                            primary_slot_id: None,
                            target_copies: None,
                        }),
                    }),
                )
                .map(|_| ()),
            Self::PreparePurge => backend
                .prepare_purge(
                    identity,
                    Ok(crate::PreparePurge {
                        history_tree_id: "history".into(),
                        version_ids: Ok(vec!["second".into(), "first".into(), "second".into()]),
                    }),
                )
                .map(|_| ()),
            Self::PrepareSplit => backend
                .prepare_split(
                    identity,
                    Ok(crate::PrepareSplit {
                        history_tree_id: "history".into(),
                        cut_commit_id: Ok("cut".into()),
                    }),
                )
                .map(|_| ()),
            Self::Operation => backend.operation(identity, fixture.operation).map(|_| ()),
            Self::OperationOutcome => backend
                .operation_outcome(identity, fixture.operation)
                .map(|_| ()),
            Self::CommitWorkspace => backend
                .commit_workspace(
                    identity,
                    crate::CommitWorkspace {
                        workspace_id: workspace.clone(),
                        operation_id: storage_contract::OperationId::generate(),
                        expected_head: fixture.workspace.base_head.clone(),
                        fencing_token: fixture.workspace.fencing_token,
                        message: String::new(),
                        request_digest: None,
                    },
                )
                .map(|_| ()),
            Self::Metadata => backend
                .update_placement_rule(
                    identity,
                    crate::UpdatePlacementRule {
                        repository_id: fixture.repo.repository_id.clone(),
                        rule_id: new_id("rule"),
                        path_prefix: String::new(),
                        primary_slot_id: "P01".into(),
                        expected_generation: 0,
                        operation_id: storage_contract::OperationId::generate(),
                    },
                )
                .map(|_| ()),
            Self::Administrator => backend
                .create_repository(
                    identity,
                    crate::CreateRepository {
                        name: storage_contract::RepositoryName::parse("Forbidden new repository")
                            .unwrap(),
                        primary_slot_id: "P01".into(),
                        target_copies: 3,
                        operation_id: storage_contract::OperationId::generate(),
                    },
                )
                .map(|_| ()),
            Self::Tree => backend
                .tree_snapshot(
                    identity,
                    crate::ReadTree {
                        branch: &fixture.repo.main_branch_id,
                        commit: None,
                    },
                    &crate::tests::history_read_control(),
                )
                .map(|_| ()),
            Self::Snapshot => backend
                .snapshot_at(
                    identity,
                    &fixture.repo.main_branch_id,
                    &fixture.workspace.base_head,
                )
                .map(|_| ()),
            Self::WorkspaceCapacity => backend.workspace_capacity(identity).map(|_| ()),
            Self::WorkspaceRead => backend.workspace(identity, workspace).map(|_| ()),
            Self::UploadStatus => backend
                .published_upload_status(identity, workspace, "missing-item")
                .map(|_| ()),
            Self::SealWorkspace => backend.seal_workspace(identity, workspace).map(|_| ()),
            Self::PublicCommit => {
                let body = json!({
                    "workspace_id": workspace,
                    "operation_id": fixture.operation,
                    "repository_id": fixture.repo.repository_id,
                    "branch_id": fixture.repo.main_branch_id,
                    "expected_head": fixture.workspace.base_head,
                    "fencing_token": fixture.workspace.fencing_token.to_string(),
                    "message": "",
                    "requested_protection": "strict",
                    "file_placement": {},
                    "expected_topology_generation": "1",
                    "sealed_manifest_hash": "manifest",
                    "request_digest": "digest"
                });
                let envelope =
                    storage_contract::PublicCommitEnvelope::parse(workspace, body).unwrap();
                backend
                    .commit_workspace_public(identity, envelope)
                    .map(|_| ())
            }
            Self::UploadAdmission => {
                backend.authorize_upload(identity, &fixture.workspace.workspace_id, Some(42))
            }
            Self::WriteSession => backend
                .create_write_session(identity, &fixture.repo.main_branch_id)
                .map(|_| ()),
            Self::WorkspaceCreate => backend
                .create_workspace(
                    identity,
                    crate::CreateWorkspace {
                        branch_id: fixture.repo.main_branch_id.clone(),
                        source_commit_id: None,
                    },
                )
                .map(|_| ()),
            Self::WorkspaceAbort => {
                backend.abort_workspace(identity, &fixture.workspace.workspace_id)
            }
            Self::WorkspaceSelection => backend.authorize_workspace_selection(
                identity,
                &fixture.workspace.workspace_id,
                &name_policy::PortablePath::parse("file.txt").unwrap(),
            ),
            Self::DirectoryRead => backend
                .read_directory(
                    identity,
                    crate::ReadDirectory {
                        branch: &fixture.repo.main_branch_id,
                        entry_id: &fixture.repo.main_branch_id,
                        path: None,
                        selection: crate::DirectorySelection::Capture,
                    },
                )
                .map(|_| ()),
            Self::CurrentLookup => backend
                .lookup_current(
                    identity,
                    &fixture.repo.main_branch_id,
                    &name_policy::LookupPath::parse("file.txt").unwrap(),
                )
                .map(|_| ()),
            Self::SnapshotLookup => backend
                .lookup_at(
                    identity,
                    &fixture.repo.main_branch_id,
                    &fixture.workspace.base_head,
                    &name_policy::LookupPath::parse("file.txt").unwrap(),
                )
                .map(|_| ()),
            // SMB directory/write handles need current admission, not a tree.
            Self::SmbAuthorizeRead => {
                backend.authorize_current(identity, &fixture.repo.main_branch_id)
            }
            // Immutable SMB file handles still authorize their pinned version.
            Self::Version => backend
                .version_snapshot(identity, &fixture.version)
                .map(|_| ()),
            Self::SmbBindingStatus => backend
                .smb_binding_status(identity, &fixture.repo.repository_id)
                .map(|_| ()),
            Self::UpdateSmbBinding => backend
                .update_smb_binding(
                    identity,
                    crate::UpdateSmbBinding {
                        repository: &fixture.repo.repository_id,
                        branch: &fixture.repo.main_branch_id,
                        enabled: false,
                        expected_history_generation: 1,
                        operation: storage_contract::OperationId::generate(),
                    },
                )
                .map(|_| ()),
            Self::ProtectionPolicy => backend
                .protection_policy(identity, "missing-policy")
                .map(|_| ()),
            Self::PlacementRule => backend.placement_rule(identity, "missing-rule").map(|_| ()),
            Self::UpdateProtectionPolicy => backend
                .update_protection_policy(
                    identity,
                    crate::UpdateProtectionPolicy {
                        repository_id: fixture.repo.repository_id.clone(),
                        policy_id: new_id("policy"),
                        path_prefix: String::new(),
                        target_copies: 3,
                        expected_generation: 0,
                        operation_id: storage_contract::OperationId::generate(),
                    },
                )
                .map(|_| ()),
            Self::CompareCommits => backend
                .compare_commits(
                    identity,
                    crate::CompareCommits {
                        left: &fixture.workspace.base_head,
                        right: "missing",
                    },
                    &crate::tests::history_read_control(),
                )
                .map(|_| ()),
            Self::DiskStatus => backend.disk_status(identity).map(|_| ()),
            Self::IntegrityCheck => backend.integrity_check(identity).map(|_| ()),
            Self::DefaultDirectoryBranch => backend
                .default_directory_branch(
                    identity,
                    "missing-repository",
                    &crate::tests::history_read_control(),
                )
                .map(|_| ()),
            Self::SelectDefaultDirectoryBranch => {
                let request = crate::SelectDefaultDirectoryBranch {
                    repository_id: "missing-repository".into(),
                    directory_branch_id: "missing-directory".into(),
                    operation_id: storage_contract::OperationId::generate(),
                };
                backend
                    .select_default_directory_branch(identity, request)
                    .map(|_| ())
            }
            Self::DirectorySelection => {
                let request = crate::ReadDirectorySelection::parse(
                    "missing-repository".into(),
                    "",
                    vec![],
                    vec![],
                )
                .unwrap();
                backend
                    .directory_selection(identity, request, &crate::tests::history_read_control())
                    .map(|_| ())
            }
            Self::Listing => backend
                .listing(
                    identity,
                    crate::ReadListing::parse("missing-slot".into(), Default::default()).unwrap(),
                    &crate::tests::history_read_control(),
                )
                .map(|_| ()),
            Self::DirectoryScopes => {
                let request = crate::ReadDirectoryScopes {
                    repository_id: "missing-repository",
                    filter: &crate::DirectoryBranchFilter::All,
                    projection: crate::DirectoryBranchProjection::Full,
                };
                backend
                    .directory_scopes(identity, request, &crate::tests::history_read_control())
                    .map(|_| ())
            }
            Self::RenameDirectoryBranch => {
                let request = crate::RenameDirectoryBranch {
                    repository_id: "missing-repository".into(),
                    directory_branch_id: "missing-directory".into(),
                    name: storage_contract::BranchName::parse("renamed").unwrap(),
                    operation_id: storage_contract::OperationId::generate(),
                };
                backend
                    .rename_directory_branch(identity, request)
                    .map(|_| ())
            }
            Self::ArchiveBranch => {
                let request = crate::ArchiveBranch {
                    branch_id: "missing-branch".into(),
                    operation_id: storage_contract::OperationId::generate(),
                };
                backend.archive_branch(identity, request).map(|_| ())
            }
            Self::CreateBranch => backend
                .create_branch(
                    identity,
                    crate::CreateBranch {
                        repository_id: "missing".into(),
                        source_commit_id: "missing".into(),
                        name: storage_contract::BranchName::parse("Side").unwrap(),
                        operation_id: storage_contract::OperationId::generate(),
                    },
                )
                .map(|_| ()),
            Self::CreateMergeLink => backend
                .create_merge_link(
                    identity,
                    crate::CreateMergeLink {
                        history_tree_id: "missing".into(),
                        source_branch_id: "missing".into(),
                        source_commit_id: "missing".into(),
                        target_branch_id: "missing".into(),
                        target_commit_id: "missing".into(),
                        operation_id: storage_contract::OperationId::generate(),
                    },
                )
                .map(|_| ()),
            Self::InitializeDirectoryScope => backend
                .initialize_directory_scope(
                    identity,
                    crate::InitializeDirectoryScope {
                        repository_id: "missing".into(),
                        path: None,
                        operation_id: storage_contract::OperationId::generate(),
                    },
                )
                .map(|_| ()),
            Self::DiskDirectoryCreate => backend
                .mutate_disk_directory(
                    identity,
                    crate::MutateDiskDirectory {
                        slot_id: "missing".into(),
                        path: None,
                        name: storage_contract::BranchName::parse("Side").unwrap(),
                        operation_id: storage_contract::OperationId::generate(),
                        action: crate::DiskDirectoryAction::Create,
                    },
                )
                .map(|_| ()),
            Self::DiskDirectorySelect => backend
                .mutate_disk_directory(
                    identity,
                    crate::MutateDiskDirectory {
                        slot_id: "missing".into(),
                        path: None,
                        name: storage_contract::BranchName::parse("Side").unwrap(),
                        operation_id: storage_contract::OperationId::generate(),
                        action: crate::DiskDirectoryAction::Select,
                    },
                )
                .map(|_| ()),
            Self::FileHistorySnapshot => backend
                .file_history_snapshot(
                    identity,
                    crate::ReadFileHistory::parse(
                        "missing-file".into(),
                        std::collections::BTreeMap::new(),
                    )
                    .unwrap(),
                    &crate::tests::history_read_control(),
                )
                .map(|_| ()),
            Self::FileHistory => backend
                .file_version_page(
                    identity,
                    crate::ReadVersionPage::parse(
                        "missing-file".into(),
                        std::collections::BTreeMap::new(),
                    )
                    .unwrap(),
                )
                .map(|_| ()),
            Self::FileHistoryNodeParents => backend
                .file_history_node_parents(
                    identity,
                    crate::ReadNodeParents::parse(
                        "missing-file".into(),
                        "node".into(),
                        rooted_query(),
                    )
                    .unwrap(),
                    &crate::tests::history_read_control(),
                )
                .map(|_| ()),
            Self::FileHistoryBranches => backend
                .file_history_branches(
                    identity,
                    crate::ReadHistoryBranches::parse("missing-file".into(), rooted_query())
                        .unwrap(),
                    &crate::tests::history_read_control(),
                )
                .map(|_| ()),
            Self::FileHistoryMerges => backend
                .file_history_merges(
                    identity,
                    crate::ReadHistoryMerges::parse("missing-file".into(), rooted_query()).unwrap(),
                )
                .map(|_| ()),
            Self::FileHistoryMergeInputs => backend
                .file_history_merge_inputs(
                    identity,
                    crate::ReadMergeInputs(
                        crate::backend_file_history::ReadIndexedSlice::parse(
                            "missing-file".into(),
                            "merge_id",
                            "merge".into(),
                            rooted_query(),
                        )
                        .unwrap(),
                    ),
                )
                .map(|_| ()),
            Self::FileHistoryMergeParents => backend
                .file_history_merge_parents(
                    identity,
                    crate::ReadMergeParents(
                        crate::backend_file_history::ReadIndexedSlice::parse(
                            "missing-file".into(),
                            "merge_id",
                            "merge".into(),
                            rooted_query(),
                        )
                        .unwrap(),
                    ),
                )
                .map(|_| ()),
            Self::FileHistoryNodeField => backend
                .file_history_node_field(
                    identity,
                    crate::ReadNodeField::parse(
                        "missing-file".into(),
                        "node".into(),
                        "message".into(),
                        rooted_query(),
                    )
                    .unwrap(),
                    &crate::tests::history_read_control(),
                )
                .map(|_| ()),
            Self::FileVersionSources => backend
                .file_version_sources(
                    identity,
                    crate::ReadVersionSources(
                        crate::backend_file_history::ReadIndexedSlice::parse(
                            "missing-file".into(),
                            "version_id",
                            "version".into(),
                            rooted_query(),
                        )
                        .unwrap(),
                    ),
                )
                .map(|_| ()),
            Self::TopologySnapshot => backend.topology_snapshot(identity).map(|_| ()),
            Self::PlacementSnapshot => backend
                .placement_snapshot(identity, &fixture.version)
                .map(|_| ()),
            Self::ActivityHistorySnapshot => backend
                .activity_history(
                    identity,
                    &crate::HistoryReadControl::until(
                        std::time::Instant::now() + std::time::Duration::from_secs(60),
                    ),
                )
                .map(|_| ()),
            Self::EventPage => backend.event_page(identity, u64::MAX).map(|_| ()),
            Self::HistoryGraph => backend
                .history_graph(
                    identity,
                    crate::ReadHistoryGraph::page(
                        fixture.repo.history_tree_id.clone(),
                        std::collections::BTreeMap::from([("collection".into(), "commits".into())]),
                    )
                    .unwrap(),
                    &crate::HistoryReadControl::until(
                        std::time::Instant::now() + std::time::Duration::from_secs(60),
                    ),
                )
                .map(|_| ()),
            Self::CommitSnapshot => backend
                .commit_snapshot(identity, &fixture.workspace.base_head)
                .map(|_| ()),
            Self::Resolve => backend
                .resolve_entry(
                    identity,
                    crate::ResolveEntry {
                        branch: &fixture.repo.main_branch_id,
                        path: &name_policy::LookupPath::parse("file.txt").unwrap(),
                        commit: None,
                    },
                )
                .map(|_| ()),
            Self::BranchSnapshot => backend
                .branch_snapshot(identity, &fixture.repo.main_branch_id)
                .map(|_| ()),
            Self::BranchesSnapshot => backend
                .branches_snapshot(identity, &fixture.repo.repository_id)
                .map(|_| ()),
            Self::RepositoriesSnapshot => backend.repositories_snapshot(identity).map(|_| ()),
            Self::RepositorySnapshot => backend
                .repository_snapshot(identity, &fixture.repo.repository_id)
                .map(|_| ()),
            Self::RepositoryAuthorization => {
                backend.authorize_repository(identity, &fixture.repo.repository_id)
            }
            Self::VersionAuthorization => backend.authorize_version(identity, &fixture.version),
            Self::Namespace => backend
                .stage_namespace(
                    identity,
                    crate::StageNamespace {
                        workspace_id: workspace.clone(),
                        change: storage_contract::NamespaceOperation::Mkdir {
                            path: name_policy::PortablePath::parse("forbidden").unwrap(),
                        },
                    },
                )
                .map(|_| ()),
            Self::Content => backend.content(identity, &fixture.version).map(|_| ()),
            Self::ContextContent => backend
                .range_content(
                    identity,
                    crate::ReadContent {
                        version: &fixture.version,
                        context: Some(&crate::DownloadContext {
                            repository_id: fixture.repo.repository_id.clone(),
                            path: name_policy::LookupPath::parse("file.txt").unwrap(),
                        }),
                    },
                )
                .map(|_| ()),
            Self::RangeContent => backend
                .range_content(
                    identity,
                    crate::ReadContent {
                        version: &fixture.version,
                        context: None,
                    },
                )
                .map(|_| ()),
            Self::WorkspaceContent => backend
                .workspace_content(identity, workspace, &fixture.item)
                .map(|_| ()),
            Self::Checksums => backend.checksums(identity, &fixture.version).map(|_| ()),
            Self::Upload => backend
                .upload(
                    identity,
                    workspace,
                    "new-item",
                    &name_policy::PortablePath::parse("new.txt").unwrap(),
                    &crate::UploadSource::from_file(
                        std::fs::File::open(&fixture.source).unwrap(),
                        u64::MAX,
                    )
                    .unwrap(),
                )
                .map(|_| ()),
            Self::UploadGeneration => backend
                .upload_generation(
                    identity,
                    workspace,
                    "next-item",
                    &name_policy::PortablePath::parse("next.txt").unwrap(),
                    &crate::UploadSource::from_file(
                        std::fs::File::open(&fixture.source).unwrap(),
                        u64::MAX,
                    )
                    .unwrap(),
                    1,
                )
                .map(|_| ()),
        }
    }
}

fn prepare_unknown_fields(
    backend: &dyn Backend,
    identity: &Identity,
) -> [Result<storage_contract::Plan, ApiError>; 3] {
    [
        backend.prepare_historical_save(identity, Err(ApiError::invalid("unknown field surprise"))),
        backend.prepare_purge(identity, Err(ApiError::invalid("unknown field surprise"))),
        backend.prepare_split(identity, Err(ApiError::invalid("unknown field surprise"))),
    ]
}

#[derive(Clone, Copy, Debug)]
enum Revocation {
    Logout,
    PasswordChange,
}

#[test]
fn completed_account_revocation_precedes_all_engine_admissions() {
    for protected in [true, false] {
        for revocation in [Revocation::Logout, Revocation::PasswordChange] {
            let fixture = Arc::new(Fixture::new(protected));
            if !protected {
                fixture
                    .backend
                    .set_management_auth(crate::lock(&fixture.auth).reader())
                    .unwrap();
            }
            for result in prepare_unknown_fields(fixture.backend.as_ref(), &fixture.identity) {
                let error = result.unwrap_err();
                assert_eq!(error.code, storage_contract::ErrorCode::InvalidRequest);
                assert_eq!(error.status, 400);
            }
            let requests = vec![
                Request::PrepareSplit,
                Request::PreparePurge,
                Request::PrepareHistoricalSave,
                Request::BackupConfiguration,
                Request::ConfigureBackups,
                Request::Plan,
                Request::PlanCancel,
                Request::PlanCommit,
                Request::RegisterDisk,
                Request::NamespaceBatch,
                Request::SelectFileBranch,
                Request::SelectFileBranchAt,
                Request::FileNote,
                Request::VersionNote,
                Request::NodeNote,
                Request::CreateFileBranch,
                Request::CreateFileBranchFromNode,
                Request::RenameFileBranch,
                Request::Operation,
                Request::OperationOutcome,
                Request::DiskWriteSessionOnBranch,
                Request::DiskWriteSession,
                Request::CreateDiskDirectory,
                Request::FileNodeTagsAt,
                Request::FileNodeTags,
                Request::MergeFileBranchesAt,
                Request::MergeFileBranches,
                Request::CommitWorkspace,
                Request::Metadata,
                Request::Administrator,
                Request::DiskStatus,
                Request::IntegrityCheck,
                Request::Tree,
                Request::SmbAuthorizeRead,
                Request::CurrentLookup,
                Request::DirectoryRead,
                Request::WorkspaceSelection,
                Request::WorkspaceCreate,
                Request::WriteSession,
                Request::WorkspaceAbort,
                Request::UploadAdmission,
                Request::SealWorkspace,
                Request::PublicCommit,
                Request::WorkspaceRead,
                Request::WorkspaceCapacity,
                Request::UploadStatus,
                Request::SnapshotLookup,
                Request::Snapshot,
                Request::Version,
                Request::VersionAuthorization,
                Request::RepositorySnapshot,
                Request::RepositoriesSnapshot,
                Request::BranchSnapshot,
                Request::Resolve,
                Request::CommitSnapshot,
                Request::HistoryGraph,
                Request::FileHistory,
                Request::FileHistorySnapshot,
                Request::FileHistoryNodeParents,
                Request::FileHistoryBranches,
                Request::FileHistoryMerges,
                Request::FileHistoryMergeInputs,
                Request::FileHistoryMergeParents,
                Request::FileHistoryNodeField,
                Request::FileVersionSources,
                Request::DiskDirectorySelect,
                Request::DiskDirectoryCreate,
                Request::InitializeDirectoryScope,
                Request::CreateMergeLink,
                Request::CreateBranch,
                Request::ArchiveBranch,
                Request::RenameDirectoryBranch,
                Request::Listing,
                Request::DirectoryScopes,
                Request::DirectorySelection,
                Request::SelectDefaultDirectoryBranch,
                Request::DefaultDirectoryBranch,
                Request::EventPage,
                Request::ActivityHistorySnapshot,
                Request::TopologySnapshot,
                Request::CompareCommits,
                Request::ProtectionPolicy,
                Request::PlacementRule,
                Request::UpdateProtectionPolicy,
                Request::PlacementSnapshot,
                Request::SmbBindingStatus,
                Request::UpdateSmbBinding,
                Request::BranchesSnapshot,
                Request::RepositoryAuthorization,
                Request::Namespace,
                Request::Content,
                Request::RangeContent,
                Request::ContextContent,
                Request::WorkspaceContent,
                Request::Checksums,
                Request::Upload,
                Request::UploadGeneration,
            ];
            let captured = Arc::new(Barrier::new(requests.len() + 1));
            let released = Arc::new(Barrier::new(requests.len() + 1));
            let backend = Arc::new(AuthorizedBackend {
                backend: Arc::new(PausedBackend {
                    inner: fixture.backend.clone(),
                    captured: captured.clone(),
                    released: released.clone(),
                }),
                auth: crate::lock(&fixture.auth).reader(),
            });
            let threads = requests
                .into_iter()
                .map(|request| {
                    let fixture = fixture.clone();
                    let backend = backend.clone();
                    (
                        request,
                        std::thread::spawn(move || request.execute(&fixture, backend.as_ref())),
                    )
                })
                .collect::<Vec<_>>();
            captured.wait();
            let revoked = match revocation {
                Revocation::Logout => fixture.auth.lock().unwrap().logout(&fixture.token),
                Revocation::PasswordChange => fixture.auth.lock().unwrap().change_password(
                    &fixture.token,
                    "integration-test-password",
                    "new-integration-test-password",
                ),
            };
            let before = fixture.snapshot();
            released.wait();
            revoked.unwrap();
            for (request, thread) in threads {
                let error = thread
                    .join()
                    .unwrap()
                    .expect_err("revoked request must be rejected");
                assert_eq!(
                    error.code,
                    storage_contract::ErrorCode::Unauthorized,
                    "{revocation:?}: {request:?}"
                );
            }
            for result in prepare_unknown_fields(fixture.backend.as_ref(), &fixture.identity) {
                let error = result.unwrap_err();
                assert_eq!(error.code, storage_contract::ErrorCode::Unauthorized);
                assert_eq!(error.status, 401);
            }
            for request in [
                Err(ApiError::invalid("unknown cancellation field")),
                Ok(crate::CancelPlan {
                    plan_id: "missing".into(),
                    operation_id: Err(ApiError::invalid("invalid operation")),
                }),
            ] {
                assert_eq!(
                    fixture
                        .backend
                        .cancel_plan(&fixture.identity, request)
                        .unwrap_err()
                        .code,
                    storage_contract::ErrorCode::Unauthorized,
                    "{revocation:?}: current admission precedes both parse stages"
                );
            }
            for request in [
                Err(ApiError::invalid("missing commit fields")),
                Ok(crate::CommitPlan {
                    plan_id: "missing".into(),
                    plan_digest: "digest".into(),
                    operation_id: Err(ApiError::invalid("bad operation")),
                }),
            ] {
                assert_eq!(
                    fixture
                        .backend
                        .commit_plan(&fixture.identity, request)
                        .unwrap_err()
                        .code,
                    storage_contract::ErrorCode::Unauthorized
                );
            }
            assert!(
                fixture.snapshot() == before,
                "rejected requests changed state, workspace or journals"
            );
        }
    }
}

#[test]
fn revoked_session_cannot_turn_public_probes_into_data_admission() {
    let fixture = Fixture::new(true);
    assert_eq!(
        fixture
            .backend
            .repositories_snapshot(&fixture.identity)
            .unwrap()
            .len(),
        1
    );
    assert!(
        Request::Metadata
            .execute(&fixture, fixture.backend.as_ref())
            .is_ok()
    );
    assert!(
        Request::Content
            .execute(&fixture, fixture.backend.as_ref())
            .is_ok()
    );
    fixture.auth.lock().unwrap().logout(&fixture.token).unwrap();
    assert_eq!(
        fixture
            .backend
            .repositories_snapshot(&fixture.identity)
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::Unauthorized
    );
    for result in [
        fixture.backend.disk_status(&fixture.identity).map(|_| ()),
        fixture
            .backend
            .integrity_check(&fixture.identity)
            .map(|_| ()),
    ] {
        assert_eq!(
            result.unwrap_err().code,
            storage_contract::ErrorCode::Unauthorized
        );
    }
    let anonymous = Identity {
        admission_session: None,
        smb_binding: None,
        principal: String::new(),
    };
    assert!(fixture.backend.capabilities().is_ok());
    assert!(fixture.backend.ready().is_ok());
    assert_eq!(
        fixture
            .backend
            .repositories_snapshot(&anonymous)
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::Unauthorized
    );
}

#[test]
fn embedded_backend_without_protected_credentials_fails_closed() {
    let fixture = Fixture::new(false);
    assert!(
        fixture
            .backend
            .engine
            .lock()
            .unwrap()
            .auth_state()
            .is_none()
    );
    let before = fixture.snapshot();
    for request in [
        Request::Operation,
        Request::OperationOutcome,
        Request::DiskWriteSessionOnBranch,
        Request::DiskWriteSession,
        Request::CreateDiskDirectory,
        Request::FileNodeTagsAt,
        Request::FileNodeTags,
        Request::MergeFileBranchesAt,
        Request::MergeFileBranches,
        Request::CommitWorkspace,
        Request::SnapshotLookup,
        Request::Metadata,
        Request::Administrator,
        Request::Tree,
        Request::SmbAuthorizeRead,
        Request::CurrentLookup,
        Request::DirectoryRead,
        Request::WorkspaceSelection,
        Request::WorkspaceCreate,
        Request::WriteSession,
        Request::WorkspaceAbort,
        Request::UploadAdmission,
        Request::SealWorkspace,
        Request::PublicCommit,
        Request::WorkspaceRead,
        Request::WorkspaceCapacity,
        Request::UploadStatus,
        Request::Snapshot,
        Request::Version,
        Request::VersionAuthorization,
        Request::RepositorySnapshot,
        Request::RepositoriesSnapshot,
        Request::BranchSnapshot,
        Request::Resolve,
        Request::CommitSnapshot,
        Request::HistoryGraph,
        Request::EventPage,
        Request::ActivityHistorySnapshot,
        Request::TopologySnapshot,
        Request::FileHistory,
        Request::FileHistorySnapshot,
        Request::FileHistoryNodeParents,
        Request::FileHistoryBranches,
        Request::FileHistoryMerges,
        Request::FileHistoryMergeInputs,
        Request::FileHistoryMergeParents,
        Request::FileHistoryNodeField,
        Request::FileVersionSources,
        Request::DefaultDirectoryBranch,
        Request::Listing,
        Request::DirectoryScopes,
        Request::DirectorySelection,
        Request::SelectDefaultDirectoryBranch,
        Request::RenameDirectoryBranch,
        Request::CreateFileBranch,
        Request::CreateFileBranchFromNode,
        Request::FileNote,
        Request::VersionNote,
        Request::NodeNote,
        Request::BackupConfiguration,
        Request::ConfigureBackups,
        Request::Plan,
        Request::PlanCancel,
        Request::PlanCommit,
        Request::PrepareHistoricalSave,
        Request::PreparePurge,
        Request::PrepareSplit,
        Request::RegisterDisk,
        Request::NamespaceBatch,
        Request::SelectFileBranch,
        Request::SelectFileBranchAt,
        Request::RenameFileBranch,
        Request::DiskDirectoryCreate,
        Request::DiskDirectorySelect,
        Request::InitializeDirectoryScope,
        Request::ArchiveBranch,
        Request::CreateBranch,
        Request::CreateMergeLink,
        Request::DiskStatus,
        Request::IntegrityCheck,
        Request::CompareCommits,
        Request::ProtectionPolicy,
        Request::PlacementRule,
        Request::UpdateProtectionPolicy,
        Request::PlacementSnapshot,
        Request::SmbBindingStatus,
        Request::UpdateSmbBinding,
        Request::BranchesSnapshot,
        Request::RepositoryAuthorization,
        Request::Namespace,
        Request::Content,
        Request::RangeContent,
        Request::ContextContent,
        Request::WorkspaceContent,
        Request::Checksums,
        Request::Upload,
        Request::UploadGeneration,
    ] {
        assert_eq!(
            request
                .execute(&fixture, fixture.backend.as_ref())
                .unwrap_err()
                .code,
            storage_contract::ErrorCode::Unauthorized
        );
    }
    assert_eq!(fixture.snapshot(), before);
}

#[test]
fn private_disk_sampling_cannot_grant_public_monitoring_access() {
    let fixture = Fixture::new(true);
    let before = fixture.snapshot();
    let observed = fixture
        .backend
        .observe_disks(&crate::InternalDiskObservation(()))
        .unwrap();
    assert_eq!(observed.len(), 3);
    assert!(fixture.snapshot() == before);

    let synthetic = Identity {
        admission_session: None,
        smb_binding: None,
        principal: "zxs-system-telemetry".into(),
    };
    assert_eq!(
        fixture.backend.disk_status(&synthetic).unwrap_err().code,
        storage_contract::ErrorCode::Unauthorized
    );
    fixture.auth.lock().unwrap().logout(&fixture.token).unwrap();
    assert_eq!(
        fixture
            .backend
            .disk_status(&fixture.identity)
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::Unauthorized
    );
    let before = fixture.snapshot();
    assert!(
        fixture
            .backend
            .observe_disks(&crate::InternalDiskObservation(()))
            .is_ok()
    );
    assert_eq!(fixture.snapshot(), before);
}

#[test]
fn both_upload_adapters_bind_the_copied_snapshot_before_acceptance() {
    for explicit_generation in [false, true] {
        let fixture = Fixture::new(true);
        let before = fixture.snapshot();
        let item = new_id("entry");
        let source =
            crate::UploadSource::from_file(std::fs::File::open(&fixture.source).unwrap(), u64::MAX)
                .unwrap();
        // Simulate mutation through the original inode after transport hashing,
        // not merely a pathname replacement. Copying must fail before acceptance.
        std::fs::write(&fixture.source, b"bad upload").unwrap();
        let result = if explicit_generation {
            fixture.backend.upload_generation(
                &fixture.identity,
                &fixture.workspace.workspace_id,
                &item,
                &name_policy::PortablePath::parse("bound.txt").unwrap(),
                &source,
                1,
            )
        } else {
            fixture.backend.upload(
                &fixture.identity,
                &fixture.workspace.workspace_id,
                &item,
                &name_policy::PortablePath::parse("bound.txt").unwrap(),
                &source,
            )
        };
        assert_eq!(
            result.unwrap_err().code,
            storage_contract::ErrorCode::ChecksumMismatch
        );
        assert_eq!(fixture.snapshot(), before);
    }
}

#[test]
fn managed_reads_use_committed_authority_without_waiting_for_the_writer_mutex() {
    let fixture = Arc::new(Fixture::new(false));
    fixture
        .backend
        .set_management_auth(crate::lock(&fixture.auth).reader())
        .unwrap();
    let mut writer = fixture.auth.lock().unwrap();
    let (done, result) = std::sync::mpsc::channel();
    let worker = std::thread::spawn({
        let fixture = Arc::clone(&fixture);
        move || {
            done.send(
                [Request::Tree, Request::Snapshot, Request::Content]
                    .into_iter()
                    .map(|request| request.execute(&fixture, fixture.backend.as_ref()))
                    .collect::<Vec<_>>(),
            )
            .unwrap();
        }
    });
    let reads = result.recv_timeout(std::time::Duration::from_secs(5));
    writer.logout(&fixture.token).unwrap();
    drop(writer);
    worker.join().unwrap();
    for read in reads.expect("committed reads must not acquire the credential writer mutex") {
        read.unwrap();
    }
    for request in [Request::Tree, Request::Snapshot, Request::Content] {
        assert_eq!(
            request
                .execute(&fixture, fixture.backend.as_ref())
                .unwrap_err()
                .status,
            401
        );
    }
}

#[test]
fn typed_workspace_abort_checks_owner_before_changing_workspace_or_journal() {
    let fixture = Fixture::new(true);
    let foreign = fixture
        .backend
        .engine
        .lock()
        .unwrap()
        .create_workspace(&fixture.repo.main_branch_id, "different-owner")
        .unwrap();
    let before = fixture.snapshot();
    assert_eq!(
        fixture
            .backend
            .abort_workspace(&fixture.identity, &foreign.workspace_id)
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::Forbidden
    );
    assert_eq!(fixture.snapshot(), before);
    let engine = fixture.backend.engine.lock().unwrap();
    assert_eq!(
        json_codec::to_value(engine.workspace_view(&foreign.workspace_id).unwrap()).unwrap(),
        json_codec::to_value(&foreign).unwrap()
    );
}
