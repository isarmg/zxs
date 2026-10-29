use super::*;
use crate::{ApiError, Backend, Content, auth::Identity};
use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

struct WriterProbe {
    stopped: AtomicBool,
    failure: Option<&'static str>,
    entered: Mutex<Option<oneshot::Sender<()>>>,
    release: Mutex<mpsc::Receiver<()>>,
    done: Mutex<Option<oneshot::Sender<()>>>,
}
impl Backend for WriterProbe {
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

    fn stop_management(&self) {
        self.stopped.store(true, Ordering::SeqCst);
    }
    fn drain_management(&self) -> Result<(), ApiError> {
        assert!(self.stopped.load(Ordering::SeqCst));
        self.entered
            .lock()
            .unwrap()
            .take()
            .unwrap()
            .send(())
            .unwrap();
        self.release.lock().unwrap().recv_timeout(DEADLINE).unwrap();
        self.done.lock().unwrap().take().unwrap().send(()).unwrap();
        match self.failure {
            Some(message) => Err(ApiError::io(io::Error::other(message))),
            None => Ok(()),
        }
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
fn writer() -> (
    Arc<WriterProbe>,
    oneshot::Receiver<()>,
    mpsc::Sender<()>,
    oneshot::Receiver<()>,
) {
    let (entered, started) = oneshot::channel();
    let (release, released) = mpsc::channel();
    let (done, finished) = oneshot::channel();
    (
        Arc::new(WriterProbe {
            stopped: AtomicBool::new(false),
            failure: None,
            entered: Mutex::new(Some(entered)),
            release: Mutex::new(released),
            done: Mutex::new(Some(done)),
        }),
        started,
        release,
        finished,
    )
}

#[tokio::test]
async fn shutdown_waits_for_native_writer_after_http_has_drained() {
    let (backend, started, release, finished) = writer();
    let (stop, shutdown) = oneshot::channel();
    let server = tokio::spawn(run_with_management(
        |stopped| async move {
            let _ = stopped.await;
            Ok(())
        },
        pending(),
        async move {
            let _ = shutdown.await;
        },
        ManagementShutdown::new(backend.clone()),
    ));
    stop.send(()).unwrap();
    notice(started).await;
    assert!(backend.stopped.load(Ordering::SeqCst));
    assert!(!server.is_finished());
    release.send(()).unwrap();
    notice(finished).await;
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn http_error_still_drains_native_writer_before_returning_error() {
    let (backend, started, release, finished) = writer();
    let server = tokio::spawn(run_with_management(
        |_| async { Err(io::Error::other("HTTP failed")) },
        pending(),
        pending(),
        ManagementShutdown::new(backend),
    ));
    notice(started).await;
    assert!(!server.is_finished());
    release.send(()).unwrap();
    notice(finished).await;
    assert_eq!(
        server.await.unwrap().unwrap_err().to_string(),
        "HTTP failed"
    );
}

#[tokio::test]
async fn cancelled_service_keeps_native_drain_in_runtime_owned_blocking_work() {
    let (backend, started, release, finished) = writer();
    let (bridge, bridge_started, bridge_dropped) = bridge();
    let server = tokio::spawn(run_with_management(
        |_| pending(),
        bridge,
        pending(),
        ManagementShutdown::new(backend.clone()),
    ));
    notice(bridge_started).await;
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
    notice(bridge_dropped).await;
    notice(started).await;
    assert!(backend.stopped.load(Ordering::SeqCst));
    release.send(()).unwrap();
    notice(finished).await;
}

#[test]
fn guard_drop_without_runtime_drains_synchronously_without_panicking() {
    let (backend, mut started, release, mut finished) = writer();
    release.send(()).unwrap();
    drop(ManagementShutdown::new(backend.clone()));
    assert!(backend.stopped.load(Ordering::SeqCst));
    assert!(started.try_recv().is_ok());
    assert!(finished.try_recv().is_ok());
}

#[test]
fn discarded_queued_blocking_closure_still_owns_native_drain() {
    let (backend, mut started, release, mut finished) = writer();
    backend.stop_management();
    release.send(()).unwrap();
    let mut drain = NativeDrain::new(backend);
    // Model cancellation before Tokio polls a queued blocking closure. Merely
    // capturing the backend Arc would detach its native writer at this point.
    let queued = move || drain.run();
    drop(queued);
    assert!(started.try_recv().is_ok());
    assert!(finished.try_recv().is_ok());
}

#[test]
fn closed_runtime_rejecting_blocking_enqueue_still_drains_native_writer() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let handle = runtime.handle().clone();
    drop(runtime);
    let _entered_runtime = handle.enter();
    let (backend, mut started, release, mut finished) = writer();
    release.send(()).unwrap();
    // try_current succeeds, but the stopped blocking pool discards this job.
    drop(ManagementShutdown::new(backend.clone()));
    assert!(backend.stopped.load(Ordering::SeqCst));
    assert!(started.try_recv().is_ok());
    assert!(finished.try_recv().is_ok());
}

#[test]
fn failed_drain_is_returned_or_reported_once_after_cancellation() {
    use std::io::{Read, Seek};
    let mut capture = tempfile::tempfile().unwrap();
    let subscriber = tracing::collector::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(Arc::new(capture.try_clone().unwrap()))
        .finish();
    // Detached blocking outputs may be dropped on any runtime worker.
    tracing::subscriber::set_global_default(subscriber).unwrap();

    for (cancel, marker) in [
        (false, "lifecycle-returned-drain-failure"),
        (true, "lifecycle-detached-drain-failure"),
    ] {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (mut backend, started, release, finished) = writer();
        Arc::get_mut(&mut backend).unwrap().failure = Some(marker);
        runtime.block_on(async {
            let mut management = ManagementShutdown::new(backend);
            let task = tokio::spawn(async move { management.finish().await });
            notice(started).await;
            if cancel {
                task.abort();
                assert!(task.await.unwrap_err().is_cancelled());
                release.send(()).unwrap();
            } else {
                release.send(()).unwrap();
                let error = task.await.unwrap().unwrap_err();
                let source = error.get_ref().unwrap().downcast_ref::<ApiError>().unwrap();
                assert!(format!("{source:?}").contains(marker));
            }
            notice(finished).await;
        });
        // Runtime teardown waits for the detached native drain and its output.
        drop(runtime);
    }

    for (claim_result, marker) in [
        (true, "lifecycle-fallback-returned-failure"),
        (false, "lifecycle-fallback-unclaimed-failure"),
    ] {
        let (runtime, _) = failing_enqueue_runtime();
        let (mut backend, mut started, release, mut finished) = writer();
        Arc::get_mut(&mut backend).unwrap().failure = Some(marker);
        let mut management = ManagementShutdown::new(backend);
        management.smb.begin();
        release.send(()).unwrap();
        runtime.block_on(async {
            if claim_result {
                let error = management.finish().await.unwrap_err();
                let source = error.get_ref().unwrap().downcast_ref::<ApiError>().unwrap();
                assert!(format!("{source:?}").contains(marker));
            } else {
                use std::panic::{AssertUnwindSafe, catch_unwind};
                assert!(
                    catch_unwind(AssertUnwindSafe(move || {
                        let _management = management;
                        panic!("unclaimed fallback result during unwind");
                    }))
                    .is_err()
                );
            }
            assert!(started.try_recv().is_ok());
            assert!(finished.try_recv().is_ok());
            tokio::task::spawn_blocking(|| ()).await.unwrap();
        });
        drop(runtime);
    }

    let runtime = tokio::runtime::Builder::new_current_thread()
        .max_blocking_threads(1)
        .build()
        .unwrap();
    let (occupied, occupied_rx) = mpsc::channel();
    let (unblock, unblock_rx) = mpsc::channel();
    let blocker = runtime.spawn_blocking(move || {
        occupied.send(()).unwrap();
        unblock_rx.recv_timeout(DEADLINE).unwrap();
    });
    occupied_rx.recv_timeout(DEADLINE).unwrap();
    let (mut backend, mut started, release, mut finished) = writer();
    Arc::get_mut(&mut backend).unwrap().failure = Some("lifecycle-queued-drain-failure");
    release.send(()).unwrap();
    {
        let _entered = runtime.enter();
        let mut management = ManagementShutdown::new(backend);
        management.begin();
        management.drain.as_ref().unwrap().abort();
        drop(management);
    }
    unblock.send(()).unwrap();
    runtime.block_on(blocker).unwrap();
    drop(runtime);
    assert!(started.try_recv().is_ok());
    assert!(finished.try_recv().is_ok());

    let (mut backend, mut started, release, mut finished) = writer();
    Arc::get_mut(&mut backend).unwrap().failure = Some("lifecycle-no-runtime-drain-failure");
    release.send(()).unwrap();
    drop(ManagementShutdown::new(backend));
    assert!(started.try_recv().is_ok());
    assert!(finished.try_recv().is_ok());

    capture.rewind().unwrap();
    let mut logged = String::new();
    capture.read_to_string(&mut logged).unwrap();
    assert_eq!(
        logged.matches("lifecycle-returned-drain-failure").count(),
        0,
        "{logged}"
    );
    assert_eq!(
        logged.matches("lifecycle-detached-drain-failure").count(),
        1,
        "{logged}"
    );
    assert_eq!(
        logged.matches("lifecycle-queued-drain-failure").count(),
        1,
        "{logged}"
    );
    assert_eq!(
        logged
            .matches("lifecycle-fallback-returned-failure")
            .count(),
        0,
        "{logged}"
    );
    assert_eq!(
        logged
            .matches("lifecycle-fallback-unclaimed-failure")
            .count(),
        1,
        "{logged}"
    );
    assert_eq!(
        logged.matches("lifecycle-no-runtime-drain-failure").count(),
        1,
        "{logged}"
    );
}

#[test]
fn simultaneous_http_and_management_failures_keep_both_concrete_causes() {
    let error = combine_shutdown_results(
        Err(io::Error::new(
            io::ErrorKind::ConnectionReset,
            "HTTP failure",
        )),
        Err(io::Error::other(ApiError::io(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "management failure",
        )))),
    )
    .unwrap_err();
    let combined = error
        .get_ref()
        .unwrap()
        .downcast_ref::<HttpAndManagementError>()
        .unwrap();
    assert_eq!(combined.http.kind(), io::ErrorKind::ConnectionReset);
    assert!(combined.management.get_ref().unwrap().is::<ApiError>());
    assert!(format!("{combined:?}").contains("management failure"));
}

fn failing_enqueue_runtime() -> (tokio::runtime::Runtime, Arc<std::sync::atomic::AtomicUsize>) {
    use std::sync::atomic::AtomicUsize;
    let attempts = Arc::new(AtomicUsize::new(0));
    let names = Arc::clone(&attempts);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .thread_name_fn(move || {
            if names.fetch_add(1, Ordering::SeqCst) == 0 {
                panic!("injected management enqueue failure after queue insertion");
            }
            "management-drain-test".into()
        })
        .build()
        .unwrap();
    (runtime, attempts)
}

#[test]
fn enqueue_failure_during_owner_unwind_drains_once_and_preserves_outer_panic() {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    let (runtime, attempts) = failing_enqueue_runtime();
    let (backend, mut started, release, mut finished) = writer();
    let mut management = ManagementShutdown::new(backend.clone());
    // Close the independent, unused SMB owner outside the runtime so the
    // failing real enqueue below belongs specifically to management drain.
    management.smb.begin();
    release.send(()).unwrap();
    runtime.block_on(async {
        let outer = catch_unwind(AssertUnwindSafe(move || {
            let _management = management;
            panic!("outer management owner unwind");
        }))
        .unwrap_err();
        assert_eq!(
            outer.downcast_ref::<&str>(),
            Some(&"outer management owner unwind")
        );
        assert!(started.try_recv().is_ok());
        assert!(finished.try_recv().is_ok());
        assert_eq!(Arc::strong_count(&backend), 1);
        // A subsequent real blocking thread consumes the queued old guard;
        // the probe would panic if it tried to drain a second time.
        assert_eq!(tokio::task::spawn_blocking(|| 42).await.unwrap(), 42);
    });
    drop(runtime);
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
}

#[test]
fn competing_management_guards_wait_for_drain_completion_before_returning() {
    let (backend, started, release, mut finished) = writer();
    backend.stop_management();
    let state = Arc::new(Mutex::new(NativeDrain::new(backend)));
    let first = ManagementDrain(Arc::clone(&state));
    let worker = std::thread::spawn(move || drop(first));
    started.blocking_recv().unwrap();
    let second = ManagementDrain(Arc::clone(&state));
    let (entered, entering) = mpsc::channel();
    let (returned, completed) = mpsc::channel();
    let fallback = std::thread::spawn(move || {
        entered.send(()).unwrap();
        drop(second);
        returned.send(()).unwrap();
    });
    entering.recv_timeout(DEADLINE).unwrap();
    assert_eq!(
        completed.recv_timeout(Duration::from_millis(20)),
        Err(mpsc::RecvTimeoutError::Timeout)
    );
    release.send(()).unwrap();
    completed.recv_timeout(DEADLINE).unwrap();
    worker.join().unwrap();
    fallback.join().unwrap();
    assert!(finished.try_recv().is_ok());
    let mut state = state.lock().unwrap();
    assert!(state.backend.is_none());
    state.completion.take().unwrap().0.take().unwrap().unwrap();
}
