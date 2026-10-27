use super::*;
use http_routing::{body::Body, http::StatusCode};
use service_stack::ServiceExt;
use std::sync::{
    Condvar,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

struct Gate {
    open: Mutex<bool>,
    changed: Condvar,
    started: tokio::sync::Semaphore,
    calls: AtomicUsize,
    finished: tokio::sync::Semaphore,
}
impl Gate {
    fn release(&self) {
        *self.open.lock().unwrap() = true;
        self.changed.notify_all();
    }
}
// Always unblock workers if a test assertion fails, too.
struct Release(Arc<Gate>);
impl Drop for Release {
    fn drop(&mut self) {
        self.0.release();
    }
}
#[derive(Clone, Copy)]
enum BlockedEndpoint {
    Backend,
    Capabilities,
    Ready,
    Capacity,
    UploadAuthorization,
    UploadPublication,
}
struct BackendProbe {
    blocked: BlockedEndpoint,
    gate: Arc<Gate>,
    outcome: u8,
    auth: Mutex<Option<Arc<Mutex<AuthStore>>>>,
}
impl BackendProbe {
    fn wait(&self) -> Result<(), ApiError> {
        self.gate.calls.fetch_add(1, Ordering::SeqCst);
        self.gate.started.add_permits(1);
        let open = self.gate.open.lock().unwrap();
        drop(self.gate.changed.wait_while(open, |open| !*open).unwrap());
        self.gate.finished.add_permits(1);
        match self.outcome {
            1 => Err(ApiError::conflict(
                storage_contract::ErrorCode::InjectedFailure,
                "backend error",
            )),
            2 => panic!("test backend panic"),
            _ => Ok(()),
        }
    }
}
impl Backend for BackendProbe {
    fn operation(
        &self,
        _: &crate::auth::Identity,
        _: storage_contract::OperationId,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
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
    ) -> Result<storage_contract::Workspace, ApiError> {
        self.wait()?;
        // This probe exercises queued error completion, not workspace storage.
        Err(ApiError::unsupported())
    }
    fn abort_workspace(
        &self,
        _identity: &crate::auth::Identity,
        _workspace: &str,
    ) -> Result<(), ApiError> {
        self.wait()
    }

    fn workspace(
        &self,
        _identity: &crate::auth::Identity,
        _workspace: &str,
    ) -> Result<storage_contract::Workspace, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn published_upload_status(
        &self,
        _identity: &crate::auth::Identity,
        _workspace: &str,
        _item: &str,
    ) -> Result<Option<crate::PublishedUploadStatus>, crate::ApiError> {
        self.wait()?;
        Ok(Some(crate::PublishedUploadStatus {
            generation: 1,
            logical_size: 0,
            content_hash: storage_contract::ContentHash::from_digest(blake3::hash(b"")),
            state: crate::PublishedUploadState::Complete,
        }))
    }
    fn seal_workspace(
        &self,
        _identity: &crate::auth::Identity,
        _workspace: &str,
    ) -> Result<storage_contract::SealedWorkspace, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn commit_workspace_public(
        &self,
        _identity: &crate::auth::Identity,
        _command: storage_contract::PublicCommitEnvelope,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn version_snapshot(
        &self,
        _identity: &crate::auth::Identity,
        _version: &str,
    ) -> Result<crate::VersionSnapshot, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn authorize_version(
        &self,
        _identity: &crate::auth::Identity,
        _version: &str,
    ) -> Result<(), crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn repository_snapshot(
        &self,
        _identity: &crate::auth::Identity,
        _id: &str,
    ) -> Result<storage_contract::Repository, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn authorize_repository(
        &self,
        _identity: &crate::auth::Identity,
        _id: &str,
    ) -> Result<(), crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn capabilities(&self) -> Result<crate::Capabilities, crate::ApiError> {
        if matches!(self.blocked, BlockedEndpoint::Capabilities) {
            self.wait()?;
        }
        Ok(crate::Capabilities::for_engine(4096))
    }
    fn ready(&self) -> Result<crate::Readiness, crate::ApiError> {
        if matches!(self.blocked, BlockedEndpoint::Ready) {
            self.wait()?;
        }
        Ok(crate::Readiness::Managed {
            history_index: crate::HistoryIndexStatus::Ready,
        })
    }
    fn workspace_capacity(
        &self,
        _identity: &crate::auth::Identity,
    ) -> Result<storage_contract::WorkspaceCapacity, crate::ApiError> {
        self.wait()?;
        Ok(probe_capacity())
    }
    fn smb_binding_status(
        &self,
        _identity: &crate::auth::Identity,
        _id: &str,
    ) -> Result<crate::SmbBindingStatus, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn update_smb_binding(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::UpdateSmbBinding<'_>,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn protection_policy(
        &self,
        _identity: &crate::auth::Identity,
        _id: &str,
    ) -> Result<storage_contract::ProtectionPolicy, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn placement_rule(
        &self,
        _identity: &crate::auth::Identity,
        _id: &str,
    ) -> Result<storage_contract::PlacementRule, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn update_protection_policy(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::UpdateProtectionPolicy,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn update_placement_rule(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::UpdatePlacementRule,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn compare_commits(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::CompareCommits<'_>,
    ) -> Result<storage_contract::CommitComparison, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn disk_status(
        &self,
        _identity: &crate::auth::Identity,
    ) -> Result<Vec<storage_contract::DiskStatus>, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn integrity_check(
        &self,
        _identity: &crate::auth::Identity,
    ) -> Result<storage_contract::CheckReport, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn create_merge_link(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::CreateMergeLink,
    ) -> Result<storage_contract::MergeLink, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn create_repository(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::CreateRepository,
    ) -> Result<storage_contract::Repository, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn create_branch(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::CreateBranch,
    ) -> Result<crate::BranchSnapshot, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn create_file_branch(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::CreateFileBranch,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn update_note(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::UpdateNote,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn merge_file_branches(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::MergeFileBranches,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn plan(
        &self,
        _identity: &crate::auth::Identity,
        _plan: &str,
    ) -> Result<storage_contract::Plan, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn cancel_plan(
        &self,
        _identity: &crate::auth::Identity,
        _request: Result<crate::CancelPlan, crate::ApiError>,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn backup_configuration(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::ReadBackupConfiguration,
    ) -> Result<storage_contract::BackupConfigurationSnapshot, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn configure_backups(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::ConfigureBackups,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn register_disk(
        &self,
        _identity: &crate::auth::Identity,
    ) -> Result<std::convert::Infallible, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn prepare_historical_save(
        &self,
        _identity: &crate::auth::Identity,
        _request: Result<crate::PrepareHistoricalSave, crate::ApiError>,
    ) -> Result<storage_contract::Plan, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn prepare_purge(
        &self,
        _identity: &crate::auth::Identity,
        _request: Result<crate::PreparePurge, crate::ApiError>,
    ) -> Result<storage_contract::Plan, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn prepare_split(
        &self,
        _identity: &crate::auth::Identity,
        _request: Result<crate::PrepareSplit, crate::ApiError>,
    ) -> Result<storage_contract::Plan, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn commit_plan(
        &self,
        _identity: &crate::auth::Identity,
        _request: Result<crate::CommitPlan, crate::ApiError>,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn update_file_node_tags(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::UpdateFileNodeTags,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn create_disk_directory(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::CreateDiskDirectory,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn create_disk_write_session(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::CreateDiskWriteSession,
    ) -> Result<crate::DiskWriteSession, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn namespace_batch(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::NamespaceBatch,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn select_file_branch(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::SelectFileBranch,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn rename_file_branch(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::RenameFileBranch,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn listing(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::ReadListing,
    ) -> Result<crate::ListingSnapshot, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn mutate_disk_directory(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::MutateDiskDirectory,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn initialize_directory_scope(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::InitializeDirectoryScope,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn archive_branch(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::ArchiveBranch,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn rename_directory_branch(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::RenameDirectoryBranch,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn select_default_directory_branch(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::SelectDefaultDirectoryBranch,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn directory_selection(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::ReadDirectorySelection,
    ) -> Result<crate::DirectorySelectionSnapshot, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn directory_scopes(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::ReadDirectoryScopes<'_>,
    ) -> Result<crate::DirectoryScopesSnapshot, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn default_directory_branch(
        &self,
        _identity: &crate::auth::Identity,
        _repository_id: &str,
    ) -> Result<crate::DefaultDirectoryBranchSnapshot, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn file_history_snapshot(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::ReadFileHistory,
    ) -> Result<crate::FileHistorySnapshot, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn file_history(
        &self,
        _identity: &crate::auth::Identity,
        _file: &str,
    ) -> Result<Vec<storage_contract::FileVersion>, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn topology_snapshot(
        &self,
        _identity: &crate::auth::Identity,
    ) -> Result<crate::TopologySnapshot, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn placement_snapshot(
        &self,
        _identity: &crate::auth::Identity,
        _id: &str,
    ) -> Result<storage_contract::Placement, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn event_page(
        &self,
        _identity: &crate::auth::Identity,
        _after: u64,
    ) -> Result<crate::EventPage, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn activity_history(
        &self,
        _identity: &crate::auth::Identity,
    ) -> Result<crate::ActivityHistorySnapshot, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn history_graph(
        &self,
        _identity: &crate::auth::Identity,
        _id: &str,
    ) -> Result<crate::HistoryGraphSnapshot, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn commit_snapshot(
        &self,
        _identity: &crate::auth::Identity,
        _id: &str,
    ) -> Result<std::sync::Arc<storage_contract::Commit>, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn resolve_entry(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::ResolveEntry<'_>,
    ) -> Result<crate::ResolvedEntry, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn branch_snapshot(
        &self,
        _identity: &crate::auth::Identity,
        _id: &str,
    ) -> Result<crate::BranchSnapshot, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn branches_snapshot(
        &self,
        _identity: &crate::auth::Identity,
        _id: &str,
    ) -> Result<Vec<crate::BranchSnapshot>, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn repositories_snapshot(
        &self,
        _identity: &crate::auth::Identity,
    ) -> Result<Vec<storage_contract::Repository>, crate::ApiError> {
        self.wait()?;
        Ok(Vec::new())
    }
    fn tree_snapshot(
        &self,
        _identity: &crate::auth::Identity,
        _request: crate::ReadTree<'_>,
    ) -> Result<crate::TreeSnapshot, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn create_write_session(
        &self,
        _identity: &crate::auth::Identity,
        _branch: &str,
    ) -> Result<crate::WorkspaceWriteSession, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn stage_namespace(
        &self,
        _identity: &crate::auth::Identity,
        _command: crate::StageNamespace,
    ) -> Result<storage_contract::Workspace, crate::ApiError> {
        self.wait()?;
        Err(crate::ApiError::unsupported())
    }
    fn authorize_upload(
        &self,
        _identity: &crate::auth::Identity,
        _workspace: &str,
        _logical_size: Option<u64>,
    ) -> Result<(), crate::ApiError> {
        match self.blocked {
            BlockedEndpoint::UploadAuthorization => self.wait(),
            BlockedEndpoint::UploadPublication => Ok(()),
            BlockedEndpoint::Backend
            | BlockedEndpoint::Capabilities
            | BlockedEndpoint::Ready
            | BlockedEndpoint::Capacity => Err(crate::ApiError::unsupported()),
        }
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

    fn start_management(&self, auth: Arc<Mutex<AuthStore>>) {
        *self.auth.lock().unwrap() = Some(auth);
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
        self.wait()?;
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
        self.wait()?;
        Ok(probe_content())
    }
    fn workspace_content(
        &self,
        identity: &Identity,
        _: &str,
        _: &str,
    ) -> Result<Content, ApiError> {
        self.content(identity, "version")
    }
    fn checksums(&self, _: &Identity, _: &str) -> Result<Vec<u8>, ApiError> {
        self.wait()?;
        Ok(vec![1, 2, 3])
    }
    fn retry_storage(&self, _: &Identity) -> Result<StorageStatus, ApiError> {
        self.wait()?;
        Err(ApiError::unsupported())
    }
    fn setup_operation(
        &self,
        _: &'static str,
        _: Value,
        _: Option<&str>,
        _: Option<&Identity>,
    ) -> Result<Value, ApiError> {
        self.wait()?;
        Err(ApiError::unsupported())
    }
}
fn fixture(root: &Path, outcome: u8, open: bool) -> (Router, Arc<Gate>, Release) {
    fixture_for(root, outcome, open, BlockedEndpoint::Backend)
}
fn fixture_for(
    root: &Path,
    outcome: u8,
    open: bool,
    blocked: BlockedEndpoint,
) -> (Router, Arc<Gate>, Release) {
    let gate = Arc::new(Gate {
        open: Mutex::new(open),
        changed: Condvar::new(),
        started: tokio::sync::Semaphore::new(0),
        calls: AtomicUsize::new(0),
        finished: tokio::sync::Semaphore::new(0),
    });
    let mut auth = AuthStore::initialize(root, "admin", "a-long-test-password").unwrap();
    let token = auth.login("admin", "a-long-test-password").unwrap().token;
    let app = router(
        Arc::new(BackendProbe {
            blocked,
            gate: gate.clone(),
            outcome,
            auth: Mutex::new(None),
        }),
        auth,
        ServerConfig::loopback(root),
    )
    .unwrap()
    .layer(http_routing::middleware::from_fn(
        move |mut request: Request, next: http_routing::middleware::Next| {
            let token = token.clone();
            async move {
                request
                    .headers_mut()
                    .insert("authorization", format!("Bearer {token}").parse().unwrap());
                next.run(request).await
            }
        },
    ));
    (app, gate.clone(), Release(gate))
}
async fn request(app: Router, path: &str) -> Response {
    app.oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
        .await
        .unwrap()
}
async fn overloaded(app: Router, path: &str) {
    let response = tokio::time::timeout(Duration::from_secs(5), request(app, path))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    let body = http_routing::body::to_bytes(response.into_body(), 8192)
        .await
        .unwrap();
    let body: Value = json_codec::from_slice(&body).unwrap();
    assert_eq!(body["error"]["code"], "RESOURCE_LIMIT_EXCEEDED");
    assert_eq!(body["error"]["retryable"], true);
}

#[tokio::test]
async fn backend_admission_saturates_without_waiters_and_survives_http_cancellation() {
    let root = crate::tests::private_tempdir();
    let (app, gate, _release) = fixture(root.path(), 0, false);
    let mut jobs = Vec::new();
    for _ in 0..HTTP_BACKEND_SLOTS {
        let app = app.clone();
        jobs.push(tokio::spawn(async move {
            request(app, "/api/repositories").await
        }));
    }
    tokio::time::timeout(
        Duration::from_secs(5),
        gate.started.acquire_many(HTTP_BACKEND_SLOTS as u32),
    )
    .await
    .unwrap()
    .unwrap()
    .forget();
    overloaded(app.clone(), "/api/repositories").await;
    overloaded(app.clone(), "/api/workspaces/ws/files/item/upload-status").await;
    overloaded(app.clone(), "/api/repositories").await;
    assert_eq!(
        request(app.clone(), "/readyz").await.status(),
        StatusCode::OK
    );
    assert_eq!(
        request(app.clone(), "/api/capabilities").await.status(),
        StatusCode::OK
    );
    assert_eq!(
        request(app.clone(), "/healthz").await.status(),
        StatusCode::OK
    );
    assert_eq!(gate.calls.load(Ordering::SeqCst), HTTP_BACKEND_SLOTS);
    let cancelled = jobs.pop().unwrap();
    cancelled.abort();
    assert!(cancelled.await.unwrap_err().is_cancelled());
    overloaded(app.clone(), "/api/repositories").await;
    assert_eq!(gate.calls.load(Ordering::SeqCst), HTTP_BACKEND_SLOTS);
    gate.release();
    for job in jobs {
        assert_eq!(job.await.unwrap().status(), StatusCode::OK);
    }
    gate.finished
        .acquire_many(HTTP_BACKEND_SLOTS as u32)
        .await
        .unwrap()
        .forget();
    *gate.open.lock().unwrap() = false;
    // Refill the entire pool, proving even the cancelled request's job releases
    // its permit. Retry only until that job has actually returned, not by sleep.
    let mut jobs = Vec::new();
    for _ in 0..HTTP_BACKEND_SLOTS {
        let app = app.clone();
        jobs.push(tokio::spawn(async move {
            loop {
                let response = request(app.clone(), "/api/repositories").await;
                if response.status() != StatusCode::TOO_MANY_REQUESTS {
                    return response;
                }
                tokio::task::yield_now().await;
            }
        }));
    }
    tokio::time::timeout(
        Duration::from_secs(5),
        gate.started.acquire_many(HTTP_BACKEND_SLOTS as u32),
    )
    .await
    .unwrap()
    .unwrap()
    .forget();
    overloaded(app, "/api/repositories").await;
    gate.release();
    for job in jobs {
        assert_eq!(job.await.unwrap().status(), StatusCode::OK);
    }
    assert_eq!(gate.calls.load(Ordering::SeqCst), HTTP_BACKEND_SLOTS * 2);
}

#[tokio::test]
async fn backend_admission_releases_after_success_error_and_panic() {
    for (outcome, status) in [
        (0, StatusCode::OK),
        (1, StatusCode::CONFLICT),
        (2, StatusCode::INTERNAL_SERVER_ERROR),
    ] {
        let root = crate::tests::private_tempdir();
        let (app, gate, _release) = fixture(root.path(), outcome, true);
        for _ in 0..HTTP_BACKEND_SLOTS * 2 {
            assert_eq!(
                request(app.clone(), "/api/repositories").await.status(),
                status
            );
        }
        assert_eq!(gate.calls.load(Ordering::SeqCst), HTTP_BACKEND_SLOTS * 2);
    }
}

// A single worker makes accidental synchronous lock waits observable. The
// holder has an independent wall-clock escape so regressions cannot hang CI.
#[tokio::test(flavor = "current_thread")]
async fn management_status_remains_live_during_authentication_contention() {
    let root = crate::tests::private_tempdir();
    let gate = Arc::new(Gate {
        open: Mutex::new(true),
        changed: Condvar::new(),
        started: tokio::sync::Semaphore::new(0),
        calls: AtomicUsize::new(0),
        finished: tokio::sync::Semaphore::new(0),
    });
    let backend = Arc::new(BackendProbe {
        blocked: BlockedEndpoint::Backend,
        gate,
        outcome: 0,
        auth: Mutex::new(None),
    });
    let auth = AuthStore::initialize(root.path(), "admin", "a-long-test-password").unwrap();
    let app = router(backend.clone(), auth, ServerConfig::loopback(root.path())).unwrap();
    let auth = backend.auth.lock().unwrap().clone().unwrap();
    let held = Arc::new(tokio::sync::Semaphore::new(0));
    let (release, wait) = std::sync::mpsc::channel();
    let holder = std::thread::spawn({
        let auth = auth.clone();
        let held = held.clone();
        move || {
            let _guard = auth.lock().unwrap();
            held.add_permits(1);
            let _ = wait.recv_timeout(Duration::from_secs(3));
        }
    });
    held.acquire().await.unwrap().forget();
    let status = tokio::spawn(request(app.clone(), "/api/management/status"));
    let health = tokio::spawn(request(app.clone(), "/healthz"));
    tokio::time::sleep(Duration::from_millis(20)).await;
    // Both requests and this independent timer progress before releasing auth.
    assert!(matches!(
        auth.try_lock(),
        Err(std::sync::TryLockError::WouldBlock)
    ));
    assert!(status.is_finished());
    assert!(health.is_finished());
    assert_eq!(health.await.unwrap().status(), StatusCode::OK);
    let response = status.await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = http_routing::body::to_bytes(response.into_body(), 8192)
        .await
        .unwrap();
    let value: Value = json_codec::from_slice(&bytes).unwrap();
    assert_eq!(value["authentication"], "busy");
    release.send(()).unwrap();
    holder.join().unwrap();
    let response = request(app, "/api/management/status").await;
    let bytes = http_routing::body::to_bytes(response.into_body(), 8192)
        .await
        .unwrap();
    let value: Value = json_codec::from_slice(&bytes).unwrap();
    assert_eq!(value["authentication"], "ready");
}

#[tokio::test]
async fn upload_status_retains_read_admission_after_cancellation() {
    let root = crate::tests::private_tempdir();
    let (app, gate, _release) = fixture(root.path(), 0, false);
    let path = "/api/workspaces/ws/files/item/upload-status";
    let mut jobs = Vec::new();
    for _ in 0..2 {
        jobs.push(tokio::spawn(request(app.clone(), path)));
    }
    tokio::time::timeout(Duration::from_secs(5), gate.started.acquire_many(2))
        .await
        .unwrap()
        .unwrap()
        .forget();
    overloaded(app.clone(), path).await;
    let canceled = jobs.pop().unwrap();
    canceled.abort();
    assert!(canceled.await.unwrap_err().is_cancelled());
    overloaded(app.clone(), path).await;
    assert_eq!(gate.calls.load(Ordering::SeqCst), 2);
    assert_eq!(request(app, "/healthz").await.status(), StatusCode::OK);
    gate.release();
    assert_eq!(jobs.pop().unwrap().await.unwrap().status(), StatusCode::OK);
    gate.finished.acquire_many(2).await.unwrap().forget();
    assert_eq!(
        gate.calls.load(Ordering::SeqCst),
        2,
        "one typed snapshot per request"
    );
}

fn workspace_body(method: &str, path: &str) -> Value {
    if path == "/api/workspaces" {
        json!({"branch_id":"branch"})
    } else if method == "PUT" && path.starts_with("/api/policies/") {
        json!({"repository_id":"repository","path_prefix":"Folder","target_copies":3,"expected_generation":"0","operation_id":"op_00000000000000000000000000000000"})
    } else if method == "PUT" && path.starts_with("/api/primary-placement-rules/") {
        json!({"repository_id":"repository","path_prefix":"Folder","primary_slot_id":"P01","expected_generation":"0","operation_id":"op_00000000000000000000000000000000"})
    } else if method == "PUT" && path.ends_with("/default-branch") {
        json!({"directory_branch_id":"directory","operation_id":"op_00000000000000000000000000000000"})
    } else if method == "PUT" && path.ends_with("/smb-binding") {
        json!({"branch_id":"branch","enabled":false,"expected_history_generation":"1","operation_id":"op_00000000000000000000000000000000"})
    } else if path.ends_with("/merge-links") {
        json!({"source_branch_id":"source","source_commit_id":"source_commit","target_branch_id":"target","target_commit_id":"target_commit","operation_id":"op_00000000000000000000000000000000"})
    } else if (method == "POST" && path.ends_with("/metadata"))
        || (method == "PATCH" && path.contains("/nodes/"))
    {
        json!({"note":"","operation_id":"op_00000000000000000000000000000000"})
    } else if method == "POST" && path.ends_with("/select") {
        json!({"branch_id":"branch","operation_id":"op_00000000000000000000000000000000"})
    } else if method == "PUT" && path.ends_with("/backup-configuration") {
        json!({"path":"","target_copies":2,"backup_prefixes":{"B01":"backup"},"expected_generation":"0","expected_scope_id":"scope","operation_id":"op_00000000000000000000000000000000"})
    } else if method == "POST" && path.starts_with("/api/plans/") && path.ends_with("/commit") {
        json!({"operation_id":"op_00000000000000000000000000000000","plan_digest":"digest"})
    } else if method == "POST" && path.ends_with("/cancel") {
        json!({"operation_id":"op_00000000000000000000000000000000"})
    } else if method == "POST" && path.ends_with("/namespace-batch") {
        json!({"kind":"delete","items":[],"operation_id":"op_00000000000000000000000000000000"})
    } else if method == "POST" && path.ends_with("/merge") {
        json!({"inputs":[],"chosen_node_id":"node","surviving_branch_id":"branch","confirmed":true,"operation_id":"op_00000000000000000000000000000000"})
    } else if method == "POST" && path.ends_with("/tags") {
        json!({"targets":[],"add":[],"remove":[],"operation_id":"op_00000000000000000000000000000000"})
    } else if method == "POST" && path == "/api/file-manager/disks/P01/namespace" {
        json!({"kind":"mkdir","path":"folder","operation_id":"op_00000000000000000000000000000000"})
    } else if method == "POST" && path == "/api/file-manager/disks/P01/write-sessions" {
        json!({"path":"file.txt"})
    } else if method == "POST" && path == "/api/history-trees/history/historical-save-plans" {
        json!({"source_commit_id":"commit","name":"Saved branch"})
    } else if method == "POST" && path == "/api/history-trees/history/purge-plans" {
        json!({"version_ids":["version"]})
    } else if method == "POST" && path == "/api/history-trees/history/split-plans" {
        json!({"cut_commit_id":"cut"})
    } else if method == "POST" && path == "/api/repositories" {
        json!({"name":"Repository","primary_slot_id":"P01","target_copies":3,"operation_id":"op_00000000000000000000000000000000"})
    } else if method == "POST"
        && path.starts_with("/api/file-manager/files/")
        && path.ends_with("/branches")
    {
        json!({"source_version_id":"version","name":"Side","operation_id":"op_00000000000000000000000000000000"})
    } else if method == "POST" && path.ends_with("/branches") {
        json!({"source_commit_id":"source","name":"Side","operation_id":"op_00000000000000000000000000000000"})
    } else if path.starts_with("/api/file-manager/disks/")
        && (path.ends_with("/directory-branches") || path.ends_with("/directory-select"))
    {
        json!({"name":"Side","operation_id":"op_00000000000000000000000000000000"})
    } else if path.ends_with("/directory-scopes") {
        json!({"path":"","operation_id":"op_00000000000000000000000000000000"})
    } else if path.ends_with("/archive") {
        json!({"operation_id":"op_00000000000000000000000000000000"})
    } else if method == "PATCH"
        && (path.contains("/directory-branches/")
            || (path.starts_with("/api/file-manager/files/") && path.contains("/branches/")))
    {
        json!({"name":"renamed","operation_id":"op_00000000000000000000000000000000"})
    } else if method == "POST" && path.ends_with("/directory-branches") {
        json!({"path":"","sources":[],"entries":[]})
    } else if path == "/api/compare" {
        json!({"left_commit_id":"left","right_commit_id":"right"})
    } else if path.ends_with("/resolve") {
        json!({"path":"file.txt"})
    } else if path.ends_with("/namespace-operations") {
        json!({"kind":"mkdir","path":"directory"})
    } else if path.ends_with("/commit") {
        json!({"workspace_id":"workspace","operation_id":"op_00000000000000000000000000000000","repository_id":"repo","branch_id":"branch","expected_head":"head","fencing_token":"1","message":"","requested_protection":"strict","file_placement":{},"expected_topology_generation":"1","sealed_manifest_hash":"manifest","request_digest":"digest"})
    } else {
        json!({})
    }
}

async fn workspace_request(app: Router, method: &str, path: &str) -> Response {
    let body = workspace_body(method, path);
    app.oneshot(
        Request::builder()
            .method(method)
            .uri(path)
            .header("content-type", "application/json")
            .body(Body::from(json_codec::to_vec(&body).unwrap()))
            .unwrap(),
    )
    .await
    .unwrap()
}

const TYPED_BACKEND_ROUTES: &[(&str, &str)] = &[
    ("POST", "/api/branches/branch/write-sessions"),
    ("POST", "/api/branches/branch/resolve"),
    ("GET", "/api/versions/version"),
    ("GET", "/api/branches/branch/tree?commit_id=commit"),
    ("GET", "/api/repositories/repository"),
    ("GET", "/api/branches/branch"),
    ("GET", "/api/file-manager/files/file/history"),
    ("POST", "/api/file-manager/files/file/metadata"),
    ("POST", "/api/file-manager/versions/version/metadata"),
    ("PATCH", "/api/file-manager/files/file/nodes/node"),
    ("POST", "/api/file-manager/files/file/merge"),
    ("POST", "/api/file-manager/files/file/tags"),
    ("POST", "/api/file-manager/disks/P01/namespace"),
    ("POST", "/api/file-manager/disks/P01/write-sessions"),
    ("GET", "/api/repositories/repo/backup-configuration"),
    ("PUT", "/api/repositories/repo/backup-configuration"),
    ("GET", "/api/plans/plan"),
    ("POST", "/api/plans/plan/cancel"),
    ("POST", "/api/plans/plan/commit"),
    ("POST", "/api/file-manager/disks/P01/namespace-batch"),
    ("POST", "/api/file-manager/files/file/select"),
    ("POST", "/api/file-manager/files/file/branches"),
    ("PATCH", "/api/file-manager/files/file/branches/branch"),
    ("POST", "/api/file-manager/disks/P01/directory-branches"),
    ("POST", "/api/file-manager/disks/P01/directory-select"),
    ("POST", "/api/repositories/repo/directory-scopes"),
    ("POST", "/api/branches/branch/archive"),
    ("POST", "/api/repositories/repo/branches"),
    ("POST", "/api/repositories"),
    ("POST", "/api/history-trees/history/merge-links"),
    ("GET", "/api/commits/commit"),
    ("GET", "/api/history-trees/history/graph"),
    ("GET", "/api/events?after=17"),
    ("GET", "/api/file-manager/history?ignored=true"),
    ("GET", "/api/backup-topologies/default"),
    ("GET", "/api/files/file/history"),
    ("GET", "/api/repositories/repository/default-branch"),
    ("GET", "/api/repositories/repository/directory-branches"),
    ("POST", "/api/repositories/repository/directory-branches"),
    (
        "PATCH",
        "/api/repositories/repository/directory-branches/directory",
    ),
    (
        "GET",
        "/api/repositories/repository/directory-branches?summary=true&path=",
    ),
    ("PUT", "/api/repositories/repository/default-branch"),
    ("GET", "/api/admin/disks"),
    ("GET", "/api/file-manager/disks"),
    ("GET", "/api/file-manager/disks/P01/entries"),
    ("GET", "/api/admin/check"),
    ("POST", "/api/compare"),
    ("GET", "/api/policies/policy"),
    ("GET", "/api/primary-placement-rules/rule"),
    ("PUT", "/api/policies/policy"),
    ("PUT", "/api/primary-placement-rules/rule"),
    ("GET", "/api/versions/version/placement"),
    ("GET", "/api/repositories/repository/smb-binding"),
    ("PUT", "/api/repositories/repository/smb-binding"),
    ("GET", "/api/repositories/repository/branches"),
    ("GET", "/api/operations/op_00000000000000000000000000000000"),
    ("POST", "/api/workspaces"),
    ("POST", "/api/workspaces/workspace/abort"),
    ("POST", "/api/workspaces/workspace/seal"),
    ("POST", "/api/workspaces/workspace/namespace-operations"),
    ("POST", "/api/workspaces/workspace/commit"),
    ("GET", "/api/workspaces/workspace"),
    ("POST", "/api/history-trees/history/historical-save-plans"),
    ("POST", "/api/history-trees/history/purge-plans"),
    ("POST", "/api/history-trees/history/split-plans"),
    ("POST", "/api/admin/disks"),
];

#[tokio::test]
async fn typed_workspace_jobs_retain_admission_until_blocking_completion_after_cancellation() {
    for &(method, path) in TYPED_BACKEND_ROUTES {
        let root = crate::tests::private_tempdir();
        let (app, gate, _release) = fixture(root.path(), 0, false);
        let mut jobs = Vec::new();
        for _ in 0..HTTP_BACKEND_SLOTS {
            jobs.push(tokio::spawn(workspace_request(app.clone(), method, path)));
        }
        tokio::time::timeout(
            Duration::from_secs(5),
            gate.started.acquire_many(HTTP_BACKEND_SLOTS as u32),
        )
        .await
        .unwrap_or_else(|error| panic!("typed route {path} did not enter its backend: {error}"))
        .unwrap()
        .forget();
        assert_eq!(
            workspace_request(app.clone(), method, path).await.status(),
            StatusCode::TOO_MANY_REQUESTS
        );
        let cancelled = jobs.pop().unwrap();
        cancelled.abort();
        assert!(cancelled.await.unwrap_err().is_cancelled());
        assert_eq!(
            workspace_request(app.clone(), method, path).await.status(),
            StatusCode::TOO_MANY_REQUESTS
        );
        assert_eq!(gate.calls.load(Ordering::SeqCst), HTTP_BACKEND_SLOTS);
        gate.release();
        for job in jobs {
            assert_ne!(job.await.unwrap().status(), StatusCode::TOO_MANY_REQUESTS);
        }
        gate.finished
            .acquire_many(HTTP_BACKEND_SLOTS as u32)
            .await
            .unwrap()
            .forget();
        loop {
            let response = workspace_request(app.clone(), method, path).await;
            if response.status() != StatusCode::TOO_MANY_REQUESTS {
                break;
            }
            tokio::task::yield_now().await;
        }
    }
}

#[tokio::test]
async fn typed_management_jobs_hold_separate_slots_after_http_cancellation() {
    for (blocked, path) in [
        (BlockedEndpoint::Capabilities, "/api/capabilities"),
        (BlockedEndpoint::Ready, "/readyz"),
        (BlockedEndpoint::Capacity, "/api/workspaces/capacity"),
    ] {
        let root = crate::tests::private_tempdir();
        let (app, gate, _release) = fixture_for(root.path(), 0, false, blocked);
        let mut jobs = Vec::new();
        for _ in 0..2 {
            jobs.push(tokio::spawn(request(app.clone(), path)));
        }
        tokio::time::timeout(Duration::from_secs(5), gate.started.acquire_many(2))
            .await
            .unwrap()
            .unwrap()
            .forget();
        overloaded(app.clone(), path).await;
        let cancelled = jobs.pop().unwrap();
        cancelled.abort();
        assert!(cancelled.await.unwrap_err().is_cancelled());
        overloaded(app.clone(), path).await;
        assert_eq!(
            request(app.clone(), "/healthz").await.status(),
            StatusCode::OK
        );
        assert_eq!(gate.calls.load(Ordering::SeqCst), 2);
        gate.release();
        assert_eq!(jobs.pop().unwrap().await.unwrap().status(), StatusCode::OK);
        gate.finished.acquire_many(2).await.unwrap().forget();
        loop {
            let response = request(app.clone(), path).await;
            if response.status() != StatusCode::TOO_MANY_REQUESTS {
                assert_eq!(response.status(), StatusCode::OK);
                break;
            }
            tokio::task::yield_now().await;
        }
    }
}

fn probe_capacity() -> storage_contract::WorkspaceCapacity {
    let measure = storage_contract::CapacityMeasure {
        used: 1,
        limit: 8,
        remaining: 7,
    };
    let scope = storage_contract::WorkspaceCapacityScope {
        records: measure.clone(),
        metadata_bytes: measure,
    };
    storage_contract::WorkspaceCapacity {
        global: scope.clone(),
        principal: scope,
        warning: storage_contract::CapacityWarning::Ok,
        automatic_retirement: false,
    }
}

#[tokio::test]
async fn typed_jobs_hold_auth_owner_until_cancelled_worker_finishes() {
    for (method, path) in TYPED_BACKEND_ROUTES.iter().copied().chain([
        ("GET", "/api/repositories"),
        ("GET", "/api/workspaces/capacity"),
        ("POST", "/api/system/storage/retry"),
        ("GET", "/api/workspaces/workspace/files/item/upload-status"),
        ("GET", "/api/versions/version/content"),
        ("GET", "/api/workspaces/workspace/files/item/content"),
        ("GET", "/api/versions/version/checksums"),
    ]) {
        assert_cancelled_job_keeps_auth(method, path, BlockedEndpoint::Backend).await;
    }
}

#[tokio::test]
async fn upload_jobs_hold_auth_until_cancelled_worker_finishes() {
    for blocked in [
        BlockedEndpoint::UploadAuthorization,
        BlockedEndpoint::UploadPublication,
    ] {
        assert_cancelled_job_keeps_auth(
            "PUT",
            "/api/workspaces/workspace/files/entry_00000000000000000000000000000000/content?path=file.txt",
            blocked,
        ).await;
    }
}

#[cfg(feature = "core")]
#[tokio::test]
async fn recovery_jobs_hold_auth_until_cancelled_worker_finishes() {
    assert_cancelled_job_keeps_auth(
        "POST",
        "/api/management/offline-check",
        BlockedEndpoint::Backend,
    )
    .await;
}

async fn assert_cancelled_job_keeps_auth(method: &str, path: &str, blocked: BlockedEndpoint) {
    let root = crate::tests::private_tempdir();
    let gate = Arc::new(Gate {
        open: Mutex::new(false),
        changed: Condvar::new(),
        started: tokio::sync::Semaphore::new(0),
        calls: AtomicUsize::new(0),
        finished: tokio::sync::Semaphore::new(0),
    });
    let _release = Release(gate.clone());
    let backend = Arc::new(BackendProbe {
        blocked,
        gate: gate.clone(),
        outcome: 0,
        auth: Mutex::new(None),
    });
    let mut auth = AuthStore::initialize(root.path(), "admin", "a-long-test-password").unwrap();
    let token = auth.login("admin", "a-long-test-password").unwrap().token;
    let app = router(backend.clone(), auth, ServerConfig::loopback(root.path())).unwrap();
    let owner = backend.auth.lock().unwrap().take().unwrap();
    let weak = Arc::downgrade(&owner);
    drop(owner);
    let upload = matches!(
        blocked,
        BlockedEndpoint::UploadAuthorization | BlockedEndpoint::UploadPublication
    );
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {token}"));
    let body = if upload {
        request = request
            .header("content-length", "3")
            .header("x-zxs-content-hash", blake3::hash(b"abc").to_hex().as_str());
        Body::from("abc")
    } else {
        Body::from(json_codec::to_vec(&workspace_body(method, path)).unwrap())
    };
    let job = tokio::spawn(app.oneshot(request.body(body).unwrap()));
    tokio::time::timeout(Duration::from_secs(5), gate.started.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    job.abort();
    assert!(job.await.unwrap_err().is_cancelled());
    let retained = weak.upgrade().is_some();
    gate.release();
    assert!(retained, "{path}: worker must retain authentication owner");
    tokio::time::timeout(Duration::from_secs(5), async {
        while weak.upgrade().is_some() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

fn probe_content() -> Content {
    let mut file = tempfile::tempfile().unwrap();
    std::io::Write::write_all(&mut file, b"abc").unwrap();
    Content {
        file,
        size: 3,
        hash: blake3::hash(b"abc").to_hex().to_string(),
        checksum_manifest: None,
        fallback_files: Vec::new(),
        lease: Box::new(()),
        download_name: None,
    }
}

#[tokio::test]
async fn completed_http_bodies_do_not_retain_auth_owner() {
    for path in [
        "/api/versions/version/content",
        "/api/workspaces/workspace/files/item/content",
        "/api/versions/version/checksums",
    ] {
        let root = crate::tests::private_tempdir();
        let gate = Arc::new(Gate {
            open: Mutex::new(true),
            changed: Condvar::new(),
            started: tokio::sync::Semaphore::new(0),
            calls: AtomicUsize::new(0),
            finished: tokio::sync::Semaphore::new(0),
        });
        let backend = Arc::new(BackendProbe {
            blocked: BlockedEndpoint::Backend,
            gate,
            outcome: 0,
            auth: Mutex::new(None),
        });
        let mut auth = AuthStore::initialize(root.path(), "admin", "a-long-test-password").unwrap();
        let token = auth.login("admin", "a-long-test-password").unwrap().token;
        let app = router(backend.clone(), auth, ServerConfig::loopback(root.path())).unwrap();
        let owner = backend.auth.lock().unwrap().take().unwrap();
        let weak = Arc::downgrade(&owner);
        drop(owner);
        let response = app
            .oneshot(
                Request::builder()
                    .uri(path)
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        tokio::time::timeout(Duration::from_secs(5), async {
            while weak.upgrade().is_some() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let body = http_routing::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        assert_eq!(body.len(), 3);
    }
}

#[tokio::test]
async fn completed_content_keeps_read_permit_but_not_auth_owner() {
    let root = crate::tests::private_tempdir();
    let auth = Arc::new(Mutex::new(
        AuthStore::initialize(root.path(), "admin", "a-long-test-password").unwrap(),
    ));
    let weak = Arc::downgrade(&auth);
    let backend = Arc::new(BackendProbe {
        blocked: BlockedEndpoint::Backend,
        gate: Arc::new(Gate {
            open: Mutex::new(true),
            changed: Condvar::new(),
            started: tokio::sync::Semaphore::new(0),
            calls: AtomicUsize::new(0),
            finished: tokio::sync::Semaphore::new(0),
        }),
        outcome: 0,
        auth: Mutex::new(None),
    });
    let limits = TransferLimits {
        slots: 1,
        ..TransferLimits::default()
    };
    let slots = transfer_limits::Admission::new(&limits);
    let content = http_job::HttpBackendJob::new(backend, auth, slots.acquire().unwrap())
        .run_with_permit(|_, permit| {
            let mut content = probe_content();
            content.lease = Box::new((content.lease, permit));
            Ok(content)
        })
        .await
        .unwrap();
    assert!(weak.upgrade().is_none());
    assert!(slots.acquire().is_err());
    drop(content);
    drop(slots.acquire().unwrap());
}

#[tokio::test]
async fn backend_job_releases_auth_and_admission_after_success_error_and_panic() {
    for outcome in 0..=2 {
        let root = crate::tests::private_tempdir();
        let auth = Arc::new(Mutex::new(
            AuthStore::initialize(root.path(), "admin", "a-long-test-password").unwrap(),
        ));
        let weak = Arc::downgrade(&auth);
        let backend = Arc::new(BackendProbe {
            blocked: BlockedEndpoint::Backend,
            gate: Arc::new(Gate {
                open: Mutex::new(true),
                changed: Condvar::new(),
                started: tokio::sync::Semaphore::new(0),
                calls: AtomicUsize::new(0),
                finished: tokio::sync::Semaphore::new(0),
            }),
            outcome,
            auth: Mutex::new(None),
        });
        let slots = Arc::new(tokio::sync::Semaphore::new(1));
        let result = http_job::HttpBackendJob::new(
            backend.clone(),
            auth,
            slots.clone().try_acquire_owned().unwrap(),
        )
        .run(move |_| backend.wait())
        .await;
        assert_eq!(result.is_ok(), outcome == 0);
        assert!(weak.upgrade().is_none());
        assert_eq!(slots.available_permits(), 1);
    }
}

#[test]
fn queued_backend_job_keeps_auth_and_admission_after_cancellation() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .unwrap();
    runtime.block_on(async {
        let root = crate::tests::private_tempdir();
        let auth = Arc::new(Mutex::new(
            AuthStore::initialize(root.path(), "admin", "a-long-test-password").unwrap(),
        ));
        let weak = Arc::downgrade(&auth);
        let gate = Arc::new(Gate {
            open: Mutex::new(false),
            changed: Condvar::new(),
            started: tokio::sync::Semaphore::new(0),
            calls: AtomicUsize::new(0),
            finished: tokio::sync::Semaphore::new(0),
        });
        let _release = Release(gate.clone());
        let backend = Arc::new(BackendProbe {
            blocked: BlockedEndpoint::Backend,
            gate: gate.clone(),
            outcome: 0,
            auth: Mutex::new(None),
        });
        let blocker = tokio::task::spawn_blocking({
            let backend = backend.clone();
            move || backend.wait()
        });
        tokio::time::timeout(Duration::from_secs(5), gate.started.acquire())
            .await
            .unwrap()
            .unwrap()
            .forget();
        let slots = Arc::new(tokio::sync::Semaphore::new(1));
        let waiting = http_job::HttpBackendJob::new(
            backend.clone(),
            auth,
            slots.clone().try_acquire_owned().unwrap(),
        )
        .run(move |_| backend.wait());
        let waiter = tokio::spawn(waiting);
        waiter.abort();
        assert!(waiter.await.unwrap_err().is_cancelled());
        let retained = weak.upgrade().is_some();
        let available = slots.available_permits();
        let entered = gate.calls.load(Ordering::SeqCst);
        gate.release();
        assert!(retained);
        assert_eq!(available, 0);
        assert_eq!(entered, 1, "authorized work must still be queued");
        blocker.await.unwrap().unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while weak.upgrade().is_some() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(slots.available_permits(), 1);
        assert_eq!(gate.calls.load(Ordering::SeqCst), 2);
    });
}

#[tokio::test]
async fn prepare_unknown_fields_reach_backend_before_its_admission_error() {
    for path in [
        "/api/history-trees/missing/historical-save-plans",
        "/api/history-trees/missing/purge-plans",
        "/api/history-trees/missing/split-plans",
    ] {
        let root = crate::tests::private_tempdir();
        let (app, gate, _release) = fixture(root.path(), 1, true);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"unknown":1}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT, "{path}");
        let bytes = http_routing::body::to_bytes(response.into_body(), 8192)
            .await
            .unwrap();
        let error: Value = json_codec::from_slice(&bytes).unwrap();
        assert_eq!(error["error"]["code"], "INJECTED_FAILURE", "{path}");
        assert_eq!(gate.calls.load(Ordering::SeqCst), 1, "{path}");
    }
}
