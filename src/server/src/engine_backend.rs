//! Authorization and wire adaptation for the storage engine.
use crate::{ApiError, Backend, Content, auth::Identity};
use json_codec::{Value, json};
use std::{path::Path, sync::Mutex};
use storage_engine::{Engine, NamespaceOperation, Repository};
mod admission;
use admission::AdmissionEngine;
pub mod application;
use application::Application;
mod content;
mod disk_monitor;
mod error_registry;
mod file_manager;
mod operations;
mod read_projection;
mod wire;
pub(crate) use wire::error;
use wire::{
    branch_wire, content_wire, current_entry_source, encode, field, keys, operation,
    operation_wire, pending_entry_source, sanitized,
};

pub struct EngineBackend {
    engine: Mutex<AdmissionEngine>,
    #[cfg(test)]
    commit_preparation_hook: Mutex<Option<std::sync::Arc<dyn Fn() + Send + Sync>>>,
    #[cfg(test)]
    operation_health_hook: Mutex<Option<std::sync::Arc<dyn Fn() + Send + Sync>>>,
}
impl EngineBackend {
    pub fn new(engine: Engine) -> Self {
        Self {
            engine: Mutex::new(AdmissionEngine::new(engine)),
            #[cfg(test)]
            commit_preparation_hook: Mutex::new(None),
            #[cfg(test)]
            operation_health_hook: Mutex::new(None),
        }
    }
    #[cfg(test)]
    pub(crate) fn test_with_engine_locked(&self, f: impl FnOnce()) {
        let _guard = self.engine.lock().unwrap();
        f();
    }
    /// Managed dispatch linearizes admission after acquiring the engine mutex.
    /// An operation already admitted may finish; queued requests see revocation.
    pub(crate) fn set_management_auth(
        &self,
        auth: std::sync::Weak<std::sync::Mutex<crate::auth::AuthStore>>,
    ) -> Result<(), ApiError> {
        self.engine
            .lock()
            .map_err(|_| ApiError::new(500, "INTERNAL_ERROR", "storage engine requires restart"))?
            .set_management_auth(auth);
        Ok(())
    }
    pub(crate) fn replicate_auth(&self, snapshot: Value) -> Result<(), ApiError> {
        let mut engine = self
            .engine
            .lock()
            .map_err(|_| ApiError::new(500, "INTERNAL_ERROR", "storage engine requires restart"))?;
        if engine.auth_state_ref() == Some(&snapshot) {
            return Ok(());
        }
        engine
            .persist_auth_state(snapshot, "system_auth", &storage_engine::new_id("op"))
            .map_err(error)
    }
    pub(crate) fn read_only(&self) -> Result<bool, ApiError> {
        Ok(self
            .engine
            .lock()
            .map_err(|_| ApiError::new(500, "INTERNAL_ERROR", "storage engine requires restart"))?
            .is_read_only())
    }
    pub fn protect_auth(
        self: &std::sync::Arc<Self>,
        mut auth: crate::auth::AuthStore,
    ) -> Result<crate::auth::AuthStore, ApiError> {
        let backend = self.clone();
        auth.attach_backup(std::sync::Arc::new(move |snapshot| {
            let mut engine = backend.engine.lock().map_err(|_| {
                ApiError::new(500, "INTERNAL_ERROR", "storage engine requires restart")
            })?;
            if engine.auth_state_ref() == Some(&snapshot) {
                return Ok(());
            }
            engine
                .persist_auth_state(snapshot, "system_auth", &storage_engine::new_id("op"))
                .map_err(error)
        }))?;
        Ok(auth)
    }
    pub fn open_auth(
        self: &std::sync::Arc<Self>,
        root: &Path,
    ) -> Result<crate::auth::AuthStore, ApiError> {
        let snapshot = self
            .engine
            .lock()
            .map_err(|_| ApiError::new(500, "INTERNAL_ERROR", "storage engine requires restart"))?
            .auth_state();
        self.protect_auth(crate::auth::AuthStore::open_with_snapshot(root, snapshot)?)
    }
}
/// Unit fixtures install real singleton credentials rather than bypassing admission.
#[cfg(test)]
fn install_test_administrator(engine: &mut Engine, username: &str) -> Identity {
    let root = crate::tests::private_tempdir();
    let auth =
        crate::auth::AuthStore::initialize(root.path(), username, "integration-test-password")
            .unwrap();
    let identity = auth.sole_identity().unwrap();
    engine
        .persist_auth_state(
            auth.snapshot().unwrap(),
            "system_auth",
            &storage_engine::new_id("op"),
        )
        .unwrap();
    identity
}

fn repository<'a>(engine: &'a Engine, id: &str) -> Result<&'a Repository, ApiError> {
    repository_in_state(engine.state(), id)
}
fn repository_in_state<'a>(
    state: &'a storage_contract::State,
    id: &str,
) -> Result<&'a Repository, ApiError> {
    state
        .repositories
        .get(id)
        .ok_or_else(|| ApiError::new(404, "NOT_FOUND", "repository not found"))
}
/// Publications append increasing sequences; snapshot recovery preserves their
/// vector order. Seek directly to the unseen suffix for resumed event polls.
fn events_after(events: &[storage_contract::Event], after: u64) -> &[storage_contract::Event] {
    &events[events.partition_point(|event| event.sequence <= after)..]
}
fn repo_for_history<'a>(engine: &'a Engine, id: &str) -> Result<&'a Repository, ApiError> {
    repo_for_history_in_state(engine.state(), id)
}
fn repo_for_history_in_state<'a>(
    state: &'a storage_contract::State,
    id: &str,
) -> Result<&'a Repository, ApiError> {
    let history = state
        .histories
        .get(id)
        .ok_or_else(|| ApiError::new(404, "NOT_FOUND", "history not found"))?;
    repository_in_state(state, &history.repository_id)
}
fn repo_for_branch<'a>(engine: &'a Engine, id: &str) -> Result<&'a Repository, ApiError> {
    repo_for_branch_in_state(engine.state(), id)
}
fn repo_for_branch_in_state<'a>(
    state: &'a storage_contract::State,
    id: &str,
) -> Result<&'a Repository, ApiError> {
    let branch = state
        .branches
        .get(id)
        .ok_or_else(|| ApiError::new(404, "NOT_FOUND", "branch not found"))?;
    if branch.state == "migrated" {
        return Err(ApiError::new(
            409,
            "TREE_MOVED",
            "branch has moved to an independent history domain",
        ));
    }
    repo_for_history_in_state(state, &branch.history_tree_id)
}
fn workspace_authorize<'a>(
    engine: &'a Engine,
    identity: &Identity,
    id: &str,
) -> Result<&'a storage_engine::Workspace, ApiError> {
    let workspace = engine.workspace_ref(id).map_err(error)?;
    if workspace.principal != identity.principal {
        return Err(ApiError::forbidden());
    }
    repo_for_branch(engine, &workspace.branch_id)?;
    Ok(workspace)
}

/// Refresh administrator admission while holding the engine lock, before capturing a
/// read plan or publishing a mutation, and again after unlocked preparation.
/// Managed admission linearizes here: engine -> short primary-auth read.
/// Managed auth publication never holds its mutex across engine work. An
/// operation admitted before a revocation may finish; queued work revalidates.
fn current_protected_identity(
    engine: &AdmissionEngine,
    identity: &Identity,
) -> Result<Identity, ApiError> {
    let current = engine.identity(identity)?;
    if let Some(binding) = &current.smb_binding {
        let bound = engine.state().repositories.get(&binding.repository_id);
        if bound.is_none_or(|repo| {
            !repo.smb_enabled
                || repo.main_branch_id != binding.branch_id
                || repo.smb_binding_generation != binding.generation
        }) {
            return Err(ApiError::new(
                409,
                "SMB_REBIND_REQUIRED",
                "SMB share binding changed; reconnect before starting another operation",
            ));
        }
    }
    Ok(current)
}

fn finish_authorized_upload(
    engine: &mut AdmissionEngine,
    identity: &Identity,
    workspace: &str,
    prepared: storage_engine::PreparedUpload,
) -> Result<Value, ApiError> {
    let current = current_protected_identity(engine, identity)?;
    workspace_authorize(engine, &current, workspace)?;
    sanitized(engine.finish_upload(prepared).map_err(error)?)
}

impl Backend for EngineBackend {
    fn call(
        &self,
        action: crate::BackendAction,
        identity: &Identity,
        input: Value,
    ) -> Result<Value, ApiError> {
        let mut engine = self
            .engine
            .lock()
            .map_err(|_| ApiError::new(500, "INTERNAL_ERROR", "storage engine requires restart"))?;
        let current = if matches!(
            action,
            crate::BackendAction::Capabilities | crate::BackendAction::Ready
        ) {
            // These probes are intentionally available without an account.
            identity.clone()
        } else {
            current_protected_identity(&engine, identity)?
        };
        let identity = &current;
        match action {
            crate::BackendAction::WorkspaceCapacity => {
                keys(&input, &[])?;
                encode(engine.workspace_capacity(&identity.principal))
            }
            crate::BackendAction::FileManagerDisks => {
                let access = file_manager::disk_access(&engine);
                let observation = engine.prepare_disk_observation();
                drop(engine);
                let disks = observation.status().map_err(error)?.into_iter().filter_map(|disk| {
                    let writable = access.get(&disk.slot_id)?;
                    Some(json!({"slot_id":disk.slot_id,"level":disk.level,"online":disk.online,"writable":writable,"available_bytes":disk.available_bytes.to_string(),"used_bytes":disk.used_bytes.to_string(),"simulation":disk.simulation}))
                }).collect::<Vec<_>>();
                Ok(json!({"disks":disks}))
            }
            crate::BackendAction::FileManagerBackupConfiguration
            | crate::BackendAction::FileManagerDownloadContext
            | crate::BackendAction::FileManagerEntries
            | crate::BackendAction::FileManagerFileHistory
            | crate::BackendAction::FileManagerHistory => {
                file_manager::read(&engine, action, input)
            }
            crate::BackendAction::FileManagerBranch
            | crate::BackendAction::FileManagerConfigureBackups
            | crate::BackendAction::FileManagerDirectoryBranches
            | crate::BackendAction::FileManagerDirectorySelect
            | crate::BackendAction::FileManagerFileMetadata
            | crate::BackendAction::FileManagerMerge
            | crate::BackendAction::FileManagerNamespace
            | crate::BackendAction::FileManagerNamespaceBatch
            | crate::BackendAction::FileManagerNodeNote
            | crate::BackendAction::FileManagerRenameBranch
            | crate::BackendAction::FileManagerSelect
            | crate::BackendAction::FileManagerTags
            | crate::BackendAction::FileManagerVersionMetadata
            | crate::BackendAction::FileManagerWriteSession => {
                file_manager::call(&mut engine, identity, action, input)
            }
            crate::BackendAction::MonitorDisks => {
                let observation = engine.prepare_disk_observation();
                drop(engine);
                disk_monitor::observe(observation)
            }
            crate::BackendAction::Capabilities => Ok(
                json!({"name_policy":name_policy::NAME_POLICY,"path_policy":"PortablePath","unicode_data_version":name_policy::UNICODE_VERSION,"supported_save_modes":["full"],"full_only":true,"full_branches":false,"branch_creation":"record-until-write","branch_content":"inherit-parent-until-first-write","file_branches":true,"branch_scope":"file","directory_branches":true,"directory_branch_creation":"inherit-directory-tree","directory_subtrees":true,"namespace_batch":true,"smb_default_branch":"selected-root-directory","file_manager":true,"historical_save_creates_branch":true,"merge_behavior":"explicit-file-node-adoption","file_branch_merge":true,"directory_branch_merge":false,"directory_merge_status":"design-pending","file_node_tags":true,"backup_configuration":true,"range_upload":false,"complete_upload":true,"max_upload_bytes":engine.config().max_file_bytes.to_string(),"authentication":"session-bearer-or-cookie","platforms":{"web":true,"smb":true},"operations":["repositories","tree","workspace","commit","history","graph","branch","merge-link","file-branch-merge","file-node-tags","backup-configuration","purge-plan","split-plan","version-range-read","file-manager","file-branches","file-branch-rename","file-path-nodes","directory-branches","directory-subtrees","namespace-batch","default-branch","file-notes","system-status"]}),
            ),
            crate::BackendAction::Ready => {
                if engine.is_read_only() {
                    return Err(ApiError::new(
                        503,
                        "RECOVERY_REQUIRED",
                        "storage is open for verified recovery reads only",
                    ));
                }
                let configured_members = engine
                    .config()
                    .disks
                    .iter()
                    .filter(|disk| disk.metadata_member)
                    .count();
                let storage_disks = engine.config().disks.len();
                let mode = engine.config().mode.clone();
                let sequence = engine.state().sequence;
                let observation = engine.prepare_disk_observation();
                drop(engine);
                let status = observation.status().map_err(error)?;
                let members = status
                    .iter()
                    .filter(|disk| disk.metadata_member && disk.online)
                    .count();
                if members != configured_members {
                    return Err(ApiError::new(
                        503,
                        "METADATA_UNAVAILABLE",
                        "all configured metadata members are required for normal writes",
                    ));
                }
                Ok(
                    json!({"ready":true,"mode":mode,"sequence":sequence.to_string(),"storage_disk_count":storage_disks,"metadata_member_count":configured_members,"metadata_redundancy":if configured_members == 1 { "none" } else { "multiple-copies" }}),
                )
            }
            crate::BackendAction::Repositories => {
                let values = engine.state().repositories.values().collect::<Vec<_>>();
                Ok(json!({"repositories":values}))
            }
            crate::BackendAction::Repository => {
                let repo = repository(&engine, field(&input, "repository_id")?)?;
                encode(repo)
            }
            crate::BackendAction::Branches => {
                let repo = repository(&engine, field(&input, "repository_id")?)?;
                Ok(
                    json!({"branches":engine.state().branches.values().filter(|branch|branch.history_tree_id==repo.history_tree_id).map(|branch| branch_wire(&engine, branch)).collect::<Result<Vec<_>, _>>()?}),
                )
            }
            crate::BackendAction::Tree => {
                let projection = read_projection::Tree::capture(&engine, &input)?;
                read_projection::render_unlocked(engine, projection)
            }
            crate::BackendAction::SmbAuthorizeRead => {
                let authorization =
                    read_projection::SmbReadAuthorization::capture(engine.state(), &input)?;
                read_projection::render_unlocked(engine, authorization)
            }
            crate::BackendAction::SmbLookup => {
                let projection = read_projection::SmbLookup::capture(&engine, &input)?;
                read_projection::render_unlocked(engine, projection)
            }
            crate::BackendAction::SmbReadDirectory => {
                let projection = read_projection::SmbDirectory::capture(&engine, &input)?;
                read_projection::render_unlocked(engine, projection)
            }
            crate::BackendAction::SmbBinding => {
                let repo = repository(&engine, field(&input, "repository_id")?)?;
                let history = engine
                    .state()
                    .histories
                    .get(&repo.history_tree_id)
                    .ok_or_else(|| {
                        ApiError::new(500, "STATE_INVALID", "repository history is missing")
                    })?;
                Ok(
                    json!({"repository_id":repo.repository_id,"branch_id":repo.main_branch_id,"enabled":repo.smb_enabled,"history_generation":history.generation.to_string()}),
                )
            }
            crate::BackendAction::DefaultBranch => {
                let repo = repository(&engine, field(&input, "repository_id")?)?;
                let selected = engine.state().directory_selections.get(&repo.repository_id);
                let name = selected
                    .and_then(|id| engine.state().directory_branches.get(id))
                    .map(|branch| branch.name.as_str())
                    .unwrap_or("main");
                let branches = engine
                    .directory_branches(&repo.repository_id, "")
                    .map_err(error)?;
                Ok(
                    json!({"repository_id":repo.repository_id,"namespace_branch_id":repo.main_branch_id,
                    "directory_branch_id":selected,"name":name,"directory_branches":branches}),
                )
            }
            crate::BackendAction::DirectoryBranchSelection => {
                file_manager::query_directory_branch_scopes(&engine, &input)
            }
            crate::BackendAction::DirectoryBranchScopes => {
                let empty_query = json!({});
                let query = input.get("query").unwrap_or(&empty_query);
                keys(query, &["path", "summary"])?;
                let path = query
                    .get("path")
                    .map(|path| {
                        path.as_str()
                            .ok_or_else(|| ApiError::invalid("path must be a string"))
                    })
                    .transpose()?;
                match query.get("summary") {
                    Some(Value::String(summary)) if summary == "true" => {
                        file_manager::directory_branch_summaries(
                            &engine,
                            field(&input, "repository_id")?,
                            path,
                        )
                    }
                    None => file_manager::directory_branch_scopes(
                        &engine,
                        field(&input, "repository_id")?,
                        path,
                    ),
                    _ => Err(ApiError::invalid("summary must be true when supplied")),
                }
            }
            crate::BackendAction::InitializeDirectoryScope => {
                keys(&input, &["repository_id", "path", "operation_id"])?;
                let repo = field(&input, "repository_id")?;
                repository(&engine, repo)?;
                // The empty path explicitly identifies the repository root.
                let path = input
                    .get("path")
                    .and_then(Value::as_str)
                    .ok_or_else(|| ApiError::invalid("path must be a string"))?;
                if !path.is_empty() {
                    name_policy::validate_path(path).map_err(ApiError::invalid)?;
                }
                let result = engine
                    .initialize_directory_scope(repo, path, &identity.principal, operation(&input)?)
                    .map_err(error)?;
                operation_wire(result)
            }
            crate::BackendAction::UpdateDefaultBranch => {
                keys(
                    &input,
                    &["repository_id", "directory_branch_id", "operation_id"],
                )?;
                let repo = field(&input, "repository_id")?;
                repository(&engine, repo)?;
                let result = engine
                    .select_default_directory_branch(
                        repo,
                        field(&input, "directory_branch_id")?,
                        &identity.principal,
                        operation(&input)?,
                    )
                    .map_err(error)?;
                operation_wire(result)
            }
            crate::BackendAction::RenameDirectoryBranch => {
                keys(
                    &input,
                    &[
                        "repository_id",
                        "directory_branch_id",
                        "name",
                        "operation_id",
                    ],
                )?;
                let repo = field(&input, "repository_id")?;
                repository(&engine, repo)?;
                let result = engine
                    .rename_directory_branch(
                        repo,
                        field(&input, "directory_branch_id")?,
                        field(&input, "name")?,
                        &identity.principal,
                        operation(&input)?,
                    )
                    .map_err(error)?;
                operation_wire(result)
            }
            crate::BackendAction::UpdateSmbBinding => {
                keys(
                    &input,
                    &[
                        "repository_id",
                        "branch_id",
                        "enabled",
                        "expected_history_generation",
                        "operation_id",
                    ],
                )?;
                let repo = field(&input, "repository_id")?;
                repository(&engine, repo)?;
                let enabled = input
                    .get("enabled")
                    .and_then(Value::as_bool)
                    .ok_or_else(|| ApiError::invalid("enabled must be a boolean"))?;
                let generation = storage_contract::parse_u64_decimal(field(
                    &input,
                    "expected_history_generation",
                )?)
                .map_err(ApiError::invalid)?;
                let result = engine
                    .set_smb_binding(
                        repo,
                        field(&input, "branch_id")?,
                        enabled,
                        generation,
                        &identity.principal,
                        operation(&input)?,
                    )
                    .map_err(error)?;
                operation_wire(result)
            }
            crate::BackendAction::Resolve => {
                keys(&input, &["branch_id", "path", "commit_id"])?;
                let branch_id = field(&input, "branch_id")?;
                repo_for_branch(&engine, branch_id)?;
                let branch = engine.state().branches.get(branch_id).unwrap();
                let path = field(&input, "path")?;
                name_policy::validate_lookup_path(path).map_err(ApiError::invalid)?;
                let commit_id =
                    wire::optional_identity(&input, "commit_id")?.unwrap_or(&branch.head_commit_id);
                let commit = engine
                    .state()
                    .commits
                    .get(commit_id)
                    .filter(|commit| commit.history_tree_id == branch.history_tree_id)
                    .ok_or_else(|| ApiError::new(404, "NOT_FOUND", "commit not found"))?;
                let key = name_policy::path_key(path);
                let entries = engine
                    .list_tree_at_ref(branch_id, commit_id)
                    .map_err(error)?;
                let entry = entries
                    .iter()
                    .find(|entry| entry.lookup_key == key)
                    .ok_or_else(|| ApiError::new(404, "NOT_FOUND", "entry not found"))?;
                let mut response = json!({"branch_id":branch_id,"commit_id":commit_id,"entry":entry,"version":engine.state().versions.get(&entry.version_id)});
                if entry.kind == "file" {
                    response.as_object_mut().unwrap().extend(
                        content_wire(
                            &engine,
                            Some(&entry.version_id),
                            pending_entry_source(&engine, branch_id, commit, entry),
                        )
                        .as_object()
                        .unwrap()
                        .clone(),
                    );
                }
                Ok(response)
            }
            crate::BackendAction::Workspace => sanitized(workspace_authorize(
                &engine,
                identity,
                field(&input, "workspace_id")?,
            )?),
            crate::BackendAction::Plan => {
                let plan = engine.plan(field(&input, "plan_id")?).map_err(error)?;
                if plan.principal != identity.principal {
                    return Err(ApiError::forbidden());
                }
                repo_for_history(&engine, field(&plan.input, "history_tree_id")?)?;
                encode(plan)
            }
            crate::BackendAction::CreateRepository => {
                keys(
                    &input,
                    &["name", "primary_slot_id", "target_copies", "operation_id"],
                )?;
                let replicas = input
                    .get("target_copies")
                    .and_then(Value::as_u64)
                    .and_then(|v| u32::try_from(v).ok())
                    .ok_or_else(|| ApiError::invalid("target_copies must be a positive integer"))?;
                encode(
                    engine
                        .create_repository(
                            field(&input, "name")?,
                            field(&input, "primary_slot_id")?,
                            replicas,
                            &identity.principal,
                            operation(&input)?,
                        )
                        .map_err(error)?,
                )
            }
            crate::BackendAction::WriteSession => {
                keys(&input, &["branch_id"])?;
                let branch = field(&input, "branch_id")?;
                repo_for_branch(&engine, branch)?;
                let workspace = engine
                    .create_workspace(branch, &identity.principal)
                    .map_err(error)?;
                Ok(
                    json!({"write_session_id":workspace.workspace_id,"workspace_id":workspace.workspace_id,"branch_id":workspace.branch_id,"principal":workspace.principal,"fencing_token":workspace.fencing_token.to_string(),"expires_at":workspace.created_at.saturating_add(engine.config().workspace_ttl_seconds).to_string(),"scope":"workspace","state":workspace.state}),
                )
            }
            crate::BackendAction::Compare => {
                keys(&input, &["left_commit_id", "right_commit_id"])?;
                let left = field(&input, "left_commit_id")?;
                let right = field(&input, "right_commit_id")?;
                for id in [left, right] {
                    let commit = engine.state().commits.get(id).ok_or_else(|| {
                        ApiError::new(404, "NOT_FOUND", "comparison commit not found")
                    })?;
                    repo_for_history(&engine, &commit.history_tree_id)?;
                }
                engine.compare_commits(left, right).map_err(error)
            }
            crate::BackendAction::PlanCancel => {
                keys(&input, &["plan_id", "operation_id"])?;
                let plan = engine.plan(field(&input, "plan_id")?).map_err(error)?;
                if plan.principal != identity.principal {
                    return Err(ApiError::forbidden());
                }
                repo_for_history(&engine, field(&plan.input, "history_tree_id")?)?;
                encode(
                    engine
                        .cancel_plan(
                            field(&input, "plan_id")?,
                            &identity.principal,
                            operation(&input)?,
                        )
                        .map_err(error)?,
                )
            }
            crate::BackendAction::CreateWorkspace => {
                keys(&input, &["branch_id", "source_commit_id"])?;
                let branch = field(&input, "branch_id")?;
                repo_for_branch(&engine, branch)?;
                let workspace =
                    if let Some(source) = wire::optional_identity(&input, "source_commit_id")? {
                        engine
                            .create_historical_workspace(branch, source, &identity.principal)
                            .map_err(error)?
                    } else {
                        engine
                            .create_workspace(branch, &identity.principal)
                            .map_err(error)?
                    };
                sanitized(workspace)
            }
            crate::BackendAction::Branch => {
                let id = field(&input, "branch_id")?;
                repo_for_branch(&engine, id)?;
                branch_wire(
                    &engine,
                    engine
                        .state()
                        .branches
                        .get(id)
                        .ok_or_else(|| ApiError::new(404, "NOT_FOUND", "branch not found"))?,
                )
            }
            crate::BackendAction::HistoricalSavePlan => {
                keys(
                    &input,
                    &[
                        "history_tree_id",
                        "source_commit_id",
                        "name",
                        "workspace_id",
                        "primary_slot_id",
                        "target_copies",
                    ],
                )?;
                let history = field(&input, "history_tree_id")?;
                repo_for_history(&engine, history)?;
                let mut options = input.clone();
                options.as_object_mut().unwrap().remove("history_tree_id");
                let options: storage_engine::HistoricalSaveOptions =
                    json_codec::from_value(options).map_err(ApiError::invalid)?;
                encode(
                    engine
                        .create_historical_save_plan(history, options, &identity.principal)
                        .map_err(error)?,
                )
            }
            crate::BackendAction::WorkspaceAuthorize => {
                workspace_authorize(&engine, identity, field(&input, "workspace_id")?)?;
                Ok(json!({"authorized":true}))
            }
            crate::BackendAction::WorkspaceSelectionAuthorize => {
                keys(&input, &["workspace_id", "path"])?;
                let id = field(&input, "workspace_id")?;
                workspace_authorize(&engine, identity, id)?;
                engine
                    .validate_workspace_file_selection(id, field(&input, "path")?)
                    .map_err(error)?;
                Ok(json!({"authorized":true}))
            }
            crate::BackendAction::UploadAuthorize => {
                keys(&input, &["workspace_id", "item_id", "logical_size"])?;
                let workspace =
                    workspace_authorize(&engine, identity, field(&input, "workspace_id")?)?;
                storage_contract::validate_id(field(&input, "item_id")?, "entry")
                    .map_err(ApiError::invalid)?;
                if workspace.state != "open" {
                    return Err(ApiError::conflict(
                        "WORKSPACE_CLOSED",
                        "workspace is no longer open for upload",
                    ));
                }
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_err(ApiError::invalid)?
                    .as_secs();
                if now.saturating_sub(workspace.created_at) > engine.config().workspace_ttl_seconds
                {
                    return Err(ApiError::conflict(
                        "LEASE_EXPIRED",
                        "workspace lease expired; retained data remains available",
                    ));
                }
                if input.get("logical_size").is_some() {
                    let size = storage_contract::parse_u64_decimal(field(&input, "logical_size")?)
                        .map_err(ApiError::invalid)?;
                    if size > engine.config().max_file_bytes
                        || size > engine.config().max_workspace_bytes
                    {
                        return Err(ApiError::new(
                            413,
                            "RESOURCE_LIMIT_EXCEEDED",
                            "declared upload exceeds the pool file or workspace limit",
                        ));
                    }
                }
                Ok(json!({"authorized":true}))
            }
            crate::BackendAction::UploadStatus => {
                let workspace =
                    workspace_authorize(&engine, identity, field(&input, "workspace_id")?)?;
                let item = field(&input, "item_id")?;
                let staged = workspace
                    .files
                    .get(item)
                    .ok_or_else(|| ApiError::new(404, "NOT_FOUND", "upload item not found"))?;
                let sealed = matches!(workspace.state.as_str(), "sealed" | "committed");
                let manifest = if sealed {
                    Some(storage_engine::workspace_manifest_hash(workspace).map_err(error)?)
                } else {
                    None
                };
                Ok(
                    json!({"workspace_id":workspace.workspace_id,"item_id":item,"upload_generation":staged.dirty_generation.to_string(),"logical_size":staged.size.to_string(),"content_hash":staged.content_hash,"state":if sealed{"SEALED"}else{"COMPLETE"},"durable_ranges":if staged.size>0{vec![json!({"offset":"0","length":staged.size.to_string(),"content_hash":staged.content_hash})]}else{vec![]},"sealed_manifest_hash":manifest}),
                )
            }
            crate::BackendAction::Namespace => {
                keys(&input, &["workspace_id", "kind", "path", "destination"])?;
                let workspace = field(&input, "workspace_id")?;
                let change = NamespaceOperation {
                    kind: field(&input, "kind")?.into(),
                    path: field(&input, "path")?.into(),
                    destination: input
                        .get("destination")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                };
                sanitized(Application::new(&mut engine, identity).stage_namespace(
                    application::StageNamespace {
                        workspace_id: workspace.into(),
                        change,
                    },
                )?)
            }
            crate::BackendAction::SealWorkspace => {
                keys(&input, &["workspace_id"])?;
                let workspace =
                    workspace_authorize(&engine, identity, field(&input, "workspace_id")?)?;
                let workspace_id = workspace.workspace_id.clone();
                let principal = workspace.principal.clone();
                engine
                    .seal_workspace(&workspace_id, &principal)
                    .map_err(error)
            }
            crate::BackendAction::CommitWorkspace
            | crate::BackendAction::CommitWorkspaceInternal => {
                if action == crate::BackendAction::CommitWorkspace {
                    keys(
                        &input,
                        &[
                            "workspace_id",
                            "operation_id",
                            "repository_id",
                            "branch_id",
                            "expected_head",
                            "fencing_token",
                            "message",
                            "requested_protection",
                            "file_placement",
                            "expected_topology_generation",
                            "sealed_manifest_hash",
                            "request_digest",
                        ],
                    )?;
                } else {
                    keys(
                        &input,
                        &[
                            "workspace_id",
                            "operation_id",
                            "expected_head",
                            "fencing_token",
                            "message",
                            "request_digest",
                        ],
                    )?;
                }
                // Keep strict string presence checks at the canonical wire boundary.
                for key in [
                    "workspace_id",
                    "operation_id",
                    "expected_head",
                    "fencing_token",
                ] {
                    field(&input, key)?;
                }
                let workspace_id = field(&input, "workspace_id")?.to_owned();
                let plan = if action == crate::BackendAction::CommitWorkspace {
                    let command = json_codec::from_value(input).map_err(ApiError::invalid)?;
                    Application::new(&mut engine, identity)
                        .prepare_commit_workspace_public(command)?
                } else {
                    let command = application::CommitWorkspace {
                        workspace_id: field(&input, "workspace_id")?.into(),
                        operation_id: operation(&input)?.into(),
                        expected_head: field(&input, "expected_head")?.into(),
                        fencing_token: storage_contract::parse_u64_decimal(field(
                            &input,
                            "fencing_token",
                        )?)
                        .map_err(ApiError::invalid)?,
                        message: input
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .into(),
                        request_digest: input
                            .get("request_digest")
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                    };
                    Application::new(&mut engine, identity).prepare_commit_workspace(command)?
                };
                drop(engine);
                #[cfg(test)]
                if let Some(hook) = self.commit_preparation_hook.lock().unwrap().clone() {
                    hook();
                }
                let prepared = plan.execute().map_err(error)?;
                let mut engine = self.engine.lock().map_err(|_| {
                    ApiError::new(500, "INTERNAL_ERROR", "storage engine requires restart")
                })?;
                let current = current_protected_identity(&engine, identity)?;
                let result = Application::new(&mut engine, &current)
                    .finish_workspace_commit(&workspace_id, prepared)?;
                operation_wire(result)
            }
            crate::BackendAction::AbortWorkspace => {
                keys(&input, &["workspace_id"])?;
                let id = field(&input, "workspace_id")?;
                workspace_authorize(&engine, identity, id)?;
                engine
                    .abort_workspace(id, &identity.principal)
                    .map_err(error)?;
                Ok(json!({"workspace_id":id,"state":"aborted"}))
            }
            crate::BackendAction::CreateBranch => {
                for key in ["repository_id", "source_commit_id", "name", "operation_id"] {
                    field(&input, key)?;
                }
                let command = json_codec::from_value(input).map_err(ApiError::invalid)?;
                let branch = Application::new(&mut engine, identity).create_branch(command)?;
                branch_wire(&engine, &branch)
            }
            crate::BackendAction::Archive => {
                for key in ["branch_id", "operation_id"] {
                    field(&input, key)?;
                }
                let command = json_codec::from_value(input).map_err(ApiError::invalid)?;
                encode(Application::new(&mut engine, identity).archive_branch(command)?)
            }
            crate::BackendAction::MergeLink => {
                for key in [
                    "history_tree_id",
                    "source_branch_id",
                    "source_commit_id",
                    "target_branch_id",
                    "target_commit_id",
                    "operation_id",
                ] {
                    field(&input, key)?;
                }
                let command = json_codec::from_value(input).map_err(ApiError::invalid)?;
                encode(Application::new(&mut engine, identity).merge_link(command)?)
            }
            crate::BackendAction::History => {
                let file = field(&input, "file_id")?;
                let versions = engine.file_history(file).map_err(error)?;
                let visible = versions
                    .into_iter()
                    .filter(|version| repo_for_history(&engine, &version.history_tree_id).is_ok())
                    .collect::<Vec<_>>();
                if visible.is_empty() {
                    return Err(ApiError::new(404, "NOT_FOUND", "file history not found"));
                }
                Ok(json!({"file_id":file,"versions":visible}))
            }
            crate::BackendAction::Graph => {
                let history = field(&input, "history_tree_id")?;
                repo_for_history(&engine, history)?;
                encode(engine.graph(history).map_err(error)?)
            }
            crate::BackendAction::Commit => {
                let commit = engine
                    .state()
                    .commits
                    .get(field(&input, "commit_id")?)
                    .ok_or_else(|| ApiError::new(404, "NOT_FOUND", "commit not found"))?;
                repo_for_history(&engine, &commit.history_tree_id)?;
                encode(commit)
            }
            crate::BackendAction::Version | crate::BackendAction::Placement => {
                let id = field(&input, "version_id")?;
                let version = engine
                    .state()
                    .versions
                    .get(id)
                    .ok_or_else(|| ApiError::new(404, "NOT_FOUND", "version not found"))?;
                repo_for_history(&engine, &version.history_tree_id)?;
                if action == crate::BackendAction::Version {
                    let mut value = encode(version)?;
                    value["content_state"] = engine
                        .state()
                        .content_states
                        .get(id)
                        .cloned()
                        .unwrap_or_else(|| "available".into())
                        .into();
                    Ok(value)
                } else {
                    encode(
                        engine.state().placements.get(id).ok_or_else(|| {
                            ApiError::new(404, "NOT_FOUND", "placement not found")
                        })?,
                    )
                }
            }
            crate::BackendAction::Operation => {
                let operation_id = field(&input, "operation_id")?;
                // Reject foreign owners before loading even the pending result
                // pages. Unknown/pending operations never trigger payload I/O.
                if let Some(owner) = engine.operation_owner(operation_id)
                    && owner != identity.principal
                {
                    return Err(ApiError::forbidden());
                }
                let record = engine.operation_record(operation_id).map_err(error)?;
                operations::authorize_record(&engine, identity, &record)?;
                let plan = engine.prepare_operation_health(record);
                drop(engine);
                #[cfg(test)]
                if let Some(hook) = self.operation_health_hook.lock().unwrap().clone() {
                    hook();
                }
                let record = plan.check();
                let engine = self.engine.lock().map_err(|_| {
                    ApiError::new(500, "INTERNAL_ERROR", "storage engine requires restart")
                })?;
                let current = current_protected_identity(&engine, identity)?;
                operations::authorize_record(&engine, &current, &record)?;
                drop(engine);
                operation_wire(record.into_result())
            }
            crate::BackendAction::PurgePlan => {
                keys(&input, &["history_tree_id", "version_ids"])?;
                let history = field(&input, "history_tree_id")?;
                repo_for_history(&engine, history)?;
                let versions: Vec<String> = json_codec::from_value(
                    input
                        .get("version_ids")
                        .cloned()
                        .ok_or_else(|| ApiError::invalid("version_ids required"))?,
                )
                .map_err(ApiError::invalid)?;
                encode(
                    engine
                        .create_purge_plan(history, versions, &identity.principal)
                        .map_err(error)?,
                )
            }
            crate::BackendAction::SplitPlan => {
                keys(&input, &["history_tree_id", "cut_commit_id"])?;
                let history = field(&input, "history_tree_id")?;
                repo_for_history(&engine, history)?;
                encode(
                    engine
                        .create_split_plan(
                            history,
                            field(&input, "cut_commit_id")?,
                            &identity.principal,
                        )
                        .map_err(error)?,
                )
            }
            crate::BackendAction::PlanCommit => {
                for key in ["plan_id", "operation_id", "plan_digest"] {
                    field(&input, key)?;
                }
                let command = json_codec::from_value(input).map_err(ApiError::invalid)?;
                encode(Application::new(&mut engine, identity).commit_plan(command)?)
            }
            crate::BackendAction::Policy => {
                let policy = engine
                    .state()
                    .protection_policies
                    .get(field(&input, "policy_id")?)
                    .ok_or_else(|| {
                        ApiError::new(404, "NOT_FOUND", "protection policy not found")
                    })?;
                repository(&engine, &policy.repository_id)?;
                encode(policy)
            }
            crate::BackendAction::PlacementRule => {
                let rule = engine
                    .state()
                    .placement_rules
                    .get(field(&input, "rule_id")?)
                    .ok_or_else(|| ApiError::new(404, "NOT_FOUND", "placement rule not found"))?;
                repository(&engine, &rule.repository_id)?;
                encode(rule)
            }
            crate::BackendAction::UpdatePolicy => {
                keys(
                    &input,
                    &[
                        "policy_id",
                        "repository_id",
                        "path_prefix",
                        "target_copies",
                        "expected_generation",
                        "operation_id",
                    ],
                )?;
                let repo = field(&input, "repository_id")?;
                repository(&engine, repo)?;
                // Retain the transport's key/admission error ordering while the
                // application also preserves domain safety checks.
                let command = json_codec::from_value(input).map_err(ApiError::invalid)?;
                let result =
                    Application::new(&mut engine, identity).update_protection_policy(command)?;
                operation_wire(result)
            }
            crate::BackendAction::UpdatePlacementRule => {
                keys(
                    &input,
                    &[
                        "rule_id",
                        "repository_id",
                        "path_prefix",
                        "primary_slot_id",
                        "expected_generation",
                        "operation_id",
                    ],
                )?;
                let repo = field(&input, "repository_id")?;
                repository(&engine, repo)?;
                // Retain the transport's key/admission error ordering while the
                // application also preserves domain safety checks.
                let command = json_codec::from_value(input).map_err(ApiError::invalid)?;
                let result =
                    Application::new(&mut engine, identity).update_placement_rule(command)?;
                operation_wire(result)
            }
            crate::BackendAction::Check => encode(engine.check().map_err(error)?),
            crate::BackendAction::Disks => {
                let observation = engine.prepare_disk_observation();
                drop(engine);
                encode(json!({"disks":observation.status().map_err(error)?}))
            }
            crate::BackendAction::Topology => Ok(
                json!({"topology_id":"default","generation":engine.state().topology_generation.to_string(),"slots":engine.config().disks.iter().map(|disk|json!({"slot_id":disk.slot_id,"disk_id":disk.disk_id,"level":disk.level,"upstream_slot_id":disk.upstream_slot_id,"metadata_member":disk.metadata_member,"binding_generation":disk.binding_generation.to_string()})).collect::<Vec<_>>()}),
            ),
            crate::BackendAction::Events => {
                let after = storage_contract::parse_u64_decimal(
                    input
                        .get("query")
                        .and_then(|query| query.get("after"))
                        .and_then(Value::as_str)
                        .unwrap_or("0"),
                )
                .map_err(ApiError::invalid)?;
                if after > engine.state().sequence {
                    return Err(ApiError::new(
                        400,
                        "INVALID_CURSOR",
                        "event cursor is ahead of the published sequence",
                    ));
                }
                let mut events = Vec::new();
                let mut inspected = after;
                for event in events_after(&engine.state().events, after) {
                    inspected = event.sequence;
                    events.push(event);
                    if events.len() >= 1000 {
                        break;
                    }
                }
                Ok(json!({"events":events,"next_cursor":inspected.to_string()}))
            }
            crate::BackendAction::RegisterDisk => Err(ApiError::new(
                409,
                "OFFLINE_CONFIGURATION_REQUIRED",
                "disk registration requires an explicit offline configuration and topology migration",
            )),
        }
    }
    fn upload(
        &self,
        identity: &Identity,
        workspace: &str,
        _item: &str,
        path: &str,
        source: &crate::UploadSource,
    ) -> Result<Value, ApiError> {
        let engine = self
            .engine
            .lock()
            .map_err(|_| ApiError::new(500, "INTERNAL_ERROR", "storage engine requires restart"))?;
        let current = current_protected_identity(&engine, identity)?;
        let workspace_record = workspace_authorize(&engine, &current, workspace)?;
        let generation = workspace_record
            .files
            .get(_item)
            .map(|file| file.dirty_generation.checked_add(1))
            .unwrap_or(Some(1))
            .ok_or_else(|| {
                error(storage_engine::Error::new(
                    "RESOURCE_LIMIT",
                    "upload generation exhausted",
                ))
            })?;
        let plan = engine
            .prepare_upload_file(
                workspace,
                _item,
                path,
                source.file.try_clone().map_err(ApiError::io)?,
                generation,
                Some((source.size, source.hash.clone())),
            )
            .map_err(error)?;
        drop(engine);
        let prepared = plan.execute().map_err(error)?;
        let mut engine = self
            .engine
            .lock()
            .map_err(|_| ApiError::new(500, "INTERNAL_ERROR", "storage engine requires restart"))?;
        finish_authorized_upload(&mut engine, identity, workspace, prepared)
    }
    fn upload_generation(
        &self,
        identity: &Identity,
        workspace: &str,
        item: &str,
        path: &str,
        source: &crate::UploadSource,
        generation: u64,
    ) -> Result<Value, ApiError> {
        let engine = self
            .engine
            .lock()
            .map_err(|_| ApiError::new(500, "INTERNAL_ERROR", "storage engine requires restart"))?;
        let current = current_protected_identity(&engine, identity)?;
        workspace_authorize(&engine, &current, workspace)?;
        let plan = engine
            .prepare_upload_file(
                workspace,
                item,
                path,
                source.file.try_clone().map_err(ApiError::io)?,
                generation,
                Some((source.size, source.hash.clone())),
            )
            .map_err(error)?;
        drop(engine);
        let prepared = plan.execute().map_err(error)?;
        let mut engine = self
            .engine
            .lock()
            .map_err(|_| ApiError::new(500, "INTERNAL_ERROR", "storage engine requires restart"))?;
        // The session or SMB binding may have changed while bytes were copied.
        finish_authorized_upload(&mut engine, identity, workspace, prepared)
    }
    fn content(&self, identity: &Identity, version: &str) -> Result<Content, ApiError> {
        self.range_content(identity, version)?.fully_verified()
    }
    fn range_content(&self, identity: &Identity, version: &str) -> Result<Content, ApiError> {
        self.open_version_content(identity, version)
    }
    fn workspace_content(
        &self,
        identity: &Identity,
        workspace: &str,
        item: &str,
    ) -> Result<Content, ApiError> {
        self.open_workspace_content(identity, workspace, item)
    }
    fn checksums(&self, identity: &Identity, version: &str) -> Result<Vec<u8>, ApiError> {
        self.read_version_checksums(identity, version)
    }
    fn observe_disks(&self, _: &crate::InternalDiskObservation) -> Result<Value, ApiError> {
        let observation = {
            let engine = self.engine.lock().map_err(|_| {
                ApiError::new(500, "INTERNAL_ERROR", "storage engine requires restart")
            })?;
            engine.prepare_disk_observation()
        };
        disk_monitor::observe(observation)
    }
}

#[cfg(test)]
#[path = "../../../test/server/unit/engine_backend/authorization_tests.rs"]
mod authorization_tests;
#[cfg(test)]
#[path = "../../../test/server/unit/engine_backend/event_tests.rs"]
mod event_tests;
#[cfg(test)]
#[path = "../../../test/server/unit/engine_backend/readiness_tests.rs"]
mod readiness_tests;
#[cfg(test)]
#[path = "../../../test/server/unit/engine_backend/revocation_tests.rs"]
mod revocation_tests;

#[cfg(test)]
#[path = "../../../test/server/unit/engine_backend/scope_dispatch_tests.rs"]
mod scope_dispatch_tests;
