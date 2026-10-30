//! Namespace kind admission and uncertain publication over the real RPC bridge.
use super::*;
use std::{os::unix::net::UnixStream, thread::JoinHandle};

fn connect(shared: Arc<Shared>) -> (FrameStream, JoinHandle<()>) {
    let repository = shared
        .backend
        .repositories_snapshot(&identity(&shared))
        .unwrap()[0]
        .repository_id
        .clone();
    // No listening filesystem socket or external service is needed. This is
    // the same framed protocol on two real, connected Unix descriptors.
    let (client, server) = UnixStream::pair().unwrap();
    let worker = std::thread::spawn(move || {
        run_connection(FrameStream::new(server).unwrap(), shared).unwrap();
    });
    let mut client = FrameStream::new(client).unwrap();
    call(
        &mut client,
        Request::Hello {
            protocol: PROTOCOL.into(),
            capabilities: REQUIRED_CAPABILITIES
                .iter()
                .map(|value| (*value).into())
                .collect(),
            username: "alice".into(),
            repository_id: repository,
        },
    )
    .unwrap();
    (client, worker)
}

fn call(client: &mut FrameStream, request: Request) -> Result<Value, Response> {
    client.send(&request, None).unwrap();
    let (response, descriptor) = client.receive::<Response>().unwrap();
    assert!(descriptor.is_none());
    match response {
        Response::Ok { result } => Ok(result),
        response => Err(response),
    }
}

#[test]
fn rpc_flush_returns_published_receipt_when_index_projection_fails() {
    use super::projection_failure::{fail_projection, projection_sequence, receipt_outcome};
    let (temp, shared, backend, branch) = fixture();
    let (mut wire, worker) = connect(shared.clone());
    wire.send(
        &Request::<String>::Open {
            path: "report.txt".into(),
            writable: true,
            create: false,
            truncate: false,
            exclusive: false,
            write_through: false,
        },
        None,
    )
    .unwrap();
    let (response, descriptor) = wire.receive::<Response>().unwrap();
    let Response::Ok { result } = response else {
        panic!("writer must open: {response:?}")
    };
    let opened: OpenResult = json_codec::from_value(result).unwrap();
    let file = File::from(descriptor.unwrap());
    file.write_all_at(b"new", 0).unwrap();
    call(
        &mut wire,
        Request::Dirty {
            handle: opened.handle,
        },
    )
    .unwrap();
    let crate::Readiness::Engine(before) = backend.ready().unwrap() else {
        panic!("expected real engine readiness")
    };
    let index = temp.path().join("state/index.sqlite");
    let projected_before = fail_projection(&index);
    assert_eq!(projected_before, before.sequence.to_string());

    let saved = call(
        &mut wire,
        Request::Flush {
            handle: opened.handle,
        },
    )
    .unwrap();
    assert_eq!(saved["commit_state"], "PUBLISHED");
    assert_eq!(saved["result"]["outcome"], "COMMITTED_AND_PROTECTED");
    assert_eq!(
        saved["result"]["publication_sequence"],
        (before.sequence + 1).to_string()
    );
    assert_eq!(projection_sequence(&index), projected_before);
    let operation = saved["operation_id"].as_str().unwrap().parse().unwrap();
    let queried = crate::operation_wire::operation_wire(
        backend
            .operation_outcome(&identity(&shared), operation)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(receipt_outcome(&queried), receipt_outcome(&saved));
    let uploads = backend.uploads.load(Ordering::SeqCst);
    assert_eq!(
        call(
            &mut wire,
            Request::Flush {
                handle: opened.handle
            }
        )
        .unwrap(),
        json!({"saved":true,"changed":false})
    );
    assert_eq!(backend.uploads.load(Ordering::SeqCst), uploads);
    let entry = session(shared.clone(), &branch)
        .entries()
        .unwrap()
        .into_iter()
        .find(|entry| entry.path == "report.txt")
        .unwrap();
    assert_eq!(
        backend
            .file_history(&identity(&shared), &entry.file_id)
            .unwrap()
            .len(),
        2
    );
    let crate::Readiness::Engine(after) = backend.ready().unwrap() else {
        panic!("expected real engine readiness")
    };
    assert_eq!(after.sequence, before.sequence + 1);
    assert_eq!(
        after.history_index,
        crate::HistoryIndexStatus::Unavailable {
            code: storage_contract::ErrorCode::IndexRebuildFailed,
        }
    );
    call(
        &mut wire,
        Request::Close {
            handle: opened.handle,
        },
    )
    .unwrap();
    drop(file);
    drop(wire);
    worker.join().unwrap();
}

#[test]
fn rpc_namespace_returns_published_receipt_and_does_not_replay_after_index_failure() {
    use super::projection_failure::{fail_projection, projection_sequence, receipt_outcome};
    let (temp, shared, backend, branch) = fixture();
    let (mut wire, worker) = connect(shared.clone());
    let operation = storage_contract::OperationId::generate();
    let request = || Request::Namespace {
        operation_id: operation.to_string(),
        kind: smb_protocol::protocol::NamespaceOperation::Rename,
        path: "report.txt".into(),
        destination: Some("renamed.txt".into()),
        replace: false,
    };
    let crate::Readiness::Engine(before) = backend.ready().unwrap() else {
        panic!("expected real engine readiness")
    };
    let index = temp.path().join("state/index.sqlite");
    let projected_before = fail_projection(&index);
    assert_eq!(projected_before, before.sequence.to_string());

    let saved = call(&mut wire, request()).unwrap();
    assert_eq!(saved["operation_id"], operation.to_string());
    assert_eq!(saved["commit_state"], "PUBLISHED");
    assert_eq!(
        saved["publication_sequence"],
        (before.sequence + 1).to_string()
    );
    assert_eq!(saved["protection_at_publish"], "NOT_APPLICABLE");
    assert_eq!(projection_sequence(&index), projected_before);
    let queried = crate::operation_wire::operation_wire(
        backend
            .operation_outcome(&identity(&shared), operation)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(receipt_outcome(&queried), receipt_outcome(&saved));
    assert_eq!(
        fs::symlink_metadata(shared.spool.join(format!("{operation}.namespace.json")))
            .unwrap_err()
            .kind(),
        io::ErrorKind::NotFound
    );
    assert!(matches!(
        call(&mut wire, request()).unwrap_err(),
        Response::Error {
            code: storage_contract::ErrorCode::OperationIdConflict,
            ..
        }
    ));
    let crate::Readiness::Engine(after) = backend.ready().unwrap() else {
        panic!("expected real engine readiness")
    };
    assert_eq!(after.sequence, before.sequence + 1);
    assert_eq!(
        after.history_index,
        crate::HistoryIndexStatus::Unavailable {
            code: storage_contract::ErrorCode::IndexRebuildFailed,
        }
    );
    let entries = session(shared.clone(), &branch).entries().unwrap();
    assert!(entries.iter().any(|entry| entry.path == "renamed.txt"));
    assert!(!entries.iter().any(|entry| entry.path == "report.txt"));
    drop(wire);
    worker.join().unwrap();
}

#[test]
fn unlink_and_rmdir_preserve_targets_of_the_wrong_kind() {
    let (_temp, shared, _backend, branch) = fixture();
    let mut session = session(shared, &branch);
    session
        .dispatch(Request::Namespace {
            operation_id: new_id("op"),
            kind: smb_protocol::protocol::NamespaceOperation::Mkdir,
            path: "empty".into(),
            destination: None,
            replace: false,
        })
        .unwrap();
    for (kind, path, expected) in [
        (
            smb_protocol::protocol::NamespaceOperation::Rmdir,
            "report.txt",
            storage_contract::ErrorCode::NotDirectory,
        ),
        (
            smb_protocol::protocol::NamespaceOperation::Unlink,
            "empty",
            storage_contract::ErrorCode::IsDirectory,
        ),
    ] {
        let error = session
            .dispatch(Request::Namespace {
                operation_id: new_id("op"),
                kind,
                path: path.into(),
                destination: None,
                replace: false,
            })
            .unwrap_err();
        assert_eq!(error.code, expected);
        assert!(
            session
                .find(&name_policy::LookupPath::parse(path).unwrap())
                .unwrap()
                .is_some()
        );
        assert!(session.writer_locks.is_empty());
    }
}

struct NamespaceFailure {
    inner: Arc<dyn Backend>,
    publish: bool,
    spool: PathBuf,
    commits: Mutex<Vec<Value>>,
    aborts: AtomicUsize,
}
impl Backend for NamespaceFailure {
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
    ) -> Result<storage_contract::OperationOutcome, ApiError> {
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
        let input = json!({"workspace_id":command.workspace_id,"operation_id":command.operation_id,
            "expected_head":command.expected_head,"fencing_token":command.fencing_token.to_string(),"message":command.message});

        let path = self.spool.join(format!(
            "{}.namespace.json",
            input["operation_id"].as_str().unwrap()
        ));
        let record: Value = json_codec::from_slice(
            &fs::read(&path).expect("recovery record must precede submission"),
        )
        .unwrap();
        assert_eq!(record["commit_request"], input);
        assert_eq!(fs::metadata(&path).unwrap().mode() & 0o7777, 0o600);
        self.commits.lock().unwrap().push(input.clone());
        if self.publish {
            self.inner.commit_workspace(identity, command)?;
        }
        Err(ApiError::new(
            ::http::StatusCode::SERVICE_UNAVAILABLE,
            storage_contract::ErrorCode::InjectedFailure,
            "injected uncertain publication outcome",
        ))
    }

    fn snapshot_at(
        &self,
        identity: &crate::auth::Identity,
        branch: &str,
        commit: &str,
    ) -> Result<std::sync::Arc<storage_contract::Commit>, crate::ApiError> {
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
        self.inner.create_workspace(identity, command)
    }
    fn abort_workspace(
        &self,
        identity: &crate::auth::Identity,
        workspace: &str,
    ) -> Result<(), crate::ApiError> {
        self.aborts.fetch_add(1, Ordering::SeqCst);
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
fn namespace_submission_errors_retain_original_operation_without_abort() {
    for publish in [false, true] {
        let (_temp, original, _backend, branch) = fixture();
        let backend = Arc::new(NamespaceFailure {
            inner: original.backend.clone(),
            publish,
            spool: original.spool.clone(),
            commits: Mutex::new(Vec::new()),
            aborts: AtomicUsize::new(0),
        });
        let shared = Arc::new(Shared {
            backend: backend.clone(),
            username: original.username.clone(),
            auth: original.auth.clone(),
            spool: original.spool.clone(),
            budget: original.budget.clone(),
            max_file_bytes: original.max_file_bytes,
            authenticated_sessions: tokio::sync::Semaphore::new(MAX_AUTHENTICATED_SESSIONS),
            file_writers: Mutex::new(BTreeSet::new()),
            namespace_operations: Mutex::new(BTreeSet::new()),
        });
        let (mut client, worker) = connect(shared.clone());
        let error = call(
            &mut client,
            Request::Namespace {
                operation_id: new_id("op"),
                kind: smb_protocol::protocol::NamespaceOperation::Rename,
                path: "report.txt".into(),
                destination: Some("renamed.txt".into()),
                replace: false,
            },
        )
        .unwrap_err();
        let Response::Error {
            code,
            operation_id,
            retryable,
            ..
        } = error
        else {
            panic!("expected an error response");
        };
        assert_eq!(code, storage_contract::ErrorCode::InjectedFailure);
        assert!(
            !retryable,
            "namespace must not suggest replaying an unknown outcome"
        );
        let commit = backend.commits.lock().unwrap()[0].clone();
        assert_eq!(operation_id.as_deref(), commit["operation_id"].as_str());
        assert_eq!(
            backend.aborts.load(Ordering::SeqCst),
            0,
            "uncertain submission must never abort"
        );
        let records = fs::read_dir(&shared.spool)
            .unwrap()
            .filter_map(|entry| {
                let path = entry.unwrap().path();
                path.to_string_lossy()
                    .ends_with(".namespace.json")
                    .then_some(path)
            })
            .collect::<Vec<_>>();
        assert_eq!(records.len(), 1);
        let record: Value = json_codec::from_slice(&fs::read(&records[0]).unwrap()).unwrap();
        assert_eq!(record["commit_request"], commit);
        assert_eq!(record["principal"], "alice");
        assert_eq!(record["branch_id"], branch);
        assert_eq!(record["path"], "report.txt");
        assert_eq!(record["destination"], "renamed.txt");
        assert!(shared.file_writers.lock().unwrap().is_empty());
        let original_exists = call(
            &mut client,
            Request::Stat {
                path: "report.txt".into(),
            },
        )
        .is_ok();
        assert_eq!(original_exists, !publish);
        if publish {
            let operation = backend
                .inner
                .operation_outcome(
                    &identity(&shared),
                    operation_id.as_deref().unwrap().parse().unwrap(),
                )
                .and_then(crate::operation_wire::operation_wire)
                .unwrap();
            assert_eq!(operation["commit_state"], "PUBLISHED");
        }
        drop(client);
        worker.join().unwrap();
    }
}

#[test]
fn uncertain_namespace_dispatch_never_aborts_its_workspace() {
    for publish in [false, true] {
        let (_temp, original, _backend, branch) = fixture();
        let backend = Arc::new(NamespaceFailure {
            inner: original.backend.clone(),
            publish,
            spool: original.spool.clone(),
            commits: Mutex::new(Vec::new()),
            aborts: AtomicUsize::new(0),
        });
        let shared = Arc::new(Shared {
            backend: backend.clone(),
            username: original.username.clone(),
            auth: original.auth.clone(),
            spool: original.spool.clone(),
            budget: original.budget.clone(),
            max_file_bytes: original.max_file_bytes,
            authenticated_sessions: tokio::sync::Semaphore::new(MAX_AUTHENTICATED_SESSIONS),
            file_writers: Mutex::new(BTreeSet::new()),
            namespace_operations: Mutex::new(BTreeSet::new()),
        });
        let mut session = session(shared, &branch);
        let error = session
            .dispatch(Request::Namespace {
                operation_id: new_id("op"),
                kind: smb_protocol::protocol::NamespaceOperation::Rename,
                path: "report.txt".into(),
                destination: Some("renamed.txt".into()),
                replace: false,
            })
            .unwrap_err();
        assert_eq!(error.code, storage_contract::ErrorCode::InjectedFailure);
        let commit = backend.commits.lock().unwrap()[0].clone();
        assert_eq!(
            error.operation_id.as_deref(),
            commit["operation_id"].as_str()
        );
        let record_path = session.shared.spool.join(format!(
            "{}.namespace.json",
            error.operation_id.as_deref().unwrap()
        ));
        let record: Value = json_codec::from_slice(&fs::read(record_path).unwrap()).unwrap();
        assert_eq!(record["commit_request"], commit);
        assert_eq!(record["path"], "report.txt");
        assert_eq!(record["destination"], "renamed.txt");
        assert_eq!(record["principal"], "alice");
        assert_eq!(record["branch_id"], branch);
        assert!(session.writer_locks.is_empty());
        assert_eq!(
            session
                .find(&name_policy::LookupPath::parse("renamed.txt").unwrap())
                .unwrap()
                .is_some(),
            publish
        );
        if publish {
            let operation = backend
                .inner
                .operation_outcome(
                    &session.identity,
                    error.operation_id.as_deref().unwrap().parse().unwrap(),
                )
                .and_then(crate::operation_wire::operation_wire)
                .unwrap();
            assert_eq!(operation["commit_state"], "PUBLISHED");
        }
        assert_eq!(
            backend.aborts.load(Ordering::SeqCst),
            0,
            "an attempted commit is not proven unpublished by an error response"
        );
    }
}

#[test]
fn legacy_untyped_delete_cannot_remove_files_or_directories() {
    // The old unlinkat callback sent exactly this request for both flags=0 and
    // AT_REMOVEDIR. Reject that ambiguous wire action rather than retaining an
    // old compatibility path which silently deletes either target kind.
    for path in ["report.txt", "empty"] {
        let (_temp, shared, _backend, branch) = fixture();
        let mut session = session(shared, &branch);
        if path == "empty" {
            session
                .dispatch(Request::Namespace {
                    operation_id: new_id("op"),
                    kind: smb_protocol::protocol::NamespaceOperation::Mkdir,
                    path: path.into(),
                    destination: None,
                    replace: false,
                })
                .unwrap();
        }
        let result = json_codec::from_value::<Request>(json!({
            "op":"namespace", "operation_id":new_id("op"), "kind":"delete", "path":path,
            "destination":null, "replace":false
        }));
        assert!(
            session
                .find(&name_policy::LookupPath::parse(path).unwrap())
                .unwrap()
                .is_some(),
            "ambiguous legacy delete actually removed {path}: {result:?}"
        );
        assert!(result.is_err());
    }
}

#[test]
fn typed_removal_succeeds_and_successful_namespace_records_are_cleaned() {
    let (_temp, shared, _backend, branch) = fixture();
    let mut session = session(shared, &branch);
    for (kind, path) in [
        (smb_protocol::protocol::NamespaceOperation::Mkdir, "empty"),
        (smb_protocol::protocol::NamespaceOperation::Rmdir, "empty"),
        (
            smb_protocol::protocol::NamespaceOperation::Unlink,
            "report.txt",
        ),
    ] {
        session
            .dispatch(Request::Namespace {
                operation_id: new_id("op"),
                kind,
                path: path.into(),
                destination: None,
                replace: false,
            })
            .unwrap();
    }
    assert!(
        session
            .find(&name_policy::LookupPath::parse("empty").unwrap())
            .unwrap()
            .is_none()
    );
    assert!(
        session
            .find(&name_policy::LookupPath::parse("report.txt").unwrap())
            .unwrap()
            .is_none()
    );
    assert!(!fs::read_dir(&session.shared.spool).unwrap().any(|entry| {
        entry
            .unwrap()
            .path()
            .to_string_lossy()
            .ends_with(".namespace.json")
    }));
}

#[test]
fn malformed_namespace_requests_are_rejected_before_creating_workspaces() {
    let (_temp, shared, backend, branch) = fixture();
    let mut session = session(shared, &branch);
    backend
        .change_before_workspace
        .store(true, Ordering::SeqCst);
    let before = backend
        .inner
        .tree_snapshot(
            &session.identity,
            crate::tests::tree_request(&(json!({"branch_id":branch}))),
        )
        .map(|snapshot| json_codec::to_value(snapshot).unwrap())
        .unwrap();
    for (kind, path, destination, replace) in [
        (
            smb_protocol::protocol::NamespaceOperation::Mkdir,
            "../bad",
            None,
            false,
        ),
        (
            smb_protocol::protocol::NamespaceOperation::Mkdir,
            "valid",
            Some("other"),
            false,
        ),
        (
            smb_protocol::protocol::NamespaceOperation::Rmdir,
            "valid",
            None,
            true,
        ),
        (
            smb_protocol::protocol::NamespaceOperation::Rename,
            "report.txt",
            None,
            false,
        ),
        (
            smb_protocol::protocol::NamespaceOperation::Rename,
            "report.txt",
            Some("../bad"),
            false,
        ),
    ] {
        assert!(
            session
                .dispatch(Request::Namespace {
                    operation_id: new_id("op"),
                    kind,
                    path: path.into(),
                    destination: destination.map(str::to_owned),
                    replace,
                })
                .is_err()
        );
        assert!(session.writer_locks.is_empty());
    }
    let after = backend
        .inner
        .tree_snapshot(
            &session.identity,
            crate::tests::tree_request(&(json!({"branch_id":branch}))),
        )
        .map(|snapshot| json_codec::to_value(snapshot).unwrap())
        .unwrap();
    assert_eq!(after, before);
    assert!(
        backend.change_before_workspace.load(Ordering::SeqCst),
        "malformed input reached workspace creation"
    );
}

#[test]
fn write_through_dirty_error_identifies_the_new_submitted_generation() {
    let (_temp, shared, backend, branch) = fixture();
    let mut session = session(shared, &branch);
    let (opened, file) = session
        .open("report.txt".into(), true, false, false, false, true)
        .unwrap();
    let opened: OpenResult = json_codec::from_value(opened).unwrap();
    let file = File::from(file.unwrap());
    file.write_all_at(b"one", 0).unwrap();
    session
        .dispatch(Request::Dirty {
            handle: opened.handle,
        })
        .unwrap();
    let first = match &session.handles[&opened.handle] {
        Handle::Write(writer) => writer.operation_id,
        _ => unreachable!(),
    };
    backend.fail.store(true, Ordering::SeqCst);
    file.write_all_at(b"two", 0).unwrap();
    let error = session
        .dispatch(Request::Dirty {
            handle: opened.handle,
        })
        .unwrap_err();
    assert_eq!(error.code, storage_contract::ErrorCode::InjectedFailure);
    let current = match &session.handles[&opened.handle] {
        Handle::Write(writer) => writer.operation_id,
        _ => unreachable!(),
    };
    assert_ne!(current, first);
    assert_eq!(
        error.operation_id.as_deref(),
        Some(current.to_string().as_str())
    );
    let operation = backend
        .inner
        .operation_outcome(&session.identity, current)
        .and_then(crate::operation_wire::operation_wire)
        .unwrap();
    assert_eq!(operation["commit_state"], "PUBLISHED");
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PreparationStage {
    Snapshot,
    Namespace,
}

struct PreparationFailure {
    inner: Arc<dyn Backend>,
    failure: Option<PreparationStage>,
    abort_fails: bool,
    aborts: AtomicUsize,
    commits: AtomicUsize,
}
impl Backend for PreparationFailure {
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
    ) -> Result<storage_contract::OperationOutcome, ApiError> {
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
        if self.failure == Some(PreparationStage::Snapshot) {
            return Err(ApiError::new(
                ::http::StatusCode::SERVICE_UNAVAILABLE,
                storage_contract::ErrorCode::InvalidRequest,
                "injected snapshot failure",
            ));
        }
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
        self.inner.create_workspace(identity, command)
    }
    fn abort_workspace(
        &self,
        identity: &crate::auth::Identity,
        workspace: &str,
    ) -> Result<(), crate::ApiError> {
        self.aborts.fetch_add(1, Ordering::SeqCst);
        if self.abort_fails {
            return Err(ApiError::new(
                ::http::StatusCode::SERVICE_UNAVAILABLE,
                storage_contract::ErrorCode::InjectedFailure,
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
        if self.failure == Some(PreparationStage::Namespace) {
            return Err(ApiError::new(
                ::http::StatusCode::SERVICE_UNAVAILABLE,
                storage_contract::ErrorCode::InvalidRequest,
                "injected staging failure",
            ));
        }
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
fn preparation_and_record_sync_failures_keep_unresolved_recovery_evidence() {
    for (failure, directory_sync) in [
        (Some(PreparationStage::Snapshot), false),
        (Some(PreparationStage::Namespace), false),
        (None, false),
        (None, true),
    ] {
        for abort_fails in [false, true] {
            let (_temp, original, _, branch) = fixture();
            let backend = Arc::new(PreparationFailure {
                inner: original.backend.clone(),
                failure,
                abort_fails,
                aborts: AtomicUsize::new(0),
                commits: AtomicUsize::new(0),
            });
            let shared = Arc::new(Shared {
                backend: backend.clone(),
                username: original.username.clone(),
                auth: original.auth.clone(),
                spool: original.spool.clone(),
                budget: original.budget.clone(),
                max_file_bytes: original.max_file_bytes,
                authenticated_sessions: tokio::sync::Semaphore::new(MAX_AUTHENTICATED_SESSIONS),
                file_writers: Mutex::new(BTreeSet::new()),
                namespace_operations: Mutex::new(BTreeSet::new()),
            });
            let mut session = session(shared, &branch);
            let operation_id = new_id("op");
            if failure.is_none() {
                super::super::namespace::fail_next_record_sync(directory_sync);
            }
            let error = session
                .dispatch(Request::Namespace {
                    operation_id: operation_id.clone(),
                    kind: smb_protocol::protocol::NamespaceOperation::Mkdir,
                    path: "new".into(),
                    destination: None,
                    replace: false,
                })
                .unwrap_err();
            assert_eq!(
                error.code,
                if failure.is_none() {
                    storage_contract::ErrorCode::IoError
                } else {
                    storage_contract::ErrorCode::InvalidRequest
                }
            );
            assert_eq!(error.operation_id.as_deref(), Some(operation_id.as_str()));
            assert_eq!(backend.commits.load(Ordering::SeqCst), 0);
            assert_eq!(backend.aborts.load(Ordering::SeqCst), 1);
            let record_path = session
                .shared
                .spool
                .join(format!("{operation_id}.namespace.json"));
            assert_eq!(
                record_path.exists(),
                abort_fails || failure.is_none(),
                "an unresolved abort or failed record sync must retain evidence"
            );
            if record_path.exists() {
                let record: Value =
                    json_codec::from_slice(&fs::read(record_path).unwrap()).unwrap();
                assert_eq!(record["operation_id"], operation_id);
                assert_eq!(record["path"], "new");
                assert!(
                    record["commit_request"]["workspace_id"]
                        .as_str()
                        .unwrap()
                        .starts_with("ws_")
                );
            }
            assert!(session.writer_locks.is_empty());
            assert!(
                backend
                    .inner
                    .operation_outcome(&session.identity, operation_id.parse().unwrap())
                    .and_then(crate::operation_wire::operation_wire)
                    .is_err(),
                "preparation failure must not be presented as publication"
            );
        }
    }
}

#[test]
fn dropped_successful_rpc_frame_keeps_the_client_allocated_operation_queryable() {
    let (_temp, shared, backend, _) = fixture();
    let (mut upstream, worker) = connect(shared.clone());
    let (client, proxy) = UnixStream::pair().unwrap();
    let proxy = std::thread::spawn(move || {
        let mut proxy = FrameStream::new(proxy).unwrap();
        let (request, descriptor) = proxy.receive::<Request>().unwrap();
        assert!(descriptor.is_none());
        upstream.send(&request, None).unwrap();
        let (response, descriptor) = upstream.receive::<Response>().unwrap();
        assert!(descriptor.is_none());
        assert!(
            matches!(response, Response::Ok { .. }),
            "core must have actually succeeded before dropping the frame"
        );
        // Deliberately discard the actual successful RPC frame and disconnect.
    });
    let mut client = FrameStream::new(client).unwrap();
    let operation_id = new_id("op");
    client
        .send(
            &Request::<String>::Namespace {
                operation_id: operation_id.clone(),
                kind: smb_protocol::protocol::NamespaceOperation::Rename,
                path: "report.txt".into(),
                destination: Some("renamed.txt".into()),
                replace: false,
            },
            None,
        )
        .unwrap();
    assert_eq!(
        client.receive::<Response>().unwrap_err().kind(),
        io::ErrorKind::UnexpectedEof
    );
    proxy.join().unwrap();
    worker.join().unwrap();
    let result = backend
        .inner
        .operation_outcome(&identity(&shared), operation_id.parse().unwrap())
        .and_then(crate::operation_wire::operation_wire)
        .unwrap();
    assert_eq!(result["commit_state"], "PUBLISHED");
}

#[test]
fn namespace_identity_validation_and_replay_admission_precede_workspace_creation() {
    let (_temp, shared, backend, branch) = fixture();
    let mut session = session(shared.clone(), &branch);
    let request = |operation_id: String| Request::Namespace {
        operation_id,
        kind: smb_protocol::protocol::NamespaceOperation::Mkdir,
        path: "new".into(),
        destination: None,
        replace: false,
    };
    backend
        .change_before_workspace
        .store(true, Ordering::SeqCst);
    for operation_id in [
        "../../outside",
        "op_short",
        "ws_0123456789abcdef0123456789abcdef",
        "op_0123456789ABCDEF0123456789ABCDEF",
    ] {
        let error = session.dispatch(request(operation_id.into())).unwrap_err();
        assert_eq!(error.code, storage_contract::ErrorCode::InvalidRequest);
        assert!(error.operation_id.is_none());
    }
    let retained_operation = new_id("op");
    let record = shared
        .spool
        .join(format!("{retained_operation}.namespace.json"));
    fs::write(&record, b"existing evidence must not be altered").unwrap();
    assert_eq!(
        session
            .dispatch(request(retained_operation))
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::RecoveryRequired
    );
    assert_eq!(
        fs::read(record).unwrap(),
        b"existing evidence must not be altered"
    );
    let active_operation = OperationId::generate();
    shared
        .namespace_operations
        .lock()
        .unwrap()
        .insert(active_operation);
    assert_eq!(
        session
            .dispatch(request(active_operation.to_string()))
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::RecoveryRequired
    );
    assert!(
        shared
            .namespace_operations
            .lock()
            .unwrap()
            .remove(&active_operation)
    );
    assert!(
        backend
            .change_before_workspace
            .swap(false, Ordering::SeqCst),
        "rejected request created a workspace"
    );
    assert!(session.writer_locks.is_empty());
    assert!(shared.namespace_operations.lock().unwrap().is_empty());

    let published_operation = new_id("op");
    session
        .dispatch(request(published_operation.clone()))
        .unwrap();
    backend
        .change_before_workspace
        .store(true, Ordering::SeqCst);
    assert_eq!(
        session
            .dispatch(request(published_operation))
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::OperationIdConflict
    );
    assert!(backend.change_before_workspace.load(Ordering::SeqCst));
    assert!(shared.namespace_operations.lock().unwrap().is_empty());
}

#[test]
fn malformed_raw_operation_ids_keep_handles_and_reject_before_side_effects() {
    let (_temp, shared, backend, branch) = fixture();
    let mut session = session(shared.clone(), &branch);
    let (opened, descriptor) = session
        .open("report.txt".into(), false, false, false, false, false)
        .unwrap();
    let opened: OpenResult = json_codec::from_value(opened).unwrap();
    let _file = File::from(descriptor.unwrap());
    let spool = || {
        fs::read_dir(&shared.spool)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<BTreeSet<_>>()
    };
    let before = spool();
    backend
        .change_before_workspace
        .store(true, Ordering::SeqCst);
    for operation_id in [
        "",
        "../../outside",
        "op_short",
        "ws_0123456789abcdef0123456789abcdef",
        "op_0123456789ABCDEF0123456789ABCDEF",
    ] {
        let bytes = json_codec::to_vec(&json!({"op":"namespace", "operation_id":operation_id,
            "kind":"mkdir", "path":"untouched", "destination":null, "replace":false}))
        .unwrap();
        let raw: Request<String> = json_codec::from_slice(&bytes).unwrap();
        let error = session.dispatch(raw).unwrap_err();
        assert_eq!(error.code, storage_contract::ErrorCode::InvalidRequest);
        assert!(error.operation_id.is_none());
        assert_eq!(session.handles.len(), 1);
        session
            .dispatch(Request::AuthorizeRead {
                handle: opened.handle,
            })
            .unwrap();
        assert_eq!(read_session(&mut session, opened.handle, 0, 1), b"o");
        assert_eq!(spool(), before);
        assert!(backend.change_before_workspace.load(Ordering::SeqCst));
        assert!(session.writer_locks.is_empty());
        assert!(shared.namespace_operations.lock().unwrap().is_empty());
    }
    session
        .dispatch(Request::Close {
            handle: opened.handle,
        })
        .unwrap();
    assert!(session.handles.is_empty());
}

#[test]
fn malformed_namespace_id_frames_keep_the_authenticated_stream_and_reader_alive() {
    let (_temp, shared, backend, _branch) = fixture();
    let (mut wire, worker) = connect(shared.clone());
    wire.send(
        &Request::<String>::Open {
            path: "report.txt".into(),
            writable: false,
            create: false,
            truncate: false,
            exclusive: false,
            write_through: false,
        },
        None,
    )
    .unwrap();
    let (response, descriptor) = wire.receive::<Response>().unwrap();
    let Response::Ok { result } = response else {
        panic!("reader must open")
    };
    let opened: OpenResult = json_codec::from_value(result).unwrap();
    let _file = File::from(descriptor.unwrap());
    let spool = || {
        fs::read_dir(&shared.spool)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<BTreeSet<_>>()
    };
    let before = spool();
    backend
        .change_before_workspace
        .store(true, Ordering::SeqCst);
    for operation_id in [
        "",
        "op_short",
        "../../outside",
        "op_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
    ] {
        wire.send(
            &json!({"op":"namespace", "operation_id":operation_id,"kind":"mkdir",
            "path":"untouched", "destination":null, "replace":false}),
            None,
        )
        .unwrap();
        let (response, descriptor) = wire.receive::<Response>().unwrap();
        assert!(descriptor.is_none());
        let Response::Error {
            code, operation_id, ..
        } = response
        else {
            panic!("invalid ID must fail")
        };
        assert_eq!(code, storage_contract::ErrorCode::InvalidRequest);
        assert!(operation_id.is_none());
        call(
            &mut wire,
            Request::AuthorizeRead {
                handle: opened.handle,
            },
        )
        .unwrap();
        assert_eq!(read_rpc(&mut wire, opened.handle, 0, 1), b"o");
        assert_eq!(spool(), before);
        assert!(backend.change_before_workspace.load(Ordering::SeqCst));
    }
    call(
        &mut wire,
        Request::Close {
            handle: opened.handle,
        },
    )
    .unwrap();
    drop(wire);
    worker.join().unwrap();
}
