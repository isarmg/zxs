use super::*;
use smb_protocol::protocol::NamespaceOperation;

#[derive(Clone, Copy, Debug)]
enum ConcurrentNamespace {
    FileToDirectory,
    RemoveChild,
    CreateMissing,
}

fn publish_namespace(
    backend: &dyn Backend,
    identity: &Identity,
    branch: &str,
    changes: &[(&str, &str)],
) -> Result<Value, ApiError> {
    let workspace = backend.create_workspace(
        identity,
        crate::CreateWorkspace {
            branch_id: branch.into(),
            source_commit_id: None,
        },
    )?;
    for (kind, path) in changes {
        backend.stage_namespace(
            identity,
            crate::StageNamespace {
                workspace_id: workspace.workspace_id.clone(),
                change: storage_contract::NamespaceOperation::parse(
                    kind.parse().unwrap(),
                    path,
                    None,
                )
                .unwrap(),
            },
        )?;
    }
    backend
        .commit_workspace(
            identity,
            crate::CommitWorkspace {
                workspace_id: workspace.workspace_id,
                operation_id: OperationId::generate(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "concurrent namespace fixture".into(),
                request_digest: None,
            },
        )
        .map(|result| json!(result))
}

struct NamespaceAfterWorkspace {
    inner: Arc<dyn Backend>,
    change: ConcurrentNamespace,
    enabled: AtomicBool,
    captured: Mutex<Option<Workspace>>,
}

impl Backend for NamespaceAfterWorkspace {
    fn operation(
        &self,
        identity: &Identity,
        operation_id: storage_contract::OperationId,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        self.inner.operation(identity, operation_id)
    }
    fn commit_workspace(
        &self,
        identity: &Identity,
        command: crate::CommitWorkspace,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        self.inner.commit_workspace(identity, command)
    }

    fn snapshot_at(
        &self,
        identity: &Identity,
        branch: &str,
        commit: &str,
    ) -> Result<Arc<storage_contract::Commit>, ApiError> {
        self.inner.snapshot_at(identity, branch, commit)
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
        let result = self.inner.create_workspace(identity, command)?;
        if self.enabled.swap(false, Ordering::SeqCst) {
            let workspace = result.clone();
            let changes: &[(&str, &str)] = match self.change {
                ConcurrentNamespace::FileToDirectory => {
                    &[("delete", "report.txt"), ("mkdir", "report.txt")]
                }
                ConcurrentNamespace::RemoveChild => &[("delete", "target/child")],
                ConcurrentNamespace::CreateMissing => &[("mkdir", "missing")],
            };
            publish_namespace(self.inner.as_ref(), identity, &workspace.branch_id, changes)?;
            *self.captured.lock().unwrap() = Some(workspace);
        }
        Ok(result)
    }
    fn abort_workspace(
        &self,
        identity: &crate::auth::Identity,
        workspace: &str,
    ) -> Result<(), crate::ApiError> {
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
    ) -> Result<crate::ListingSnapshot, crate::ApiError> {
        self.inner.listing(identity, request)
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
    ) -> Result<crate::FileHistorySnapshot, crate::ApiError> {
        self.inner.file_history_snapshot(identity, request)
    }
    fn file_history(
        &self,
        identity: &crate::auth::Identity,
        file: &str,
    ) -> Result<Vec<storage_contract::FileVersion>, crate::ApiError> {
        self.inner.file_history(identity, file)
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
    ) -> Result<crate::ActivityHistorySnapshot, crate::ApiError> {
        self.inner.activity_history(identity)
    }
    fn history_graph(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<crate::HistoryGraphSnapshot, crate::ApiError> {
        self.inner.history_graph(identity, id)
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
    ) -> Result<crate::TreeSnapshot, crate::ApiError> {
        self.inner.tree_snapshot(identity, request)
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
        identity: &Identity,
        branch: &str,
        commit: &str,
        path: &name_policy::LookupPath,
    ) -> Result<Option<Entry>, ApiError> {
        self.inner.lookup_at(identity, branch, commit, path)
    }

    fn call(
        &self,
        action: crate::BackendAction,
        identity: &Identity,
        input: Value,
    ) -> Result<Value, ApiError> {
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

#[test]
fn namespace_preconditions_use_workspace_base_after_concurrent_head_changes() {
    for (change, kind, path, destination, expected) in [
        (
            ConcurrentNamespace::FileToDirectory,
            NamespaceOperation::Rmdir,
            "report.txt",
            None,
            storage_contract::ErrorCode::NotDirectory,
        ),
        (
            ConcurrentNamespace::RemoveChild,
            NamespaceOperation::Rmdir,
            "target",
            None,
            storage_contract::ErrorCode::DirectoryNotEmpty,
        ),
        (
            ConcurrentNamespace::RemoveChild,
            NamespaceOperation::Rename,
            "source",
            Some("target"),
            storage_contract::ErrorCode::DirectoryNotEmpty,
        ),
        (
            ConcurrentNamespace::CreateMissing,
            NamespaceOperation::Rmdir,
            "missing",
            None,
            storage_contract::ErrorCode::NotFound,
        ),
    ] {
        let (_temp, original, _backend, branch) = fixture();
        publish_namespace(
            original.backend.as_ref(),
            &identity(&original),
            &branch,
            &[
                ("mkdir", "source"),
                ("mkdir", "target"),
                ("mkdir", "target/child"),
            ],
        )
        .unwrap();
        let backend = Arc::new(NamespaceAfterWorkspace {
            inner: Arc::clone(&original.backend),
            change,
            enabled: AtomicBool::new(true),
            captured: Mutex::new(None),
        });
        let shared = Arc::new(Shared {
            backend: backend.clone(),
            username: original.username.clone(),
            auth: original.auth.clone(),
            spool: original.spool.clone(),
            budget: Arc::clone(&original.budget),
            max_file_bytes: original.max_file_bytes,
            authenticated_sessions: tokio::sync::Semaphore::new(MAX_AUTHENTICATED_SESSIONS),
            file_writers: Mutex::new(BTreeSet::new()),
            namespace_operations: Mutex::new(BTreeSet::new()),
        });
        let mut session = session(shared, &branch);
        let operation_id = new_id("op");
        let error = session
            .dispatch(Request::Namespace {
                operation_id: operation_id.clone(),
                kind,
                path: path.into(),
                destination: destination.map(str::to_owned),
                replace: destination.is_some(),
            })
            .unwrap_err();
        assert_eq!(error.code, expected, "{change:?} {kind:?}");
        assert_eq!(error.operation_id.as_deref(), Some(operation_id.as_str()));
        let captured = backend.captured.lock().unwrap().take().unwrap();
        let current = session.entries().unwrap();
        match change {
            ConcurrentNamespace::FileToDirectory => assert_eq!(
                current
                    .iter()
                    .find(|entry| entry.path() == "report.txt")
                    .unwrap()
                    .kind,
                storage_contract::NamespaceEntryKind::Directory
            ),
            ConcurrentNamespace::RemoveChild => {
                assert!(current.iter().any(|entry| entry.path() == "target"));
                assert!(!current.iter().any(|entry| entry.path() == "target/child"));
            }
            ConcurrentNamespace::CreateMissing => {
                assert!(current.iter().any(|entry| entry.path() == "missing"));
            }
        }
        let aborted = backend
            .inner
            .workspace(&session.identity, &captured.workspace_id)
            .unwrap();
        assert_eq!(aborted.state, storage_contract::WorkspaceState::Aborted);
        assert!(session.writer_locks.is_empty());
        assert!(session.shared.file_writers.lock().unwrap().is_empty());
        assert!(
            session
                .shared
                .namespace_operations
                .lock()
                .unwrap()
                .is_empty()
        );
        assert!(
            !session
                .shared
                .spool
                .join(format!("{operation_id}.namespace.json"))
                .exists()
        );
        assert!(
            backend
                .inner
                .operation(&session.identity, operation_id.parse().unwrap())
                .and_then(crate::operation_wire::operation_wire)
                .is_err()
        );
    }
}

#[test]
fn publication_comparison_uses_entry_identity_in_renamed_commit() {
    let (_temp, shared, backend, branch) = fixture();
    let mut session = session(shared, &branch);
    let (opened, file) = open_file(&mut session, true);
    file.write_all_at(b"new", 0).unwrap();
    session.dirty(opened.handle).unwrap();
    backend.fail.store(true, Ordering::SeqCst);
    assert_eq!(
        session.flush(opened.handle).unwrap_err().code,
        storage_contract::ErrorCode::InjectedFailure
    );
    session.flush(opened.handle).unwrap();
    assert_eq!(backend.uploads.load(Ordering::SeqCst), 1);

    let workspace = backend
        .create_workspace(
            &session.identity,
            crate::CreateWorkspace {
                branch_id: branch.clone(),
                source_commit_id: None,
            },
        )
        .unwrap();
    backend
        .stage_namespace(
            &session.identity,
            crate::StageNamespace {
                workspace_id: workspace.workspace_id.clone(),
                change: storage_contract::NamespaceOperation::Rename {
                    source: name_policy::LookupPath::parse("report.txt").unwrap(),
                    destination: name_policy::PortablePath::parse("renamed.txt").unwrap(),
                },
            },
        )
        .unwrap();
    let renamed_publication = backend
        .commit_workspace(
            &session.identity,
            crate::CommitWorkspace {
                workspace_id: workspace.workspace_id,
                operation_id: OperationId::generate(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "rename identity fixture".into(),
                request_digest: None,
            },
        )
        .unwrap();
    assert!(
        session
            .find(&name_policy::LookupPath::parse("report.txt").unwrap())
            .unwrap()
            .is_none()
    );
    let renamed = session
        .find(&name_policy::LookupPath::parse("renamed.txt").unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(renamed.entry_id, opened.entry_id);

    // Compare the actual renamed snapshot while the open descriptor keeps its old path.
    let Handle::Write(writer) = session.handles.get_mut(&opened.handle).unwrap() else {
        panic!("expected write handle");
    };
    super::super::publication::finish_publication(
        &session.shared,
        &session.identity,
        &branch,
        writer,
        renamed_publication,
    )
    .unwrap();
    assert!(writer.committed);
    assert!(!writer.dirty);
    assert_eq!(writer.path.as_str(), "report.txt");
    assert_eq!(writer.item_id, renamed.entry_id);
    assert_eq!(backend.uploads.load(Ordering::SeqCst), 1);
    let content = backend
        .content(&session.identity, &renamed.version_id)
        .unwrap();
    let mut bytes = [0; 3];
    content.file.read_exact_at(&mut bytes, 0).unwrap();
    assert_eq!(&bytes, b"new");
    session.close(opened.handle, false).unwrap();
    assert!(session.writer_locks.is_empty());
    assert!(session.shared.file_writers.lock().unwrap().is_empty());
}
