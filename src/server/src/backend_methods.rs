//! Single typed contract for ordinary backend operations and their two adapters.
//! Required methods stay required; optional defaults stay exactly as declared.
//! Expansion only calls the same named method, never a runtime action dispatcher.
macro_rules! methods {
    ($emit:ident) => {
        $emit! {
            required {

                fn commit_workspace(
                    &self,
                    identity: &$crate::Identity,
                    command: $crate::CommitWorkspace,
                ) -> Result<storage_contract::OperationResult, $crate::ApiError>;


                /// Share a frozen published commit after current branch and identity admission.
                /// The returned snapshot owns no authorization for later reads or writes.
                fn snapshot_at(
                    &self,
                    identity: &$crate::Identity,
                    branch: &str,
                    commit: &str,
                ) -> Result<::std::sync::Arc<storage_contract::Commit>, $crate::ApiError>;

                /// Create a workspace using the current authenticated actor and pool admission limits.
                fn create_workspace(
                    &self,
                    identity: &$crate::Identity,
                    command: crate::CreateWorkspace,
                ) -> Result<storage_contract::Workspace, $crate::ApiError>;

                /// Abort only a currently owned workspace; retain existing pending-operation decisions.
                fn abort_workspace(&self, identity: &$crate::Identity, workspace: &str) -> Result<(), $crate::ApiError>;

                fn seal_workspace(
                    &self,
                    identity: &$crate::Identity,
                    workspace: &str,
                ) -> Result<storage_contract::SealedWorkspace, $crate::ApiError>;

                fn commit_workspace_public(
                    &self,
                    identity: &$crate::Identity,
                    command: storage_contract::PublicCommitEnvelope,
                ) -> Result<storage_contract::OperationResult, $crate::ApiError>;

                fn version_snapshot(
                    &self,
                    identity: &$crate::Identity,
                    version: &str,
                ) -> Result<$crate::VersionSnapshot, $crate::ApiError>;

                fn authorize_version(&self, identity: &$crate::Identity, version: &str) -> Result<(), $crate::ApiError>;

                fn repository_snapshot(
                    &self,
                    identity: &$crate::Identity,
                    repository: &str,
                ) -> Result<storage_contract::Repository, $crate::ApiError>;

                fn authorize_repository(&self, identity: &$crate::Identity, repository: &str) -> Result<(), $crate::ApiError>;

                fn workspace_capacity(
                    &self,
                    identity: &$crate::Identity,
                ) -> Result<storage_contract::WorkspaceCapacity, $crate::ApiError>;

                /// Resolve the default share's displayed disk path with file-manager
                /// ownership rules, preserving named repositories and existing roots.
                fn resolve_smb_path(
                    &self,
                    identity: &$crate::Identity,
                    request: crate::ResolveSmbPath,
                ) -> Result<crate::SmbPathLocation, $crate::ApiError>;

                fn smb_binding_status(
                    &self,
                    identity: &$crate::Identity,
                    repository: &str,
                ) -> Result<$crate::SmbBindingStatus, $crate::ApiError>;

                fn update_smb_binding(
                    &self,
                    identity: &$crate::Identity,
                    request: $crate::UpdateSmbBinding<'_>,
                ) -> Result<storage_contract::OperationResult, $crate::ApiError>;

                fn protection_policy(
                    &self,
                    identity: &$crate::Identity,
                    id: &str,
                ) -> Result<storage_contract::ProtectionPolicy, $crate::ApiError>;

                fn placement_rule(
                    &self,
                    identity: &$crate::Identity,
                    id: &str,
                ) -> Result<storage_contract::PlacementRule, $crate::ApiError>;

                fn update_protection_policy(
                    &self,
                    identity: &$crate::Identity,
                    request: $crate::UpdateProtectionPolicy,
                ) -> Result<storage_contract::OperationResult, $crate::ApiError>;

                fn update_placement_rule(
                    &self,
                    identity: &$crate::Identity,
                    request: $crate::UpdatePlacementRule,
                ) -> Result<storage_contract::OperationResult, $crate::ApiError>;

                fn compare_commits(
                    &self,
                    identity: &$crate::Identity,
                    request: $crate::CompareCommits<'_>,
                ) -> Result<storage_contract::CommitComparison, $crate::ApiError>;

                fn integrity_check(
                    &self,
                    identity: &$crate::Identity,
                ) -> Result<storage_contract::CheckReport, $crate::ApiError>;

                fn create_merge_link(
                    &self,
                    identity: &$crate::Identity,
                    request: $crate::CreateMergeLink,
                ) -> Result<storage_contract::MergeLink, $crate::ApiError>;

                fn create_repository(
                    &self,
                    identity: &$crate::Identity,
                    request: $crate::CreateRepository,
                ) -> Result<storage_contract::Repository, $crate::ApiError>;

                fn create_branch(
                    &self,
                    identity: &$crate::Identity,
                    request: $crate::CreateBranch,
                ) -> Result<$crate::BranchSnapshot, $crate::ApiError>;

                fn create_file_branch(
                    &self,
                    identity: &$crate::Identity,
                    request: $crate::CreateFileBranch,
                ) -> Result<storage_contract::OperationResult, $crate::ApiError>;

                fn update_note(
                    &self,
                    identity: &$crate::Identity,
                    request: $crate::UpdateNote,
                ) -> Result<storage_contract::OperationResult, $crate::ApiError>;

                fn merge_file_branches(
                    &self,
                    identity: &$crate::Identity,
                    request: $crate::MergeFileBranches,
                ) -> Result<storage_contract::OperationResult, $crate::ApiError>;

                fn update_file_node_tags(
                    &self,
                    identity: &$crate::Identity,
                    request: $crate::UpdateFileNodeTags,
                ) -> Result<storage_contract::OperationResult, $crate::ApiError>;

                fn create_disk_directory(
                    &self,
                    identity: &$crate::Identity,
                    request: $crate::CreateDiskDirectory,
                ) -> Result<storage_contract::OperationResult, $crate::ApiError>;

                fn create_disk_write_session(
                    &self,
                    identity: &$crate::Identity,
                    request: $crate::CreateDiskWriteSession,
                ) -> Result<crate::DiskWriteSession, $crate::ApiError>;

                fn backup_configuration(
                    &self,
                    identity: &$crate::Identity,
                    request: $crate::ReadBackupConfiguration,
                ) -> Result<storage_contract::BackupConfigurationSnapshot, $crate::ApiError>;

                fn configure_backups(
                    &self,
                    identity: &$crate::Identity,
                    request: $crate::ConfigureBackups,
                ) -> Result<storage_contract::OperationResult, $crate::ApiError>;

                fn plan(&self, identity: &$crate::Identity, plan: &str) -> Result<storage_contract::Plan, $crate::ApiError>;

                /// The outer error belongs to the HTTP keys phase, after current admission.
                /// A successful envelope may still carry an operation error for the later
                /// plan/owner/history phase; only a successful OperationId reaches the engine.
                fn cancel_plan(
                    &self,
                    identity: &$crate::Identity,
                    request: Result<$crate::CancelPlan, crate::ApiError>,
                ) -> Result<storage_contract::OperationResult, $crate::ApiError>;

                /// Online registration is refused after current admission; it has no successful outcome.
                fn register_disk(
                    &self,
                    identity: &crate::auth::Identity,
                ) -> Result<std::convert::Infallible, crate::ApiError>;

                /// Required text/key errors follow current identity; operation format errors follow plan admission.
                fn commit_plan(
                    &self,
                    identity: &$crate::Identity,
                    request: Result<$crate::CommitPlan, $crate::ApiError>,
                ) -> Result<storage_contract::OperationResult, $crate::ApiError>;

                fn prepare_historical_save(
                    &self,
                    identity: &$crate::Identity,
                    request: Result<crate::PrepareHistoricalSave, crate::ApiError>,
                ) -> Result<storage_contract::Plan, $crate::ApiError>;

                fn prepare_purge(
                    &self,
                    identity: &$crate::Identity,
                    request: Result<crate::PreparePurge, crate::ApiError>,
                ) -> Result<storage_contract::Plan, $crate::ApiError>;

                fn prepare_split(
                    &self,
                    identity: &$crate::Identity,
                    request: Result<crate::PrepareSplit, crate::ApiError>,
                ) -> Result<storage_contract::Plan, $crate::ApiError>;

                fn namespace_batch(
                    &self,
                    identity: &$crate::Identity,
                    request: $crate::NamespaceBatch,
                ) -> Result<storage_contract::OperationResult, $crate::ApiError>;

                fn select_file_branch(
                    &self,
                    identity: &$crate::Identity,
                    request: $crate::SelectFileBranch,
                ) -> Result<storage_contract::OperationResult, $crate::ApiError>;

                fn rename_file_branch(
                    &self,
                    identity: &$crate::Identity,
                    request: $crate::RenameFileBranch,
                ) -> Result<storage_contract::OperationResult, $crate::ApiError>;

                fn listing(
                    &self,
                    identity: &$crate::Identity,
                    request: $crate::ReadListing,
                ) -> Result<$crate::ListingSnapshot, $crate::ApiError>;

                fn mutate_disk_directory(
                    &self,
                    identity: &$crate::Identity,
                    request: $crate::MutateDiskDirectory,
                ) -> Result<storage_contract::OperationResult, $crate::ApiError>;

                fn initialize_directory_scope(
                    &self,
                    identity: &$crate::Identity,
                    request: $crate::InitializeDirectoryScope,
                ) -> Result<storage_contract::OperationResult, $crate::ApiError>;

                fn archive_branch(
                    &self,
                    identity: &$crate::Identity,
                    request: $crate::ArchiveBranch,
                ) -> Result<storage_contract::OperationResult, $crate::ApiError>;

                fn rename_directory_branch(
                    &self,
                    identity: &$crate::Identity,
                    request: $crate::RenameDirectoryBranch,
                ) -> Result<storage_contract::OperationResult, $crate::ApiError>;

                fn select_default_directory_branch(
                    &self,
                    identity: &$crate::Identity,
                    request: $crate::SelectDefaultDirectoryBranch,
                ) -> Result<storage_contract::OperationResult, $crate::ApiError>;

                fn directory_selection(
                    &self,
                    identity: &$crate::Identity,
                    request: $crate::ReadDirectorySelection,
                ) -> Result<$crate::DirectorySelectionSnapshot, $crate::ApiError>;

                fn directory_scopes(
                    &self,
                    identity: &$crate::Identity,
                    request: $crate::ReadDirectoryScopes<'_>,
                ) -> Result<$crate::DirectoryScopesSnapshot, $crate::ApiError>;

                fn default_directory_branch(
                    &self,
                    identity: &$crate::Identity,
                    repository_id: &str,
                ) -> Result<$crate::DefaultDirectoryBranchSnapshot, $crate::ApiError>;

                fn history_window(
                    &self,
                    identity: &$crate::Identity,
                    request: storage_contract::history_window::HistoryWindowRequest,
                    cancellation: storage_contract::history_window::HistoryWindowCancellation,
                ) -> Result<storage_contract::history_window::HistoryWindowPage, $crate::ApiError>;

                fn file_history_snapshot(
                    &self,
                    identity: &crate::auth::Identity,
                    request: crate::ReadFileHistory,
                ) -> Result<crate::FileHistorySnapshot, crate::ApiError>;

                fn file_history(
                    &self,
                    identity: &$crate::Identity,
                    file: &str,
                ) -> Result<Vec<storage_contract::FileVersion>, $crate::ApiError>;

                fn placement_snapshot(
                    &self,
                    identity: &$crate::Identity,
                    version: &str,
                ) -> Result<storage_contract::Placement, $crate::ApiError>;

                fn activity_history(&self, identity: &$crate::Identity) -> Result<$crate::ActivityHistorySnapshot, $crate::ApiError>;

                fn history_graph(
                    &self,
                    identity: &$crate::Identity,
                    history: &str,
                ) -> Result<$crate::HistoryGraphSnapshot, $crate::ApiError>;

                fn commit_snapshot(
                    &self,
                    identity: &$crate::Identity,
                    commit: &str,
                ) -> Result<::std::sync::Arc<storage_contract::Commit>, $crate::ApiError>;

                fn resolve_entry(
                    &self,
                    identity: &$crate::Identity,
                    request: $crate::ResolveEntry<'_>,
                ) -> Result<$crate::ResolvedEntry, $crate::ApiError>;

                fn branch_snapshot(
                    &self,
                    identity: &$crate::Identity,
                    branch: &str,
                ) -> Result<$crate::BranchSnapshot, $crate::ApiError>;

                fn branches_snapshot(
                    &self,
                    identity: &$crate::Identity,
                    repository: &str,
                ) -> Result<Vec<$crate::BranchSnapshot>, $crate::ApiError>;

                fn repositories_snapshot(
                    &self,
                    identity: &$crate::Identity,
                ) -> Result<Vec<storage_contract::Repository>, $crate::ApiError>;

                fn tree_snapshot(
                    &self,
                    identity: &$crate::Identity,
                    request: $crate::ReadTree<'_>,
                ) -> Result<$crate::TreeSnapshot, $crate::ApiError>;

                fn create_write_session(
                    &self,
                    identity: &$crate::Identity,
                    branch: &str,
                ) -> Result<$crate::WorkspaceWriteSession, $crate::ApiError>;

                fn stage_namespace(
                    &self,
                    identity: &$crate::Identity,
                    command: $crate::StageNamespace,
                ) -> Result<storage_contract::Workspace, $crate::ApiError>;

                fn workspace(
                    &self,
                    identity: &$crate::Identity,
                    workspace: &str,
                ) -> Result<storage_contract::Workspace, $crate::ApiError>;

                /// None means an absent item in an authorized workspace, never failed admission.
                fn published_upload_status(
                    &self,
                    identity: &$crate::Identity,
                    workspace: &str,
                    item: &str,
                ) -> Result<Option<$crate::PublishedUploadStatus>, $crate::ApiError>;

                /// Admit one write under current authority, retaining the selected file context.
                /// Uploads and active files also require an open lease and enforce size limits.
                fn authorize_workspace_write(
                    &self,
                    identity: &$crate::Identity,
                    workspace: &str,
                    access: crate::WorkspaceWrite<'_>,
                ) -> Result<(), $crate::ApiError>;

                /// Capture or page a directory after current admission; enforce the page byte budget.
                fn read_directory(
                    &self,
                    identity: &$crate::Identity,
                    request: crate::ReadDirectory<'_>,
                ) -> Result<crate::DirectorySnapshot, $crate::ApiError>;

                /// Revalidate current branch admission without capturing namespace entries.
                fn authorize_current(&self, identity: &$crate::Identity, branch: &str) -> Result<(), $crate::ApiError>;

                /// Look up one current entry after live identity and branch admission.
                fn lookup_current(
                    &self,
                    identity: &$crate::Identity,
                    branch: &str,
                    path: &name_policy::LookupPath,
                ) -> Result<Option<storage_contract::Entry>, $crate::ApiError>;

                /// Look up one entry in a frozen published snapshot, with current admission.
                /// A missing path is `Ok(None)`; admission and snapshot failures are errors.
                fn lookup_at(
                    &self,
                    identity: &$crate::Identity,
                    branch: &str,
                    commit: &str,
                    path: &name_policy::LookupPath,
                ) -> Result<Option<storage_contract::Entry>, $crate::ApiError>;

                /// Consume the held inode with positional reads and verify its declared
                /// size/hash against the copied snapshot before accepting any generation.
                /// Publish the next workspace generation chosen under current admission.
                fn upload(
                    &self,
                    identity: &$crate::Identity,
                    workspace: &str,
                    item: &str,
                    path: &name_policy::PortablePath,
                    source: &crate::UploadSource,
                ) -> Result<storage_contract::StagedFile, $crate::ApiError>;

                /// Return a fully verified raw descriptor for writable staging and local consumers.
                fn content(&self, identity: &$crate::Identity, version: &str) -> Result<$crate::Content, $crate::ApiError>;

                /// HTTP and SMB readers verify each requested chunk and retain the read lease.
                fn range_content(
                    &self,
                    identity: &$crate::Identity,
                    request: $crate::ReadContent<'_>,
                ) -> Result<$crate::Content, $crate::ApiError>;

                /// Publish the exact caller generation; adapters must not silently discard it.
                fn upload_generation(
                    &self,
                    identity: &$crate::Identity,
                    workspace: &str,
                    item: &str,
                    path: &name_policy::PortablePath,
                    source: &crate::UploadSource,
                    generation: u64,
                ) -> Result<storage_contract::StagedFile, $crate::ApiError>;
            }
            optional {


                /// Require the original native save's terminal workspace reconciliation.
                /// A published receipt alone does not authorize SMB source cleanup.
                fn require_native_workspace_commit_completion(
                    &self,
                    _identity: &$crate::Identity,
                    _command: $crate::CommitWorkspace,
                ) -> Result<(), $crate::ApiError> {
                    Err($crate::ApiError::unsupported())
                }

                fn merge_file_history(
                    &self,
                    _identity: &$crate::Identity,
                    _request: $crate::MergeFileHistory,
                ) -> Result<storage_contract::OperationResult, $crate::ApiError> {
                    Err($crate::ApiError::unsupported())
                }

                fn move_disk_object(
                    &self,
                    _identity: &$crate::Identity,
                    _request: $crate::MoveDiskObject,
                ) -> Result<storage_contract::OperationResult, $crate::ApiError> {
                    Err($crate::ApiError::unsupported())
                }

                fn placement_configuration(
                    &self,
                    _identity: &$crate::Identity,
                    _request: $crate::ReadPlacementConfiguration,
                ) -> Result<storage_contract::PlacementConfigurationSnapshot, $crate::ApiError> {
                    Err($crate::ApiError::unsupported())
                }

                fn configure_placement(
                    &self,
                    _identity: &$crate::Identity,
                    _request: $crate::ConfigurePlacement,
                ) -> Result<storage_contract::OperationResult, $crate::ApiError> {
                    Err($crate::ApiError::unsupported())
                }

                #[cfg(feature = "core")]
                fn recursive_backup_configuration(
                    &self,
                    _identity: &$crate::Identity,
                    _request: $crate::ReadBackupConfiguration,
                ) -> Result<$crate::RecursiveBackupConfiguration, $crate::ApiError> {
                    Err($crate::ApiError::unsupported())
                }

                #[cfg(feature = "core")]
                fn configure_recursive_backups(
                    &self,
                    _identity: &$crate::Identity,
                    _request: $crate::ConfigureRecursiveBackups,
                ) -> Result<storage_contract::OperationResult, $crate::ApiError> {
                    Err($crate::ApiError::unsupported())
                }

                #[cfg(feature = "core")]
                fn configure_recursive_backup_tree(
                    &self,
                    _identity: &$crate::Identity,
                    _request: $crate::ConfigureRecursiveBackups,
                ) -> Result<storage_contract::OperationResult, $crate::ApiError> {
                    Err($crate::ApiError::unsupported())
                }

                fn soft_delete_file(
                    &self,
                    _identity: &$crate::Identity,
                    _command: crate::DeleteFile,
                ) -> Result<storage_contract::OperationResult, $crate::ApiError> {
                    Err($crate::ApiError::unsupported())
                }

                fn move_current_file(
                    &self,
                    _identity: &$crate::Identity,
                    _command: crate::MoveCurrentFile,
                ) -> Result<storage_contract::OperationResult, $crate::ApiError> {
                    Err($crate::ApiError::unsupported())
                }

                fn move_current_object(
                    &self,
                    _identity: &$crate::Identity,
                    _command: crate::MoveCurrentObject,
                ) -> Result<storage_contract::OperationResult, $crate::ApiError> {
                    Err($crate::ApiError::unsupported())
                }

                fn replace_current_file(
                    &self,
                    _identity: &$crate::Identity,
                    _command: crate::ReplaceCurrentFile,
                ) -> Result<storage_contract::OperationResult, $crate::ApiError> {
                    Err($crate::ApiError::unsupported())
                }

                fn move_current_file_to_empty_disk(
                    &self,
                    _identity: &$crate::Identity,
                    _command: crate::MoveCurrentFileToEmptyDisk,
                ) -> Result<storage_contract::OperationResult, $crate::ApiError> {
                    Err($crate::ApiError::unsupported())
                }

                /// Delete an exact current file through its reversible lifecycle. `None`
                /// identifies a legacy structural namespace without publishing a deletion.
                fn delete_current_file(
                    &self,
                    _identity: &$crate::Identity,
                    _command: crate::DeleteCurrentFile,
                ) -> Result<Option<storage_contract::OperationResult>, $crate::ApiError> {
                    Err($crate::ApiError::unsupported())
                }

                fn prepare_current_directory_deletion(
                    &self,
                    _identity: &$crate::Identity,
                    _command: crate::PrepareCurrentDirectoryDeletion,
                ) -> Result<Option<crate::CurrentDirectoryDeletion>, $crate::ApiError> {
                    Err($crate::ApiError::unsupported())
                }

                fn delete_current_directory(
                    &self,
                    _identity: &$crate::Identity,
                    _command: crate::DeleteCurrentDirectory,
                ) -> Result<storage_contract::OperationResult, $crate::ApiError> {
                    Err($crate::ApiError::unsupported())
                }

                fn directory_deletions(
                    &self,
                    _identity: &$crate::Identity,
                    _repository: &str,
                    _owner: &str,
                    _request: crate::ReadDirectoryDeletions,
                ) -> Result<storage_contract::directory_git::DirectoryDeletionsPage, $crate::ApiError> {
                    Err($crate::ApiError::unsupported())
                }

                fn current_directory_deletion(
                    &self,
                    _identity: &$crate::Identity,
                    _repository: &str,
                    _owner: &str,
                    _directory: &str,
                ) -> Result<Option<storage_contract::directory_git::DirectoryDeletionNode>, $crate::ApiError> {
                    Err($crate::ApiError::unsupported())
                }

                fn restore_deleted_directory(
                    &self,
                    _identity: &$crate::Identity,
                    _repository: &str,
                    _request: &storage_contract::directory_git::UndoDirectoryDeletionBody,
                ) -> Result<storage_contract::OperationResult, $crate::ApiError> {
                    Err($crate::ApiError::unsupported())
                }

                fn current_file_deletion(
                    &self,
                    _identity: &$crate::Identity,
                    _file: &storage_contract::history_window::HistoryFileId,
                ) -> Result<Option<storage_contract::FileDeletionNode>, $crate::ApiError> {
                    Err($crate::ApiError::unsupported())
                }

                fn file_deletions(
                    &self,
                    _identity: &$crate::Identity,
                    _request: crate::ReadFileDeletions,
                ) -> Result<crate::FileDeletionsPage, $crate::ApiError> {
                    Err($crate::ApiError::unsupported())
                }

                fn file_deletion_batch_undo_preview(
                    &self,
                    _identity: &$crate::Identity,
                    _operation: storage_contract::OperationId,
                ) -> Result<storage_contract::file_deletion_batch::FileDeletionBatchUndoPreview, $crate::ApiError> {
                    Err($crate::ApiError::unsupported())
                }

                fn restore_deleted_file_batch(
                    &self,
                    _identity: &$crate::Identity,
                    _request: &storage_contract::file_deletion_batch::UndoFileDeletionBatchBody,
                ) -> Result<storage_contract::OperationResult, $crate::ApiError> {
                    Err($crate::ApiError::unsupported())
                }

                fn restore_deleted_file(
                    &self,
                    _identity: &$crate::Identity,
                    _command: crate::RestoreDeletedFile,
                ) -> Result<storage_contract::OperationResult, $crate::ApiError> {
                    Err($crate::ApiError::unsupported())
                }

                fn authorize_listing_snapshot(
                    &self,
                    _identity: &$crate::Identity,
                    _snapshot: &str,
                ) -> Result<(), $crate::ApiError> {
                    Err($crate::ApiError::unsupported())
                }

                fn preview_node_retirement(
                    &self,
                    _identity: &$crate::Identity,
                    _request: storage_contract::node_retirement::Selection,
                    _cancellation: storage_contract::history_window::HistoryWindowCancellation,
                ) -> Result<storage_contract::node_retirement::Preview, $crate::ApiError> {
                    Err($crate::ApiError::unsupported())
                }

                fn retire_file_nodes(
                    &self,
                    _identity: &$crate::Identity,
                    _request: storage_contract::node_retirement::Selection,
                    _operation: storage_contract::OperationId,
                ) -> Result<storage_contract::node_retirement::Outcome, $crate::ApiError> {
                    Err($crate::ApiError::unsupported())
                }

                fn resume_node_retirement(
                    &self,
                    _identity: &$crate::Identity,
                    _file: storage_contract::history_window::HistoryFileId,
                    _operation: storage_contract::OperationId,
                ) -> Result<storage_contract::node_retirement::Outcome, $crate::ApiError> {
                    Err($crate::ApiError::unsupported())
                }

                fn detached_history(
                    &self,
                    _identity: &$crate::Identity,
                    _request: storage_contract::detached_history::Request,
                    _cancellation: storage_contract::history_window::HistoryWindowCancellation,
                ) -> Result<storage_contract::detached_history::Page, $crate::ApiError> {
                    Err($crate::ApiError::unsupported())
                }

                fn shared_history(
                    &self,
                    _identity: &$crate::Identity,
                    _request: storage_contract::shared_history::SharedHistoryRequest,
                    _cancellation: storage_contract::history_window::HistoryWindowCancellation,
                ) -> Result<storage_contract::shared_history::SharedHistoryGraph, $crate::ApiError> {
                    Err($crate::ApiError::unsupported())
                }

                fn content_matches(
                    &self,
                    _identity: &$crate::Identity,
                    _request: storage_contract::content_matches::ContentMatchRequest,
                    _cancellation: storage_contract::history_window::HistoryWindowCancellation,
                ) -> Result<storage_contract::content_matches::ContentMatchPage, $crate::ApiError> {
                    Err($crate::ApiError::unsupported())
                }

                fn file_history_source(
                    &self,
                    _identity: &$crate::Identity,
                    _request: storage_contract::file_history_merge::FileHistorySourceRequest,
                    _cancellation: storage_contract::history_window::HistoryWindowCancellation,
                ) -> Result<storage_contract::file_history_merge::FileHistorySource, $crate::ApiError> {
                    Err($crate::ApiError::unsupported())
                }

                fn directory_git_status(
                    &self,
                    _identity: &$crate::Identity,
                    _repository: &str,
                    _path: &str,
                    _cancellation: storage_contract::history_window::HistoryWindowCancellation,
                ) -> Result<storage_contract::directory_git::DirectoryGitStatus, $crate::ApiError> {
                    Err($crate::ApiError::unsupported())
                }

                fn reconcile_directory_layout(
                    &self,
                    _identity: &$crate::Identity,
                    _repository: &str,
                    _request: storage_contract::directory_git::DirectoryLayoutReconcileBody,
                ) -> Result<storage_contract::directory_git::DirectoryLayoutResult, $crate::ApiError> {
                    Err($crate::ApiError::unsupported())
                }

                fn initialize_directory_git(
                    &self,
                    _identity: &$crate::Identity,
                    _repository: &str,
                    _request: storage_contract::directory_git::DirectoryGitInitializeBody,
                ) -> Result<storage_contract::OperationResult, $crate::ApiError> {
                    Err($crate::ApiError::unsupported())
                }

                fn stage_directory_git_child(
                    &self,
                    _identity: &$crate::Identity,
                    _repository: &str,
                    _request: storage_contract::directory_git::DirectoryGitStageChildBody,
                ) -> Result<storage_contract::OperationResult, $crate::ApiError> {
                    Err($crate::ApiError::unsupported())
                }

                fn stage_directory_git_owned(
                    &self,
                    _identity: &$crate::Identity,
                    _repository: &str,
                    _request: storage_contract::directory_git::DirectoryGitStageOwnedBody,
                ) -> Result<storage_contract::OperationResult, $crate::ApiError> {
                    Err($crate::ApiError::unsupported())
                }

                fn directory_staging(
                    &self,
                    _identity: &$crate::Identity,
                    _repository: &str,
                    _branch: &str,
                ) -> Result<storage_contract::directory_staging::DirectoryStagingStatus, $crate::ApiError> {
                    Err($crate::ApiError::unsupported())
                }

                fn commit_directory_changes(
                    &self,
                    _identity: &$crate::Identity,
                    _repository: &str,
                    _branch: &str,
                    _request: storage_contract::directory_staging::DirectoryCommitBody,
                ) -> Result<storage_contract::OperationResult, $crate::ApiError> {
                    Err($crate::ApiError::unsupported())
                }

                fn directory_history(
                    &self,
                    _identity: &$crate::Identity,
                    _request: storage_contract::directory_history::DirectoryHistoryRequest,
                    _cancellation: storage_contract::history_window::HistoryWindowCancellation,
                ) -> Result<storage_contract::directory_history::DirectoryHistoryPage, $crate::ApiError> {
                    Err($crate::ApiError::unsupported())
                }

                fn workspace_content(
                    &self,
                    _identity: &$crate::Identity,
                    _workspace: &str,
                    _item: &str,
                ) -> Result<$crate::Content, $crate::ApiError> {
                    Err($crate::ApiError::unsupported())
                }

                fn checksums(&self, _identity: &$crate::Identity, _version: &str) -> Result<Vec<u8>, $crate::ApiError> {
                    Err($crate::ApiError::unsupported())
                }
            }
        }
    };
}

macro_rules! declare_backend_methods {
    (required { $($required:tt)* } optional { $($optional:tt)* }) => {
        $($required)*
        $($optional)*
    };
}
macro_rules! forward_authorized_backend_methods {
    (
        required { $(
            $(#[$required_attribute:meta])*
            fn $required_method:ident(
                &$required_self:ident,
                $required_identity:ident: $required_identity_type:ty
                $(, $required_argument:ident: $required_type:ty)* $(,)?
            ) -> $required_result:ty;
        )* }
        optional { $(
            $(#[$optional_attribute:meta])*
            fn $optional_method:ident(
                &$optional_self:ident,
                $optional_identity:ident: $optional_identity_type:ty
                $(, $optional_argument:ident: $optional_type:ty)* $(,)?
            ) -> $optional_result:ty { $($default:tt)* }
        )* }
    ) => {
        $(
            $(#[$required_attribute])*
            fn $required_method(
                &$required_self,
                $required_identity: $required_identity_type,
                $($required_argument: $required_type),*
            ) -> $required_result {
                $required_self.backend.$required_method(&$required_self.current($required_identity)?, $($required_argument),*)
            }
        )*
        $(
            $(#[$optional_attribute])*
            fn $optional_method(
                &$optional_self,
                $optional_identity: $optional_identity_type,
                $($optional_argument: $optional_type),*
            ) -> $optional_result {
                $optional_self.backend.$optional_method(&$optional_self.current($optional_identity)?, $($optional_argument),*)
            }
        )*
    };
}

#[cfg(feature = "core")]
macro_rules! forward_managed_backend_methods {
    (
        required { $(
            $(#[$required_attribute:meta])*
            fn $required_method:ident(
                &$required_self:ident,
                $required_identity:ident: $required_identity_type:ty
                $(, $required_argument:ident: $required_type:ty)* $(,)?
            ) -> $required_result:ty;
        )* }
        optional { $(
            $(#[$optional_attribute:meta])*
            fn $optional_method:ident(
                &$optional_self:ident,
                $optional_identity:ident: $optional_identity_type:ty
                $(, $optional_argument:ident: $optional_type:ty)* $(,)?
            ) -> $optional_result:ty { $($default:tt)* }
        )* }
    ) => {
        $(
            $(#[$required_attribute])*
            fn $required_method(
                &$required_self,
                $required_identity: $required_identity_type,
                $($required_argument: $required_type),*
            ) -> $required_result {
                $required_self.inner.backend()?.$required_method($required_identity, $($required_argument),*)
            }
        )*
        $(
            $(#[$optional_attribute])*
            fn $optional_method(
                &$optional_self,
                $optional_identity: $optional_identity_type,
                $($optional_argument: $optional_type),*
            ) -> $optional_result {
                $optional_self.inner.backend()?.$optional_method($optional_identity, $($optional_argument),*)
            }
        )*
    };
}

#[cfg(feature = "core")]
pub(crate) use forward_managed_backend_methods;
pub(crate) use {declare_backend_methods, forward_authorized_backend_methods, methods};
