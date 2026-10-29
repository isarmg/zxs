use super::*;
use json_codec::{Value, json};

#[test]
fn unavailable_and_invalid_samples_are_not_fabricated_zero_usage() {
    assert_eq!(normalized_cpu_usage(f32::NAN), None);
    assert_eq!(normalized_cpu_usage(f32::INFINITY), None);
    assert_eq!(normalized_cpu_usage(150.0), Some(100.0));
    assert!(memory_status(0, 0).total_bytes.is_none());
    assert_eq!(
        memory_status(u64::MAX, u64::MAX - 123).used_bytes,
        Some(123)
    );
}

#[test]
fn real_host_sampler_starts_with_a_baseline_and_reads_memory() {
    let mut sampler = SystemSampler::new();
    let (cpu, memory) = sampler.sample();
    assert!(cpu.usage_percent.is_none());
    assert!(cpu.logical_count > 0);
    assert!(memory.total_bytes.is_some());
    std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
    assert!(
        sampler
            .sample()
            .0
            .usage_percent
            .is_some_and(|usage| (0.0..=100.0).contains(&usage))
    );
}

struct ObserveGate {
    entered: std::sync::mpsc::Sender<()>,
    release: std::sync::mpsc::Receiver<()>,
}

#[derive(Default)]
struct MonitorBackend {
    observe_gate: Mutex<Option<ObserveGate>>,
    // Receiver disconnection observes destruction without fallible Drop code.
    _dropped: Option<std::sync::mpsc::Sender<()>>,
}

impl Backend for MonitorBackend {
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
        let gate = self.observe_gate.lock().unwrap().take();
        if let Some(gate) = gate {
            gate.entered.send(()).unwrap();
            gate.release.recv_timeout(Duration::from_secs(15)).unwrap();
        }
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
    fn content(&self, _: &Identity, _: &str) -> Result<crate::Content, ApiError> {
        Err(ApiError::unsupported())
    }
}

#[test]
fn unavailable_cache_falls_back_to_bounded_memory_without_losing_live_metrics() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("cache");
    std::fs::write(&directory, b"unwritable cache path").unwrap();
    let service = Telemetry::start(Arc::new(MonitorBackend::default()), &directory).unwrap();
    let live = service.current().unwrap();
    assert!(live.cpu.logical_count > 0);
    assert!(live.memory.total_bytes.is_some());
    let history = service.history.lock().unwrap();
    let now = date_time::OffsetDateTime::now_utc().unix_timestamp();
    assert!(HistoryWriter::open(&directory, now).is_err());
    assert!(!history.samples(now - 60, now, 5).is_empty());
    assert_eq!(std::fs::read(directory).unwrap(), b"unwritable cache path");
}

#[cfg(target_os = "linux")]
#[test]
fn read_only_sysfs_cache_falls_back_to_live_memory_monitoring() {
    let service = Telemetry::start(
        Arc::new(MonitorBackend::default()),
        Path::new("/sys/zxs-telemetry-cache-unit-test"),
    )
    .unwrap();
    let now = date_time::OffsetDateTime::now_utc().unix_timestamp();
    assert!(HistoryWriter::open(Path::new("/sys/zxs-telemetry-cache-unit-test"), now).is_err());
    assert!(
        !service
            .history
            .lock()
            .unwrap()
            .samples(now - 60, now, 5)
            .is_empty()
    );
    assert!(service.current().unwrap().memory.total_bytes.is_some());
}

#[test]
fn supported_ranges_bound_chart_points_and_reject_other_strings() {
    for range in ["1min", "30min", "1h", "24h", "1month"] {
        let (seconds, interval) = json_codec::from_value::<HistoryRange>(json!(range))
            .unwrap()
            .parameters();
        assert!(seconds / interval <= 360);
    }
    assert!(json_codec::from_value::<HistoryRange>(json!("1year")).is_err());
    assert!(json_codec::from_value::<HistoryRange>(json!("../1h")).is_err());
}

#[test]
fn active_periodic_refresh_releases_weak_service_and_backend_after_completion() {
    use std::sync::mpsc;

    let root = tempfile::tempdir().unwrap();
    let (dropped_tx, dropped_rx) = mpsc::channel();
    let backend = Arc::new(MonitorBackend {
        observe_gate: Mutex::new(None),
        _dropped: Some(dropped_tx),
    });
    let weak_backend = Arc::downgrade(&backend);
    let service = Telemetry::start(backend.clone(), &root.path().join("cache")).unwrap();
    let weak_service = Arc::downgrade(&service);
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    *backend.observe_gate.lock().unwrap() = Some(ObserveGate {
        entered: entered_tx,
        release: release_rx,
    });
    drop(backend);
    // Wait for the real periodic thread, without timing a guessed sleep or
    // inserting a production hook. Its active refresh temporarily owns service.
    entered_rx.recv_timeout(Duration::from_secs(15)).unwrap();
    drop(service);
    assert!(weak_service.upgrade().is_some());
    assert!(weak_backend.upgrade().is_some());
    release_tx.send(()).unwrap();
    assert!(matches!(
        dropped_rx.recv_timeout(Duration::from_secs(15)),
        Err(mpsc::RecvTimeoutError::Disconnected)
    ));
    assert!(weak_service.upgrade().is_none());
    assert!(weak_backend.upgrade().is_none());
    // This proves resource release after the active refresh, not thread join or
    // a shutdown flush: the native worker has no exposed JoinHandle.
}

#[test]
fn monitoring_rechecks_the_current_session_after_logout() {
    let root = crate::tests::private_tempdir();
    let mut auth =
        crate::auth::AuthStore::initialize(root.path(), "admin", "monitor-test-password").unwrap();
    let token = auth.login("admin", "monitor-test-password").unwrap().token;
    let captured = auth.authenticate(&token).unwrap();
    let auth = Mutex::new(auth);
    assert!(require_current_session(&auth, &captured).is_ok());
    auth.lock().unwrap().logout(&token).unwrap();
    assert_eq!(
        require_current_session(&auth, &captured).unwrap_err().code,
        storage_contract::ErrorCode::Unauthorized
    );
}

#[tokio::test]
async fn telemetry_admission_survives_cancellation_without_consuming_management_capacity() {
    use http_routing::{
        Router,
        body::Body,
        http::{Request, StatusCode},
    };
    use service_stack::ServiceExt;
    async fn request(app: &Router, path: &str, token: &str) -> StatusCode {
        app.clone()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
            .status()
    }
    let root = crate::tests::private_tempdir();
    let config = crate::ServerConfig::loopback(root.path());
    config.prepare_spool().unwrap();
    let backend = Arc::new(MonitorBackend::default());
    let service = Telemetry::start(backend.clone(), &root.path().join("cache")).unwrap();
    let mut auth =
        crate::auth::AuthStore::initialize(root.path(), "admin", "a-long-test-password").unwrap();
    let token = auth.login("admin", "a-long-test-password").unwrap().token;
    let spool_budget = crate::http_spool::HttpSpool::open_blocking(&config);
    let app = crate::router_shared(
        backend,
        Arc::new(Mutex::new(auth)),
        config.admit().unwrap(),
        service.clone(),
        spool_budget,
    )
    .unwrap();
    let started = Arc::new(tokio::sync::Semaphore::new(0));
    let mut releases = Vec::new();
    let mut jobs = Vec::new();
    for _ in 0..2 {
        let (release, wait) = std::sync::mpsc::channel();
        releases.push(release);
        let started = started.clone();
        jobs.push(tokio::spawn(telemetry_job(
            service.request_slots.clone(),
            move || {
                started.add_permits(1);
                wait.recv_timeout(Duration::from_secs(10))
                    .expect("invariant: test releases blocked telemetry worker");
                Ok(())
            },
        )));
    }
    tokio::time::timeout(Duration::from_secs(3), started.acquire_many(2))
        .await
        .unwrap()
        .unwrap()
        .forget();
    for path in ["/api/system/status", "/api/system/history?range=1min"] {
        assert_eq!(
            request(&app, path, &token).await,
            StatusCode::TOO_MANY_REQUESTS
        );
    }
    let canceled = jobs.pop().unwrap();
    canceled.abort();
    assert!(canceled.await.unwrap_err().is_cancelled());
    assert_eq!(service.request_slots.available_permits(), 0);
    assert_eq!(
        request(&app, "/api/system/status", &token).await,
        StatusCode::TOO_MANY_REQUESTS
    );
    for path in ["/healthz", "/api/management/status"] {
        assert_eq!(request(&app, path, &token).await, StatusCode::OK);
    }
    for release in releases {
        release.send(()).unwrap();
    }
    jobs.pop().unwrap().await.unwrap().unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while service.request_slots.available_permits() != 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    for path in ["/api/system/status", "/api/system/history?range=1min"] {
        assert_eq!(request(&app, path, &token).await, StatusCode::OK);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn rejected_symlink_cache_preserves_target_and_management_across_restarts() {
    use http_routing::{body::Body, http::Request};
    use service_stack::ServiceExt;
    use std::os::unix::fs::{PermissionsExt, symlink};

    let root = crate::tests::private_tempdir();
    let target = root.path().join("external-cache-target");
    std::fs::create_dir(&target).unwrap();
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o750)).unwrap();
    let sentinel = target.join("sentinel");
    std::fs::write(&sentinel, b"preserve external contents").unwrap();
    std::fs::set_permissions(&sentinel, std::fs::Permissions::from_mode(0o640)).unwrap();
    let cache = root.path().join("telemetry-cache");
    symlink(&target, &cache).unwrap();
    let web = root.path().join("web");
    std::fs::create_dir(&web).unwrap();
    std::fs::write(web.join("index.html"), b"management page").unwrap();
    let auth_root = root.path().join("auth");
    drop(crate::auth::AuthStore::initialize(&auth_root, "admin", "monitor-test-password").unwrap());

    for _ in 0..3 {
        // Exercise fallback directly, including a history write, before
        // constructing the complete router with the same rejected cache.
        let service = Telemetry::start(Arc::new(MonitorBackend::default()), &cache).unwrap();
        let now = date_time::OffsetDateTime::now_utc().unix_timestamp();
        assert!(HistoryWriter::open(&cache, now).is_err());
        assert!(
            !service
                .history
                .lock()
                .unwrap()
                .samples(now - 60, now, 5)
                .is_empty()
        );
        assert!(service.current().unwrap().memory.total_bytes.is_some());
        drop(service);

        let mut config = crate::ServerConfig::loopback(root.path());
        config.web_dir = Some(web.clone());
        let app = crate::router(
            Arc::new(MonitorBackend::default()),
            crate::auth::AuthStore::open(&auth_root).unwrap(),
            config,
        )
        .expect("rejected optional cache must not block the management router");
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/auth/login")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"username":"admin","password":"monitor-test-password"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let login: Value = json_codec::from_slice(
            &http_routing::body::to_bytes(response.into_body(), 8192)
                .await
                .unwrap(),
        )
        .unwrap();
        let token = login["token"].as_str().unwrap();
        for path in [
            "/",
            "/healthz",
            "/api/management/status",
            "/api/auth/me",
            "/api/system/status",
            "/api/system/history?range=1min",
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(path)
                        .header("authorization", format!("Bearer {token}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), 200, "{path}");
            // Complete response lifetimes before reopening the same directories.
            http_routing::body::to_bytes(response.into_body(), 1024 * 1024)
                .await
                .unwrap();
        }
        drop(app);
        assert_eq!(std::fs::read_link(&cache).unwrap(), target);
        assert_eq!(
            std::fs::read(&sentinel).unwrap(),
            b"preserve external contents"
        );
        assert_eq!(
            std::fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o750
        );
        assert_eq!(
            std::fs::metadata(&sentinel).unwrap().permissions().mode() & 0o777,
            0o640
        );
        assert_eq!(std::fs::read_dir(&target).unwrap().count(), 1);
    }
}

#[test]
fn history_range_rejects_enum_objects() {
    assert!(json_codec::from_value::<HistoryRange>(json!({"1min": null})).is_err());
}

#[test]
fn typed_monitor_sample_and_history_envelope_keep_exact_wire() {
    let disk = DiskStatus::from(crate::DiskSample {
        slot_id: "P01".into(),
        online: true,
        read_only: false,
        simulation: true,
        total_bytes: Some(u64::MAX),
        available_bytes: None,
        used_bytes: Some(7),
        usage_percent: Some(30.0),
        power_watts: None,
        power_unavailable_reason: Some(crate::PowerUnavailable::SimulatedDisk),
    });
    let expected = json!({"id":"P01","name":"P01","online":true,"read_only":false,"simulation":true,"total_bytes":"18446744073709551615","available_bytes":null,"used_bytes":"7","usage_percent":30.0,"power_watts":null,"power_unavailable_reason":"simulated_disk"});
    assert_eq!(json_codec::to_value(&disk).unwrap(), expected);
    assert_eq!(
        json_codec::to_vec(&disk).unwrap(),
        br#"{"id":"P01","name":"P01","online":true,"read_only":false,"simulation":true,"total_bytes":"18446744073709551615","available_bytes":null,"used_bytes":"7","usage_percent":30.0,"power_watts":null,"power_unavailable_reason":"simulated_disk"}"#
    );
    let restored: DiskStatus = json_codec::from_value(expected).unwrap();
    assert_eq!(restored.total_bytes, Some(u64::MAX));
    assert_eq!(restored.available_bytes, None);
    let memory = memory_status(u64::MAX, u64::MAX - 123);
    assert_eq!(
        json_codec::to_value(memory).unwrap(),
        json!({"total_bytes":"18446744073709551615","used_bytes":"123","available_bytes":"18446744073709551492"})
    );
    let response = HistoryResponse {
        range: HistoryRange::Hour,
        interval_seconds: 10,
        from: "from".into(),
        to: "to".into(),
        samples: vec![],
    };
    assert_eq!(
        storage_contract::canonical_json(&response).unwrap(),
        br#"{"from":"from","interval_seconds":10,"range":"1h","samples":[],"to":"to"}"#
    );
}
