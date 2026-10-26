//! The authorization decorator must preserve backend overrides, not trait defaults.
use super::*;
use std::{
    os::fd::AsRawFd,
    sync::atomic::{AtomicBool, Ordering},
};

struct Recorded {
    method: &'static str,
    identity: Option<Identity>,
    arguments: Value,
}

struct RecordingBackend {
    calls: Mutex<Vec<Recorded>>,
    fail: AtomicBool,
    file: std::fs::File,
}

fn sentinel_error(method: &str) -> ApiError {
    ApiError {
        status: ::http::StatusCode::CONFLICT,
        source: None,
        code: storage_contract::ErrorCode::InjectedFailure,
        message: method.into(),
        retryable: true,
        request_id: format!("req_{method}"),
    }
}

impl RecordingBackend {
    fn record(
        &self,
        method: &'static str,
        identity: Option<&Identity>,
        arguments: Value,
    ) -> Result<Value, ApiError> {
        self.calls.lock().unwrap().push(Recorded {
            method,
            identity: identity.cloned(),
            arguments,
        });
        if self.fail.load(Ordering::SeqCst) {
            Err(sentinel_error(method))
        } else {
            Ok(json!({"returned_by": method}))
        }
    }

    fn read(
        &self,
        method: &'static str,
        identity: &Identity,
        arguments: Value,
    ) -> Result<Content, ApiError> {
        self.record(method, Some(identity), arguments)?;
        Ok(Content {
            file: self.file.try_clone().unwrap(),
            fallback_files: vec![],
            checksum_manifest: None,
            size: self.file.metadata().unwrap().len(),
            hash: method.into(),
            lease: Box::new(()),
            download_name: Some("sentinel.bin".into()),
        })
    }
}

fn upload_arguments(workspace: &str, item: &str, path: &str, source: &UploadSource) -> Value {
    json!({"workspace": workspace, "item": item, "path": path,
        "fd": source.file.as_raw_fd(), "size": source.size, "hash": source.hash})
}

fn recorded_operation(
    operation_id: storage_contract::OperationId,
    method: &str,
) -> storage_contract::OperationResult {
    storage_contract::OperationResult {
        operation_id,
        kind: method.into(),
        commit_id: None,
        history_tree_id: None,
        branch_id: None,
        commit_state: storage_contract::CommitState::Published,
        protection_at_publish: storage_contract::ProtectionAtPublish::NotApplicable,
        current_health: storage_contract::CurrentHealth::Healthy,
        request_digest: "digest".into(),
        sequence: 7,
        result: json!({"returned_by":method}),
    }
}

impl Backend for RecordingBackend {
    fn operation(
        &self,
        identity: &Identity,
        operation_id: storage_contract::OperationId,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        self.record(
            "operation",
            Some(identity),
            json!({"operation_id":operation_id}),
        )?;
        Ok(recorded_operation(operation_id, "operation"))
    }
    fn commit_workspace(
        &self,
        identity: &Identity,
        command: crate::CommitWorkspace,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        self.record("commit_workspace", Some(identity), json!({"operation_id":command.operation_id,
            "workspace_id":command.workspace_id,"expected_head":command.expected_head,
            "fencing_token":command.fencing_token,"message":command.message,"request_digest":command.request_digest}))?;
        Ok(recorded_operation(command.operation_id, "commit_workspace"))
    }

    fn snapshot_at(
        &self,
        identity: &Identity,
        branch: &str,
        commit: &str,
    ) -> Result<Arc<storage_contract::Commit>, ApiError> {
        self.record(
            "snapshot_at",
            Some(identity),
            json!({"branch":branch,"commit":commit}),
        )?;
        Ok(recorded_commit(commit))
    }

    fn authorize_current(
        &self,
        identity: &crate::auth::Identity,
        branch: &str,
    ) -> Result<(), crate::ApiError> {
        self.record(
            "authorize_current",
            Some(identity),
            json!({"branch":branch}),
        )
        .map(|_| ())
    }

    fn read_directory(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadDirectory<'_>,
    ) -> Result<crate::DirectorySnapshot, crate::ApiError> {
        self.record(
            "read_directory",
            Some(identity),
            json!({"branch":request.branch,"entry_id":request.entry_id}),
        )?;
        Ok(crate::DirectorySnapshot {
            commit_id: "commit".into(),
            entries: Vec::new(),
            more: false,
        })
    }

    fn authorize_workspace_selection(
        &self,
        identity: &crate::auth::Identity,
        workspace: &str,
        path: &name_policy::PortablePath,
    ) -> Result<(), crate::ApiError> {
        self.record(
            "authorize_workspace_selection",
            Some(identity),
            json!({"workspace":workspace,"path":path.as_str(),"key":path.key()}),
        )
        .map(|_| ())
    }

    fn create_workspace(
        &self,
        identity: &Identity,
        command: crate::CreateWorkspace,
    ) -> Result<storage_contract::Workspace, ApiError> {
        self.record(
            "create_workspace",
            Some(identity),
            json!({"branch_id":command.branch_id,"source_commit_id":command.source_commit_id}),
        )?;
        Ok(storage_contract::Workspace {
            workspace_id: "created-workspace".into(),
            purpose: storage_contract::WorkspacePurpose::Normal,
            branch_id: command.branch_id,
            principal: identity.principal.clone(),
            base_head: "head".into(),
            fencing_token: 11,
            files: BTreeMap::new(),
            selected_file_branches: BTreeMap::new(),
            selected_file_contexts: BTreeMap::new(),
            selected_directory_branches: BTreeMap::new(),
            directory_selection_generations: BTreeMap::new(),
            file_selection_generations: BTreeMap::new(),
            namespace_operations: Vec::new(),
            created_at: 10,
            state: storage_contract::WorkspaceState::Open,
        })
    }
    fn abort_workspace(&self, identity: &Identity, workspace: &str) -> Result<(), ApiError> {
        self.record(
            "abort_workspace",
            Some(identity),
            json!({"workspace_id":workspace}),
        )
        .map(|_| ())
    }

    fn workspace(
        &self,
        identity: &Identity,
        workspace: &str,
    ) -> Result<storage_contract::Workspace, ApiError> {
        self.record("workspace", Some(identity), json!({"workspace":workspace}))?;
        Ok(storage_contract::Workspace {
            workspace_id: workspace.into(),
            purpose: storage_contract::WorkspacePurpose::Normal,
            branch_id: "branch".into(),
            principal: identity.principal.clone(),
            base_head: "head".into(),
            fencing_token: 11,
            files: BTreeMap::new(),
            selected_file_branches: BTreeMap::new(),
            selected_file_contexts: BTreeMap::new(),
            selected_directory_branches: BTreeMap::new(),
            directory_selection_generations: BTreeMap::new(),
            file_selection_generations: BTreeMap::new(),
            namespace_operations: Vec::new(),
            created_at: 10,
            state: storage_contract::WorkspaceState::Open,
        })
    }
    fn published_upload_status(
        &self,
        identity: &Identity,
        workspace: &str,
        item: &str,
    ) -> Result<Option<crate::PublishedUploadStatus>, ApiError> {
        self.record(
            "published_upload_status",
            Some(identity),
            json!({"workspace":workspace,"item":item}),
        )?;
        Ok(Some(crate::PublishedUploadStatus {
            generation: 42,
            logical_size: 0,
            content_hash: storage_contract::ContentHash::from_digest(blake3::hash(b"")),
            state: crate::PublishedUploadState::Complete,
        }))
    }
    fn seal_workspace(
        &self,
        identity: &Identity,
        workspace: &str,
    ) -> Result<storage_contract::SealedWorkspace, ApiError> {
        self.record(
            "seal_workspace",
            Some(identity),
            json!({"workspace":workspace}),
        )?;
        Ok(public_seal())
    }
    fn commit_workspace_public(
        &self,
        identity: &Identity,
        command: storage_contract::PublicCommitEnvelope,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        let operation = command.operation_id();
        self.record(
            "commit_workspace_public",
            Some(identity),
            command.into_body(),
        )?;
        Ok(recorded_operation(operation, "commit_workspace_public"))
    }
    fn version_snapshot(
        &self,
        identity: &crate::auth::Identity,
        version: &str,
    ) -> Result<crate::VersionSnapshot, crate::ApiError> {
        self.record(
            "version_snapshot",
            Some(identity),
            json!({"version_id":version}),
        )?;
        Ok(recorded_version(version))
    }
    fn authorize_version(
        &self,
        identity: &crate::auth::Identity,
        version: &str,
    ) -> Result<(), crate::ApiError> {
        self.record(
            "authorize_version",
            Some(identity),
            json!({"version_id":version}),
        )?;
        Ok(())
    }
    fn repository_snapshot(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<storage_contract::Repository, crate::ApiError> {
        self.record(
            "repository_snapshot",
            Some(identity),
            json!({"repository_id":id}),
        )?;
        Ok(recorded_repository(id))
    }
    fn authorize_repository(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<(), crate::ApiError> {
        self.record(
            "authorize_repository",
            Some(identity),
            json!({"repository_id":id}),
        )?;
        Ok(())
    }
    fn capabilities(&self) -> Result<crate::Capabilities, crate::ApiError> {
        self.record("capabilities", None, Value::Null)?;
        Ok(crate::Capabilities::for_engine(42))
    }
    fn ready(&self) -> Result<crate::Readiness, crate::ApiError> {
        self.record("ready", None, Value::Null)?;
        Ok(crate::Readiness::Managed)
    }
    fn workspace_capacity(
        &self,
        identity: &crate::auth::Identity,
    ) -> Result<storage_contract::WorkspaceCapacity, crate::ApiError> {
        self.record("workspace_capacity", Some(identity), Value::Null)?;
        Ok(recorded_capacity())
    }
    fn smb_binding_status(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<crate::SmbBindingStatus, crate::ApiError> {
        self.record(
            "smb_binding_status",
            Some(identity),
            json!({"repository_id":id}),
        )?;
        Ok(crate::SmbBindingStatus {
            repository_id: id.into(),
            branch_id: "branch".into(),
            enabled: true,
            history_generation: 7,
        })
    }
    fn update_smb_binding(
        &self,
        identity: &crate::auth::Identity,
        request: crate::UpdateSmbBinding<'_>,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.record("update_smb_binding",Some(identity),json!({"repository_id":request.repository,"branch_id":request.branch,"enabled":request.enabled,"expected_history_generation":request.expected_history_generation.to_string(),"operation_id":request.operation}))?;
        Ok(recorded_operation(request.operation, "update_smb_binding"))
    }
    fn protection_policy(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<storage_contract::ProtectionPolicy, crate::ApiError> {
        self.record("protection_policy", Some(identity), json!({"id":id}))?;
        Ok(recorded_protection_policy(id))
    }
    fn placement_rule(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<storage_contract::PlacementRule, crate::ApiError> {
        self.record("placement_rule", Some(identity), json!({"id":id}))?;
        Ok(recorded_placement_rule(id))
    }
    fn update_protection_policy(
        &self,
        identity: &crate::auth::Identity,
        request: crate::UpdateProtectionPolicy,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.record("update_protection_policy",Some(identity),json!({"policy_id":request.policy_id,"target_copies":request.target_copies,"repository_id":request.repository_id,"path_prefix":request.path_prefix,"expected_generation":request.expected_generation.to_string(),"operation_id":request.operation_id}))?;
        Ok(recorded_operation(
            request.operation_id,
            "update_protection_policy",
        ))
    }
    fn update_placement_rule(
        &self,
        identity: &crate::auth::Identity,
        request: crate::UpdatePlacementRule,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.record("update_placement_rule",Some(identity),json!({"rule_id":request.rule_id,"primary_slot_id":request.primary_slot_id,"repository_id":request.repository_id,"path_prefix":request.path_prefix,"expected_generation":request.expected_generation.to_string(),"operation_id":request.operation_id}))?;
        Ok(recorded_operation(
            request.operation_id,
            "update_placement_rule",
        ))
    }
    fn compare_commits(
        &self,
        identity: &crate::auth::Identity,
        request: crate::CompareCommits<'_>,
    ) -> Result<storage_contract::CommitComparison, crate::ApiError> {
        self.record(
            "compare_commits",
            Some(identity),
            json!({"left_commit_id":request.left,"right_commit_id":request.right}),
        )?;
        Ok(storage_contract::CommitComparison::between(
            recorded_commit(request.left),
            recorded_commit(request.right),
        ))
    }
    fn disk_status(
        &self,
        identity: &crate::auth::Identity,
    ) -> Result<Vec<storage_contract::DiskStatus>, crate::ApiError> {
        self.record("disk_status", Some(identity), Value::Null)?;
        Ok(vec![])
    }
    fn integrity_check(
        &self,
        identity: &crate::auth::Identity,
    ) -> Result<storage_contract::CheckReport, crate::ApiError> {
        self.record("integrity_check", Some(identity), Value::Null)?;
        Ok(storage_contract::CheckReport {
            checked_versions: 17,
            healthy_replicas: 23,
            tombstoned_versions: 7,
            issues: vec!["diagnostic".into()],
        })
    }
    fn file_history(
        &self,
        identity: &crate::auth::Identity,
        file: &str,
    ) -> Result<Vec<storage_contract::FileVersion>, crate::ApiError> {
        self.record("file_history", Some(identity), json!({"file_id":file}))?;
        let mut version = recorded_version("version").version;
        version.file_id = file.into();
        Ok(vec![version])
    }
    fn topology_snapshot(
        &self,
        identity: &crate::auth::Identity,
    ) -> Result<crate::TopologySnapshot, crate::ApiError> {
        self.record("topology_snapshot", Some(identity), Value::Null)?;
        Ok(crate::TopologySnapshot {
            generation: 17,
            slots: vec![],
        })
    }
    fn placement_snapshot(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<storage_contract::Placement, crate::ApiError> {
        self.record(
            "placement_snapshot",
            Some(identity),
            json!({"version_id":id}),
        )?;
        Ok(storage_contract::Placement {
            version_id: id.into(),
            primary_slot_id: "P01".into(),
            target_copies: 1,
            route_slot_ids: vec!["P01".into()],
            topology_generation: 17,
            binding_generations: std::collections::BTreeMap::from([("P01".into(), 23)]),
        })
    }
    fn event_page(
        &self,
        identity: &crate::auth::Identity,
        after: u64,
    ) -> Result<crate::EventPage, crate::ApiError> {
        self.record(
            "event_page",
            Some(identity),
            json!({"after":after.to_string()}),
        )?;
        Ok(crate::EventPage {
            events: vec![],
            next_cursor: after,
        })
    }
    fn history_graph(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<crate::HistoryGraphSnapshot, crate::ApiError> {
        self.record(
            "history_graph",
            Some(identity),
            json!({"history_tree_id":id}),
        )?;
        Ok(recorded_graph(id))
    }
    fn commit_snapshot(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<std::sync::Arc<storage_contract::Commit>, crate::ApiError> {
        self.record("commit_snapshot", Some(identity), json!({"commit_id":id}))?;
        Ok(recorded_commit(id))
    }
    fn resolve_entry(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ResolveEntry<'_>,
    ) -> Result<crate::ResolvedEntry, crate::ApiError> {
        self.record("resolve_entry",Some(identity),json!({"branch_id":request.branch,"path":request.path.as_str(),"key":request.path.key(),"commit_id":request.commit}))?;
        Ok(recorded_resolved())
    }
    fn branch_snapshot(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<crate::BranchSnapshot, crate::ApiError> {
        self.record("branch_snapshot", Some(identity), json!({"id":id}))?;
        Ok(recorded_branch())
    }
    fn branches_snapshot(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<Vec<crate::BranchSnapshot>, crate::ApiError> {
        self.record("branches_snapshot", Some(identity), json!({"id":id}))?;
        Ok(vec![recorded_branch()])
    }
    fn repositories_snapshot(
        &self,
        identity: &crate::auth::Identity,
    ) -> Result<Vec<storage_contract::Repository>, crate::ApiError> {
        self.record("repositories_snapshot", Some(identity), Value::Null)?;
        Ok(vec![recorded_repository("repository")])
    }
    fn tree_snapshot(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadTree<'_>,
    ) -> Result<crate::TreeSnapshot, crate::ApiError> {
        self.record(
            "tree_snapshot",
            Some(identity),
            json!({"branch_id":request.branch,"commit_id":request.commit}),
        )?;
        Ok(recorded_tree())
    }
    fn create_write_session(
        &self,
        identity: &crate::auth::Identity,
        branch: &str,
    ) -> Result<crate::WorkspaceWriteSession, crate::ApiError> {
        self.record(
            "create_write_session",
            Some(identity),
            json!({"branch_id":branch}),
        )?;
        Ok(crate::WorkspaceWriteSession {
            workspace_id: "workspace".into(),
            branch_id: branch.into(),
            principal: identity.principal.clone(),
            fencing_token: 11,
            expires_at: 70,
            state: storage_contract::WorkspaceState::Open,
        })
    }
    fn stage_namespace(
        &self,
        identity: &Identity,
        command: crate::StageNamespace,
    ) -> Result<storage_contract::Workspace, ApiError> {
        self.record(
            "stage_namespace",
            Some(identity),
            json!({"workspace_id":command.workspace_id,"change":command.change}),
        )?;
        Ok(storage_contract::Workspace {
            workspace_id: command.workspace_id,
            purpose: storage_contract::WorkspacePurpose::Normal,
            branch_id: "branch".into(),
            principal: identity.principal.clone(),
            base_head: "head".into(),
            fencing_token: 11,
            files: BTreeMap::new(),
            selected_file_branches: BTreeMap::new(),
            selected_file_contexts: BTreeMap::new(),
            selected_directory_branches: BTreeMap::new(),
            directory_selection_generations: BTreeMap::new(),
            file_selection_generations: BTreeMap::new(),
            namespace_operations: vec![command.change],
            created_at: 10,
            state: storage_contract::WorkspaceState::Open,
        })
    }
    fn authorize_upload(
        &self,
        identity: &crate::auth::Identity,
        workspace: &str,
        logical_size: Option<u64>,
    ) -> Result<(), crate::ApiError> {
        self.record(
            "authorize_upload",
            Some(identity),
            json!({"workspace":workspace,"logical_size":logical_size}),
        )
        .map(|_| ())
    }

    fn lookup_current(
        &self,
        identity: &Identity,
        branch: &str,
        path: &name_policy::LookupPath,
    ) -> Result<Option<storage_contract::Entry>, ApiError> {
        self.record(
            "lookup_current",
            Some(identity),
            json!({"branch":branch,"path":path.as_str(),"key":path.key()}),
        )?;
        Ok(None)
    }

    fn lookup_at(
        &self,
        identity: &Identity,
        branch: &str,
        commit: &str,
        path: &name_policy::LookupPath,
    ) -> Result<Option<storage_contract::Entry>, ApiError> {
        self.record(
            "lookup_at",
            Some(identity),
            json!({"branch":branch,"commit":commit,"path":path.as_str(),"key":path.key()}),
        )?;
        Ok(None)
    }
    fn setup_operation(
        &self,
        action: &'static str,
        input: Value,
        bootstrap: Option<&str>,
        identity: Option<&Identity>,
    ) -> Result<Value, ApiError> {
        self.record(
            "setup_operation",
            identity,
            json!({"action": action, "input": input, "bootstrap": bootstrap}),
        )
    }
    fn start_management(&self, _: Arc<Mutex<AuthStore>>) {
        self.record("start_management", None, Value::Null).unwrap();
    }
    fn stop_management(&self) {
        self.record("stop_management", None, Value::Null).unwrap();
    }
    fn drain_management(&self) -> Result<(), ApiError> {
        self.record("drain_management", None, Value::Null)
            .map(|_| ())
    }
    fn management_status(&self) -> crate::StorageStatus {
        self.record("management_status", None, Value::Null).unwrap();
        crate::StorageStatus::Managed {
            state: crate::StorageState::ReadOnly,
            recovering: true,
        }
    }
    fn retry_storage(&self, identity: &Identity) -> Result<crate::StorageStatus, ApiError> {
        self.record("retry_storage", Some(identity), Value::Null)?;
        Ok(crate::StorageStatus::Managed {
            state: crate::StorageState::RecoveryRequired,
            recovering: true,
        })
    }
    fn call(
        &self,
        action: BackendAction,
        identity: &Identity,
        input: Value,
    ) -> Result<Value, ApiError> {
        self.record(
            "call",
            Some(identity),
            json!({"action": format!("{action:?}"), "input": input}),
        )
    }
    fn upload(
        &self,
        identity: &Identity,
        workspace: &str,
        item: &str,
        path: &name_policy::PortablePath,
        source: &UploadSource,
    ) -> Result<storage_contract::StagedFile, ApiError> {
        self.record(
            "upload",
            Some(identity),
            upload_arguments(workspace, item, path, source),
        )?;
        Ok(staged_result("upload", path, source, 1))
    }
    fn upload_generation(
        &self,
        identity: &Identity,
        workspace: &str,
        item: &str,
        path: &name_policy::PortablePath,
        source: &UploadSource,
        generation: u64,
    ) -> Result<storage_contract::StagedFile, ApiError> {
        let mut arguments = upload_arguments(workspace, item, path, source);
        arguments["generation"] = generation.into();
        self.record("upload_generation", Some(identity), arguments)?;
        Ok(staged_result("upload_generation", path, source, generation))
    }
    fn content(&self, identity: &Identity, version: &str) -> Result<Content, ApiError> {
        self.read("content", identity, json!({"version": version}))
    }
    fn range_content(
        &self,
        identity: &Identity,
        request: crate::ReadContent<'_>,
    ) -> Result<Content, ApiError> {
        let context=request.context.map(|context|json!({"repository_id":context.repository_id,"path":context.path.as_str(),"key":context.path.key()}));
        self.read(
            "range_content",
            identity,
            json!({"version": request.version,"context":context}),
        )
    }
    fn workspace_content(
        &self,
        identity: &Identity,
        workspace: &str,
        item: &str,
    ) -> Result<Content, ApiError> {
        self.read(
            "workspace_content",
            identity,
            json!({"workspace": workspace, "item": item}),
        )
    }
    fn checksums(&self, identity: &Identity, version: &str) -> Result<Vec<u8>, ApiError> {
        self.record("checksums", Some(identity), json!({"version": version}))?;
        Ok(vec![19, 23])
    }
    fn observe_disks(
        &self,
        _: &InternalDiskObservation,
    ) -> Result<Vec<crate::DiskSample>, ApiError> {
        self.record("observe_disks", None, Value::Null)
            .map(|_| vec![])
    }
}

struct Fixture {
    auth: Arc<Mutex<AuthStore>>,
    _root: tempfile::TempDir,
    backend: AuthorizedBackend,
    probe: Arc<RecordingBackend>,
    identity: Identity,
    token: String,
    source: UploadSource,
}

impl Fixture {
    fn new() -> Self {
        let root = crate::tests::private_tempdir();
        let mut auth = AuthStore::initialize(root.path(), "admin", "a-long-test-password").unwrap();
        let token = auth.login("admin", "a-long-test-password").unwrap().token;
        let mut identity = auth.authenticate(&token).unwrap();
        identity.smb_binding = Some(auth::SmbBinding {
            repository_id: "repo-bound".into(),
            branch_id: "branch-bound".into(),
            generation: 29,
        });
        let path = root.path().join("body");
        std::fs::write(&path, b"forwarding probe").unwrap();
        let source = UploadSource::from_file(std::fs::File::open(path).unwrap(), 1024).unwrap();
        let probe = Arc::new(RecordingBackend {
            calls: Mutex::new(vec![]),
            fail: AtomicBool::new(false),
            file: source.file.try_clone().unwrap(),
        });
        let auth = Arc::new(Mutex::new(auth));
        let backend = AuthorizedBackend {
            backend: probe.clone(),
            auth: crate::lock(&auth).reader(),
        };
        Self {
            _root: root,
            auth,
            backend,
            probe,
            identity,
            token,
            source,
        }
    }

    fn protected_calls(&self) -> Vec<(&'static str, Result<Value, ApiError>)> {
        let (backend, identity, source) = (&self.backend, &self.identity, &self.source);
        let content = |value: Content| {
            assert_eq!(value.size, source.size);
            assert_eq!(value.download_name.as_deref(), Some("sentinel.bin"));
            json!({"returned_by": value.hash})
        };
        let operation_id = "op_00000000000000000000000000000000".parse().unwrap();
        vec![
            (
                "operation",
                backend.operation(identity, operation_id).map(|result| {
                    assert_eq!(result, recorded_operation(operation_id, "operation"));
                    result.result
                }),
            ),
            (
                "commit_workspace",
                backend
                    .commit_workspace(
                        identity,
                        crate::CommitWorkspace {
                            operation_id,
                            workspace_id: "workspace".into(),
                            expected_head: "head".into(),
                            fencing_token: 41,
                            message: "commit".into(),
                            request_digest: Some("digest".into()),
                        },
                    )
                    .map(|result| {
                        assert_eq!(result, recorded_operation(operation_id, "commit_workspace"));
                        result.result
                    }),
            ),
            (
                "setup_operation",
                backend.setup_operation(
                    "status",
                    json!({"probe": 3}),
                    Some("bootstrap-probe"),
                    Some(identity),
                ),
            ),
            (
                "retry_storage",
                backend.retry_storage(identity).map(|status| {
                    assert_eq!(
                        status,
                        crate::StorageStatus::Managed {
                            state: crate::StorageState::RecoveryRequired,
                            recovering: true
                        }
                    );
                    json!({"returned_by":"retry_storage"})
                }),
            ),
            (
                "snapshot_at",
                backend
                    .snapshot_at(identity, "branch", "commit")
                    .map(|snapshot| {
                        assert_eq!(snapshot.commit_id, "commit");
                        json!({"returned_by":"snapshot_at"})
                    }),
            ),
            (
                "workspace",
                backend.workspace(identity, "workspace").map(|workspace| {
                    assert_eq!(workspace.workspace_id, "workspace");
                    json!({"returned_by":"workspace"})
                }),
            ),
            (
                "published_upload_status",
                backend
                    .published_upload_status(identity, "workspace", "item")
                    .map(|status| {
                        assert_eq!(status.unwrap().generation, 42);
                        json!({"returned_by":"published_upload_status"})
                    }),
            ),
            (
                "seal_workspace",
                backend.seal_workspace(identity, "workspace").map(|seal| {
                    assert_eq!(seal.fencing_token, 41);
                    json!({"returned_by":"seal_workspace"})
                }),
            ),
            (
                "commit_workspace_public",
                backend
                    .commit_workspace_public(identity, public_envelope())
                    .map(|result| result.result),
            ),
            (
                "version_snapshot",
                backend
                    .version_snapshot(identity, "version")
                    .map(|snapshot| {
                        assert_eq!(snapshot.version.version_id, "version");
                        assert_eq!(snapshot.version.logical_size, u64::MAX);
                        json!({"returned_by":"version_snapshot"})
                    }),
            ),
            (
                "authorize_version",
                backend
                    .authorize_version(identity, "version")
                    .map(|()| json!({"returned_by":"authorize_version"})),
            ),
            (
                "repository_snapshot",
                backend
                    .repository_snapshot(identity, "repository")
                    .map(|repository| {
                        assert_eq!(repository.repository_id, "repository");
                        assert_eq!(repository.smb_binding_generation, u64::MAX);
                        json!({"returned_by":"repository_snapshot"})
                    }),
            ),
            (
                "authorize_repository",
                backend
                    .authorize_repository(identity, "repository")
                    .map(|()| json!({"returned_by":"authorize_repository"})),
            ),
            (
                "workspace_capacity",
                backend.workspace_capacity(identity).map(|capacity| {
                    assert_eq!(capacity, recorded_capacity());
                    json!({"returned_by":"workspace_capacity"})
                }),
            ),
            (
                "smb_binding_status",
                backend
                    .smb_binding_status(identity, "repository")
                    .map(|status| {
                        assert_eq!(status.history_generation, 7);
                        json!({"returned_by":"smb_binding_status"})
                    }),
            ),
            (
                "update_smb_binding",
                backend
                    .update_smb_binding(
                        identity,
                        crate::UpdateSmbBinding {
                            repository: "repository",
                            branch: "branch",
                            enabled: false,
                            expected_history_generation: 7,
                            operation: operation_id,
                        },
                    )
                    .map(|result| {
                        assert_eq!(
                            result,
                            recorded_operation(operation_id, "update_smb_binding")
                        );
                        json!({"returned_by":"update_smb_binding"})
                    }),
            ),
            (
                "protection_policy",
                backend.protection_policy(identity, "policy").map(|policy| {
                    assert_eq!(policy.generation, 17);
                    json!({"returned_by":"protection_policy"})
                }),
            ),
            (
                "placement_rule",
                backend.placement_rule(identity, "rule").map(|rule| {
                    assert_eq!(rule.generation, 17);
                    json!({"returned_by":"placement_rule"})
                }),
            ),
            (
                "update_protection_policy",
                backend
                    .update_protection_policy(
                        identity,
                        crate::UpdateProtectionPolicy {
                            policy_id: "policy".into(),
                            repository_id: "repository".into(),
                            path_prefix: "Folder".into(),
                            target_copies: 3,
                            expected_generation: 17,
                            operation_id,
                        },
                    )
                    .map(|result| {
                        assert_eq!(
                            result,
                            recorded_operation(operation_id, "update_protection_policy")
                        );
                        json!({"returned_by":"update_protection_policy"})
                    }),
            ),
            (
                "update_placement_rule",
                backend
                    .update_placement_rule(
                        identity,
                        crate::UpdatePlacementRule {
                            rule_id: "rule".into(),
                            repository_id: "repository".into(),
                            path_prefix: "Folder".into(),
                            primary_slot_id: "P01".into(),
                            expected_generation: 17,
                            operation_id,
                        },
                    )
                    .map(|result| {
                        assert_eq!(
                            result,
                            recorded_operation(operation_id, "update_placement_rule")
                        );
                        json!({"returned_by":"update_placement_rule"})
                    }),
            ),
            (
                "compare_commits",
                backend
                    .compare_commits(
                        identity,
                        crate::CompareCommits {
                            left: "left",
                            right: "right",
                        },
                    )
                    .map(|comparison| {
                        assert_eq!(comparison.left_id(), "left");
                        assert_eq!(comparison.right_id(), "right");
                        json!({"returned_by":"compare_commits"})
                    }),
            ),
            (
                "disk_status",
                backend.disk_status(identity).map(|disks| {
                    assert!(disks.is_empty());
                    json!({"returned_by":"disk_status"})
                }),
            ),
            (
                "integrity_check",
                backend.integrity_check(identity).map(|report| {
                    assert_eq!(report.checked_versions, 17);
                    assert_eq!(report.healthy_replicas, 23);
                    assert_eq!(report.tombstoned_versions, 7);
                    assert_eq!(report.issues, ["diagnostic"]);
                    json!({"returned_by":"integrity_check"})
                }),
            ),
            (
                "file_history",
                backend.file_history(identity, "file").map(|versions| {
                    assert_eq!(versions[0].logical_size, u64::MAX);
                    assert_eq!(versions[0].file_id, "file");
                    json!({"returned_by":"file_history"})
                }),
            ),
            (
                "topology_snapshot",
                backend.topology_snapshot(identity).map(|snapshot| {
                    assert_eq!(snapshot.generation, 17);
                    json!({"returned_by":"topology_snapshot"})
                }),
            ),
            (
                "placement_snapshot",
                backend
                    .placement_snapshot(identity, "version")
                    .map(|placement| {
                        assert_eq!(placement.binding_generations["P01"], 23);
                        json!({"returned_by":"placement_snapshot"})
                    }),
            ),
            (
                "event_page",
                backend.event_page(identity, 17).map(|page| {
                    assert_eq!(page.next_cursor, 17);
                    assert!(page.events.is_empty());
                    json!({"returned_by":"event_page"})
                }),
            ),
            (
                "history_graph",
                backend.history_graph(identity, "history").map(|graph| {
                    assert_eq!(graph.history.generation, u64::MAX);
                    assert_eq!(graph.commits[0].commit_id, "commit");
                    json!({"returned_by":"history_graph"})
                }),
            ),
            (
                "commit_snapshot",
                backend.commit_snapshot(identity, "commit").map(|commit| {
                    assert_eq!(commit.commit_id, "commit");
                    json!({"returned_by":"commit_snapshot"})
                }),
            ),
            (
                "resolve_entry",
                backend
                    .resolve_entry(
                        identity,
                        crate::ResolveEntry {
                            branch: "branch",
                            path: &name_policy::LookupPath::parse("File.txt").unwrap(),
                            commit: Some("commit"),
                        },
                    )
                    .map(|resolved| {
                        assert_eq!(resolved.version.unwrap().logical_size, u64::MAX);
                        json!({"returned_by":"resolve_entry"})
                    }),
            ),
            (
                "branch_snapshot",
                backend.branch_snapshot(identity, "branch").map(|snapshot| {
                    assert_eq!(snapshot.generation, u64::MAX);
                    json!({"returned_by":"branch_snapshot"})
                }),
            ),
            (
                "branches_snapshot",
                backend
                    .branches_snapshot(identity, "repository")
                    .map(|snapshots| {
                        assert_eq!(snapshots[0].generation, u64::MAX);
                        json!({"returned_by":"branches_snapshot"})
                    }),
            ),
            (
                "repositories_snapshot",
                backend.repositories_snapshot(identity).map(|repositories| {
                    assert_eq!(repositories, vec![recorded_repository("repository")]);
                    json!({"returned_by":"repositories_snapshot"})
                }),
            ),
            (
                "tree_snapshot",
                backend
                    .tree_snapshot(
                        identity,
                        crate::ReadTree {
                            branch: "branch",
                            commit: Some("commit"),
                        },
                    )
                    .map(|snapshot| {
                        assert_eq!(
                            json_codec::to_value(snapshot).unwrap(),
                            json_codec::to_value(recorded_tree()).unwrap()
                        );
                        json!({"returned_by":"tree_snapshot"})
                    }),
            ),
            (
                "create_write_session",
                backend
                    .create_write_session(identity, "branch")
                    .map(|session| {
                        assert_eq!(session.branch_id, "branch");
                        assert_eq!(session.expires_at, 70);
                        json!({"returned_by":"create_write_session"})
                    }),
            ),
            (
                "stage_namespace",
                backend
                    .stage_namespace(
                        identity,
                        crate::StageNamespace {
                            workspace_id: "workspace".into(),
                            change: storage_contract::NamespaceOperation::Mkdir {
                                path: name_policy::PortablePath::parse("File.txt").unwrap(),
                            },
                        },
                    )
                    .map(|workspace| {
                        assert_eq!(workspace.namespace_operations[0].path(), "File.txt");
                        json!({"returned_by":"stage_namespace"})
                    }),
            ),
            (
                "authorize_upload",
                backend
                    .authorize_upload(identity, "workspace", Some(42))
                    .map(|()| json!({"returned_by":"authorize_upload"})),
            ),
            (
                "create_workspace",
                backend
                    .create_workspace(
                        identity,
                        crate::CreateWorkspace {
                            branch_id: "branch".into(),
                            source_commit_id: Some("source".into()),
                        },
                    )
                    .map(|workspace| {
                        assert_eq!(workspace.workspace_id, "created-workspace");
                        json!({"returned_by":"create_workspace"})
                    }),
            ),
            (
                "abort_workspace",
                backend
                    .abort_workspace(identity, "workspace")
                    .map(|()| json!({"returned_by":"abort_workspace"})),
            ),
            (
                "authorize_workspace_selection",
                backend
                    .authorize_workspace_selection(
                        identity,
                        "workspace",
                        &name_policy::PortablePath::parse("File.txt").unwrap(),
                    )
                    .map(|()| json!({"returned_by":"authorize_workspace_selection"})),
            ),
            (
                "authorize_current",
                backend
                    .authorize_current(identity, "branch")
                    .map(|()| json!({"returned_by":"authorize_current"})),
            ),
            (
                "read_directory",
                backend
                    .read_directory(
                        identity,
                        crate::ReadDirectory {
                            branch: "branch",
                            entry_id: "branch",
                            path: None,
                            selection: crate::DirectorySelection::Capture,
                        },
                    )
                    .map(|snapshot| {
                        assert_eq!(snapshot.commit_id, "commit");
                        assert!(snapshot.entries.is_empty());
                        assert!(!snapshot.more);
                        json!({"returned_by":"read_directory"})
                    }),
            ),
            (
                "lookup_current",
                backend
                    .lookup_current(
                        identity,
                        "branch",
                        &name_policy::LookupPath::parse("File.txt").unwrap(),
                    )
                    .map(|entry| {
                        assert!(entry.is_none());
                        json!({"returned_by":"lookup_current"})
                    }),
            ),
            (
                "lookup_at",
                backend
                    .lookup_at(
                        identity,
                        "branch",
                        "commit",
                        &name_policy::LookupPath::parse("File.txt").unwrap(),
                    )
                    .map(|entry| {
                        assert!(entry.is_none());
                        json!({"returned_by":"lookup_at"})
                    }),
            ),
            (
                "call",
                backend.call(BackendAction::Archive, identity, json!({"probe": 7})),
            ),
            (
                "upload",
                backend
                    .upload(
                        identity,
                        "workspace",
                        "item",
                        &name_policy::PortablePath::parse("path").unwrap(),
                        source,
                    )
                    .map(|staged| {
                        assert_eq!(staged.item_id, "upload");
                        json!({"returned_by":"upload"})
                    }),
            ),
            (
                "upload_generation",
                backend
                    .upload_generation(
                        identity,
                        "workspace",
                        "item",
                        &name_policy::PortablePath::parse("path").unwrap(),
                        source,
                        37,
                    )
                    .map(|staged| {
                        assert_eq!(staged.item_id, "upload_generation");
                        assert_eq!(staged.dirty_generation, 37);
                        json!({"returned_by":"upload_generation"})
                    }),
            ),
            (
                "content",
                backend.content(identity, "version-content").map(content),
            ),
            (
                "range_content",
                backend
                    .range_content(
                        identity,
                        crate::ReadContent {
                            version: "version-range",
                            context: None,
                        },
                    )
                    .map(content),
            ),
            (
                "range_content",
                backend
                    .range_content(
                        identity,
                        crate::ReadContent {
                            version: "version-context",
                            context: Some(&crate::DownloadContext {
                                repository_id: "repository".into(),
                                path: name_policy::LookupPath::parse("File.txt").unwrap(),
                            }),
                        },
                    )
                    .map(content),
            ),
            (
                "workspace_content",
                backend
                    .workspace_content(identity, "workspace-read", "item-read")
                    .map(content),
            ),
            (
                "checksums",
                backend
                    .checksums(identity, "version-checksums")
                    .map(|bytes| json!(bytes)),
            ),
        ]
    }
}

fn assert_identity(actual: Option<&Identity>, expected: &Identity) {
    let actual = actual.expect("the request identity must be forwarded");
    assert_eq!(actual.principal, expected.principal);
    assert_eq!(actual.admission_session, expected.admission_session);
    assert_eq!(actual.smb_binding, expected.smb_binding);
}

fn assert_error(actual: ApiError, method: &str) {
    let expected = sentinel_error(method);
    assert_eq!(actual.status, expected.status);
    assert_eq!(actual.code, expected.code);
    assert_eq!(actual.message, expected.message);
    assert_eq!(actual.retryable, expected.retryable);
    assert_eq!(actual.request_id, expected.request_id);
}

#[test]
fn protected_overrides_preserve_identity_arguments_results_and_errors() {
    let fixture = Fixture::new();
    let results = fixture.protected_calls();
    for (method, result) in &results {
        let expected = if *method == "checksums" {
            json!([19, 23])
        } else {
            json!({"returned_by": method})
        };
        assert_eq!(result.as_ref().unwrap(), &expected, "{method}");
    }
    let upload = upload_arguments("workspace", "item", "path", &fixture.source);
    let mut generation = upload.clone();
    generation["generation"] = 37.into();
    let arguments = [
        json!({"operation_id":"op_00000000000000000000000000000000"}),
        json!({"operation_id":"op_00000000000000000000000000000000", "workspace_id":"workspace", "expected_head":"head", "fencing_token":41, "message":"commit", "request_digest":"digest"}),
        json!({"action": "status", "input": {"probe": 3}, "bootstrap": "bootstrap-probe"}),
        Value::Null,
        json!({"branch":"branch","commit":"commit"}),
        json!({"workspace":"workspace"}),
        json!({"workspace":"workspace","item":"item"}),
        json!({"workspace":"workspace"}),
        public_envelope().into_body(),
        json!({"version_id":"version"}),
        json!({"version_id":"version"}),
        json!({"repository_id":"repository"}),
        json!({"repository_id":"repository"}),
        Value::Null,
        json!({"repository_id":"repository"}),
        json!({"repository_id":"repository","branch_id":"branch","enabled":false,"expected_history_generation":"7","operation_id":"op_00000000000000000000000000000000"}),
        json!({"id":"policy"}),
        json!({"id":"rule"}),
        json!({"policy_id":"policy","repository_id":"repository","path_prefix":"Folder","target_copies":3,"expected_generation":"17","operation_id":"op_00000000000000000000000000000000"}),
        json!({"rule_id":"rule","repository_id":"repository","path_prefix":"Folder","primary_slot_id":"P01","expected_generation":"17","operation_id":"op_00000000000000000000000000000000"}),
        json!({"left_commit_id":"left","right_commit_id":"right"}),
        Value::Null,
        Value::Null,
        json!({"file_id":"file"}),
        Value::Null,
        json!({"version_id":"version"}),
        json!({"after":"17"}),
        json!({"history_tree_id":"history"}),
        json!({"commit_id":"commit"}),
        json!({"branch_id":"branch","path":"File.txt","key":"file.txt","commit_id":"commit"}),
        json!({"id":"branch"}),
        json!({"id":"repository"}),
        Value::Null,
        json!({"branch_id":"branch","commit_id":"commit"}),
        json!({"branch_id":"branch"}),
        json!({"workspace_id":"workspace","change":{"kind":"mkdir","path":"File.txt","destination":null}}),
        json!({"workspace":"workspace","logical_size":42}),
        json!({"branch_id":"branch","source_commit_id":"source"}),
        json!({"workspace_id":"workspace"}),
        json!({"workspace":"workspace","path":"File.txt","key":"file.txt"}),
        json!({"branch":"branch"}),
        json!({"branch":"branch","entry_id":"branch"}),
        json!({"branch":"branch","path":"File.txt","key":"file.txt"}),
        json!({"branch":"branch","commit":"commit","path":"File.txt","key":"file.txt"}),
        json!({"action": format!("{:?}", BackendAction::Archive), "input": {"probe": 7}}),
        upload,
        generation,
        json!({"version": "version-content"}),
        json!({"version":"version-range","context":null}),
        json!({"version":"version-context","context":{"repository_id":"repository","path":"File.txt","key":"file.txt"}}),
        json!({"workspace": "workspace-read", "item": "item-read"}),
        json!({"version": "version-checksums"}),
    ];
    let calls = std::mem::take(&mut *fixture.probe.calls.lock().unwrap());
    assert_eq!(calls.len(), results.len());
    assert_eq!(arguments.len(), results.len());
    for ((call, (method, _)), arguments) in calls.iter().zip(&results).zip(arguments) {
        assert_eq!(call.method, *method);
        assert_identity(call.identity.as_ref(), &fixture.identity);
        assert_eq!(call.arguments, arguments, "{method}");
    }
    fixture.probe.fail.store(true, Ordering::SeqCst);
    for (method, result) in fixture.protected_calls() {
        assert_error(result.unwrap_err(), method);
    }
    assert_eq!(fixture.probe.calls.lock().unwrap().len(), results.len());
}

#[test]
fn revoked_sessions_never_reach_protected_backend_methods() {
    let fixture = Fixture::new();
    fixture.auth.lock().unwrap().logout(&fixture.token).unwrap();
    for (method, result) in fixture.protected_calls() {
        let error = result.unwrap_err();
        assert_eq!(error.status, 401, "{method}");
        assert_eq!(
            error.code,
            storage_contract::ErrorCode::Unauthorized,
            "{method}"
        );
    }
    assert!(fixture.probe.calls.lock().unwrap().is_empty());
}

#[test]
fn public_probes_and_bootstrap_setup_keep_their_explicit_exceptions() {
    let fixture = Fixture::new();
    fixture.auth.lock().unwrap().logout(&fixture.token).unwrap();
    assert_eq!(
        fixture.backend.capabilities().unwrap().max_upload_bytes(),
        42
    );
    assert_eq!(fixture.backend.ready().unwrap(), crate::Readiness::Managed);
    assert_eq!(
        fixture
            .backend
            .setup_operation("status", json!({"probe": 13}), Some("bootstrap-only"), None)
            .unwrap(),
        json!({"returned_by": "setup_operation"})
    );
    let calls = fixture.probe.calls.lock().unwrap();
    assert_eq!(calls.len(), 3);
    assert_eq!(calls[0].method, "capabilities");
    assert!(calls[0].identity.is_none());
    assert_eq!(calls[0].arguments, Value::Null);
    assert_eq!(calls[1].method, "ready");
    assert!(calls[1].identity.is_none());
    assert_eq!(calls[1].arguments, Value::Null);
    assert_eq!(calls[2].method, "setup_operation");
    assert!(calls[2].identity.is_none());
    assert_eq!(
        calls[2].arguments,
        json!({"action": "status", "input": {"probe": 13}, "bootstrap": "bootstrap-only"})
    );
}

#[test]
fn lifecycle_delegation_preserves_errors_and_private_hook_boundaries() {
    let fixture = Fixture::new();
    fixture.backend.start_management(fixture.auth.clone());
    assert_eq!(
        fixture
            .backend
            .observe_disks(&InternalDiskObservation(()))
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::UnsupportedOperation
    );
    assert!(fixture.probe.calls.lock().unwrap().is_empty());
    fixture.backend.stop_management();
    fixture.backend.drain_management().unwrap();
    assert_eq!(
        fixture.backend.management_status(),
        crate::StorageStatus::Managed {
            state: crate::StorageState::ReadOnly,
            recovering: true
        }
    );
    fixture.probe.fail.store(true, Ordering::SeqCst);
    assert_error(
        fixture.backend.drain_management().unwrap_err(),
        "drain_management",
    );
    let calls = fixture.probe.calls.lock().unwrap();
    assert_eq!(
        calls.iter().map(|call| call.method).collect::<Vec<_>>(),
        [
            "stop_management",
            "drain_management",
            "management_status",
            "drain_management"
        ]
    );
    assert!(calls.iter().all(|call| call.identity.is_none()));
}

fn public_seal() -> storage_contract::SealedWorkspace {
    storage_contract::SealedWorkspace {
        operation_id: "op_00000000000000000000000000000000".parse().unwrap(),
        workspace_id: "workspace".into(),
        repository_id: "repo".into(),
        branch_id: "branch".into(),
        expected_head: "head".into(),
        fencing_token: 41,
        message: "public".into(),
        requested_protection: "strict",
        file_placement: BTreeMap::new(),
        expected_topology_generation: 3,
        sealed_manifest_hash: "manifest".into(),
    }
}
fn public_envelope() -> storage_contract::PublicCommitEnvelope {
    let mut body = json_codec::to_value(public_seal()).unwrap();
    body["request_digest"] = "digest".into();
    storage_contract::PublicCommitEnvelope::parse("workspace", body).unwrap()
}

fn staged_result(
    item: &str,
    path: &name_policy::PortablePath,
    source: &UploadSource,
    generation: u64,
) -> storage_contract::StagedFile {
    storage_contract::StagedFile {
        item_id: item.into(),
        path: path.clone(),
        source_path: "/private/recorded-upload".into(),
        size: source.size,
        content_hash: source.hash,
        dirty_generation: generation,
        base_version_id: None,
    }
}

fn recorded_version(version: &str) -> crate::VersionSnapshot {
    crate::VersionSnapshot {
        version: storage_contract::FileVersion {
            history_tree_id: "history".into(),
            version_id: version.into(),
            file_id: "file".into(),
            creation_primary_slot_id: "P01".into(),
            logical_size: u64::MAX,
            version_sequence: 42,
            content_hash: storage_contract::ContentHash::from_digest(blake3::hash(
                b"version fixture",
            )),
            checksum_chunk_bytes: 4096,
            checksum_manifest_hash: "manifest".into(),
            creation_name: "file.txt".into(),
            storage_name: "body".into(),
            created_at: "now".into(),
            source_version_ids: vec![],
            created_as_full_checkpoint: true,
        },
        content_state: storage_contract::ContentState::Available,
    }
}

fn recorded_repository(id: &str) -> storage_contract::Repository {
    storage_contract::Repository {
        repository_id: id.into(),
        name: "repository".into(),
        history_tree_id: "history".into(),
        main_branch_id: "branch".into(),
        primary_slot_id: "P01".into(),
        target_copies: 3,
        name_policy: name_policy::NAME_POLICY.into(),
        unicode_version: name_policy::UNICODE_VERSION.into(),
        smb_enabled: true,
        smb_binding_generation: u64::MAX,
        file_manager_root: false,
    }
}

fn recorded_capacity() -> storage_contract::WorkspaceCapacity {
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

fn recorded_tree() -> crate::TreeSnapshot {
    crate::TreeSnapshot {
        branch_id: "branch".into(),
        commit_id: "commit".into(),
        entries: vec![],
        pending_file_versions: std::collections::BTreeMap::from([(
            "file".into(),
            "version".into(),
        )]),
    }
}

fn recorded_branch() -> crate::BranchSnapshot {
    crate::BranchSnapshot {
        branch_id: "branch".into(),
        history_tree_id: "history".into(),
        name: "Main".into(),
        head_commit_id: "head".into(),
        fork_commit_id: "fork".into(),
        branch_kind: storage_contract::BranchKind::Full,
        file_id: None,
        file_node_id: None,
        pending_file_versions: std::collections::BTreeMap::new(),
        generation: u64::MAX,
        state: storage_contract::BranchState::Active,
        file: None,
    }
}

fn recorded_resolved() -> crate::ResolvedEntry {
    crate::ResolvedEntry {
        branch_id: "branch".into(),
        commit_id: "commit".into(),
        entry: storage_contract::Entry {
            entry_id: "entry".into(),
            path: name_policy::PortablePath::parse("File.txt").unwrap(),
            kind: storage_contract::NamespaceEntryKind::File,
            file_id: "file".into(),
            version_id: "version".into(),
            size: u64::MAX,
        },
        version: Some(recorded_version("version").version),
        content: Some(crate::ContentDispositionSnapshot {
            version_id: Some("version".into()),
            source_version_id: None,
            stored_state: Some(storage_contract::ContentState::Available),
        }),
    }
}

fn recorded_commit(commit: &str) -> Arc<storage_contract::Commit> {
    Arc::new(storage_contract::Commit {
        commit_id: commit.into(),
        history_tree_id: "history".into(),
        kind: "snapshot".into(),
        parent_commit_ids: vec![],
        tree_id: "tree".into(),
        sequence: 1,
        operation_id: storage_contract::OperationId::generate(),
        author: "author".into(),
        message: "snapshot fixture".into(),
        entries: vec![],
        namespace_deletions: None,
        pending_file_versions: Default::default(),
        provenance: None,
    })
}

fn recorded_graph(id: &str) -> crate::HistoryGraphSnapshot {
    crate::HistoryGraphSnapshot {
        history: storage_contract::HistoryTree {
            history_tree_id: id.into(),
            repository_id: "repository".into(),
            root_commit_id: "commit".into(),
            main_branch_id: Some("branch".into()),
            generation: u64::MAX,
            state: storage_contract::HistoryState::Active,
            provenance: None,
        },
        commits: vec![recorded_commit("commit")],
        branches: vec![],
        merge_links: vec![],
        boundary_marks: vec![],
    }
}

fn recorded_protection_policy(id: &str) -> storage_contract::ProtectionPolicy {
    storage_contract::ProtectionPolicy {
        policy_id: id.into(),
        repository_id: "repository".into(),
        path_prefix: "".into(),
        target_copies: 3,
        inheritable: true,
        scope_directory_id: None,
        scope_file_id: None,
        backup_prefixes: Default::default(),
        generation: 17,
    }
}
fn recorded_placement_rule(id: &str) -> storage_contract::PlacementRule {
    storage_contract::PlacementRule {
        rule_id: id.into(),
        repository_id: "repository".into(),
        path_prefix: "".into(),
        primary_slot_id: "P01".into(),
        generation: 17,
    }
}
