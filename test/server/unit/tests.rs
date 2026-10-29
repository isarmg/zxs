use super::*;
use http_routing::{
    body::{Body, Bytes},
    http::StatusCode,
};
use service_stack::ServiceExt;
use std::time::Duration;

pub(crate) fn assert_contract_input(error: &ApiError, expected: &storage_contract::Error) {
    assert_eq!(error.status, StatusCode::BAD_REQUEST);
    assert_eq!(error.code, expected.code());
    assert_eq!(error.message, expected.message());
    assert!(!error.retryable);
    let Some(crate::http_boundary::ApiErrorSource::Contract(source)) = error.source.as_deref()
    else {
        panic!("contract input lost its concrete source");
    };
    assert_eq!(source.code(), expected.code());
    assert_eq!(source.to_string(), expected.to_string());
    assert_eq!(
        std::error::Error::source(source).map(ToString::to_string),
        std::error::Error::source(expected).map(ToString::to_string)
    );
}

// Authentication roots obey the same 0700 contract as deployed control state.
// tempfile's default directory mode is 0777 filtered by umask, not 0700.
pub(crate) fn private_tempdir() -> tempfile::TempDir {
    let mut builder = tempfile::Builder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    builder.tempdir().unwrap()
}
struct TestBackend {
    path: PathBuf,
}
impl Backend for TestBackend {
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
        Ok(None)
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
        Ok(crate::Capabilities::for_engine(u64::MAX))
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
        request: crate::CreateRepository,
    ) -> Result<storage_contract::Repository, crate::ApiError> {
        Ok(storage_contract::Repository {
            repository_id: "repository".into(),
            name: request.name.as_str().into(),
            history_tree_id: "history".into(),
            main_branch_id: "branch".into(),
            primary_slot_id: request.primary_slot_id,
            target_copies: request.target_copies,
            name_policy: name_policy::NAME_POLICY.into(),
            unicode_version: name_policy::UNICODE_VERSION.into(),
            smb_enabled: true,
            smb_binding_generation: 1,
            file_manager_root: false,
        })
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
        request: crate::ReadListing,
    ) -> Result<crate::ListingSnapshot, crate::ApiError> {
        Ok(crate::ListingSnapshot {
            slot_id: request.slot_id().into(),
            path: request.path().into(),
            backup: false,
            entries: vec![],
            selected_entry: None,
            context: None,
            directory_file_count: 0,
            directory_branches: vec![],
            directory_branches_supported: true,
            default_branch_scope: false,
            total_entries: 0,
            snapshot: "fixture".into(),
            previous_cursor: None,
            next_cursor: None,
        })
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
        request: crate::ReadDirectorySelection,
    ) -> Result<crate::DirectorySelectionSnapshot, crate::ApiError> {
        assert_eq!(request.repository_id(), "repo");
        assert!(request.scope().is_none());
        assert!(request.sources().is_empty());
        assert!(request.entries().is_empty());
        Ok(crate::DirectorySelectionSnapshot {
            repository_id: request.repository_id().into(),
            branches: vec![],
        })
    }
    fn directory_scopes(
        &self,
        _identity: &crate::auth::Identity,
        request: crate::ReadDirectoryScopes<'_>,
    ) -> Result<crate::DirectoryScopesSnapshot, crate::ApiError> {
        assert_eq!(request.repository_id, "repo");
        assert!(matches!(request.filter, crate::DirectoryBranchFilter::All));
        assert!(matches!(
            request.projection,
            crate::DirectoryBranchProjection::Full
        ));
        Ok(crate::DirectoryScopesSnapshot {
            repository_id: request.repository_id.into(),
            branches: crate::DirectoryScopeRows::Full(vec![]),
        })
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
        item: &str,
        path: &name_policy::PortablePath,
        source: &crate::UploadSource,
    ) -> Result<storage_contract::StagedFile, ApiError> {
        let mut bytes = vec![0; source.size as usize];
        std::os::unix::fs::FileExt::read_exact_at(&source.file, &mut bytes, 0)
            .map_err(ApiError::io)?;
        Ok(storage_contract::StagedFile {
            item_id: item.into(),
            path: path.clone(),
            source_path: "/private/upload-fixture".into(),
            size: bytes.len() as u64,
            content_hash: storage_contract::ContentHash::from_digest(blake3::hash(&bytes)),
            dirty_generation: 1,
            base_version_id: None,
        })
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
        let mut staged = self.upload(identity, workspace, item, path, source)?;
        staged.dirty_generation = generation;
        Ok(staged)
    }

    fn checksums(&self, _identity: &Identity, _version: &str) -> Result<Vec<u8>, ApiError> {
        Ok(vec![7; 128 * 1024])
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
    fn content(&self, _identity: &Identity, _version: &str) -> Result<Content, ApiError> {
        Ok(Content {
            checksum_manifest: None,
            fallback_files: Vec::new(),
            file: std::fs::File::open(&self.path).map_err(ApiError::io)?,
            size: 10,
            hash: blake3::hash(b"0123456789").to_hex().to_string(),
            lease: Box::new(()),
            download_name: None,
        })
    }
}
fn application(root: &Path) -> Router {
    application_config(root, ServerConfig::loopback(root))
}
fn application_config(root: &Path, config: ServerConfig) -> Router {
    std::fs::write(root.join("content"), b"0123456789").unwrap();
    let auth = AuthStore::initialize(root, "admin", "a-long-test-password").unwrap();
    router(
        Arc::new(TestBackend {
            path: root.join("content"),
        }),
        auth,
        config,
    )
    .unwrap()
}

#[tokio::test]
async fn static_assets_never_shadow_unknown_api_routes_or_their_error_envelope() {
    let root = crate::tests::private_tempdir();
    let assets = root.path().join("web");
    std::fs::create_dir_all(assets.join("api")).unwrap();
    std::fs::write(assets.join("index.html"), b"<html>fixture</html>").unwrap();
    std::fs::write(assets.join("api/unknown"), b"must never be an API response").unwrap();
    let mut config = ServerConfig::loopback(root.path());
    config.web_dir = Some(assets);
    let app = application_config(root.path(), config);
    for path in ["/api", "/api/unknown", "/api/missing", "/missing.css"] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .header(header::IF_NONE_MATCH, "*")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
        if http_boundary::is_api_path(path) {
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        }
        let error: Value = json_codec::from_slice(
            &http_routing::body::to_bytes(response.into_body(), 8192)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(error["error"]["code"], "NOT_FOUND");
    }
    let response = app
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response.headers()[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("text/html")
    );
    assert_eq!(
        http_routing::body::to_bytes(response.into_body(), 8192)
            .await
            .unwrap(),
        "<html>fixture</html>"
    );
}
#[test]
fn router_prepares_a_fresh_nested_spool_before_monitor_cache_initialization() {
    let root = crate::tests::private_tempdir();
    let nested = root.path().join("fresh/nested/system");
    let auth = AuthStore::initialize(root.path(), "admin", "a-long-test-password").unwrap();
    let app = router(
        Arc::new(TestBackend {
            path: root.path().join("content"),
        }),
        auth,
        ServerConfig::loopback(&nested),
    );
    assert!(
        app.is_ok(),
        "fresh nested spool must initialize before telemetry"
    );
    assert!(nested.join("api-spool").is_dir());
    assert!(nested.join("telemetry-cache").is_dir());
}

#[test]
fn an_unwritable_monitor_cache_does_not_prevent_the_business_router_starting() {
    let root = crate::tests::private_tempdir();
    let auth = AuthStore::initialize(root.path(), "admin", "a-long-test-password").unwrap();
    // A preexisting ordinary file deterministically prevents cache writes,
    // including when tests run as root and chmod would not deny access.
    std::fs::write(root.path().join("telemetry-cache"), b"keep this file").unwrap();
    let app = router(
        Arc::new(TestBackend {
            path: root.path().join("content"),
        }),
        auth,
        ServerConfig::loopback(root.path()),
    );
    assert!(
        app.is_ok(),
        "optional monitoring persistence must not stop business routes"
    );
    assert_eq!(
        std::fs::read(root.path().join("telemetry-cache")).unwrap(),
        b"keep this file"
    );
}

async fn authenticated(app: &Router) -> (String, String, String) {
    let response = app
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
    assert_eq!(response.status(), StatusCode::OK);
    let cookie = response.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let body: Value = json_codec::from_slice(
        &http_routing::body::to_bytes(response.into_body(), 8192)
            .await
            .unwrap(),
    )
    .unwrap();
    (
        body["token"].as_str().unwrap().into(),
        body["csrf_token"].as_str().unwrap().into(),
        cookie,
    )
}

#[tokio::test]
async fn cookie_mutations_require_csrf_and_foreign_origins_are_rejected() {
    let root = crate::tests::private_tempdir();
    let app = application(root.path());
    let (token, csrf, cookie) = authenticated(&app).await;
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/repositories")
                .header("cookie", &cookie)
                .header("host", "127.0.0.1:7400")
                .header("origin", "http://127.0.0.1:7400")
                .body(Body::from(json!({"name":"Repository","primary_slot_id":"P01","target_copies":3,"operation_id":storage_contract::OperationId::generate()}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/repositories")
                .header("cookie", &cookie)
                .header("x-csrf-token", csrf)
                .header("host", "127.0.0.1:7400")
                .header("origin", "http://127.0.0.1:7400")
                .body(Body::from(json!({"name":"Repository","primary_slot_id":"P01","target_copies":3,"operation_id":storage_contract::OperationId::generate()}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/auth/me")
                .header("authorization", format!("Bearer {token}"))
                .header("origin", "https://foreign.invalid")
                .header("host", "127.0.0.1:7400")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn authenticated_range_download_returns_exact_immutable_bytes() {
    let root = crate::tests::private_tempdir();
    let app = application(root.path());
    let (token, _, _) = authenticated(&app).await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/versions/test/content")
                .header("authorization", format!("Bearer {token}"))
                .header("range", "bytes=2-5")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(response.headers()[header::CONTENT_RANGE], "bytes 2-5/10");
    assert_eq!(
        http_routing::body::to_bytes(response.into_body(), 8192)
            .await
            .unwrap(),
        "2345"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn authentication_lock_wait_does_not_block_the_async_executor() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let root = crate::tests::private_tempdir();
    let mut auth = AuthStore::initialize(root.path(), "admin", "a-long-test-password").unwrap();
    let token = auth.login("admin", "a-long-test-password").unwrap().token;
    let (started, waiting) = tokio::sync::oneshot::channel();
    let started = Mutex::new(Some(started));
    let (release, released) = std::sync::mpsc::channel();
    let released = Mutex::new(released);
    let armed = Arc::new(AtomicBool::new(false));
    let timed_out = Arc::new(AtomicBool::new(false));
    let (hook_armed, hook_timeout) = (armed.clone(), timed_out.clone());
    auth.attach_backup(Arc::new(move |_| {
        if hook_armed.load(Ordering::SeqCst)
            && let Some(started) = started.lock().unwrap().take()
        {
            let _ = started.send(());
            if released
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(2))
                .is_err()
            {
                hook_timeout.store(true, Ordering::SeqCst);
            }
        }
        Ok(())
    }))
    .unwrap();
    armed.store(true, Ordering::SeqCst);
    let app = router(
        Arc::new(TestBackend {
            path: root.path().join("unused"),
        }),
        auth,
        ServerConfig::loopback(root.path()),
    )
    .unwrap();
    let login_app = app.clone();
    let login = tokio::spawn(async move {
        login_app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/auth/login")
                    .body(Body::from(
                        r#"{"username":"admin","password":"a-long-test-password"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap()
    });
    waiting.await.unwrap();
    let release_task = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(20)).await;
        release.send(()).unwrap();
    });
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/auth/me")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    release_task.await.unwrap();
    assert_eq!(login.await.unwrap().status(), StatusCode::OK);
    assert!(
        !timed_out.load(Ordering::SeqCst),
        "authentication starved the timer that releases its blocking backup"
    );
}

#[tokio::test]
async fn interrupted_upload_retains_durable_bytes_and_rejects_different_retry() {
    let root = crate::tests::private_tempdir();
    let app = application(root.path());
    let (token, _, _) = authenticated(&app).await;
    let stream = futures::stream::iter([
        Ok(Bytes::from_static(b"original")),
        Err(std::io::Error::other("connection interrupted")),
    ]);
    let request = Request::builder()
        .method("PUT")
        .uri("/api/workspaces/ws/files/entry_00000000000000000000000000000001/content?path=file")
        .header("authorization", format!("Bearer {token}"))
        .header("x-zxs-logical-size", "20")
        .header("x-zxs-content-hash", "0".repeat(64))
        .body(Body::from_stream(stream))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let record = uploads::load(
        &root.path().join("api-spool"),
        "ws",
        "entry_00000000000000000000000000000001",
    )
    .unwrap()
    .unwrap();
    assert_eq!(record.persisted_bytes, 8);
    assert_eq!(
        std::fs::read(root.path().join("api-spool").join(&record.data_file)).unwrap(),
        b"original"
    );
    let response = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/workspaces/ws/files/entry_00000000000000000000000000000001/content?path=file")
                .header("authorization", format!("Bearer {token}"))
                .header("x-zxs-logical-size", "20")
                .header("x-zxs-content-hash", "0".repeat(64))
                .body(Body::from("different retry data"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        uploads::load(
            &root.path().join("api-spool"),
            "ws",
            "entry_00000000000000000000000000000001"
        )
        .unwrap()
        .unwrap()
        .data_file,
        record.data_file
    );
    let data_files = std::fs::read_dir(root.path().join("api-spool"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "data")
        })
        .count();
    assert_eq!(
        data_files, 1,
        "conflicting retries must remove their unreferenced staging file"
    );
}

#[tokio::test]
async fn durable_retry_progress_appends_without_copying_the_same_generation() {
    let root = crate::tests::private_tempdir();
    let app = application(root.path());
    let (token, _, _) = authenticated(&app).await;
    let spool = root.path().join("api-spool");
    for content in ["original", "original plus", "original plus more"] {
        let stream = futures::stream::iter([
            Ok(Bytes::copy_from_slice(content.as_bytes())),
            Err(std::io::Error::other("interrupted")),
        ]);
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/workspaces/ws/files/entry_00000000000000000000000000000001/content?path=file")
                    .header("authorization", format!("Bearer {token}"))
                    .header("x-zxs-logical-size", "30")
                    .header("x-zxs-content-hash", "0".repeat(64))
                    .body(Body::from_stream(stream))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let current = uploads::load(&spool, "ws", "entry_00000000000000000000000000000001")
            .unwrap()
            .unwrap();
        assert_eq!(
            std::fs::read(spool.join(current.data_file)).unwrap(),
            content.as_bytes()
        );
        assert_eq!(
            std::fs::read_dir(&spool)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .filter(|path| path
                    .extension()
                    .is_some_and(|extension| extension == "data"))
                .count(),
            1
        );
    }
}

#[tokio::test]
async fn unpublished_retry_cleanup_covers_short_long_and_cancelled_bodies() {
    let root = crate::tests::private_tempdir();
    let app = application(root.path());
    let (token, _, _) = authenticated(&app).await;
    let bytes = vec![42u8; 2 * 1024 * 1024];
    let hash = blake3::hash(&bytes).to_hex().to_string();
    let request = |body| {
        Request::builder()
            .method("PUT")
            .uri(
                "/api/workspaces/ws/files/entry_00000000000000000000000000000001/content?path=file",
            )
            .header("authorization", format!("Bearer {token}"))
            .header("x-zxs-logical-size", bytes.len().to_string())
            .header("x-zxs-content-hash", &hash)
            .body(body)
            .unwrap()
    };
    assert_eq!(
        app.clone()
            .oneshot(request(Body::from(bytes.clone())))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let spool = root.path().join("api-spool");
    let original = uploads::load(&spool, "ws", "entry_00000000000000000000000000000001")
        .unwrap()
        .unwrap();
    // A retry must not supersede old progress until it has checked all of it.
    for body in [
        Body::from(vec![42u8; 1024]),
        Body::from(vec![42u8; bytes.len() + 1]),
        Body::from_stream(futures::stream::iter([
            Ok(Bytes::from(vec![42u8; 1024])),
            Err(std::io::Error::other("interrupted")),
        ])),
    ] {
        assert!(
            !app.clone()
                .oneshot(request(body))
                .await
                .unwrap()
                .status()
                .is_success()
        );
    }
    let (sent, received) = tokio::sync::oneshot::channel();
    let stream = futures::stream::once(async move {
        let _ = sent.send(());
        Ok::<_, std::io::Error>(Bytes::from(vec![42u8; 1024]))
    });
    let stream = futures::StreamExt::chain(stream, futures::stream::pending());
    let retry_app = app.clone();
    let retry_request = request(Body::from_stream(stream));
    let retry = tokio::spawn(async move { retry_app.oneshot(retry_request).await });
    received.await.unwrap();
    retry.abort();
    let _ = retry.await;
    // Cancellation may leave a running prefix-check job holding the guard.
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let files = std::fs::read_dir(&spool)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .filter(|path| {
                    path.extension()
                        .is_some_and(|extension| extension == "data")
                })
                .count();
            if files == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let current = uploads::load(&spool, "ws", "entry_00000000000000000000000000000001")
        .unwrap()
        .unwrap();
    assert_eq!(current.data_file, original.data_file);
    assert_eq!(std::fs::read(spool.join(current.data_file)).unwrap(), bytes);
}

#[tokio::test]
async fn successful_new_generation_preserves_old_bytes_and_retry_reuses_its_file() {
    let root = crate::tests::private_tempdir();
    let app = application(root.path());
    let (token, _, _) = authenticated(&app).await;
    let spool = root.path().join("api-spool");
    let mut records = Vec::new();
    for (generation, bytes) in [(1, "original"), (2, "replacement"), (2, "replacement")] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/workspaces/ws/files/entry_00000000000000000000000000000001/content?path=file")
                    .header("authorization", format!("Bearer {token}"))
                    .header("x-zxs-logical-size", bytes.len().to_string())
                    .header(
                        "x-zxs-content-hash",
                        blake3::hash(bytes.as_bytes()).to_hex().as_str(),
                    )
                    .header("x-zxs-upload-generation", generation.to_string())
                    .body(Body::from(bytes))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        records.push(
            uploads::load(&spool, "ws", "entry_00000000000000000000000000000001")
                .unwrap()
                .unwrap(),
        );
        assert_eq!(
            std::fs::read(spool.join(&records[0].data_file)).unwrap(),
            b"original"
        );
    }
    assert_eq!(records[1].data_file, records[2].data_file);
    assert_eq!(
        std::fs::read(spool.join(&records[2].data_file)).unwrap(),
        b"replacement"
    );
    let retained = std::fs::read_dir(&spool)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("retained-")
        })
        .collect::<Vec<_>>();
    assert_eq!(retained.len(), 1);
    let retained: uploads::UploadRecord =
        json_codec::from_slice(&std::fs::read(&retained[0]).unwrap()).unwrap();
    assert_eq!(retained.generation, 1);
    assert_eq!(retained.data_file, records[0].data_file);
}

#[test]
fn disconnected_upload_keeps_its_claim_until_publication_and_status_are_durable() {
    upload_publication_keeps_its_claim(true);
}

#[test]
fn upload_publication_timeout_reports_an_unknown_outcome_until_status_is_durable() {
    upload_publication_keeps_its_claim(false);
}

fn upload_publication_keeps_its_claim(disconnect: bool) {
    struct BlockingUpload {
        delegate: TestBackend,
        started: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
        gate: Arc<(Mutex<bool>, std::sync::Condvar)>,
    }
    impl Backend for BlockingUpload {
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
            identity: &crate::auth::Identity,
            workspace: &str,
            item: &str,
        ) -> Result<Option<crate::PublishedUploadStatus>, crate::ApiError> {
            self.delegate
                .published_upload_status(identity, workspace, item)
        }
        fn seal_workspace(
            &self,
            identity: &crate::auth::Identity,
            workspace: &str,
        ) -> Result<storage_contract::SealedWorkspace, crate::ApiError> {
            self.delegate.seal_workspace(identity, workspace)
        }
        fn commit_workspace_public(
            &self,
            identity: &crate::auth::Identity,
            command: storage_contract::PublicCommitEnvelope,
        ) -> Result<storage_contract::OperationResult, crate::ApiError> {
            self.delegate.commit_workspace_public(identity, command)
        }
        fn version_snapshot(
            &self,
            identity: &crate::auth::Identity,
            version: &str,
        ) -> Result<crate::VersionSnapshot, crate::ApiError> {
            self.delegate.version_snapshot(identity, version)
        }
        fn authorize_version(
            &self,
            identity: &crate::auth::Identity,
            version: &str,
        ) -> Result<(), crate::ApiError> {
            self.delegate.authorize_version(identity, version)
        }
        fn repository_snapshot(
            &self,
            identity: &crate::auth::Identity,
            id: &str,
        ) -> Result<storage_contract::Repository, crate::ApiError> {
            self.delegate.repository_snapshot(identity, id)
        }
        fn authorize_repository(
            &self,
            identity: &crate::auth::Identity,
            id: &str,
        ) -> Result<(), crate::ApiError> {
            self.delegate.authorize_repository(identity, id)
        }
        fn capabilities(&self) -> Result<crate::Capabilities, crate::ApiError> {
            self.delegate.capabilities()
        }
        fn ready(&self) -> Result<crate::Readiness, crate::ApiError> {
            self.delegate.ready()
        }
        fn workspace_capacity(
            &self,
            identity: &crate::auth::Identity,
        ) -> Result<storage_contract::WorkspaceCapacity, crate::ApiError> {
            self.delegate.workspace_capacity(identity)
        }
        fn smb_binding_status(
            &self,
            identity: &crate::auth::Identity,
            id: &str,
        ) -> Result<crate::SmbBindingStatus, crate::ApiError> {
            self.delegate.smb_binding_status(identity, id)
        }
        fn update_smb_binding(
            &self,
            identity: &crate::auth::Identity,
            request: crate::UpdateSmbBinding<'_>,
        ) -> Result<storage_contract::OperationResult, crate::ApiError> {
            self.delegate.update_smb_binding(identity, request)
        }
        fn protection_policy(
            &self,
            identity: &crate::auth::Identity,
            id: &str,
        ) -> Result<storage_contract::ProtectionPolicy, crate::ApiError> {
            self.delegate.protection_policy(identity, id)
        }
        fn placement_rule(
            &self,
            identity: &crate::auth::Identity,
            id: &str,
        ) -> Result<storage_contract::PlacementRule, crate::ApiError> {
            self.delegate.placement_rule(identity, id)
        }
        fn update_protection_policy(
            &self,
            identity: &crate::auth::Identity,
            request: crate::UpdateProtectionPolicy,
        ) -> Result<storage_contract::OperationResult, crate::ApiError> {
            self.delegate.update_protection_policy(identity, request)
        }
        fn update_placement_rule(
            &self,
            identity: &crate::auth::Identity,
            request: crate::UpdatePlacementRule,
        ) -> Result<storage_contract::OperationResult, crate::ApiError> {
            self.delegate.update_placement_rule(identity, request)
        }
        fn compare_commits(
            &self,
            identity: &crate::auth::Identity,
            request: crate::CompareCommits<'_>,
        ) -> Result<storage_contract::CommitComparison, crate::ApiError> {
            self.delegate.compare_commits(identity, request)
        }
        fn disk_status(
            &self,
            identity: &crate::auth::Identity,
        ) -> Result<Vec<storage_contract::DiskStatus>, crate::ApiError> {
            self.delegate.disk_status(identity)
        }
        fn integrity_check(
            &self,
            identity: &crate::auth::Identity,
        ) -> Result<storage_contract::CheckReport, crate::ApiError> {
            self.delegate.integrity_check(identity)
        }
        fn create_merge_link(
            &self,
            identity: &crate::auth::Identity,
            request: crate::CreateMergeLink,
        ) -> Result<storage_contract::MergeLink, crate::ApiError> {
            self.delegate.create_merge_link(identity, request)
        }
        fn create_repository(
            &self,
            identity: &crate::auth::Identity,
            request: crate::CreateRepository,
        ) -> Result<storage_contract::Repository, crate::ApiError> {
            self.delegate.create_repository(identity, request)
        }
        fn create_branch(
            &self,
            identity: &crate::auth::Identity,
            request: crate::CreateBranch,
        ) -> Result<crate::BranchSnapshot, crate::ApiError> {
            self.delegate.create_branch(identity, request)
        }
        fn create_file_branch(
            &self,
            identity: &crate::auth::Identity,
            request: crate::CreateFileBranch,
        ) -> Result<storage_contract::OperationResult, crate::ApiError> {
            self.delegate.create_file_branch(identity, request)
        }
        fn update_note(
            &self,
            identity: &crate::auth::Identity,
            request: crate::UpdateNote,
        ) -> Result<storage_contract::OperationResult, crate::ApiError> {
            self.delegate.update_note(identity, request)
        }
        fn merge_file_branches(
            &self,
            identity: &crate::auth::Identity,
            request: crate::MergeFileBranches,
        ) -> Result<storage_contract::OperationResult, crate::ApiError> {
            self.delegate.merge_file_branches(identity, request)
        }
        fn update_file_node_tags(
            &self,
            identity: &crate::auth::Identity,
            request: crate::UpdateFileNodeTags,
        ) -> Result<storage_contract::OperationResult, crate::ApiError> {
            self.delegate.update_file_node_tags(identity, request)
        }
        fn create_disk_directory(
            &self,
            identity: &crate::auth::Identity,
            request: crate::CreateDiskDirectory,
        ) -> Result<storage_contract::OperationResult, crate::ApiError> {
            self.delegate.create_disk_directory(identity, request)
        }
        fn create_disk_write_session(
            &self,
            identity: &crate::auth::Identity,
            request: crate::CreateDiskWriteSession,
        ) -> Result<crate::DiskWriteSession, crate::ApiError> {
            self.delegate.create_disk_write_session(identity, request)
        }
        fn backup_configuration(
            &self,
            identity: &crate::auth::Identity,
            request: crate::ReadBackupConfiguration,
        ) -> Result<storage_contract::BackupConfigurationSnapshot, crate::ApiError> {
            self.delegate.backup_configuration(identity, request)
        }
        fn configure_backups(
            &self,
            identity: &crate::auth::Identity,
            request: crate::ConfigureBackups,
        ) -> Result<storage_contract::OperationResult, crate::ApiError> {
            self.delegate.configure_backups(identity, request)
        }
        fn plan(
            &self,
            identity: &crate::auth::Identity,
            plan: &str,
        ) -> Result<storage_contract::Plan, crate::ApiError> {
            self.delegate.plan(identity, plan)
        }
        fn cancel_plan(
            &self,
            identity: &crate::auth::Identity,
            request: Result<crate::CancelPlan, crate::ApiError>,
        ) -> Result<storage_contract::OperationResult, crate::ApiError> {
            self.delegate.cancel_plan(identity, request)
        }
        fn register_disk(
            &self,
            identity: &crate::auth::Identity,
        ) -> Result<std::convert::Infallible, crate::ApiError> {
            self.delegate.register_disk(identity)
        }
        fn commit_plan(
            &self,
            identity: &crate::auth::Identity,
            request: Result<crate::CommitPlan, crate::ApiError>,
        ) -> Result<storage_contract::OperationResult, crate::ApiError> {
            self.delegate.commit_plan(identity, request)
        }
        fn prepare_historical_save(
            &self,
            identity: &crate::auth::Identity,
            request: Result<crate::PrepareHistoricalSave, crate::ApiError>,
        ) -> Result<storage_contract::Plan, crate::ApiError> {
            self.delegate.prepare_historical_save(identity, request)
        }
        fn prepare_purge(
            &self,
            identity: &crate::auth::Identity,
            request: Result<crate::PreparePurge, crate::ApiError>,
        ) -> Result<storage_contract::Plan, crate::ApiError> {
            self.delegate.prepare_purge(identity, request)
        }
        fn prepare_split(
            &self,
            identity: &crate::auth::Identity,
            request: Result<crate::PrepareSplit, crate::ApiError>,
        ) -> Result<storage_contract::Plan, crate::ApiError> {
            self.delegate.prepare_split(identity, request)
        }
        fn namespace_batch(
            &self,
            identity: &crate::auth::Identity,
            request: crate::NamespaceBatch,
        ) -> Result<storage_contract::OperationResult, crate::ApiError> {
            self.delegate.namespace_batch(identity, request)
        }
        fn select_file_branch(
            &self,
            identity: &crate::auth::Identity,
            request: crate::SelectFileBranch,
        ) -> Result<storage_contract::OperationResult, crate::ApiError> {
            self.delegate.select_file_branch(identity, request)
        }
        fn rename_file_branch(
            &self,
            identity: &crate::auth::Identity,
            request: crate::RenameFileBranch,
        ) -> Result<storage_contract::OperationResult, crate::ApiError> {
            self.delegate.rename_file_branch(identity, request)
        }
        fn listing(
            &self,
            identity: &crate::auth::Identity,
            request: crate::ReadListing,
        ) -> Result<crate::ListingSnapshot, crate::ApiError> {
            self.delegate.listing(identity, request)
        }
        fn mutate_disk_directory(
            &self,
            identity: &crate::auth::Identity,
            request: crate::MutateDiskDirectory,
        ) -> Result<storage_contract::OperationResult, crate::ApiError> {
            self.delegate.mutate_disk_directory(identity, request)
        }
        fn initialize_directory_scope(
            &self,
            identity: &crate::auth::Identity,
            request: crate::InitializeDirectoryScope,
        ) -> Result<storage_contract::OperationResult, crate::ApiError> {
            self.delegate.initialize_directory_scope(identity, request)
        }
        fn archive_branch(
            &self,
            identity: &crate::auth::Identity,
            request: crate::ArchiveBranch,
        ) -> Result<storage_contract::OperationResult, crate::ApiError> {
            self.delegate.archive_branch(identity, request)
        }
        fn rename_directory_branch(
            &self,
            identity: &crate::auth::Identity,
            request: crate::RenameDirectoryBranch,
        ) -> Result<storage_contract::OperationResult, crate::ApiError> {
            self.delegate.rename_directory_branch(identity, request)
        }
        fn select_default_directory_branch(
            &self,
            identity: &crate::auth::Identity,
            request: crate::SelectDefaultDirectoryBranch,
        ) -> Result<storage_contract::OperationResult, crate::ApiError> {
            self.delegate
                .select_default_directory_branch(identity, request)
        }
        fn directory_selection(
            &self,
            identity: &crate::auth::Identity,
            request: crate::ReadDirectorySelection,
        ) -> Result<crate::DirectorySelectionSnapshot, crate::ApiError> {
            self.delegate.directory_selection(identity, request)
        }
        fn directory_scopes(
            &self,
            identity: &crate::auth::Identity,
            request: crate::ReadDirectoryScopes<'_>,
        ) -> Result<crate::DirectoryScopesSnapshot, crate::ApiError> {
            self.delegate.directory_scopes(identity, request)
        }
        fn default_directory_branch(
            &self,
            identity: &crate::auth::Identity,
            repository_id: &str,
        ) -> Result<crate::DefaultDirectoryBranchSnapshot, crate::ApiError> {
            self.delegate
                .default_directory_branch(identity, repository_id)
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
            identity: &crate::auth::Identity,
        ) -> Result<crate::TopologySnapshot, crate::ApiError> {
            self.delegate.topology_snapshot(identity)
        }
        fn placement_snapshot(
            &self,
            identity: &crate::auth::Identity,
            id: &str,
        ) -> Result<storage_contract::Placement, crate::ApiError> {
            self.delegate.placement_snapshot(identity, id)
        }
        fn event_page(
            &self,
            identity: &crate::auth::Identity,
            after: u64,
        ) -> Result<crate::EventPage, crate::ApiError> {
            self.delegate.event_page(identity, after)
        }
        fn activity_history(
            &self,
            identity: &crate::auth::Identity,
        ) -> Result<crate::ActivityHistorySnapshot, crate::ApiError> {
            self.delegate.activity_history(identity)
        }
        fn history_graph(
            &self,
            identity: &crate::auth::Identity,
            id: &str,
        ) -> Result<crate::HistoryGraphSnapshot, crate::ApiError> {
            self.delegate.history_graph(identity, id)
        }
        fn commit_snapshot(
            &self,
            identity: &crate::auth::Identity,
            id: &str,
        ) -> Result<std::sync::Arc<storage_contract::Commit>, crate::ApiError> {
            self.delegate.commit_snapshot(identity, id)
        }
        fn resolve_entry(
            &self,
            identity: &crate::auth::Identity,
            request: crate::ResolveEntry<'_>,
        ) -> Result<crate::ResolvedEntry, crate::ApiError> {
            self.delegate.resolve_entry(identity, request)
        }
        fn branch_snapshot(
            &self,
            identity: &crate::auth::Identity,
            id: &str,
        ) -> Result<crate::BranchSnapshot, crate::ApiError> {
            self.delegate.branch_snapshot(identity, id)
        }
        fn branches_snapshot(
            &self,
            identity: &crate::auth::Identity,
            id: &str,
        ) -> Result<Vec<crate::BranchSnapshot>, crate::ApiError> {
            self.delegate.branches_snapshot(identity, id)
        }
        fn repositories_snapshot(
            &self,
            identity: &crate::auth::Identity,
        ) -> Result<Vec<storage_contract::Repository>, crate::ApiError> {
            self.delegate.repositories_snapshot(identity)
        }
        fn tree_snapshot(
            &self,
            identity: &crate::auth::Identity,
            request: crate::ReadTree<'_>,
        ) -> Result<crate::TreeSnapshot, crate::ApiError> {
            self.delegate.tree_snapshot(identity, request)
        }
        fn create_write_session(
            &self,
            identity: &crate::auth::Identity,
            branch: &str,
        ) -> Result<crate::WorkspaceWriteSession, crate::ApiError> {
            self.delegate.create_write_session(identity, branch)
        }
        fn stage_namespace(
            &self,
            identity: &crate::auth::Identity,
            command: crate::StageNamespace,
        ) -> Result<storage_contract::Workspace, crate::ApiError> {
            self.delegate.stage_namespace(identity, command)
        }
        fn authorize_upload(
            &self,
            identity: &crate::auth::Identity,
            workspace: &str,
            logical_size: Option<u64>,
        ) -> Result<(), crate::ApiError> {
            self.delegate
                .authorize_upload(identity, workspace, logical_size)
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

        fn range_content(
            &self,
            identity: &crate::auth::Identity,
            request: crate::ReadContent<'_>,
        ) -> Result<crate::Content, crate::ApiError> {
            self.delegate.range_content(identity, request)
        }
        fn content(&self, identity: &Identity, version: &str) -> Result<Content, ApiError> {
            self.delegate.content(identity, version)
        }
        fn upload(
            &self,
            identity: &Identity,
            workspace: &str,
            item: &str,
            path: &name_policy::PortablePath,
            source: &crate::UploadSource,
        ) -> Result<storage_contract::StagedFile, ApiError> {
            if let Some(started) = self.started.lock().unwrap().take() {
                let _ = started.send(());
            }
            let (ready, wake) = &*self.gate;
            let mut ready = ready.lock().unwrap();
            while !*ready {
                ready = wake.wait(ready).unwrap();
            }
            self.delegate
                .upload(identity, workspace, item, path, source)
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
            let mut staged = self.upload(identity, workspace, item, path, source)?;
            staged.dirty_generation = generation;
            Ok(staged)
        }
    }
    struct Release(Arc<(Mutex<bool>, std::sync::Condvar)>);
    impl Drop for Release {
        fn drop(&mut self) {
            *self.0.0.lock().unwrap() = true;
            self.0.1.notify_all();
        }
    }
    let root = crate::tests::private_tempdir();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let auth = AuthStore::initialize(root.path(), "admin", "a-long-test-password").unwrap();
        let (started, started_rx) = tokio::sync::oneshot::channel();
        let gate = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
        let release = Release(gate.clone());
        let mut config = ServerConfig::loopback(root.path());
        if !disconnect {
            config.http_transfer_limits.total_timeout = Duration::from_secs(3);
            config.http_transfer_limits.idle_timeout = Duration::from_secs(3);
        }
        let app = router(
            Arc::new(BlockingUpload {
                delegate: TestBackend {
                    path: root.path().join("content"),
                },
                started: Mutex::new(Some(started)),
                gate,
            }),
            auth,
            config,
        )
        .unwrap();
        let (token, _, _) = authenticated(&app).await;
        let request = || {
            Request::builder()
                .method("PUT")
                .uri("/api/workspaces/ws/files/entry_00000000000000000000000000000001/content?path=file")
                .header("authorization", format!("Bearer {token}"))
                .header("x-zxs-logical-size", "4")
                .header(
                    "x-zxs-content-hash",
                    blake3::hash(b"body").to_hex().to_string(),
                )
                .body(Body::from("body"))
                .unwrap()
        };
        let pending = tokio::spawn(app.clone().oneshot(request()));
        tokio::time::timeout(Duration::from_secs(5), started_rx)
            .await
            .unwrap()
            .unwrap();
        let timed_out = if disconnect {
            pending.abort();
            assert!(pending.await.unwrap_err().is_cancelled());
            None
        } else {
            Some(pending.await.unwrap().unwrap())
        };
        let retry = app.clone().oneshot(request()).await.unwrap();
        assert_eq!(retry.status(), StatusCode::CONFLICT);
        let error: Value = json_codec::from_slice(
            &http_routing::body::to_bytes(retry.into_body(), 8192)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(error["error"]["code"], "FILE_WRITE_BUSY");
        drop(release);
        if let Some(response) = timed_out {
            // The admitted backend can complete despite the HTTP timeout.
            // It is therefore not a definite client-side rejection (4xx).
            assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
            assert!(storage_contract::client_protocol::mutation_outcome_unknown(
                response.status().as_u16()
            ));
            let body: Value = json_codec::from_slice(
                &http_routing::body::to_bytes(response.into_body(), 8192)
                    .await
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(body["error"]["code"], "TRANSFER_TIMEOUT");
            assert!(
                body["error"]["message"]
                    .as_str()
                    .unwrap()
                    .contains("status")
            );
        }
    });
    // Wait for admitted blocking publication, including its final atomic status
    // replacement, before reading that status or removing the temporary root.
    drop(runtime);
    assert_eq!(
        uploads::load(
            &root.path().join("api-spool"),
            "ws",
            "entry_00000000000000000000000000000001"
        )
        .unwrap()
        .unwrap()
        .state,
        crate::uploads::UploadState::Complete
    );
}

#[path = "tests/spool_budget.rs"]
mod spool_budget_tests;

#[path = "tests/http_boundary.rs"]
mod http_boundary_tests;

#[path = "tests/auxiliary_startup.rs"]
mod auxiliary_startup_tests;

#[tokio::test]
async fn workspace_capacity_requires_authentication_and_preserves_unsupported_backend() {
    let root = private_tempdir();
    let app = application(root.path());
    let request = Request::builder()
        .uri("/api/workspaces/capacity")
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(request).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    let (token, _, _) = authenticated(&app).await;
    for scheme in ["Bearer", "bearer", "BEARER"] {
        let request = Request::builder()
            .uri("/api/workspaces/capacity")
            .header(header::AUTHORIZATION, format!("{scheme} {token}"))
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            app.clone().oneshot(request).await.unwrap().status(),
            StatusCode::NOT_IMPLEMENTED
        );
    }
    let request = Request::builder()
        .uri("/api/workspaces/capacity")
        .header(
            header::AUTHORIZATION,
            format!("Bearer {}", token.to_uppercase()),
        )
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.oneshot(request).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn directory_scope_routes_select_typed_reads_and_separate_selection_with_csrf() {
    let root = private_tempdir();
    let app = application(root.path());
    let (_, csrf, cookie) = authenticated(&app).await;
    let uri = "/api/repositories/repo/directory-branches";
    for method in ["GET", "POST"] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .header(header::COOKIE, &cookie)
                    .header(header::HOST, "127.0.0.1:7400")
                    .header(header::ORIGIN, "http://127.0.0.1:7400")
                    .header("x-csrf-token", &csrf)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"path":"","sources":[],"entries":[]}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: Value = json_codec::from_slice(
            &http_routing::body::to_bytes(response.into_body(), 8192)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            body,
            json!({"repository_id":"repo","directory_branches":[]})
        );
    }
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header(header::COOKIE, cookie)
                .header(header::HOST, "127.0.0.1:7400")
                .header(header::ORIGIN, "http://127.0.0.1:7400")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn upload_path_admission_rejects_nonportable_names_and_keeps_display_spelling() {
    let root = private_tempdir();
    let app = application(root.path());
    let (token, _, _) = authenticated(&app).await;
    for (index, path) in ["/outside", "CON", "e\u{301}.txt", "Café.txt"]
        .iter()
        .enumerate()
    {
        let item = format!("entry_{index:032x}");
        let query = url_encoding::form::Serializer::new(String::new())
            .append_pair("path", path)
            .finish();
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/api/workspaces/ws/files/{item}/content?{query}"))
                    .header("authorization", format!("Bearer {token}"))
                    .header("content-length", "4")
                    .header(
                        "x-zxs-content-hash",
                        blake3::hash(b"body").to_hex().to_string(),
                    )
                    .body(Body::from("body"))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let body: Value = json_codec::from_slice(
            &http_routing::body::to_bytes(response.into_body(), 8192)
                .await
                .unwrap(),
        )
        .unwrap();
        if *path == "Café.txt" {
            assert_eq!(status, StatusCode::OK);
            assert_eq!(body["path"], *path);
            assert_eq!(body["size"], "4");
        } else {
            assert_eq!(status, StatusCode::BAD_REQUEST);
            assert_eq!(body["error"]["code"], "INVALID_REQUEST");
            assert!(
                uploads::load(&root.path().join("api-spool"), "ws", &item)
                    .unwrap()
                    .is_none()
            );
        }
    }
}

#[test]
fn public_router_rejects_invalid_static_origins_before_preparing_spool() {
    for origin in [
        "not a URL",
        "",
        "null",
        "http://example.com",
        "https://example.com/",
        "https://example.com/path",
        "https://user@example.com",
        "https://user:password@example.com",
        "https://example.com?query=1",
        "https://example.com#fragment",
    ] {
        let root = private_tempdir();
        let mut config = ServerConfig::loopback(root.path());
        config.external_origin = Some(origin.into());
        let spool = config.spool_dir.clone();
        let auth = AuthStore::initialize(root.path(), "admin", "a-long-test-password").unwrap();
        let result = router(
            Arc::new(TestBackend {
                path: root.path().join("content"),
            }),
            auth,
            config,
        );
        let error = match result {
            Err(error) => error,
            Ok(_) => panic!("accepted invalid origin {origin}"),
        };
        assert_eq!(error.status, 400, "{origin}");
        assert!(
            !spool.exists(),
            "invalid configuration must precede spool I/O"
        );
    }
}

#[tokio::test]
async fn canonical_static_origin_drives_http_login_cookie_and_request_rejection() {
    let root = private_tempdir();
    let mut config = ServerConfig::loopback(root.path());
    config.external_origin = Some("https://EXAMPLE.COM:443".into());
    let app = application_config(root.path(), config);
    for (origin, expected) in [
        ("https://example.com", StatusCode::OK),
        ("https://example.com:8443", StatusCode::FORBIDDEN),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/auth/login")
                    .header(header::ORIGIN, origin)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"username":"admin","password":"a-long-test-password"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
        if expected == StatusCode::OK {
            assert!(
                response.headers()[header::SET_COOKIE]
                    .to_str()
                    .unwrap()
                    .ends_with("; Secure")
            );
        }
    }
}

// Existing JSON fixture vectors are test-client input, never an internal backend path.
#[cfg(feature = "core")]
pub(crate) fn tree_request(value: &Value) -> crate::ReadTree<'_> {
    crate::ReadTree {
        branch: value["branch_id"].as_str().expect("fixture branch id"),
        commit: value
            .get("query")
            .and_then(|q| q.get("commit_id"))
            .and_then(Value::as_str),
    }
}

/// Build a typed command from this suite's valid directory fixtures.
#[cfg(feature = "core")]
pub(crate) fn disk_directory_request(
    value: Value,
    action: crate::DiskDirectoryAction,
) -> crate::MutateDiskDirectory {
    let path = value
        .get("path")
        .map(|value| value.as_str().unwrap())
        .filter(|path| !path.is_empty())
        .map(|path| name_policy::LookupPath::parse(path).unwrap());
    crate::MutateDiskDirectory {
        slot_id: value["slot_id"].as_str().unwrap().into(),
        path,
        name: storage_contract::BranchName::parse(value["name"].as_str().unwrap()).unwrap(),
        operation_id: value["operation_id"].as_str().unwrap().parse().unwrap(),
        action,
    }
}

#[tokio::test]
async fn listing_http_boundary_preserves_root_shape_and_rejects_invalid_queries() {
    let root = private_tempdir();
    let app = application(root.path());
    let (_, _, cookie) = authenticated(&app).await;
    for (query, expected) in [
        ("", StatusCode::OK),
        ("?path=Cafe%CC%81&limit=500", StatusCode::OK),
        ("?extra=1", StatusCode::BAD_REQUEST),
        ("?limit=0", StatusCode::BAD_REQUEST),
        ("?limit=501", StatusCode::BAD_REQUEST),
        ("?path=../bad", StatusCode::BAD_REQUEST),
        ("?selected_path=item", StatusCode::BAD_REQUEST),
        ("?cursor=gg", StatusCode::BAD_REQUEST),
        ("?path=a&%70ath=b", StatusCode::BAD_REQUEST),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/file-manager/disks/P01/entries{query}"))
                    .header(header::COOKIE, &cookie)
                    .header(header::HOST, "127.0.0.1:7400")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), expected, "{query}");
        let body: Value = json_codec::from_slice(
            &http_routing::body::to_bytes(response.into_body(), 8192)
                .await
                .unwrap(),
        )
        .unwrap();
        if expected == StatusCode::OK {
            assert_eq!(body["slot_id"], "P01");
            assert_eq!(body["total_entries"], "0");
            assert_eq!(body["directory_file_count"], "0");
            assert!(body["selected_entry"].is_null());
            assert!(body["context"].is_null());
            assert!(body.get("backup").is_none());
            assert_eq!(body["writable"], true);
            assert_eq!(body["snapshot"], "fixture");
            assert_eq!(
                body["path"],
                if query.is_empty() { "" } else { "Cafe\u{301}" }
            );
        }
    }
}
