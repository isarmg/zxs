//! Deterministic account-publication races at the auth -> engine boundary.
use super::*;
use crate::{AuthorizedBackend, BackendAction, auth::AuthStore};
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
    fn operation(
        &self,
        identity: &Identity,
        operation_id: storage_contract::OperationId,
    ) -> Result<storage_contract::OperationResult, ApiError> {
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
    ) -> Result<storage_contract::CommitComparison, crate::ApiError> {
        self.enter();
        self.inner.compare_commits(identity, request)
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
    fn file_history(
        &self,
        identity: &crate::auth::Identity,
        file: &str,
    ) -> Result<Vec<storage_contract::FileVersion>, crate::ApiError> {
        self.enter();
        self.inner.file_history(identity, file)
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
    fn history_graph(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<crate::HistoryGraphSnapshot, crate::ApiError> {
        self.enter();
        self.inner.history_graph(identity, id)
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
    ) -> Result<crate::TreeSnapshot, crate::ApiError> {
        self.enter();
        self.inner.tree_snapshot(identity, request)
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

    fn call(
        &self,
        action: BackendAction,
        identity: &Identity,
        input: Value,
    ) -> Result<Value, ApiError> {
        self.enter();
        self.inner.call(action, identity, input)
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
                "Authorization",
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

#[derive(Clone, Copy, Debug)]
enum Request {
    Operation,
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
    TopologySnapshot,
    FileHistory,
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
            Self::Operation => backend.operation(identity, fixture.operation).map(|_| ()),
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
                .call(
                    BackendAction::CreateRepository,
                    identity,
                    json!({
                        "name":"Forbidden new repository","primary_slot_id":"P01","target_copies":3,
                        "operation_id":new_id("op")
                    }),
                )
                .map(|_| ()),
            Self::Tree => backend
                .tree_snapshot(
                    identity,
                    crate::ReadTree {
                        branch: &fixture.repo.main_branch_id,
                        commit: None,
                    },
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
                )
                .map(|_| ()),
            Self::DiskStatus => backend.disk_status(identity).map(|_| ()),
            Self::IntegrityCheck => backend.integrity_check(identity).map(|_| ()),
            Self::FileHistory => backend.file_history(identity, "missing-file").map(|_| ()),
            Self::TopologySnapshot => backend.topology_snapshot(identity).map(|_| ()),
            Self::PlacementSnapshot => backend
                .placement_snapshot(identity, &fixture.version)
                .map(|_| ()),
            Self::EventPage => backend.event_page(identity, u64::MAX).map(|_| ()),
            Self::HistoryGraph => backend
                .history_graph(identity, &fixture.repo.history_tree_id)
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
            let requests = vec![
                Request::Operation,
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
                Request::EventPage,
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
        Request::TopologySnapshot,
        Request::FileHistory,
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
        json_codec::to_value(engine.workspace_ref(&foreign.workspace_id).unwrap()).unwrap(),
        json_codec::to_value(&foreign).unwrap()
    );
}
