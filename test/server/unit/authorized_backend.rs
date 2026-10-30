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
    history_gates: Mutex<std::collections::VecDeque<HistoryGate>>,
    selection_repository: Mutex<Option<String>>,
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
    fn wait_for_history_scan(&self, control: &crate::HistoryReadControl) -> Result<(), ApiError> {
        let gate = self.history_gates.lock().unwrap().pop_front();
        if let Some(gate) = gate {
            gate.started.send(()).unwrap();
            gate.release
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            let result = control.checkpoint();
            gate.finished
                .send(result.as_ref().map(|()| ()).map_err(|error| error.code()))
                .unwrap();
            result.map_err(|source| {
                ApiError::new(
                    ::http::StatusCode::REQUEST_TIMEOUT,
                    source.code(),
                    source.message(),
                )
                .with_source(source)
            })?;
        } else {
            control.checkpoint().unwrap();
        }
        Ok(())
    }

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

fn recorded_plan(id: &str) -> storage_contract::Plan {
    storage_contract::Plan {
        plan_id: id.into(),
        kind: storage_contract::PlanKind::Purge,
        principal: "admin".into(),
        expected_sequence: u64::MAX,
        digest: "digest".into(),
        input: json!({"history_tree_id":"history"}),
        state: storage_contract::PlanState::Ready,
    }
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

fn recorded_backup_configuration() -> storage_contract::BackupConfigurationSnapshot {
    storage_contract::BackupConfigurationSnapshot {
        scope: storage_contract::BackupScope {
            id: "scope".into(),
            kind: storage_contract::NamespaceEntryKind::Directory,
            path: "docs".into(),
        },
        policy_id: None,
        effective_policy_id: Some("parent".into()),
        generation: 0,
        effective_generation: 7,
        inherited: true,
        target_copies: 2,
        backup_prefixes: BTreeMap::from([("B01".into(), "backup".into())]),
        route_slot_ids: vec!["P01".into(), "B01".into()],
        available_route_slot_ids: vec!["P01".into(), "B01".into()],
        backup_routes: vec![vec!["P01".into(), "B01".into()]],
        backup_slot_ids: std::collections::BTreeSet::from(["B01".into()]),
        max_copies: 2,
        file_count: 3,
        backup_paths: BTreeMap::new(),
    }
}

fn rooted_query() -> BTreeMap<String, String> {
    BTreeMap::from([("history_root".to_owned(), recorded_root())])
}
fn recorded_root() -> String {
    "ab".repeat(32)
}
fn recorded_continuation(kind: crate::backend_file_history::CursorKind) -> crate::Continuation {
    crate::Continuation {
        direction: crate::backend_file_history::PageDirection::Forward,
        seed: crate::backend_file_history::HistoryCursorSeed::new(
            kind,
            &recorded_root(),
            "history",
            "file",
            &json!({"recorded":true}),
        ),
        more_before: false,
        more_after: false,
    }
}
fn recorded_slice<T>(
    request: &crate::backend_file_history::ReadIndexedSlice,
    items: Vec<T>,
) -> crate::backend_file_history::IndexedSlice<T> {
    crate::backend_file_history::IndexedSlice {
        file_id: request.file_id.as_str().into(),
        history_tree_id: "history".into(),
        history_root: recorded_root(),
        subject_id: request.subject_id.as_str().into(),
        offset: request.offset,
        total: items.len(),
        items,
    }
}

impl Backend for RecordingBackend {
    fn operation_outcome(
        &self,
        identity: &Identity,
        operation_id: storage_contract::OperationId,
    ) -> Result<Option<storage_contract::OperationOutcome>, ApiError> {
        self.record(
            "operation_outcome",
            Some(identity),
            json!({"operation_id":operation_id}),
        )?;
        Ok(Some(
            recorded_operation(operation_id, "operation_outcome").into(),
        ))
    }
    fn operation(
        &self,
        identity: &Identity,
        operation_id: storage_contract::OperationId,
    ) -> Result<crate::OperationObservation, ApiError> {
        self.record(
            "operation",
            Some(identity),
            json!({"operation_id":operation_id}),
        )?;
        Ok(crate::OperationObservation {
            outcome: recorded_operation(operation_id, "operation").into(),
            health: None,
        })
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
        Ok(crate::Readiness::Managed {
            history_index: crate::HistoryIndexStatus::Ready,
        })
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
            vec![],
            recorded_commit(request.right),
            vec![],
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
    fn create_merge_link(
        &self,
        identity: &crate::auth::Identity,
        request: crate::CreateMergeLink,
    ) -> Result<storage_contract::MergeLink, crate::ApiError> {
        self.record("create_merge_link",Some(identity),json!({"history_tree_id":request.history_tree_id,"source_branch_id":request.source_branch_id,"source_commit_id":request.source_commit_id,"target_branch_id":request.target_branch_id,"target_commit_id":request.target_commit_id,"operation_id":request.operation_id}))?;
        Ok(recorded_merge_link(request.operation_id))
    }
    fn create_repository(
        &self,
        identity: &crate::auth::Identity,
        request: crate::CreateRepository,
    ) -> Result<storage_contract::Repository, crate::ApiError> {
        self.record("create_repository",Some(identity),json!({"name":request.name.as_str(),"key":request.name.key(),"primary_slot_id":request.primary_slot_id,"target_copies":request.target_copies,"operation_id":request.operation_id}))?;
        Ok(recorded_repository("created"))
    }
    fn create_branch(
        &self,
        identity: &crate::auth::Identity,
        request: crate::CreateBranch,
    ) -> Result<crate::BranchSnapshot, crate::ApiError> {
        self.record("create_branch",Some(identity),json!({"repository_id":request.repository_id,"source_commit_id":request.source_commit_id,"name":request.name.as_str(),"key":request.name.key(),"operation_id":request.operation_id}))?;
        Ok(recorded_branch())
    }
    fn create_file_branch(
        &self,
        identity: &crate::auth::Identity,
        request: crate::CreateFileBranch,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.record(
            "create_file_branch",
            Some(identity),
            json!({"file_id":request.file_id,"source_version_id":request.source_version_id,"source_node_id":request.source_node_id,"name":request.name.as_str(),"key":request.name.key(),"operation_id":request.operation_id}),
        )?;
        Ok(recorded_operation(
            request.operation_id,
            "create_file_branch",
        ))
    }
    fn update_note(
        &self,
        identity: &crate::auth::Identity,
        request: crate::UpdateNote,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.record(
            "update_note",
            Some(identity),
            json!({"target":format!("{:?}",request.target),"note":request.note.as_str(),"operation_id":request.operation_id}),
        )?;
        Ok(recorded_operation(request.operation_id, "update_note"))
    }
    fn merge_file_branches(
        &self,
        identity: &crate::auth::Identity,
        request: crate::MergeFileBranches,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.record(
            "merge_file_branches",
            Some(identity),
            json!({"file_id":request.file_id,"inputs":request.inputs,"chosen_node_id":request.chosen_node_id,"surviving_branch_id":request.surviving_branch_id,"confirmed":request.confirmed,"location":request.location,"operation_id":request.operation_id}),
        )?;
        Ok(recorded_operation(
            request.operation_id,
            "merge_file_branches",
        ))
    }
    fn update_file_node_tags(
        &self,
        identity: &crate::auth::Identity,
        request: crate::UpdateFileNodeTags,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.record(
            "update_file_node_tags",
            Some(identity),
            json!({"file_id":request.file_id,"targets":request.targets,"add":request.add,"remove":request.remove,"location":request.location,"operation_id":request.operation_id}),
        )?;
        Ok(recorded_operation(
            request.operation_id,
            "update_file_node_tags",
        ))
    }
    fn create_disk_directory(
        &self,
        identity: &crate::auth::Identity,
        request: crate::CreateDiskDirectory,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.record(
            "create_disk_directory",
            Some(identity),
            json!({"slot_id":request.slot_id,"path":request.path.as_str(),"key":request.path.key(),"operation_id":request.operation_id}),
        )?;
        Ok(recorded_operation(
            request.operation_id,
            "create_disk_directory",
        ))
    }
    fn create_disk_write_session(
        &self,
        identity: &crate::auth::Identity,
        request: crate::CreateDiskWriteSession,
    ) -> Result<crate::DiskWriteSession, crate::ApiError> {
        self.record(
            "create_disk_write_session",
            Some(identity),
            json!({"slot_id":request.slot_id,"path":request.path.as_str(),"key":request.path.key(),"branch_id":request.branch_id.as_ref().map(crate::WriteSessionBranchId::as_str)}),
        )?;
        Ok(crate::DiskWriteSession {
            workspace: storage_contract::Workspace {
                workspace_id: "disk-workspace".into(),
                purpose: storage_contract::WorkspacePurpose::Normal,
                branch_id: "namespace-branch".into(),
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
            },
            relative_path: "file.txt".into(),
            display_path: request.path.into_string(),
        })
    }
    fn backup_configuration(
        &self,
        identity: &Identity,
        request: crate::ReadBackupConfiguration,
    ) -> Result<storage_contract::BackupConfigurationSnapshot, ApiError> {
        self.record(
            "backup_configuration",
            Some(identity),
            json!({"repository_id":request.repository_id,"path":request.path}),
        )?;
        Ok(recorded_backup_configuration())
    }
    fn configure_backups(
        &self,
        identity: &Identity,
        request: crate::ConfigureBackups,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        self.record("configure_backups", Some(identity), json!({"repository_id":request.repository_id,"path":request.path,"target_copies":request.target_copies,"backup_prefixes":request.backup_prefixes,"expected_generation":request.expected_generation,"expected_scope_id":request.expected_scope_id,"operation_id":request.operation_id}))?;
        Ok(recorded_operation(
            request.operation_id,
            "configure_backups",
        ))
    }
    fn plan(
        &self,
        identity: &crate::auth::Identity,
        plan: &str,
    ) -> Result<storage_contract::Plan, crate::ApiError> {
        self.record("plan", Some(identity), json!({"plan_id":plan}))?;
        Ok(recorded_plan(plan))
    }
    fn cancel_plan(
        &self,
        identity: &crate::auth::Identity,
        request: Result<crate::CancelPlan, crate::ApiError>,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        let request = request?;
        let operation_id = request.operation_id?;
        self.record(
            "cancel_plan",
            Some(identity),
            json!({"plan_id":request.plan_id,"operation_id":operation_id}),
        )?;
        Ok(recorded_operation(operation_id, "cancel_plan"))
    }
    fn register_disk(
        &self,
        identity: &crate::auth::Identity,
    ) -> Result<std::convert::Infallible, crate::ApiError> {
        self.record("register_disk", Some(identity), Value::Null)?;
        Err(crate::ApiError::conflict(
            storage_contract::ErrorCode::OfflineConfigurationRequired,
            "offline registration",
        ))
    }
    fn commit_plan(
        &self,
        identity: &crate::auth::Identity,
        request: Result<crate::CommitPlan, crate::ApiError>,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        let request = request?;
        let operation_id = request.operation_id?;
        self.record(
            "commit_plan",
            Some(identity),
            json!({"plan_id":request.plan_id,"plan_digest":request.plan_digest,"operation_id":operation_id}),
        )?;
        Ok(recorded_operation(operation_id, "commit_plan"))
    }
    fn prepare_historical_save(
        &self,
        identity: &crate::auth::Identity,
        request: Result<crate::PrepareHistoricalSave, crate::ApiError>,
    ) -> Result<storage_contract::Plan, crate::ApiError> {
        let request = request?;
        self.record(
            "prepare_historical_save",
            Some(identity),
            json!({"history_tree_id":request.history_tree_id,"options":request.options?}),
        )?;
        Ok(storage_contract::Plan {
            plan_id: "plan".into(),
            kind: storage_contract::PlanKind::HistoricalSave,
            principal: identity.principal.clone(),
            expected_sequence: 7,
            digest: "digest".into(),
            input: json!({}),
            state: storage_contract::PlanState::Ready,
        })
    }
    fn prepare_purge(
        &self,
        identity: &crate::auth::Identity,
        request: Result<crate::PreparePurge, crate::ApiError>,
    ) -> Result<storage_contract::Plan, crate::ApiError> {
        let request = request?;
        self.record(
            "prepare_purge",
            Some(identity),
            json!({"history_tree_id":request.history_tree_id,"version_ids":request.version_ids?}),
        )?;
        Ok(storage_contract::Plan {
            plan_id: "purge-plan".into(),
            kind: storage_contract::PlanKind::Purge,
            principal: identity.principal.clone(),
            expected_sequence: 7,
            digest: "digest".into(),
            input: json!({}),
            state: storage_contract::PlanState::Ready,
        })
    }
    fn prepare_split(
        &self,
        identity: &crate::auth::Identity,
        request: Result<crate::PrepareSplit, crate::ApiError>,
    ) -> Result<storage_contract::Plan, crate::ApiError> {
        let request = request?;
        self.record("prepare_split",Some(identity),json!({"history_tree_id":request.history_tree_id,"cut_commit_id":request.cut_commit_id?}))?;
        Ok(storage_contract::Plan {
            plan_id: "split-plan".into(),
            kind: storage_contract::PlanKind::Split,
            principal: identity.principal.clone(),
            expected_sequence: 7,
            digest: "digest".into(),
            input: json!({}),
            state: storage_contract::PlanState::Ready,
        })
    }
    fn namespace_batch(
        &self,
        identity: &crate::auth::Identity,
        request: crate::NamespaceBatch,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.record(
            "namespace_batch",
            Some(identity),
            json!({"slot_id":request.slot_id,"request":request.request,"operation_id":request.operation_id}),
        )?;
        Ok(recorded_operation(request.operation_id, "namespace_batch"))
    }
    fn select_file_branch(
        &self,
        identity: &crate::auth::Identity,
        request: crate::SelectFileBranch,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.record(
            "select_file_branch",
            Some(identity),
            json!({"file_id":request.file_id,"branch_id":request.branch_id,"location":request.location.as_ref().map(|location|json!({"repository_id":location.repository_id,"path":location.path.as_str(),"key":location.path.key()})),"operation_id":request.operation_id}),
        )?;
        Ok(recorded_operation(
            request.operation_id,
            "select_file_branch",
        ))
    }
    fn rename_file_branch(
        &self,
        identity: &crate::auth::Identity,
        request: crate::RenameFileBranch,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.record(
            "rename_file_branch",
            Some(identity),
            json!({"file_id":request.file_id,"branch_id":request.branch_id,"name":request.name.as_str(),"key":request.name.key(),"operation_id":request.operation_id}),
        )?;
        Ok(recorded_operation(
            request.operation_id,
            "rename_file_branch",
        ))
    }
    fn listing(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadListing,
        control: &crate::HistoryReadControl,
    ) -> Result<crate::ListingSnapshot, crate::ApiError> {
        self.wait_for_history_scan(control)?;
        self.record(
            "listing",
            Some(identity),
            json!({"slot_id":request.slot_id(),"path":request.path(),"limit":request.limit(),"query":request.query(),"cursor":request.cursor().is_some(),"selected":request.selected()}),
        )?;
        Ok(crate::ListingSnapshot {
            slot_id: request.slot_id().into(),
            path: request.path().into(),
            backup: false,
            entries: vec![],
            selected_entry: None,
            context: None,
            directory_file_count: 0,
            directory_branches: vec![],
            directory_branches_supported: false,
            default_branch_scope: false,
            total_entries: 0,
            snapshot: "listing-snapshot".into(),
            previous_cursor: None,
            next_cursor: None,
        })
    }
    fn mutate_disk_directory(
        &self,
        identity: &crate::auth::Identity,
        request: crate::MutateDiskDirectory,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.record(
            "mutate_disk_directory",
            Some(identity),
            json!({"slot_id":request.slot_id,"path":request.path.as_ref().map_or("",name_policy::LookupPath::as_str),"operation_id":request.operation_id,"name":request.name.as_str(),"action":format!("{:?}",request.action)}),
        )?;
        Ok(recorded_operation(
            request.operation_id,
            "mutate_disk_directory",
        ))
    }
    fn initialize_directory_scope(
        &self,
        identity: &crate::auth::Identity,
        request: crate::InitializeDirectoryScope,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.record(
            "initialize_directory_scope",
            Some(identity),
            json!({"repository_id":request.repository_id,"path":request.path.as_ref().map_or("",name_policy::LookupPath::as_str),"operation_id":request.operation_id}),
        )?;
        Ok(recorded_operation(
            request.operation_id,
            "initialize_directory_scope",
        ))
    }
    fn archive_branch(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ArchiveBranch,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.record(
            "archive_branch",
            Some(identity),
            json!({"branch_id":request.branch_id,"operation_id":request.operation_id}),
        )?;
        Ok(recorded_operation(request.operation_id, "archive_branch"))
    }
    fn rename_directory_branch(
        &self,
        identity: &crate::auth::Identity,
        request: crate::RenameDirectoryBranch,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.record("rename_directory_branch",Some(identity),json!({"repository_id":request.repository_id,"directory_branch_id":request.directory_branch_id,"name":request.name.as_str(),"key":request.name.key(),"operation_id":request.operation_id}))?;
        Ok(recorded_operation(
            request.operation_id,
            "rename_directory_branch",
        ))
    }
    fn select_default_directory_branch(
        &self,
        identity: &crate::auth::Identity,
        request: crate::SelectDefaultDirectoryBranch,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        self.record("select_default_directory_branch",Some(identity),json!({"repository_id":request.repository_id,"directory_branch_id":request.directory_branch_id,"operation_id":request.operation_id}))?;
        Ok(recorded_operation(
            request.operation_id,
            "select_default_directory_branch",
        ))
    }
    fn directory_selection(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadDirectorySelection,
        control: &crate::HistoryReadControl,
    ) -> Result<crate::DirectorySelectionSnapshot, crate::ApiError> {
        self.wait_for_history_scan(control)?;
        self.record("directory_selection",Some(identity),json!({"repository_id":request.repository_id(),"key":request.scope().map(name_policy::LookupPath::key),"sources":request.sources().len(),"entries":request.entries().len()}))?;
        Ok(crate::DirectorySelectionSnapshot {
            repository_id: self
                .selection_repository
                .lock()
                .unwrap()
                .take()
                .unwrap_or_else(|| request.repository_id().into()),
            branches: vec![],
        })
    }
    fn directory_scopes(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadDirectoryScopes<'_>,
    ) -> Result<crate::DirectoryScopesSnapshot, crate::ApiError> {
        self.record("directory_scopes",Some(identity),json!({"repository_id":request.repository_id,"key":request.filter.key(),"summary":matches!(request.projection,crate::DirectoryBranchProjection::Summary)}))?;
        Ok(crate::DirectoryScopesSnapshot {
            repository_id: request.repository_id.into(),
            branches: crate::DirectoryScopeRows::Summary(vec![]),
        })
    }
    fn default_directory_branch(
        &self,
        identity: &crate::auth::Identity,
        repository_id: &str,
    ) -> Result<crate::DefaultDirectoryBranchSnapshot, crate::ApiError> {
        self.record(
            "default_directory_branch",
            Some(identity),
            json!({"repository_id":repository_id}),
        )?;
        Ok(crate::DefaultDirectoryBranchSnapshot {
            repository_id: repository_id.into(),
            namespace_branch_id: "namespace".into(),
            selected: Some(crate::SelectedDirectoryBranch {
                directory_branch_id: "directory".into(),
                name: "selected".into(),
            }),
            directory_branches: vec![],
        })
    }
    fn file_history_snapshot(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadFileHistory,
    ) -> Result<crate::FileHistoryPage, crate::ApiError> {
        self.record(
            "file_history_snapshot",
            Some(identity),
            json!({"file_id": request.file_id(), "limit": request.limit, "location": request.location().map(|(repository_id, path)| json!({"repository_id":repository_id,"path":path.as_str(),"key":path.key()}))}),
        )?;
        Ok(crate::FileHistoryPage {
            file_id: request.file_id().into(),
            repository_id: "repo".into(),
            history_tree_id: "history".into(),
            history_root: recorded_root(),
            context: crate::HistoryContext {
                path: None,
                note: String::new(),
                current_node_id: None,
                selected_branch_id: None,
                content: None,
            },
            nodes: vec![],
            continuation: recorded_continuation(crate::backend_file_history::CursorKind::Nodes),
            anchor: None,
            limit: request.limit,
        })
    }
    fn file_history_node_parents(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadNodeParents,
    ) -> Result<crate::NodeParentsPage, crate::ApiError> {
        self.record(
            "file_history_node_parents",
            Some(identity),
            json!({"file_id": request.collection.file_id.as_str(), "node_id": request.node_id.as_str(), "limit": request.collection.limit, "root": request.collection.root.as_str()}),
        )?;
        Ok(crate::NodeParentsPage {
            file_id: request.collection.file_id.as_str().into(),
            history_tree_id: "history".into(),
            history_root: recorded_root(),
            node_id: request.node_id.as_str().into(),
            parents: vec!["parent".into()],
            continuation: recorded_continuation(crate::backend_file_history::CursorKind::Parents),
        })
    }
    fn file_history_branches(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadHistoryBranches,
    ) -> Result<crate::HistoryBranchesPage, crate::ApiError> {
        self.record(
            "file_history_branches",
            Some(identity),
            json!({"file_id": request.0.file_id.as_str(), "limit": request.0.limit, "root": request.0.root.as_str()}),
        )?;
        Ok(crate::HistoryBranchesPage {
            file_id: request.0.file_id.as_str().into(),
            history_tree_id: "history".into(),
            history_root: recorded_root(),
            selected_branch_id: None,
            branches: vec![],
            continuation: recorded_continuation(crate::backend_file_history::CursorKind::Branches),
        })
    }
    fn file_history_merges(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadHistoryMerges,
    ) -> Result<crate::HistoryMergesPage, crate::ApiError> {
        self.record(
            "file_history_merges",
            Some(identity),
            json!({"file_id": request.0.file_id.as_str(), "limit": request.0.limit, "root": request.0.root.as_str()}),
        )?;
        Ok(crate::HistoryMergesPage {
            file_id: request.0.file_id.as_str().into(),
            history_tree_id: "history".into(),
            history_root: recorded_root(),
            merges: vec![],
            continuation: recorded_continuation(crate::backend_file_history::CursorKind::Merges),
        })
    }
    fn file_history_merge_inputs(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadMergeInputs,
    ) -> Result<crate::MergeInputsPage, crate::ApiError> {
        self.record(
            "file_history_merge_inputs",
            Some(identity),
            json!({"file_id": request.0.file_id.as_str(), "merge_id": request.0.subject_id.as_str(), "offset": request.0.offset, "limit": request.0.limit, "root": request.0.root.as_str()}),
        )?;
        Ok(recorded_slice(
            &request.0,
            vec![("branch".into(), "node".into())],
        ))
    }
    fn file_history_merge_parents(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadMergeParents,
    ) -> Result<crate::MergeParentsPage, crate::ApiError> {
        self.record(
            "file_history_merge_parents",
            Some(identity),
            json!({"file_id": request.0.file_id.as_str(), "merge_id": request.0.subject_id.as_str(), "offset": request.0.offset, "limit": request.0.limit, "root": request.0.root.as_str()}),
        )?;
        Ok(recorded_slice(&request.0, vec!["parent".into()]))
    }
    fn file_history_node_field(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadNodeField,
    ) -> Result<crate::NodeFieldSlice, crate::ApiError> {
        self.record(
            "file_history_node_field",
            Some(identity),
            json!({"file_id": request.file_id.as_str(), "node_id": request.node_id.as_str(), "field": request.field.as_str(), "offset": request.offset, "max_bytes": request.max_bytes, "root": request.root.as_str()}),
        )?;
        Ok(crate::NodeFieldSlice {
            file_id: request.file_id.as_str().into(),
            history_tree_id: "history".into(),
            history_root: recorded_root(),
            node_id: request.node_id.as_str().into(),
            field: request.field,
            total_bytes: 4,
            offset: request.offset,
            text: "text".into(),
            complete: true,
        })
    }
    fn file_version_page(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadVersionPage,
    ) -> Result<crate::VersionPage, crate::ApiError> {
        self.record(
            "file_version_page",
            Some(identity),
            json!({"file_id": request.file_id.as_str(), "limit": request.limit, "root": request.root().map(|root| root.as_str())}),
        )?;
        let mut version = recorded_version("version").version;
        version.file_id = request.file_id.as_str().into();
        Ok(crate::VersionPage {
            file_id: request.file_id.as_str().into(),
            history_tree_id: "history".into(),
            history_root: recorded_root(),
            versions: vec![crate::VersionSummary::from(&version)],
            continuation: recorded_continuation(crate::backend_file_history::CursorKind::Versions),
        })
    }
    fn file_version_sources(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadVersionSources,
    ) -> Result<crate::VersionSourcesPage, crate::ApiError> {
        self.record(
            "file_version_sources",
            Some(identity),
            json!({"file_id": request.0.file_id.as_str(), "version_id": request.0.subject_id.as_str(), "offset": request.0.offset, "limit": request.0.limit, "root": request.0.root.as_str()}),
        )?;
        Ok(recorded_slice(&request.0, vec!["source".into()]))
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
    fn activity_history(
        &self,
        identity: &crate::auth::Identity,
        control: &crate::HistoryReadControl,
    ) -> Result<crate::ActivityHistorySnapshot, crate::ApiError> {
        self.wait_for_history_scan(control)?;
        self.record("activity_history", Some(identity), json!({}))?;
        Ok(crate::ActivityHistorySnapshot { events: vec![] })
    }
    fn history_graph(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadHistoryGraph,
        control: &crate::HistoryReadControl,
    ) -> Result<crate::HistoryGraphResponse, crate::ApiError> {
        self.wait_for_history_scan(control)?;
        let crate::GraphRead::Page {
            collection,
            after,
            limit,
        } = request.selection()
        else {
            panic!("recording graph fixture expects a page request");
        };
        self.record(
            "history_graph",
            Some(identity),
            json!({"history_tree_id":request.history_id(), "collection":collection, "after":after, "limit":limit.get()}),
        )?;
        Ok(crate::HistoryGraphResponse::Graph(recorded_graph(
            request.history_id(),
        )))
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
        _control: &crate::HistoryReadControl,
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
            history_gates: Mutex::new(std::collections::VecDeque::new()),
            selection_repository: Mutex::new(None),
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
                "operation_outcome",
                backend.operation_outcome(identity, operation_id).map(|result| {
                    let result = result.unwrap();
                    assert_eq!(result, recorded_operation(operation_id, "operation_outcome").into());
                    result.result
                }),
            ),
            (
                "operation",
                backend.operation(identity, operation_id).map(|result| {
                    assert_eq!(result.outcome, recorded_operation(operation_id, "operation").into());
                    assert!(result.health.is_none());
                    result.outcome.result
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
            ("create_merge_link", {
                backend.create_merge_link(identity,crate::CreateMergeLink {history_tree_id:"history".into(),source_branch_id:"source_branch".into(),source_commit_id:"source_commit".into(),target_branch_id:"target_branch".into(),target_commit_id:"target_commit".into(),operation_id}).map(|result| {assert_eq!(result,recorded_merge_link(operation_id));json!({"returned_by":"create_merge_link"})})
            }),
            ("create_repository", {
                backend.create_repository(identity,crate::CreateRepository {name:storage_contract::RepositoryName::parse("Café").unwrap(),primary_slot_id:"P01".into(),target_copies:3,operation_id}).map(|result| {assert_eq!(result,recorded_repository("created"));json!({"returned_by":"create_repository"})})
            }),
            ("create_branch", {
                backend.create_branch(identity,crate::CreateBranch {repository_id:"repository".into(),source_commit_id:"source".into(),name:storage_contract::BranchName::parse("Café").unwrap(),operation_id}).map(|result| {
                    assert_eq!(result.branch_id,"branch"); assert_eq!(result.generation,u64::MAX);
                    json!({"returned_by":"create_branch"})
                })
            }),
            ("create_file_branch", {
                backend.create_file_branch(identity,crate::CreateFileBranch {file_id:"file".into(),source_version_id:"version".into(),source_node_id:None,name:storage_contract::BranchName::parse("Café").unwrap(),operation_id}).map(|result| {assert_eq!(result,recorded_operation(operation_id,"create_file_branch"));json!({"returned_by":"create_file_branch"})})
            }),
            ("create_file_branch", {
                backend.create_file_branch(identity,crate::CreateFileBranch {file_id:"file".into(),source_version_id:"version".into(),source_node_id:Some("node".into()),name:storage_contract::BranchName::parse("Café").unwrap(),operation_id}).map(|result| {assert_eq!(result,recorded_operation(operation_id,"create_file_branch"));json!({"returned_by":"create_file_branch"})})
            }),
            ("update_note", {
                backend.update_note(identity,crate::UpdateNote {target:crate::NoteTarget::File("file".into()),note:storage_contract::Note::parse("".into()).unwrap(),operation_id}).map(|result| {assert_eq!(result,recorded_operation(operation_id,"update_note"));json!({"returned_by":"update_note"})})
            }),
            ("update_note", {
                backend.update_note(identity,crate::UpdateNote {target:crate::NoteTarget::Version("version".into()),note:storage_contract::Note::parse("".into()).unwrap(),operation_id}).map(|result| {assert_eq!(result,recorded_operation(operation_id,"update_note"));json!({"returned_by":"update_note"})})
            }),
            ("update_note", {
                backend.update_note(identity,crate::UpdateNote {target:crate::NoteTarget::Node {file:"file".into(),node:"node".into()},note:storage_contract::Note::parse("".into()).unwrap(),operation_id}).map(|result| {assert_eq!(result,recorded_operation(operation_id,"update_note"));json!({"returned_by":"update_note"})})
            }),
            ("merge_file_branches", {backend.merge_file_branches(identity,crate::MergeFileBranches {file_id:"file".into(),inputs:vec![storage_contract::FileMergeInput {branch_id:"branch".into(),expected_node_id:"head".into()}],chosen_node_id:"chosen".into(),surviving_branch_id:"branch".into(),confirmed:true,location:None,operation_id}).map(|result|{assert_eq!(result,recorded_operation(operation_id,"merge_file_branches"));json!({"returned_by":"merge_file_branches"})})}),
            ("merge_file_branches", {backend.merge_file_branches(identity,crate::MergeFileBranches {file_id:"file".into(),inputs:vec![storage_contract::FileMergeInput {branch_id:"branch".into(),expected_node_id:"head".into()}],chosen_node_id:"chosen".into(),surviving_branch_id:"branch".into(),confirmed:true,location:Some(storage_contract::FileLocation {repository_id:"repo".into(),path:"file.txt".into()}),operation_id}).map(|result|{assert_eq!(result,recorded_operation(operation_id,"merge_file_branches"));json!({"returned_by":"merge_file_branches"})})}),
            ("update_file_node_tags", {backend.update_file_node_tags(identity,crate::UpdateFileNodeTags {file_id:"file".into(),targets:vec![storage_contract::FileTagTarget {branch_id:"branch".into(),expected_node_id:"head".into(),source_node_id:"source".into()}],add:vec!["added".into()],remove:vec!["removed".into()],location:None,operation_id}).map(|result|{assert_eq!(result,recorded_operation(operation_id,"update_file_node_tags"));json!({"returned_by":"update_file_node_tags"})})}),
            ("update_file_node_tags", {backend.update_file_node_tags(identity,crate::UpdateFileNodeTags {file_id:"file".into(),targets:vec![storage_contract::FileTagTarget {branch_id:"branch".into(),expected_node_id:"head".into(),source_node_id:"source".into()}],add:vec!["added".into()],remove:vec!["removed".into()],location:Some(storage_contract::FileLocation {repository_id:"repo".into(),path:"file.txt".into()}),operation_id}).map(|result|{assert_eq!(result,recorded_operation(operation_id,"update_file_node_tags"));json!({"returned_by":"update_file_node_tags"})})}),
            ("create_disk_directory", {backend.create_disk_directory(identity,crate::CreateDiskDirectory {slot_id:"P01".into(),path:name_policy::PortablePath::parse("Café/folder").unwrap(),operation_id}).map(|result|{assert_eq!(result,recorded_operation(operation_id,"create_disk_directory"));json!({"returned_by":"create_disk_directory"})})}),
            ("create_disk_write_session", {backend.create_disk_write_session(identity,crate::CreateDiskWriteSession {slot_id:"P01".into(),path:name_policy::PortablePath::parse("Café/file.txt").unwrap(),branch_id:None}).map(|session|{assert_eq!(session.workspace.workspace_id,"disk-workspace");assert_eq!(session.relative_path,"file.txt");assert_eq!(session.display_path,"Café/file.txt");json!({"returned_by":"create_disk_write_session"})})}),
            ("create_disk_write_session", {backend.create_disk_write_session(identity,crate::CreateDiskWriteSession {slot_id:"P01".into(),path:name_policy::PortablePath::parse("Café/file.txt").unwrap(),branch_id:Some("br_00000000000000000000000000000000".to_owned().try_into().unwrap())}).map(|session|{assert_eq!(session.workspace.workspace_id,"disk-workspace");assert_eq!(session.relative_path,"file.txt");assert_eq!(session.display_path,"Café/file.txt");json!({"returned_by":"create_disk_write_session"})})}),
            ("backup_configuration", {
                backend.backup_configuration(identity, crate::ReadBackupConfiguration {repository_id:"repository".into(),path:"docs".into()}).map(|result| {assert_eq!(result,recorded_backup_configuration());json!({"returned_by":"backup_configuration"})})
            }),
            ("configure_backups", {
                backend.configure_backups(identity,crate::ConfigureBackups {repository_id:"repository".into(),path:"docs".into(),target_copies:2,backup_prefixes:BTreeMap::from([("B01".into(),"backup".into())]),expected_generation:7,expected_scope_id:"scope".into(),operation_id}).map(|result| {assert_eq!(result,recorded_operation(operation_id,"configure_backups"));json!({"returned_by":"configure_backups"})})
            }),
            ("plan", backend.plan(identity,"plan").map(|plan| { assert_eq!(plan,recorded_plan("plan")); json!({"returned_by":"plan"}) })),
            ("cancel_plan", backend.cancel_plan(identity,Ok(crate::CancelPlan {plan_id:"plan".into(),operation_id:Ok(operation_id)})).map(|operation| { assert_eq!(operation,recorded_operation(operation_id,"cancel_plan")); json!({"returned_by":"cancel_plan"}) })),
            ("register_disk", backend.register_disk(identity).map(|never| match never {})),
            ("commit_plan", backend.commit_plan(identity,Ok(crate::CommitPlan {plan_id:"plan".into(),plan_digest:"digest".into(),operation_id:Ok(operation_id)})).map(|operation| { assert_eq!(operation,recorded_operation(operation_id,"commit_plan")); json!({"returned_by":"commit_plan"}) })),
            ("prepare_historical_save", {backend.prepare_historical_save(identity,Ok(crate::PrepareHistoricalSave {history_tree_id:"history".into(),options:Ok(storage_contract::HistoricalSaveOptions {source_commit_id:"commit".into(),name:"Saved branch".into(),workspace_id:None,primary_slot_id:None,target_copies:None})})).map(|plan|{assert_eq!(plan.plan_id,"plan");assert_eq!(plan.expected_sequence,7);json!({"returned_by":"prepare_historical_save"})})}),
            ("prepare_purge", {backend.prepare_purge(identity,Ok(crate::PreparePurge {history_tree_id:"history".into(),version_ids:Ok(vec!["second".into(),"first".into(),"second".into()])})).map(|plan|{assert_eq!(plan.plan_id,"purge-plan");assert_eq!(plan.expected_sequence,7);json!({"returned_by":"prepare_purge"})})}),
            ("prepare_split", {backend.prepare_split(identity,Ok(crate::PrepareSplit {history_tree_id:"history".into(),cut_commit_id:Ok("cut".into())})).map(|plan|{assert_eq!(plan.plan_id,"split-plan");assert_eq!(plan.expected_sequence,7);json!({"returned_by":"prepare_split"})})}),
            ("namespace_batch", {
                backend.namespace_batch(identity,crate::NamespaceBatch {slot_id:"P01".into(),request:storage_contract::DirectoryNamespaceBatch {kind:storage_contract::DirectoryBatchKind::Delete,items:vec![]},operation_id}).map(|result| {assert_eq!(result,recorded_operation(operation_id,"namespace_batch"));json!({"returned_by":"namespace_batch"})})
            }),
            ("select_file_branch", {
                backend.select_file_branch(identity,crate::SelectFileBranch {file_id:"file".into(),branch_id:"branch".into(),location:None,operation_id}).map(|result| {assert_eq!(result,recorded_operation(operation_id,"select_file_branch"));json!({"returned_by":"select_file_branch"})})
            }),
            ("select_file_branch", {
                backend.select_file_branch(identity,crate::SelectFileBranch {file_id:"file".into(),branch_id:"branch".into(),location:Some(crate::FileSelectionLocation {repository_id:"repository".into(),path:name_policy::PortablePath::parse("Café/file.txt").unwrap()}),operation_id}).map(|result| {assert_eq!(result,recorded_operation(operation_id,"select_file_branch"));json!({"returned_by":"select_file_branch"})})
            }),
            ("rename_file_branch", {
                backend.rename_file_branch(identity,crate::RenameFileBranch {file_id:"file".into(),branch_id:"branch".into(),name:storage_contract::BranchName::parse("Café").unwrap(),operation_id}).map(|result| {assert_eq!(result,recorded_operation(operation_id,"rename_file_branch"));json!({"returned_by":"rename_file_branch"})})
            }),
            ("mutate_disk_directory", {
                backend.mutate_disk_directory(identity,crate::MutateDiskDirectory {slot_id:"P01".into(),path:Some(name_policy::LookupPath::parse("Café").unwrap()),name:storage_contract::BranchName::parse("Side").unwrap(),operation_id,action:crate::DiskDirectoryAction::Create}).map(|result| {assert_eq!(result,recorded_operation(operation_id,"mutate_disk_directory"));json!({"returned_by":"mutate_disk_directory"})})
            }),
            ("mutate_disk_directory", {
                backend.mutate_disk_directory(identity,crate::MutateDiskDirectory {slot_id:"P01".into(),path:Some(name_policy::LookupPath::parse("Café").unwrap()),name:storage_contract::BranchName::parse("Side").unwrap(),operation_id,action:crate::DiskDirectoryAction::Select}).map(|result| {assert_eq!(result,recorded_operation(operation_id,"mutate_disk_directory"));json!({"returned_by":"mutate_disk_directory"})})
            }),
            ("initialize_directory_scope", {
                backend.initialize_directory_scope(identity,crate::InitializeDirectoryScope {repository_id:"repository".into(),path:Some(name_policy::LookupPath::parse("Café").unwrap()),operation_id}).map(|result| {assert_eq!(result,recorded_operation(operation_id,"initialize_directory_scope"));json!({"returned_by":"initialize_directory_scope"})})
            }),
            ("archive_branch", {
                let request = crate::ArchiveBranch {branch_id:"branch".into(),operation_id};
                backend.archive_branch(identity,request).map(|result| {
                    assert_eq!(result,recorded_operation(operation_id,"archive_branch"));
                    json!({"returned_by":"archive_branch"})
                })
            }),
            ("rename_directory_branch", {
                let request = crate::RenameDirectoryBranch {
                    repository_id: "repository".into(),
                    directory_branch_id: "directory".into(),
                    name: storage_contract::BranchName::parse("Café").unwrap(),
                    operation_id,
                };
                backend.rename_directory_branch(identity, request).map(|result| {
                    assert_eq!(result, recorded_operation(operation_id, "rename_directory_branch"));
                    json!({"returned_by":"rename_directory_branch"})
                })
            }),
            (
                "select_default_directory_branch",
                backend
                    .select_default_directory_branch(
                        identity,
                        crate::SelectDefaultDirectoryBranch {
                            repository_id: "repository".into(),
                            directory_branch_id: "directory".into(),
                            operation_id,
                        },
                    )
                    .map(|result| {
                        assert_eq!(
                            result,
                            recorded_operation(operation_id, "select_default_directory_branch")
                        );
                        json!({"returned_by":"select_default_directory_branch"})
                    }),
            ),
            ("directory_selection", {
                let request = crate::ReadDirectorySelection::parse(
                    "repository".into(),
                    "Folder",
                    vec![crate::DirectorySelectionSourceInput {
                        directory_branch_id: "directory".into(),
                        expected_tree_id: "tree".into(),
                    }],
                    vec![crate::DirectorySelectionEntryInput {
                        entry_id: "entry".into(),
                        path: "Folder/File.txt".into(),
                    }],
                ).unwrap();
                backend.directory_selection(identity, request, &crate::tests::history_read_control()).map(|snapshot| {
                    assert_eq!(snapshot.repository_id, "repository");
                    assert!(snapshot.branches.is_empty());
                    json!({"returned_by":"directory_selection"})
                })
            }),
            ("listing", backend.listing(identity, crate::ReadListing::parse("P01".into(), [("path".into(), "Folder".into())].into()).unwrap(), &crate::tests::history_read_control()).map(|snapshot| { assert_eq!(snapshot.path, "Folder"); json!({"returned_by":"listing"}) })),
            ("directory_scopes",backend.directory_scopes(identity,crate::ReadDirectoryScopes {repository_id:"repository",filter:&crate::DirectoryBranchFilter::Directory(name_policy::LookupPath::parse("Folder").unwrap()),projection:crate::DirectoryBranchProjection::Summary}).map(|snapshot|{assert_eq!(snapshot.repository_id,"repository");assert!(matches!(snapshot.branches,crate::DirectoryScopeRows::Summary(rows) if rows.is_empty()));json!({"returned_by":"directory_scopes"})})),
            (
                "default_directory_branch",
                backend
                    .default_directory_branch(identity, "repository")
                    .map(|snapshot| {
                        assert_eq!(snapshot.repository_id, "repository");
                        assert_eq!(snapshot.namespace_branch_id, "namespace");
                        assert_eq!(snapshot.selected.unwrap().name, "selected");
                        assert!(snapshot.directory_branches.is_empty());
                        json!({"returned_by":"default_directory_branch"})
                    }),
            ),
            (
                "file_history_snapshot",
                backend
                    .file_history_snapshot(
                        identity,
                        crate::ReadFileHistory::parse(
                            "file".into(),
                            BTreeMap::from([("repository_id".to_owned(),"repository".to_owned()),("path".to_owned(),"Café/report.txt".to_owned())]),
                        ).unwrap(),
                    )
                    .map(|page| {
                        assert_eq!(page.file_id, "file");
                        assert!(page.context.path.is_none());
                        assert_eq!(page.history_root, recorded_root());
                        json!({"returned_by":"file_history_snapshot"})
                    }),
            ),
            ("file_history_node_parents", backend.file_history_node_parents(identity, crate::ReadNodeParents::parse("file".into(), "node".into(), rooted_query()).unwrap()).map(|page| { assert_eq!(page.parents, ["parent"]); json!({"returned_by":"file_history_node_parents"}) })),
            ("file_history_branches", backend.file_history_branches(identity, crate::ReadHistoryBranches::parse("file".into(), rooted_query()).unwrap()).map(|page| { assert!(page.branches.is_empty()); json!({"returned_by":"file_history_branches"}) })),
            ("file_history_merges", backend.file_history_merges(identity, crate::ReadHistoryMerges::parse("file".into(), rooted_query()).unwrap()).map(|page| { assert!(page.merges.is_empty()); json!({"returned_by":"file_history_merges"}) })),
            ("file_history_merge_inputs", backend.file_history_merge_inputs(identity, crate::ReadMergeInputs(crate::backend_file_history::ReadIndexedSlice::parse("file".into(), "merge_id", "merge".into(), rooted_query()).unwrap())).map(|page| { assert_eq!(page.total, 1); json!({"returned_by":"file_history_merge_inputs"}) })),
            ("file_history_merge_parents", backend.file_history_merge_parents(identity, crate::ReadMergeParents(crate::backend_file_history::ReadIndexedSlice::parse("file".into(), "merge_id", "merge".into(), rooted_query()).unwrap())).map(|page| { assert_eq!(page.items, ["parent"]); json!({"returned_by":"file_history_merge_parents"}) })),
            ("file_history_node_field", backend.file_history_node_field(identity, crate::ReadNodeField::parse("file".into(), "node".into(), "message".into(), rooted_query()).unwrap()).map(|slice| { assert_eq!(slice.text, "text"); json!({"returned_by":"file_history_node_field"}) })),
            (
                "file_version_page",
                backend.file_version_page(identity, crate::ReadVersionPage::parse("file".into(), BTreeMap::new()).unwrap()).map(|page| {
                    assert_eq!(page.versions[0].logical_size, u64::MAX);
                    assert_eq!(page.versions[0].file_id, "file");
                    json!({"returned_by":"file_version_page"})
                }),
            ),
            ("file_version_sources", backend.file_version_sources(identity, crate::ReadVersionSources(crate::backend_file_history::ReadIndexedSlice::parse("file".into(), "version_id", "version".into(), rooted_query()).unwrap())).map(|page| { assert_eq!(page.items, ["source"]); json!({"returned_by":"file_version_sources"}) })),
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
                "activity_history",
                backend
                    .activity_history(
                        identity,
                        &crate::HistoryReadControl::until(
                            std::time::Instant::now() + std::time::Duration::from_secs(60),
                        ),
                    )
                    .map(|history| {
                        assert!(history.events.is_empty());
                        json!({"returned_by":"activity_history"})
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
                backend
                    .history_graph(
                        identity,
                        crate::ReadHistoryGraph::page(
                            "history".into(),
                            std::collections::BTreeMap::from([
                                ("collection".into(), "commits".into()),
                            ]),
                        )
                        .unwrap(),
                        &crate::HistoryReadControl::until(
                            std::time::Instant::now() + std::time::Duration::from_secs(60),
                        ),
                    )
                    .map(|response| {
                        let crate::HistoryGraphResponse::Graph(graph) = response else {
                            panic!("expected graph page")
                        };
                        assert_eq!(graph.history.generation, u64::MAX);
                        let storage_contract::history_graph::GraphItems::Commits(commits) = graph.items else {
                            panic!("expected commits")
                        };
                        assert_eq!(commits[0].commit_id, "commit");
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
                        &crate::tests::history_read_control(),
                    )
                    .map(|snapshot| {
                        assert_eq!(
                            crate::tests::tree_value(&snapshot),
                            crate::tests::tree_value(&recorded_tree())
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
        if *method == "register_disk" {
            let error = result.as_ref().unwrap_err();
            assert_eq!(error.status, 409);
            assert_eq!(
                error.code,
                storage_contract::ErrorCode::OfflineConfigurationRequired
            );
            assert_eq!(error.message, "offline registration");
            continue;
        }
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
        json!({"history_tree_id":"history","source_branch_id":"source_branch","source_commit_id":"source_commit","target_branch_id":"target_branch","target_commit_id":"target_commit","operation_id":"op_00000000000000000000000000000000"}),
        json!({"name":"Café","key":"café","primary_slot_id":"P01","target_copies":3,"operation_id":"op_00000000000000000000000000000000"}),
        json!({"repository_id":"repository","source_commit_id":"source","name":"Café","key":"café","operation_id":"op_00000000000000000000000000000000"}),
        json!({"file_id":"file","source_version_id":"version","source_node_id":null,"name":"Café","key":"café","operation_id":"op_00000000000000000000000000000000"}),
        json!({"file_id":"file","source_version_id":"version","source_node_id":"node","name":"Café","key":"café","operation_id":"op_00000000000000000000000000000000"}),
        json!({"target":"File(\"file\")","note":"","operation_id":"op_00000000000000000000000000000000"}),
        json!({"target":"Version(\"version\")","note":"","operation_id":"op_00000000000000000000000000000000"}),
        json!({"target":"Node { file: \"file\", node: \"node\" }","note":"","operation_id":"op_00000000000000000000000000000000"}),
        json!({"file_id":"file","inputs":[{"branch_id":"branch","expected_node_id":"head"}],"chosen_node_id":"chosen","surviving_branch_id":"branch","confirmed":true,"location":null,"operation_id":"op_00000000000000000000000000000000"}),
        json!({"file_id":"file","inputs":[{"branch_id":"branch","expected_node_id":"head"}],"chosen_node_id":"chosen","surviving_branch_id":"branch","confirmed":true,"location":{"repository_id":"repo","path":"file.txt"},"operation_id":"op_00000000000000000000000000000000"}),
        json!({"file_id":"file","targets":[{"branch_id":"branch","expected_node_id":"head","source_node_id":"source"}],"add":["added"],"remove":["removed"],"location":null,"operation_id":"op_00000000000000000000000000000000"}),
        json!({"file_id":"file","targets":[{"branch_id":"branch","expected_node_id":"head","source_node_id":"source"}],"add":["added"],"remove":["removed"],"location":{"repository_id":"repo","path":"file.txt"},"operation_id":"op_00000000000000000000000000000000"}),
        json!({"slot_id":"P01","path":"Café/folder","key":"café/folder","operation_id":"op_00000000000000000000000000000000"}),
        json!({"slot_id":"P01","path":"Café/file.txt","key":"café/file.txt","branch_id":null}),
        json!({"slot_id":"P01","path":"Café/file.txt","key":"café/file.txt","branch_id":"br_00000000000000000000000000000000"}),
        json!({"repository_id":"repository","path":"docs"}),
        json!({"repository_id":"repository","path":"docs","target_copies":2,"backup_prefixes":{"B01":"backup"},"expected_generation":7,"expected_scope_id":"scope","operation_id":"op_00000000000000000000000000000000"}),
        json!({"plan_id":"plan"}),
        json!({"plan_id":"plan","operation_id":"op_00000000000000000000000000000000"}),
        Value::Null,
        json!({"plan_id":"plan","plan_digest":"digest","operation_id":"op_00000000000000000000000000000000"}),
        json!({"history_tree_id":"history","options":{"source_commit_id":"commit","name":"Saved branch","workspace_id":null,"primary_slot_id":null,"target_copies":null}}),
        json!({"history_tree_id":"history","version_ids":["second","first","second"]}),
        json!({"history_tree_id":"history","cut_commit_id":"cut"}),
        json!({"slot_id":"P01","request":{"kind":"delete","items":[]},"operation_id":"op_00000000000000000000000000000000"}),
        json!({"file_id":"file","branch_id":"branch","location":null,"operation_id":"op_00000000000000000000000000000000"}),
        json!({"file_id":"file","branch_id":"branch","location":{"repository_id":"repository","path":"Café/file.txt","key":"café/file.txt"},"operation_id":"op_00000000000000000000000000000000"}),
        json!({"file_id":"file","branch_id":"branch","name":"Café","key":"café","operation_id":"op_00000000000000000000000000000000"}),
        json!({"slot_id":"P01","path":"Café","name":"Side","action":"Create","operation_id":"op_00000000000000000000000000000000"}),
        json!({"slot_id":"P01","path":"Café","name":"Side","action":"Select","operation_id":"op_00000000000000000000000000000000"}),
        json!({"repository_id":"repository","path":"Café","operation_id":"op_00000000000000000000000000000000"}),
        json!({"branch_id":"branch","operation_id":"op_00000000000000000000000000000000"}),
        json!({"repository_id":"repository","directory_branch_id":"directory","name":"Café","key":"café","operation_id":"op_00000000000000000000000000000000"}),
        json!({"repository_id":"repository","directory_branch_id":"directory","operation_id":"op_00000000000000000000000000000000"}),
        json!({"repository_id":"repository","key":"folder","sources":1,"entries":1}),
        json!({"slot_id":"P01","path":"Folder","limit":200,"query":"","cursor":false,"selected":null}),
        json!({"repository_id":"repository","key":"folder","summary":true}),
        json!({"repository_id":"repository"}),
        json!({"file_id":"file","limit":50,"location":{"repository_id":"repository","path":"Café/report.txt","key":"café/report.txt"}}),
        json!({"file_id":"file","node_id":"node","limit":100,"root":"abababababababababababababababababababababababababababababababab"}),
        json!({"file_id":"file","limit":100,"root":"abababababababababababababababababababababababababababababababab"}),
        json!({"file_id":"file","limit":50,"root":"abababababababababababababababababababababababababababababababab"}),
        json!({"file_id":"file","merge_id":"merge","offset":0,"limit":200,"root":"abababababababababababababababababababababababababababababababab"}),
        json!({"file_id":"file","merge_id":"merge","offset":0,"limit":200,"root":"abababababababababababababababababababababababababababababababab"}),
        json!({"file_id":"file","node_id":"node","field":"message","offset":0,"max_bytes":65536,"root":"abababababababababababababababababababababababababababababababab"}),
        json!({"file_id":"file","limit":100,"root":null}),
        json!({"file_id":"file","version_id":"version","offset":0,"limit":200,"root":"abababababababababababababababababababababababababababababababab"}),
        Value::Null,
        json!({"version_id":"version"}),
        json!({}),
        json!({"after":"17"}),
        json!({"history_tree_id":"history","collection":"commits","after":null,"limit":50}),
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
    assert_eq!(
        fixture.backend.ready().unwrap(),
        crate::Readiness::Managed {
            history_index: crate::HistoryIndexStatus::Ready,
        }
    );
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
        cold_records: 0,
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
    crate::TreeSnapshot::encode(
        "branch",
        "commit",
        std::iter::empty(),
        &std::collections::BTreeMap::from([("file".into(), "version".into())]),
    )
    .unwrap()
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
        namespace: crate::fixture_commit_namespace(0),
        namespace_deletions: None,
        pending_file_versions: Default::default(),
        provenance: None,
    })
}

fn recorded_graph(id: &str) -> storage_contract::history_graph::GraphPage {
    use storage_contract::history_graph::{
        CommitSummary, GraphItems, GraphPage, HistorySummary, TextSummary,
    };
    GraphPage {
        history: HistorySummary {
            history_tree_id: id.into(),
            repository_id: "repository".into(),
            root_commit_id: "commit".into(),
            main_branch_id: Some("branch".into()),
            generation: u64::MAX,
            state: storage_contract::HistoryState::Active,
            provenance: None,
        },
        history_root: "ab".repeat(32),
        items: GraphItems::Commits(vec![CommitSummary {
            commit_id: "commit".into(),
            history_tree_id: id.into(),
            kind: TextSummary::Inline {
                text: "snapshot".into(),
            },
            parent_commit_ids: vec![],
            tree_id: TextSummary::Inline {
                text: "tree".into(),
            },
            sequence: 1,
            operation_id: storage_contract::OperationId::generate(),
            author: TextSummary::Inline {
                text: "author".into(),
            },
            message: TextSummary::Inline {
                text: "snapshot fixture".into(),
            },
            namespace: crate::fixture_commit_namespace(0),
            namespace_deletions: None,
            pending_file_versions: 0,
            provenance: None,
        }]),
        next_cursor: None,
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

fn recorded_merge_link(operation_id: storage_contract::OperationId) -> storage_contract::MergeLink {
    storage_contract::MergeLink {
        merge_link_id: "link".into(),
        history_tree_id: "history".into(),
        source_branch_id: "source_branch".into(),
        source_commit_id: "source_commit".into(),
        target_branch_id: "target_branch".into(),
        target_commit_id: "target_commit".into(),
        author: "author".into(),
        operation_id,
    }
}

struct HistoryGate {
    started: tokio::sync::oneshot::Sender<()>,
    release: std::sync::mpsc::Receiver<()>,
    finished: tokio::sync::oneshot::Sender<Result<(), storage_contract::ErrorCode>>,
}

fn history_route_fixture(
    fixture: &Fixture,
) -> (
    Router,
    Arc<tokio::sync::Semaphore>,
    Arc<tokio::sync::Semaphore>,
) {
    let backend: Arc<dyn Backend> = Arc::new(AuthorizedBackend {
        backend: fixture.probe.clone(),
        auth: crate::lock(&fixture.auth).reader(),
    });
    let config = ServerConfig::loopback(fixture._root.path())
        .admit()
        .unwrap();
    let history_read_slots = Arc::new(tokio::sync::Semaphore::new(2));
    let backend_slots = Arc::new(tokio::sync::Semaphore::new(HTTP_BACKEND_SLOTS));
    let state = AppState {
        auth_reader: crate::lock(&fixture.auth).reader(),
        spool_budget: http_spool::HttpSpool::open_blocking(&config, backend.as_ref()),
        telemetry: telemetry::Telemetry::start(
            backend.clone(),
            &fixture._root.path().join("graph-telemetry"),
        )
        .unwrap(),
        backend,
        auth: Arc::clone(&fixture.auth),
        config: Arc::clone(&config),
        login_admission: Arc::new(http_auth::LoginAdmission::default()),
        password_attempts: Arc::new(Mutex::new(HashMap::new())),
        #[cfg(feature = "core")]
        setup_attempts: Arc::new(Mutex::new(HashMap::new())),
        #[cfg(feature = "core")]
        recovery_attempts: Arc::new(Mutex::new(HashMap::new())),
        password_slots: Arc::new(tokio::sync::Semaphore::new(4)),
        authorization_slots: Arc::new(tokio::sync::Semaphore::new(8)),
        backend_slots: Arc::clone(&backend_slots),
        history_read_slots: Arc::clone(&history_read_slots),
        management_slots: Arc::new(tokio::sync::Semaphore::new(2)),
        upload_slots: transfer_limits::Admission::new(&config.http_transfer_limits),
        read_slots: transfer_limits::Admission::new(&config.http_transfer_limits),
        upload_claims: Arc::new(Mutex::new(Default::default())),
        static_etags: Arc::new(BTreeMap::new()),
        web_assets_available: false,
    };
    let app = Router::new()
        .route(
            "/api/history-trees/{history}/graph",
            get(crate::http_graph::graph),
        )
        .route(
            "/api/repositories/{repo}/directory-branches",
            post(crate::http_directory_selection::read),
        )
        .merge(crate::file_manager::routes())
        .layer(http_routing::Extension(fixture.identity.clone()))
        .with_state(state);
    (app, history_read_slots, backend_slots)
}

#[tokio::test]
async fn activity_and_graph_share_capacity_until_cancelled_worker_checkpoint() {
    cancelled_history_route_keeps_capacity(activity_request()).await;
}

#[tokio::test]
async fn graph_route_cancellation_retains_capacity_until_worker_checkpoint() {
    cancelled_history_route_keeps_capacity(graph_request()).await;
}

#[tokio::test]
async fn listing_route_cancellation_retains_shared_capacity_until_worker_checkpoint() {
    cancelled_history_route_keeps_capacity(listing_request()).await;
}

#[tokio::test]
async fn selected_facts_route_cancellation_retains_shared_capacity_until_worker_checkpoint() {
    cancelled_history_route_keeps_capacity(selection_request()).await;
}

#[tokio::test]
async fn selected_facts_response_retains_backend_permit_until_collected_bytes_drop() {
    use service_stack::ServiceExt;
    let fixture = Fixture::new();
    let (app, scans, backends) = history_route_fixture(&fixture);
    let response = app.oneshot(selection_request()).await.unwrap();
    assert_eq!(response.status(), ::http::StatusCode::OK);
    assert_eq!(scans.available_permits(), 2);
    assert_eq!(backends.available_permits(), HTTP_BACKEND_SLOTS - 1);
    let bytes = http_routing::body::to_bytes(
        response.into_body(),
        storage_contract::client_protocol::MAX_RESPONSE_BYTES,
    )
    .await
    .unwrap();
    assert_eq!(
        json_codec::from_slice::<Value>(&bytes).unwrap(),
        json!({"repository_id":"repository","directory_branches":[]})
    );
    assert_eq!(backends.available_permits(), HTTP_BACKEND_SLOTS - 1);
    drop(bytes);
    assert_eq!(backends.available_permits(), HTTP_BACKEND_SLOTS);
}

#[tokio::test]
async fn selected_facts_complete_response_uses_eight_mib_not_history_page_cap() {
    use service_stack::ServiceExt;
    use storage_contract::client_protocol::{MAX_HISTORY_PAGE_BYTES, MAX_RESPONSE_BYTES};
    let fixture = Fixture::new();
    let (app, scans, backends) = history_route_fixture(&fixture);
    // The backend fixture isolates the route's complete-response encoder.
    let shell = json_codec::to_vec(&json!({"repository_id":"","directory_branches":[]}))
        .unwrap()
        .len();
    let payload = MAX_RESPONSE_BYTES - shell;
    *fixture.probe.selection_repository.lock().unwrap() = Some("x".repeat(payload));
    let response = app.clone().oneshot(selection_request()).await.unwrap();
    assert_eq!(response.status(), ::http::StatusCode::OK);
    let bytes = http_routing::body::to_bytes(response.into_body(), MAX_RESPONSE_BYTES)
        .await
        .unwrap();
    assert!(bytes.len() > MAX_HISTORY_PAGE_BYTES);
    assert_eq!(bytes.len(), MAX_RESPONSE_BYTES);
    drop(bytes);
    *fixture.probe.selection_repository.lock().unwrap() = Some("x".repeat(payload + 1));
    let response = app.oneshot(selection_request()).await.unwrap();
    assert_eq!(response.status(), ::http::StatusCode::TOO_MANY_REQUESTS);
    let bytes = http_routing::body::to_bytes(response.into_body(), MAX_RESPONSE_BYTES)
        .await
        .unwrap();
    let error: Value = json_codec::from_slice(&bytes).unwrap();
    assert_eq!(error["error"]["code"], "RESOURCE_LIMIT_EXCEEDED");
    assert!(error.get("directory_branches").is_none());
    drop(bytes);
    assert_eq!(scans.available_permits(), 2);
    assert_eq!(backends.available_permits(), HTTP_BACKEND_SLOTS);
}

#[tokio::test]
async fn listing_response_retains_backend_permit_until_collected_bytes_drop() {
    use service_stack::ServiceExt;
    let fixture = Fixture::new();
    let (app, scans, backends) = history_route_fixture(&fixture);
    let response = app.oneshot(listing_request()).await.unwrap();
    assert_eq!(response.status(), ::http::StatusCode::OK);
    assert_eq!(scans.available_permits(), 2);
    assert_eq!(backends.available_permits(), HTTP_BACKEND_SLOTS - 1);
    let bytes = http_routing::body::to_bytes(
        response.into_body(),
        storage_contract::client_protocol::MAX_RESPONSE_BYTES,
    )
    .await
    .unwrap();
    let value: Value = json_codec::from_slice(&bytes).unwrap();
    assert_eq!(value["slot_id"], "P01");
    assert_eq!(value["entries"], json!([]));
    assert_eq!(backends.available_permits(), HTTP_BACKEND_SLOTS - 1);
    drop(bytes);
    assert_eq!(backends.available_permits(), HTTP_BACKEND_SLOTS);
}

async fn cancelled_history_route_keeps_capacity(
    first_request: http_routing::http::Request<http_routing::body::Body>,
) {
    use service_stack::ServiceExt;
    use std::time::Duration;
    let fixture = Fixture::new();
    let (app, history_read_slots, backend_slots) = history_route_fixture(&fixture);
    let mut requests = Vec::new();
    let mut releases = Vec::new();
    let mut finishes = Vec::new();
    for request in [first_request, graph_request()] {
        let (started, start) = tokio::sync::oneshot::channel();
        let (release, released) = std::sync::mpsc::channel();
        let (finished, finish) = tokio::sync::oneshot::channel();
        fixture
            .probe
            .history_gates
            .lock()
            .unwrap()
            .push_back(HistoryGate {
                started,
                release: released,
                finished,
            });
        let request = app.clone().oneshot(request);
        requests.push(tokio::spawn(async move { request.await.unwrap() }));
        releases.push(release);
        finishes.push(finish);
        tokio::time::timeout(Duration::from_secs(5), start)
            .await
            .unwrap()
            .unwrap();
    }
    assert_eq!(history_read_slots.available_permits(), 0);
    assert_eq!(backend_slots.available_permits(), HTTP_BACKEND_SLOTS - 2);
    let cancelled = requests.remove(0);
    cancelled.abort();
    assert!(cancelled.await.unwrap_err().is_cancelled());
    let overloaded = app.clone().oneshot(graph_request()).await.unwrap();
    assert_eq!(overloaded.status(), ::http::StatusCode::TOO_MANY_REQUESTS);
    let overloaded = app.clone().oneshot(activity_request()).await.unwrap();
    assert_eq!(overloaded.status(), ::http::StatusCode::TOO_MANY_REQUESTS);
    let overloaded = app.clone().oneshot(listing_request()).await.unwrap();
    assert_eq!(overloaded.status(), ::http::StatusCode::TOO_MANY_REQUESTS);
    let overloaded = app.clone().oneshot(selection_request()).await.unwrap();
    assert_eq!(overloaded.status(), ::http::StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(history_read_slots.available_permits(), 0);
    assert_eq!(backend_slots.available_permits(), HTTP_BACKEND_SLOTS - 2);
    releases.remove(0).send(()).unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), finishes.remove(0))
            .await
            .unwrap()
            .unwrap(),
        Err(storage_contract::ErrorCode::RequestTimeout),
    );
    let released_scan = tokio::time::timeout(
        Duration::from_secs(5),
        history_read_slots.clone().acquire_owned(),
    )
    .await
    .unwrap()
    .unwrap();
    let released_backend = tokio::time::timeout(
        Duration::from_secs(5),
        backend_slots
            .clone()
            .acquire_many_owned((HTTP_BACKEND_SLOTS - 1) as u32),
    )
    .await
    .unwrap()
    .unwrap();
    drop((released_scan, released_backend));
    let resumed_activity = app.clone().oneshot(activity_request()).await.unwrap();
    assert_eq!(resumed_activity.status(), ::http::StatusCode::OK);
    let activity = http_routing::body::to_bytes(
        resumed_activity.into_body(),
        storage_contract::client_protocol::MAX_RESPONSE_BYTES,
    )
    .await
    .unwrap();
    assert_eq!(
        json_codec::from_slice::<Value>(&activity).unwrap(),
        json!({"events": [], "limit": 200}),
    );
    // A single collected chunk still owns its transport permit through Bytes.
    assert_eq!(backend_slots.available_permits(), HTTP_BACKEND_SLOTS - 2);
    drop(activity);
    assert_eq!(backend_slots.available_permits(), HTTP_BACKEND_SLOTS - 1);
    let resumed = app.clone().oneshot(graph_request()).await.unwrap();
    assert_eq!(resumed.status(), ::http::StatusCode::OK);
    let bytes = http_routing::body::to_bytes(resumed.into_body(), 4 * 1024 * 1024)
        .await
        .unwrap();
    let response: storage_contract::history_graph::GraphPage =
        json_codec::from_slice(&bytes).unwrap();
    drop(bytes);
    assert_eq!(response.history.history_tree_id, "history");
    releases.remove(0).send(()).unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), finishes.remove(0))
            .await
            .unwrap()
            .unwrap(),
        Ok(())
    );
    let response = tokio::time::timeout(Duration::from_secs(5), requests.remove(0))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.status(), ::http::StatusCode::OK);
    http_routing::body::to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .unwrap();
    let all_scans = history_read_slots
        .clone()
        .try_acquire_many_owned(2)
        .unwrap();
    let all_backends = backend_slots
        .clone()
        .try_acquire_many_owned(HTTP_BACKEND_SLOTS as u32)
        .unwrap();
    drop((all_scans, all_backends));
}

fn graph_request() -> http_routing::http::Request<http_routing::body::Body> {
    http_routing::http::Request::builder()
        .uri("/api/history-trees/history/graph?collection=commits")
        .body(http_routing::body::Body::empty())
        .unwrap()
}

fn activity_request() -> http_routing::http::Request<http_routing::body::Body> {
    http_routing::http::Request::builder()
        .uri("/api/file-manager/history?unused=unchanged")
        .body(http_routing::body::Body::empty())
        .unwrap()
}

fn listing_request() -> http_routing::http::Request<http_routing::body::Body> {
    http_routing::http::Request::builder()
        .uri("/api/file-manager/disks/P01/entries")
        .body(http_routing::body::Body::empty())
        .unwrap()
}

fn selection_request() -> http_routing::http::Request<http_routing::body::Body> {
    http_routing::http::Request::builder()
        .method("POST")
        .uri("/api/repositories/repository/directory-branches")
        .header(::http::header::CONTENT_TYPE, "application/json")
        .body(http_routing::body::Body::from(
            r#"{"path":"","sources":[],"entries":[]}"#,
        ))
        .unwrap()
}
