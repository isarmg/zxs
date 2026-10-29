//! Authorization and wire adaptation for the storage engine.
use crate::{ApiError, Backend, Content, auth::Identity};
use json_codec::{Value, json};
use std::{
    path::Path,
    sync::{Arc, Mutex},
};
use storage_engine::{Engine, NamespaceOperation, Repository};
mod admission;
use admission::AdmissionEngine;
pub mod application;
use application::Application;
mod branch;
mod content;
mod directory_scopes;
mod directory_selection;
mod disk_monitor;
mod error_registry;
mod file_manager;
mod graph;
mod integrity_scrub;
mod operations;
mod read_projection;
mod wire;
use wire::field;
pub(crate) use wire::{error, error_response};

pub(crate) fn report_recovery_warnings(engine: &mut Engine) {
    let storage_engine::RecoveryWarnings {
        seals,
        checkpoint_load,
        checkpoint_save,
    } = engine.take_recovery_warnings();
    for failure in seals {
        tracing::warn!(
            context = ?failure.context(),
            error = ?failure.cause(),
            "recovery observed a seal verification failure"
        );
    }
    if let Some(error) = checkpoint_load {
        tracing::warn!(error = ?error, "recovery checkpoint unavailable; authoritative journal replay completed");
    }
    if let Some(error) = checkpoint_save {
        tracing::warn!(error = ?error, "recovery completed without refreshing its optional checkpoint");
    }
}

fn history_index_status(health: storage_engine::IndexHealth) -> crate::HistoryIndexStatus {
    match health {
        storage_engine::IndexHealth::Unbuilt => crate::HistoryIndexStatus::Unbuilt,
        storage_engine::IndexHealth::Ready { .. } => crate::HistoryIndexStatus::Ready,
        storage_engine::IndexHealth::Unavailable(failure) => {
            crate::HistoryIndexStatus::Unavailable {
                code: failure.cause().code(),
            }
        }
    }
}

pub struct EngineBackend {
    engine: Mutex<AdmissionEngine>,
    #[cfg(test)]
    commit_preparation_hook: Mutex<Option<std::sync::Arc<dyn Fn() + Send + Sync>>>,
    #[cfg(test)]
    operation_health_hook: Mutex<Option<std::sync::Arc<dyn Fn() + Send + Sync>>>,
}
impl EngineBackend {
    fn finish_file_history_page<T>(
        &self,
        identity: &Identity,
        file_id: &str,
        page: Result<T, ApiError>,
    ) -> Result<T, ApiError> {
        let engine = self.engine.lock().expect("service state lock poisoned");
        current_protected_identity(&engine, identity)?;
        file_manager::file_repo(&engine, file_id)?;
        page
    }

    fn finish_directory_page(
        &self,
        identity: &Identity,
        branch: &str,
        page: Result<crate::DirectorySnapshot, ApiError>,
    ) -> Result<crate::DirectorySnapshot, ApiError> {
        let engine = self.engine.lock().expect("service state lock poisoned");
        current_protected_identity(&engine, identity)?;
        repo_for_branch(&engine, branch)?;
        page
    }

    pub fn new(mut engine: Engine) -> Self {
        report_recovery_warnings(&mut engine);
        Self {
            engine: Mutex::new(AdmissionEngine::new(engine)),
            #[cfg(test)]
            commit_preparation_hook: Mutex::new(None),
            #[cfg(test)]
            operation_health_hook: Mutex::new(None),
        }
    }
    #[cfg(test)]
    pub(crate) fn test_with_engine_locked<T>(&self, f: impl FnOnce(&mut Engine) -> T) -> T {
        let mut engine = self.engine.lock().unwrap();
        f(&mut engine)
    }
    /// Managed dispatch linearizes admission after acquiring the engine mutex.
    /// An operation already admitted may finish; queued requests see revocation.
    pub(crate) fn set_management_auth(
        &self,
        auth: crate::auth::AuthReader,
    ) -> Result<(), ApiError> {
        self.engine
            .lock()
            .expect("service state lock poisoned")
            .set_management_auth(auth);
        Ok(())
    }
    pub(crate) fn replicate_auth(&self, snapshot: Value) -> Result<(), ApiError> {
        let mut engine = self.engine.lock().expect("service state lock poisoned");
        if engine.auth_state_ref() == Some(&snapshot) {
            return Ok(());
        }
        engine
            .persist_auth_state(
                snapshot,
                "system_auth",
                storage_contract::OperationId::generate(),
            )
            .map_err(error)
    }
    /// Current optional-index state only; no member probe, State clone, or file I/O.
    pub(crate) fn history_index_status(&self) -> crate::HistoryIndexStatus {
        let engine = self.engine.lock().expect("service state lock poisoned");
        history_index_status(engine.index_health())
    }

    pub(crate) fn read_only(&self) -> Result<bool, ApiError> {
        Ok(self
            .engine
            .lock()
            .expect("service state lock poisoned")
            .is_read_only())
    }
    pub fn protect_auth(
        self: &std::sync::Arc<Self>,
        mut auth: crate::auth::AuthStore,
    ) -> Result<crate::auth::AuthStore, ApiError> {
        let backend = self.clone();
        auth.attach_backup(std::sync::Arc::new(move |snapshot| {
            let mut engine = backend.engine.lock().expect("service state lock poisoned");
            if engine.auth_state_ref() == Some(&snapshot) {
                return Ok(());
            }
            engine
                .persist_auth_state(
                    snapshot,
                    "system_auth",
                    storage_contract::OperationId::generate(),
                )
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
            .expect("service state lock poisoned")
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
            storage_contract::OperationId::generate(),
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
    state.repositories.get(id).ok_or_else(|| {
        ApiError::new(
            ::http::StatusCode::NOT_FOUND,
            storage_contract::ErrorCode::NotFound,
            "repository not found",
        )
    })
}
fn capture_event_page(events: Vec<storage_contract::Event>, after: u64) -> crate::EventPage {
    let next_cursor = events.last().map_or(after, |event| event.sequence);
    crate::EventPage {
        events,
        next_cursor,
    }
}
fn repo_for_history<'a>(engine: &'a Engine, id: &str) -> Result<&'a Repository, ApiError> {
    repo_for_history_in_state(engine.state(), id)
}
fn repo_for_history_in_state<'a>(
    state: &'a storage_contract::State,
    id: &str,
) -> Result<&'a Repository, ApiError> {
    let history = state.histories.get(id).ok_or_else(|| {
        ApiError::new(
            ::http::StatusCode::NOT_FOUND,
            storage_contract::ErrorCode::NotFound,
            "history not found",
        )
    })?;
    repository_in_state(state, &history.repository_id)
}
fn repo_for_branch<'a>(engine: &'a Engine, id: &str) -> Result<&'a Repository, ApiError> {
    repo_for_branch_in_state(engine.state(), id)
}
fn repo_for_branch_in_state<'a>(
    state: &'a storage_contract::State,
    id: &str,
) -> Result<&'a Repository, ApiError> {
    let branch = state.branches.get(id).ok_or_else(|| {
        ApiError::new(
            ::http::StatusCode::NOT_FOUND,
            storage_contract::ErrorCode::NotFound,
            "branch not found",
        )
    })?;
    if branch.state == storage_contract::BranchState::Migrated {
        return Err(ApiError::new(
            ::http::StatusCode::CONFLICT,
            storage_contract::ErrorCode::TreeMoved,
            "branch has moved to an independent history domain",
        ));
    }
    repo_for_history_in_state(state, &branch.history_tree_id)
}
fn authorized_version<'a>(
    engine: &'a Engine,
    id: &str,
) -> Result<&'a storage_contract::FileVersion, ApiError> {
    let version = engine.state().versions.get(id).ok_or_else(|| {
        ApiError::new(
            ::http::StatusCode::NOT_FOUND,
            storage_contract::ErrorCode::NotFound,
            "version not found",
        )
    })?;
    repo_for_history(engine, &version.history_tree_id)?;
    Ok(version)
}
fn workspace_authorize<'a>(
    engine: &'a Engine,
    identity: &Identity,
    id: &str,
) -> Result<std::borrow::Cow<'a, storage_engine::Workspace>, ApiError> {
    let workspace = engine.workspace_view(id).map_err(error)?;
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
                ::http::StatusCode::CONFLICT,
                storage_contract::ErrorCode::SmbRebindRequired,
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
) -> Result<storage_contract::StagedFile, ApiError> {
    let current = current_protected_identity(engine, identity)?;
    workspace_authorize(engine, &current, workspace)?;
    engine.finish_upload(prepared).map_err(error)
}

impl EngineBackend {
    fn operation_admitted(
        &self,
        engine: std::sync::MutexGuard<'_, AdmissionEngine>,
        identity: &Identity,
        operation_id: storage_contract::OperationId,
    ) -> Result<crate::OperationObservation, ApiError> {
        let record = operations::authorized_record(&engine, identity, operation_id)?;
        let plan = engine.prepare_operation_health(record);
        drop(engine);
        #[cfg(test)]
        if let Some(hook) = self.operation_health_hook.lock().unwrap().clone() {
            hook();
        }
        let observed = plan.observe();
        let health = observed
            .health()
            .map(crate::operation_observation::CheckedOperationHealth::completed)
            .transpose()?;
        let record = observed.into_record();
        let engine = self.engine.lock().expect("service state lock poisoned");
        let current = current_protected_identity(&engine, identity)?;
        operations::authorize_record(&engine, &current, &record)?;
        drop(engine);
        Ok(crate::OperationObservation {
            outcome: record.into_result().into(),
            health,
        })
    }
}

impl Backend for EngineBackend {
    fn http_spool_workspace(
        &self,
        identity: &Identity,
        workspace: &str,
    ) -> Result<Option<storage_contract::Workspace>, ApiError> {
        let engine = self.engine.lock().expect("service state lock poisoned");
        let current = current_protected_identity(&engine, identity)?;
        if engine.is_read_only() {
            return Err(ApiError::new(
                ::http::StatusCode::SERVICE_UNAVAILABLE,
                storage_contract::ErrorCode::RecoveryRequired,
                "storage recovery must complete before staging retirement",
            ));
        }
        let workspace = workspace_authorize(&engine, &current, workspace)?;
        Ok(
            (workspace.state == storage_contract::WorkspaceState::Committed)
                .then(|| workspace.into_owned()),
        )
    }

    fn qualify_http_spool_copy(
        &self,
        _: &crate::InternalHttpSpoolRetirement,
        candidate: &crate::HttpSpoolCandidate,
    ) -> Result<crate::HttpSpoolQualification, ApiError> {
        use crate::HttpSpoolQualification;
        let engine = self.engine.lock().expect("service state lock poisoned");
        if engine.is_read_only() {
            return Ok(HttpSpoolQualification::Unavailable);
        }
        match engine.workspace_view(candidate.workspace_id()) {
            Ok(workspace) => Ok(if candidate.matches(&workspace) {
                HttpSpoolQualification::Eligible
            } else {
                HttpSpoolQualification::Retain
            }),
            Err(source) if source.code() == storage_contract::ErrorCode::WorkspaceNotFound => {
                Ok(HttpSpoolQualification::Retain)
            }
            Err(source) => Err(error(source)),
        }
    }

    fn operation(
        &self,
        identity: &Identity,
        operation_id: storage_contract::OperationId,
    ) -> Result<crate::OperationObservation, ApiError> {
        let engine = self.engine.lock().expect("service state lock poisoned");
        let current = current_protected_identity(&engine, identity)?;
        let identity = &current;
        self.operation_admitted(engine, identity, operation_id)
    }

    fn operation_outcome(
        &self,
        identity: &Identity,
        operation_id: storage_contract::OperationId,
    ) -> Result<Option<storage_contract::OperationOutcome>, ApiError> {
        let engine = self.engine.lock().expect("service state lock poisoned");
        let current = current_protected_identity(&engine, identity)?;
        let record = operations::find_authorized_record(&engine, &current, operation_id)?;
        Ok(record.map(|record| record.into_result().into()))
    }

    fn commit_workspace(
        &self,
        identity: &Identity,
        command: crate::CommitWorkspace,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        let mut engine = self.engine.lock().expect("service state lock poisoned");
        let current = current_protected_identity(&engine, identity)?;
        let identity = &current;
        let workspace_id = command.workspace_id.clone();
        let plan = Application::new(&mut engine, identity).prepare_commit_workspace(command)?;
        drop(engine);
        #[cfg(test)]
        if let Some(hook) = self.commit_preparation_hook.lock().unwrap().clone() {
            hook();
        }
        let prepared = plan.execute().map_err(error)?;
        let mut engine = self.engine.lock().expect("service state lock poisoned");
        let current = current_protected_identity(&engine, identity)?;
        let result = Application::new(&mut engine, &current)
            .finish_workspace_commit(&workspace_id, prepared)?;
        Ok(result)
    }

    fn snapshot_at(
        &self,
        identity: &Identity,
        branch: &str,
        commit: &str,
    ) -> Result<Arc<storage_contract::Commit>, ApiError> {
        let engine = self.engine.lock().expect("service state lock poisoned");
        current_protected_identity(&engine, identity)?;
        let (commit, _) = read_projection::branch_snapshot(engine.state(), branch, Some(commit))?;
        Ok(engine
            .state()
            .commits
            .get_shared(commit)
            .expect("invariant: admitted commit exists under the engine lock"))
    }

    fn snapshot_entry(
        &self,
        identity: &Identity,
        branch: &str,
        commit: &str,
        key: &str,
    ) -> Result<Option<storage_contract::Entry>, ApiError> {
        let engine = self.engine.lock().expect("service state lock poisoned");
        current_protected_identity(&engine, identity)?;
        read_projection::branch_snapshot(engine.state(), branch, Some(commit))?;
        let tree = engine.namespace_tree_at(branch, commit).map_err(error)?;
        tree.get_key(key).map_err(error)
    }

    fn snapshot_directory_has_children(
        &self,
        identity: &Identity,
        branch: &str,
        commit: &str,
        key: &str,
    ) -> Result<bool, ApiError> {
        let engine = self.engine.lock().expect("service state lock poisoned");
        current_protected_identity(&engine, identity)?;
        read_projection::branch_snapshot(engine.state(), branch, Some(commit))?;
        let tree = engine.namespace_tree_at(branch, commit).map_err(error)?;
        Ok(tree
            .directory_page(key, None, 1)
            .map_err(error)?
            .is_some_and(|children| !children.is_empty()))
    }

    fn snapshot_entry_by_identity(
        &self,
        identity: &Identity,
        branch: &str,
        commit: &str,
        key: &str,
        entry_id: &str,
    ) -> Result<Option<storage_contract::Entry>, ApiError> {
        let engine = self.engine.lock().expect("service state lock poisoned");
        current_protected_identity(&engine, identity)?;
        read_projection::branch_snapshot(engine.state(), branch, Some(commit))?;
        let tree = engine.namespace_tree_at(branch, commit).map_err(error)?;
        if let Some(entry) = tree.get_key(key).map_err(error)?
            && entry.entry_id == entry_id
        {
            return Ok(Some(entry));
        }
        // A concurrent rename moved the stable entry away from the caller's
        // last-known key: resolve by identity with an early-stop walk.
        let mut found = None;
        tree.walk(|entry| {
            if entry.entry_id == entry_id {
                found = Some(entry.clone());
                return Ok(false);
            }
            Ok(true)
        })
        .map_err(error)?;
        Ok(found)
    }

    fn create_workspace(
        &self,
        identity: &Identity,
        command: crate::CreateWorkspace,
    ) -> Result<storage_contract::Workspace, ApiError> {
        let mut engine = self.engine.lock().expect("service state lock poisoned");
        let current = current_protected_identity(&engine, identity)?;
        let crate::CreateWorkspace {
            branch_id,
            source_commit_id,
        } = command;
        repo_for_branch(&engine, &branch_id)?;
        match source_commit_id {
            Some(source) => {
                engine.create_historical_workspace(&branch_id, &source, &current.principal)
            }
            None => engine.create_workspace(&branch_id, &current.principal),
        }
        .map_err(error)
    }
    fn abort_workspace(&self, identity: &Identity, workspace: &str) -> Result<(), ApiError> {
        let mut engine = self.engine.lock().expect("service state lock poisoned");
        let current = current_protected_identity(&engine, identity)?;
        workspace_authorize(&engine, &current, workspace)?;
        engine
            .abort_workspace(workspace, &current.principal)
            .map_err(error)
    }

    fn workspace(
        &self,
        identity: &Identity,
        workspace: &str,
    ) -> Result<storage_contract::Workspace, ApiError> {
        let engine = self.engine.lock().expect("service state lock poisoned");
        let current = current_protected_identity(&engine, identity)?;
        workspace_authorize(&engine, &current, workspace).map(std::borrow::Cow::into_owned)
    }

    fn require_native_workspace_commit_completion(
        &self,
        identity: &Identity,
        command: crate::CommitWorkspace,
    ) -> Result<(), ApiError> {
        let engine = self.engine.lock().expect("service state lock poisoned");
        let current = current_protected_identity(&engine, identity)?;
        workspace_authorize(&engine, &current, &command.workspace_id)?;
        let original = storage_contract::CommitRequest {
            workspace_id: command.workspace_id,
            operation_id: command.operation_id,
            principal: current.principal,
            expected_head: command.expected_head,
            fencing_token: command.fencing_token,
            message: command.message,
            request_digest: command.request_digest,
        };
        if engine
            .native_workspace_commit_completed(&original)
            .map_err(error)?
        {
            return Ok(());
        }
        let mut error = ApiError::new(
            ::http::StatusCode::SERVICE_UNAVAILABLE,
            storage_contract::ErrorCode::RecoveryRequired,
            format!(
                "operation {} is already published; its original workspace binding and completion require reconciliation; retain the original SMB work",
                original.operation_id
            ),
        );
        error.retryable = false;
        Err(error)
    }

    fn published_upload_status(
        &self,
        identity: &Identity,
        workspace: &str,
        item: &str,
    ) -> Result<Option<crate::PublishedUploadStatus>, ApiError> {
        let engine = self.engine.lock().expect("service state lock poisoned");
        let current = current_protected_identity(&engine, identity)?;
        let workspace = workspace_authorize(&engine, &current, workspace)?;
        let Some(staged) = workspace.files.get(item) else {
            return Ok(None);
        };
        let state = if matches!(
            workspace.state,
            storage_contract::WorkspaceState::Sealed | storage_contract::WorkspaceState::Committed
        ) {
            crate::PublishedUploadState::Sealed {
                manifest_hash: storage_engine::workspace_manifest_hash(&workspace)
                    .map_err(error)?,
            }
        } else {
            crate::PublishedUploadState::Complete
        };
        Ok(Some(crate::PublishedUploadStatus {
            generation: staged.dirty_generation,
            logical_size: staged.size,
            content_hash: staged.content_hash,
            state,
        }))
    }

    fn seal_workspace(
        &self,
        identity: &Identity,
        workspace: &str,
    ) -> Result<storage_contract::SealedWorkspace, ApiError> {
        let mut engine = self.engine.lock().expect("service state lock poisoned");
        let current = current_protected_identity(&engine, identity)?;
        workspace_authorize(&engine, &current, workspace)?;
        engine
            .seal_workspace(workspace, &current.principal)
            .map_err(error)
    }
    fn commit_workspace_public(
        &self,
        identity: &Identity,
        command: storage_contract::PublicCommitEnvelope,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        let mut engine = self.engine.lock().expect("service state lock poisoned");
        let current = current_protected_identity(&engine, identity)?;
        let workspace_id = command.workspace_id().to_owned();
        let plan =
            Application::new(&mut engine, &current).prepare_commit_workspace_public(command)?;
        drop(engine);
        #[cfg(test)]
        if let Some(hook) = self.commit_preparation_hook.lock().unwrap().clone() {
            hook();
        }
        let prepared = plan.execute().map_err(error)?;
        let mut engine = self.engine.lock().expect("service state lock poisoned");
        let current = current_protected_identity(&engine, identity)?;
        Application::new(&mut engine, &current).finish_workspace_commit(&workspace_id, prepared)
    }

    fn version_snapshot(
        &self,
        identity: &crate::auth::Identity,
        version: &str,
    ) -> Result<crate::VersionSnapshot, crate::ApiError> {
        let engine = self.engine.lock().expect("service state lock poisoned");
        current_protected_identity(&engine, identity)?;
        let version = authorized_version(&engine, version)?.clone();
        let content_state = wire::stored_version_content_state(
            engine.state().content_states.get(&version.version_id),
        );
        Ok(crate::VersionSnapshot {
            version,
            content_state,
        })
    }
    fn authorize_version(
        &self,
        identity: &crate::auth::Identity,
        version: &str,
    ) -> Result<(), crate::ApiError> {
        let engine = self.engine.lock().expect("service state lock poisoned");
        current_protected_identity(&engine, identity)?;
        authorized_version(&engine, version)?;
        Ok(())
    }
    fn repository_snapshot(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<storage_contract::Repository, crate::ApiError> {
        let engine = self.engine.lock().expect("service state lock poisoned");
        current_protected_identity(&engine, identity)?;
        Ok(repository(&engine, id)?.clone())
    }
    fn authorize_repository(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<(), crate::ApiError> {
        let engine = self.engine.lock().expect("service state lock poisoned");
        current_protected_identity(&engine, identity)?;
        repository(&engine, id)?;
        Ok(())
    }
    fn capabilities(&self) -> Result<crate::Capabilities, crate::ApiError> {
        let engine = self.engine.lock().expect("service state lock poisoned");
        Ok(crate::Capabilities::for_engine(
            engine.config().max_file_bytes,
        ))
    }
    fn ready(&self) -> Result<crate::Readiness, crate::ApiError> {
        let engine = self.engine.lock().expect("service state lock poisoned");

        if engine.is_read_only() {
            return Err(ApiError::new(
                ::http::StatusCode::SERVICE_UNAVAILABLE,
                storage_contract::ErrorCode::RecoveryRequired,
                "storage is open for verified recovery reads only",
            ));
        }
        let probe = engine.prepare_metadata_member_probe();
        let configured_members = probe.member_count();
        let storage_disks = engine.config().disks.len();
        let mode = engine.config().mode.clone();
        let sequence = engine.state().sequence;
        let history_index = history_index_status(engine.index_health());
        drop(engine);
        probe.verify().map_err(|source| {
            ApiError::new(
                ::http::StatusCode::SERVICE_UNAVAILABLE,
                storage_contract::ErrorCode::MetadataUnavailable,
                "all configured metadata members are required for normal writes",
            )
            .with_source(source)
        })?;
        Ok(crate::Readiness::Engine(crate::EngineReadiness {
            mode,
            sequence,
            storage_disk_count: storage_disks,
            metadata_member_count: configured_members,
            history_index,
        }))
    }
    fn workspace_capacity(
        &self,
        identity: &crate::auth::Identity,
    ) -> Result<storage_contract::WorkspaceCapacity, crate::ApiError> {
        let engine = self.engine.lock().expect("service state lock poisoned");
        let current = current_protected_identity(&engine, identity)?;
        Ok(engine.workspace_capacity(&current.principal))
    }
    fn smb_binding_status(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<crate::SmbBindingStatus, crate::ApiError> {
        let engine = self.engine.lock().expect("service state lock poisoned");
        current_protected_identity(&engine, identity)?;
        let repo = repository(&engine, id)?;
        let history = engine
            .state()
            .histories
            .get(&repo.history_tree_id)
            .ok_or_else(|| {
                ApiError::new(
                    ::http::StatusCode::INTERNAL_SERVER_ERROR,
                    storage_contract::ErrorCode::StateInvalid,
                    "repository history is missing",
                )
            })?;
        Ok(crate::SmbBindingStatus {
            repository_id: repo.repository_id.clone(),
            branch_id: repo.main_branch_id.clone(),
            enabled: repo.smb_enabled,
            history_generation: history.generation,
        })
    }
    fn update_smb_binding(
        &self,
        identity: &crate::auth::Identity,
        request: crate::UpdateSmbBinding<'_>,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        let mut engine = self.engine.lock().expect("service state lock poisoned");
        let current = current_protected_identity(&engine, identity)?;
        repository(&engine, request.repository)?;
        engine
            .set_smb_binding(
                request.repository,
                request.branch,
                request.enabled,
                request.expected_history_generation,
                &current.principal,
                request.operation,
            )
            .map_err(error)
    }
    fn protection_policy(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<storage_contract::ProtectionPolicy, crate::ApiError> {
        let engine = self.engine.lock().expect("service state lock poisoned");
        current_protected_identity(&engine, identity)?;
        let record = engine.state().protection_policies.get(id).ok_or_else(|| {
            ApiError::new(
                ::http::StatusCode::NOT_FOUND,
                storage_contract::ErrorCode::NotFound,
                "protection policy not found",
            )
        })?;
        repository(&engine, &record.repository_id)?;
        Ok(record.clone())
    }
    fn placement_rule(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<storage_contract::PlacementRule, crate::ApiError> {
        let engine = self.engine.lock().expect("service state lock poisoned");
        current_protected_identity(&engine, identity)?;
        let record = engine.state().placement_rules.get(id).ok_or_else(|| {
            ApiError::new(
                ::http::StatusCode::NOT_FOUND,
                storage_contract::ErrorCode::NotFound,
                "placement rule not found",
            )
        })?;
        repository(&engine, &record.repository_id)?;
        Ok(record.clone())
    }
    fn update_protection_policy(
        &self,
        identity: &crate::auth::Identity,
        request: crate::UpdateProtectionPolicy,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        let mut engine = self.engine.lock().expect("service state lock poisoned");
        let current = current_protected_identity(&engine, identity)?;
        repository(&engine, &request.repository_id)?;
        engine
            .update_protection_policy(storage_engine::ProtectionPolicyUpdate {
                policy_id: &request.policy_id,
                target_copies: request.target_copies,
                repository_id: &request.repository_id,
                path_prefix: &request.path_prefix,
                expected_generation: request.expected_generation,
                principal: &current.principal,
                operation_id: request.operation_id,
            })
            .map_err(error)
    }
    fn update_placement_rule(
        &self,
        identity: &crate::auth::Identity,
        request: crate::UpdatePlacementRule,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        let mut engine = self.engine.lock().expect("service state lock poisoned");
        let current = current_protected_identity(&engine, identity)?;
        repository(&engine, &request.repository_id)?;
        engine
            .update_placement_rule(storage_engine::PlacementRuleUpdate {
                rule_id: &request.rule_id,
                primary_slot_id: &request.primary_slot_id,
                repository_id: &request.repository_id,
                path_prefix: &request.path_prefix,
                expected_generation: request.expected_generation,
                principal: &current.principal,
                operation_id: request.operation_id,
            })
            .map_err(error)
    }
    fn compare_commits(
        &self,
        identity: &crate::auth::Identity,
        request: crate::CompareCommits<'_>,
    ) -> Result<storage_contract::CommitComparison, crate::ApiError> {
        let engine = self.engine.lock().expect("service state lock poisoned");
        current_protected_identity(&engine, identity)?;
        {
            let validate = |id: &str| {
                let commit = engine.state().commits.get(id).ok_or_else(|| {
                    ApiError::new(
                        ::http::StatusCode::NOT_FOUND,
                        storage_contract::ErrorCode::NotFound,
                        "comparison commit not found",
                    )
                })?;
                repo_for_history(&engine, &commit.history_tree_id)?;
                Ok::<_, ApiError>(())
            };
            validate(request.left)?;
            validate(request.right)?;
        }
        // Whole-commit comparison materializes both sides once in the engine.
        engine
            .compare_commits(request.left, request.right)
            .map_err(error)
    }
    fn disk_status(
        &self,
        identity: &crate::auth::Identity,
    ) -> Result<Vec<storage_contract::DiskStatus>, crate::ApiError> {
        let engine = self.engine.lock().expect("service state lock poisoned");
        current_protected_identity(&engine, identity)?;
        let observation = engine.prepare_disk_observation();
        drop(engine);
        observation.status().map_err(error)
    }
    fn integrity_check(
        &self,
        identity: &crate::auth::Identity,
    ) -> Result<storage_contract::CheckReport, crate::ApiError> {
        let engine = self.engine.lock().expect("service state lock poisoned");
        current_protected_identity(&engine, identity)?;
        engine.check().map_err(error)
    }
    fn create_merge_link(
        &self,
        identity: &crate::auth::Identity,
        request: crate::CreateMergeLink,
    ) -> Result<storage_contract::MergeLink, crate::ApiError> {
        let mut engine = self
            .engine
            .lock()
            .expect("invariant: service state lock poisoned");
        let current = current_protected_identity(&engine, identity)?;
        Application::new(&mut engine, &current).merge_link(request)
    }
    fn create_repository(
        &self,
        identity: &crate::auth::Identity,
        request: crate::CreateRepository,
    ) -> Result<storage_contract::Repository, crate::ApiError> {
        let mut engine = self
            .engine
            .lock()
            .expect("invariant: service state lock poisoned");
        let current = current_protected_identity(&engine, identity)?;
        engine
            .create_repository(
                &request.name,
                &request.primary_slot_id,
                request.target_copies,
                &current.principal,
                request.operation_id,
            )
            .map_err(error)
    }
    fn create_branch(
        &self,
        identity: &crate::auth::Identity,
        request: crate::CreateBranch,
    ) -> Result<crate::BranchSnapshot, crate::ApiError> {
        let mut engine = self
            .engine
            .lock()
            .expect("invariant: service state lock poisoned");
        let current = current_protected_identity(&engine, identity)?;
        let branch = Application::new(&mut engine, &current).create_branch(request)?;
        branch::capture_owned(&engine, branch).map_err(error)
    }
    fn create_file_branch(
        &self,
        identity: &Identity,
        request: crate::CreateFileBranch,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        let mut engine = self
            .engine
            .lock()
            .expect("invariant: service state lock poisoned");
        let current = current_protected_identity(&engine, identity)?;
        file_manager::file_repo(&engine, &request.file_id)?;
        match request.source_node_id.as_deref() {
            Some(node) => engine.create_file_branch_from_node(
                &request.file_id,
                &request.source_version_id,
                node,
                &request.name,
                &current.principal,
                request.operation_id,
            ),
            None => engine.create_file_branch(
                &request.file_id,
                &request.source_version_id,
                &request.name,
                &current.principal,
                request.operation_id,
            ),
        }
        .map_err(error)
    }
    fn update_note(
        &self,
        identity: &Identity,
        request: crate::UpdateNote,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        let mut engine = self
            .engine
            .lock()
            .expect("invariant: service state lock poisoned");
        let current = current_protected_identity(&engine, identity)?;
        match request.target {
            crate::NoteTarget::File(file) => {
                file_manager::file_repo(&engine, &file)?;
                engine.set_file_note(
                    &file,
                    None,
                    &request.note,
                    &current.principal,
                    request.operation_id,
                )
            }
            crate::NoteTarget::Version(version) => {
                let file = engine
                    .state()
                    .versions
                    .get(&version)
                    .ok_or_else(|| {
                        ApiError::new(
                            ::http::StatusCode::NOT_FOUND,
                            storage_contract::ErrorCode::VersionNotFound,
                            "version not found",
                        )
                    })?
                    .file_id
                    .clone();
                file_manager::file_repo(&engine, &file)?;
                engine.set_file_note(
                    &file,
                    Some(&version),
                    &request.note,
                    &current.principal,
                    request.operation_id,
                )
            }
            crate::NoteTarget::Node { file, node } => {
                file_manager::file_repo(&engine, &file)?;
                engine.set_file_node_note(
                    &file,
                    &node,
                    &request.note,
                    &current.principal,
                    request.operation_id,
                )
            }
        }
        .map_err(error)
    }
    fn merge_file_branches(
        &self,
        identity: &Identity,
        request: crate::MergeFileBranches,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        let mut engine = self
            .engine
            .lock()
            .expect("invariant: service state lock poisoned");
        let current = current_protected_identity(&engine, identity)?;
        application::Application::new(&mut engine, &current).merge_file_branches(request)
    }
    fn update_file_node_tags(
        &self,
        identity: &Identity,
        request: crate::UpdateFileNodeTags,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        let mut engine = self
            .engine
            .lock()
            .expect("invariant: service state lock poisoned");
        let current = current_protected_identity(&engine, identity)?;
        application::Application::new(&mut engine, &current).authorize_file(&request.file_id)?;
        engine
            .update_file_node_tags(storage_engine::FileNodeTagsUpdateRequest {
                file_id: &request.file_id,
                targets: &request.targets,
                add: &request.add,
                remove: &request.remove,
                location: request.location.as_ref(),
                principal: &current.principal,
                operation_id: request.operation_id,
            })
            .map_err(error)
    }
    fn create_disk_directory(
        &self,
        identity: &Identity,
        request: crate::CreateDiskDirectory,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        let mut engine = self
            .engine
            .lock()
            .expect("invariant: service state lock poisoned");
        let current = current_protected_identity(&engine, identity)?;
        file_manager::mkdir(&mut engine, &current, request)
    }
    fn create_disk_write_session(
        &self,
        identity: &Identity,
        request: crate::CreateDiskWriteSession,
    ) -> Result<crate::DiskWriteSession, ApiError> {
        let mut engine = self
            .engine
            .lock()
            .expect("invariant: service state lock poisoned");
        let current = current_protected_identity(&engine, identity)?;
        file_manager::write_session(&mut engine, &current, request)
    }
    fn prepare_historical_save(
        &self,
        identity: &Identity,
        request: Result<crate::PrepareHistoricalSave, crate::ApiError>,
    ) -> Result<storage_contract::Plan, ApiError> {
        let mut engine = self
            .engine
            .lock()
            .expect("invariant: service state lock poisoned");
        let current = current_protected_identity(&engine, identity)?;
        let request = request?;
        repo_for_history(&engine, &request.history_tree_id)?;
        engine
            .create_historical_save_plan(
                &request.history_tree_id,
                request.options?,
                &current.principal,
            )
            .map_err(error)
    }
    fn prepare_purge(
        &self,
        identity: &Identity,
        request: Result<crate::PreparePurge, crate::ApiError>,
    ) -> Result<storage_contract::Plan, ApiError> {
        let mut engine = self
            .engine
            .lock()
            .expect("invariant: service state lock poisoned");
        let current = current_protected_identity(&engine, identity)?;
        let request = request?;
        repo_for_history(&engine, &request.history_tree_id)?;
        engine
            .create_purge_plan(
                &request.history_tree_id,
                request.version_ids?,
                &current.principal,
            )
            .map_err(error)
    }
    fn prepare_split(
        &self,
        identity: &Identity,
        request: Result<crate::PrepareSplit, crate::ApiError>,
    ) -> Result<storage_contract::Plan, ApiError> {
        let mut engine = self
            .engine
            .lock()
            .expect("invariant: service state lock poisoned");
        let current = current_protected_identity(&engine, identity)?;
        let request = request?;
        repo_for_history(&engine, &request.history_tree_id)?;
        engine
            .create_split_plan(
                &request.history_tree_id,
                &request.cut_commit_id?,
                &current.principal,
            )
            .map_err(error)
    }
    fn backup_configuration(
        &self,
        identity: &Identity,
        request: crate::ReadBackupConfiguration,
    ) -> Result<storage_contract::BackupConfigurationSnapshot, ApiError> {
        let engine = self
            .engine
            .lock()
            .expect("invariant: service state lock poisoned");
        current_protected_identity(&engine, identity)?;
        repository(&engine, &request.repository_id)?;
        engine
            .backup_configuration(&request.repository_id, &request.path)
            .map_err(error)
    }
    fn configure_backups(
        &self,
        identity: &Identity,
        request: crate::ConfigureBackups,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        let mut engine = self
            .engine
            .lock()
            .expect("invariant: service state lock poisoned");
        let current = current_protected_identity(&engine, identity)?;
        repository(&engine, &request.repository_id)?;
        engine
            .configure_backups_scoped(storage_engine::ScopedBackupConfigurationRequest {
                repository_id: &request.repository_id,
                path: &request.path,
                expected_scope_id: Some(&request.expected_scope_id),
                target_copies: request.target_copies,
                backup_prefixes: &request.backup_prefixes,
                expected_generation: request.expected_generation,
                principal: &current.principal,
                operation_id: request.operation_id,
            })
            .map_err(|failure| match failure.code() {
                storage_contract::ErrorCode::RouteIncomplete
                | storage_contract::ErrorCode::ScopeChanged => {
                    ApiError::conflict(failure.code(), &failure.message()).with_source(failure)
                }
                _ => error(failure),
            })
    }
    fn namespace_batch(
        &self,
        identity: &Identity,
        request: crate::NamespaceBatch,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        let mut engine = self
            .engine
            .lock()
            .expect("invariant: service state lock poisoned");
        let current = current_protected_identity(&engine, identity)?;
        application::Application::new(&mut engine, &current).namespace_batch(request)
    }
    fn select_file_branch(
        &self,
        identity: &Identity,
        request: crate::SelectFileBranch,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        let mut engine = self
            .engine
            .lock()
            .expect("invariant: service state lock poisoned");
        let current = current_protected_identity(&engine, identity)?;
        let repository = file_manager::file_repo(&engine, &request.file_id)?;
        match request.location {
            Some(location) => {
                if location.repository_id != repository.repository_id {
                    return Err(ApiError::new(
                        ::http::StatusCode::NOT_FOUND,
                        storage_contract::ErrorCode::FileNotFound,
                        "file is not in this repository",
                    ));
                }
                engine.select_file_branch_at(
                    &location.repository_id,
                    &location.path,
                    &request.file_id,
                    &request.branch_id,
                    &current.principal,
                    request.operation_id,
                )
            }
            None => engine.select_file_branch(
                &request.file_id,
                &request.branch_id,
                &current.principal,
                request.operation_id,
            ),
        }
        .map_err(error)
    }
    fn rename_file_branch(
        &self,
        identity: &Identity,
        request: crate::RenameFileBranch,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        let mut engine = self
            .engine
            .lock()
            .expect("invariant: service state lock poisoned");
        let current = current_protected_identity(&engine, identity)?;
        file_manager::file_repo(&engine, &request.file_id)?;
        engine
            .rename_file_branch(
                &request.file_id,
                &request.branch_id,
                &request.name,
                &current.principal,
                request.operation_id,
            )
            .map_err(error)
    }
    fn listing(
        &self,
        identity: &Identity,
        request: crate::ReadListing,
    ) -> Result<crate::ListingSnapshot, ApiError> {
        let engine = self
            .engine
            .lock()
            .expect("invariant: service state lock poisoned");
        current_protected_identity(&engine, identity)?;
        let mut snapshot = file_manager::listing::entries(&engine, request)?;
        drop(engine);
        directory_scopes::sort_choices(&mut snapshot.directory_branches);
        Ok(snapshot)
    }
    fn mutate_disk_directory(
        &self,
        identity: &Identity,
        request: crate::MutateDiskDirectory,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        let mut engine = self
            .engine
            .lock()
            .expect("invariant: service state lock poisoned");
        let current = current_protected_identity(&engine, identity)?;
        file_manager::mutate_directory(&mut engine, &current, request)
    }
    fn initialize_directory_scope(
        &self,
        identity: &Identity,
        request: crate::InitializeDirectoryScope,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        let mut engine = self
            .engine
            .lock()
            .expect("invariant: service state lock poisoned");
        let current = current_protected_identity(&engine, identity)?;
        repository(&engine, &request.repository_id)?;
        engine
            .initialize_directory_scope(
                &request.repository_id,
                request.path.as_ref(),
                &current.principal,
                request.operation_id,
            )
            .map_err(error)
    }
    fn archive_branch(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ArchiveBranch,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        let mut engine = self
            .engine
            .lock()
            .expect("invariant: service state lock poisoned");
        let current = current_protected_identity(&engine, identity)?;
        repo_for_branch(&engine, &request.branch_id)?;
        engine
            .archive_branch(&request.branch_id, &current.principal, request.operation_id)
            .map_err(error)
    }
    fn rename_directory_branch(
        &self,
        identity: &crate::auth::Identity,
        request: crate::RenameDirectoryBranch,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        let mut engine = self
            .engine
            .lock()
            .expect("invariant: service state lock poisoned");
        let current = current_protected_identity(&engine, identity)?;
        repository(&engine, &request.repository_id)?;
        engine
            .rename_directory_branch(
                &request.repository_id,
                &request.directory_branch_id,
                &request.name,
                &current.principal,
                request.operation_id,
            )
            .map_err(error)
    }
    fn select_default_directory_branch(
        &self,
        identity: &crate::auth::Identity,
        request: crate::SelectDefaultDirectoryBranch,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        let mut engine = self
            .engine
            .lock()
            .expect("invariant: service state lock poisoned");
        let current = current_protected_identity(&engine, identity)?;
        repository(&engine, &request.repository_id)?;
        engine
            .select_default_directory_branch(
                &request.repository_id,
                &request.directory_branch_id,
                &current.principal,
                request.operation_id,
            )
            .map_err(error)
    }
    fn directory_selection(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadDirectorySelection,
    ) -> Result<crate::DirectorySelectionSnapshot, crate::ApiError> {
        let engine = self
            .engine
            .lock()
            .expect("invariant: service state lock poisoned");
        current_protected_identity(&engine, identity)?;
        directory_selection::read(&engine, &request)
    }
    fn directory_scopes(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadDirectoryScopes<'_>,
    ) -> Result<crate::DirectoryScopesSnapshot, crate::ApiError> {
        let engine = self
            .engine
            .lock()
            .expect("invariant: service state lock poisoned");
        current_protected_identity(&engine, identity)?;
        let mut snapshot = directory_scopes::capture(&engine, request)?;
        drop(engine);
        directory_scopes::sort(&mut snapshot);
        Ok(snapshot)
    }
    fn default_directory_branch(
        &self,
        identity: &crate::auth::Identity,
        repository_id: &str,
    ) -> Result<crate::DefaultDirectoryBranchSnapshot, crate::ApiError> {
        let engine = self
            .engine
            .lock()
            .expect("invariant: service state lock poisoned");
        current_protected_identity(&engine, identity)?;
        let repo = repository(&engine, repository_id)?;
        let state = engine.state();
        let selected = state.directory_selections.get(repository_id).map(|id| {
            let branch = state
                .directory_branches
                .get(id)
                .expect("invariant: admitted directory selection references its branch");
            crate::SelectedDirectoryBranch {
                directory_branch_id: id.clone(),
                name: branch.name.clone(),
            }
        });
        Ok(crate::DefaultDirectoryBranchSnapshot {
            repository_id: repo.repository_id.clone(),
            namespace_branch_id: repo.main_branch_id.clone(),
            selected,
            directory_branches: storage_contract::directory_branch_summaries(
                state,
                storage_contract::DirectoryBranchScope {
                    repository_id,
                    directory_id: repository_id,
                    path: "",
                },
            ),
        })
    }
    fn file_history_snapshot(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadFileHistory,
    ) -> Result<crate::FileHistoryPage, crate::ApiError> {
        let engine = self
            .engine
            .lock()
            .expect("invariant: service state lock poisoned");
        current_protected_identity(&engine, identity)?;
        file_manager::history_pages::file_history(&engine, request)
    }
    fn file_history_node_parents(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadNodeParents,
    ) -> Result<crate::NodeParentsPage, crate::ApiError> {
        let engine = self.engine.lock().expect("service state lock poisoned");
        current_protected_identity(&engine, identity)?;
        file_manager::history_pages::node_parents(&engine, request)
    }
    fn file_history_branches(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadHistoryBranches,
    ) -> Result<crate::HistoryBranchesPage, crate::ApiError> {
        let engine = self.engine.lock().expect("service state lock poisoned");
        current_protected_identity(&engine, identity)?;
        file_manager::history_pages::branches(&engine, request)
    }
    fn file_history_merges(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadHistoryMerges,
    ) -> Result<crate::HistoryMergesPage, crate::ApiError> {
        let engine = self.engine.lock().expect("service state lock poisoned");
        current_protected_identity(&engine, identity)?;
        let file_id = request.0.file_id.as_str().to_owned();
        let plan = file_manager::history_pages::prepare_merge_page(&engine, request)?;
        drop(engine);
        self.finish_file_history_page(identity, &file_id, Ok(plan.read()))
    }
    fn file_history_merge_inputs(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadMergeInputs,
    ) -> Result<crate::MergeInputsPage, crate::ApiError> {
        let engine = self.engine.lock().expect("service state lock poisoned");
        current_protected_identity(&engine, identity)?;
        file_manager::history_pages::merge_inputs(&engine, &request.0)
    }
    fn file_history_merge_parents(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadMergeParents,
    ) -> Result<crate::MergeParentsPage, crate::ApiError> {
        let engine = self.engine.lock().expect("service state lock poisoned");
        current_protected_identity(&engine, identity)?;
        file_manager::history_pages::merge_parents(&engine, &request.0)
    }
    fn file_history_node_field(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadNodeField,
    ) -> Result<crate::NodeFieldSlice, crate::ApiError> {
        let engine = self.engine.lock().expect("service state lock poisoned");
        current_protected_identity(&engine, identity)?;
        file_manager::history_pages::node_field(&engine, request)
    }
    fn file_version_page(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadVersionPage,
    ) -> Result<crate::VersionPage, crate::ApiError> {
        let engine = self.engine.lock().expect("service state lock poisoned");
        current_protected_identity(&engine, identity)?;
        let file_id = request.file_id.as_str().to_owned();
        let plan = file_manager::history_pages::prepare_version_page(&engine, request)?;
        drop(engine);
        self.finish_file_history_page(identity, &file_id, plan.read())
    }
    fn file_version_sources(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadVersionSources,
    ) -> Result<crate::VersionSourcesPage, crate::ApiError> {
        let engine = self.engine.lock().expect("service state lock poisoned");
        current_protected_identity(&engine, identity)?;
        file_manager::history_pages::version_sources(&engine, &request.0)
    }
    fn topology_snapshot(
        &self,
        identity: &crate::auth::Identity,
    ) -> Result<crate::TopologySnapshot, crate::ApiError> {
        let engine = self.engine.lock().expect("service state lock poisoned");
        current_protected_identity(&engine, identity)?;
        let slots = engine
            .config()
            .disks
            .iter()
            .map(|disk| {
                let storage_contract::DiskConfig {
                    slot_id,
                    disk_id,
                    root: _,
                    level,
                    upstream_slot_id,
                    metadata_member,
                    binding_generation,
                    capacity_bytes: _,
                } = disk;
                crate::TopologySlot {
                    slot_id: slot_id.clone(),
                    disk_id: disk_id.clone(),
                    level: *level,
                    upstream_slot_id: upstream_slot_id.clone(),
                    metadata_member: *metadata_member,
                    binding_generation: *binding_generation,
                }
            })
            .collect();
        Ok(crate::TopologySnapshot {
            generation: engine.state().topology_generation,
            slots,
        })
    }
    fn placement_snapshot(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<storage_contract::Placement, crate::ApiError> {
        let engine = self.engine.lock().expect("service state lock poisoned");
        current_protected_identity(&engine, identity)?;
        authorized_version(&engine, id)?;
        engine.state().placements.get(id).cloned().ok_or_else(|| {
            ApiError::new(
                ::http::StatusCode::NOT_FOUND,
                storage_contract::ErrorCode::NotFound,
                "placement not found",
            )
        })
    }
    fn event_page(
        &self,
        identity: &crate::auth::Identity,
        after: u64,
    ) -> Result<crate::EventPage, crate::ApiError> {
        let engine = self.engine.lock().expect("service state lock poisoned");
        current_protected_identity(&engine, identity)?;
        if after > engine.state().sequence {
            return Err(ApiError::new(
                ::http::StatusCode::BAD_REQUEST,
                storage_contract::ErrorCode::InvalidCursor,
                "event cursor is ahead of the published sequence",
            ));
        }
        let events = engine.events_page(after, 1000).map_err(error)?;
        Ok(capture_event_page(events, after))
    }
    fn activity_history(
        &self,
        identity: &Identity,
    ) -> Result<crate::ActivityHistorySnapshot, ApiError> {
        let engine = self.engine.lock().expect("service state lock poisoned");
        current_protected_identity(&engine, identity)?;
        file_manager::activity::read(&engine)
    }
    fn history_graph(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadHistoryGraph,
        control: &crate::GraphReadControl,
    ) -> Result<crate::HistoryGraphResponse, crate::ApiError> {
        graph::read(self, identity, request, control)
    }
    fn commit_snapshot(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<std::sync::Arc<storage_contract::Commit>, crate::ApiError> {
        let engine = self.engine.lock().expect("service state lock poisoned");
        current_protected_identity(&engine, identity)?;
        let commit = engine.state().commits.get_shared(id).ok_or_else(|| {
            ApiError::new(
                ::http::StatusCode::NOT_FOUND,
                storage_contract::ErrorCode::NotFound,
                "commit not found",
            )
        })?;
        repo_for_history(&engine, &commit.history_tree_id)?;
        Ok(commit)
    }
    fn resolve_entry(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ResolveEntry<'_>,
    ) -> Result<crate::ResolvedEntry, crate::ApiError> {
        let engine = self.engine.lock().expect("service state lock poisoned");
        current_protected_identity(&engine, identity)?;
        read_projection::resolve_entry(&engine, request)
    }
    fn branch_snapshot(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<crate::BranchSnapshot, crate::ApiError> {
        let engine = self.engine.lock().expect("service state lock poisoned");
        current_protected_identity(&engine, identity)?;
        repo_for_branch(&engine, id)?;
        branch::capture(&engine, &engine.state().branches[id]).map_err(error)
    }
    fn branches_snapshot(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<Vec<crate::BranchSnapshot>, crate::ApiError> {
        let engine = self.engine.lock().expect("service state lock poisoned");
        current_protected_identity(&engine, identity)?;
        let repo = repository(&engine, id)?;
        engine
            .state()
            .branches
            .values()
            .filter(|branch| branch.history_tree_id == repo.history_tree_id)
            .map(|branch| branch::capture(&engine, branch).map_err(error))
            .collect()
    }
    fn repositories_snapshot(
        &self,
        identity: &crate::auth::Identity,
    ) -> Result<Vec<storage_contract::Repository>, crate::ApiError> {
        let engine = self.engine.lock().expect("service state lock poisoned");
        current_protected_identity(&engine, identity)?;
        Ok(engine.state().repositories.values().cloned().collect())
    }
    fn tree_snapshot(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadTree<'_>,
    ) -> Result<crate::TreeSnapshot, crate::ApiError> {
        let engine = self.engine.lock().expect("service state lock poisoned");
        current_protected_identity(&engine, identity)?;
        crate::TreeSnapshot::capture(&engine, request)
    }
    fn create_write_session(
        &self,
        identity: &crate::auth::Identity,
        branch: &str,
    ) -> Result<crate::WorkspaceWriteSession, crate::ApiError> {
        let mut engine = self.engine.lock().expect("service state lock poisoned");
        let current = current_protected_identity(&engine, identity)?;
        repo_for_branch(&engine, branch)?;
        let workspace = engine
            .create_workspace(branch, &current.principal)
            .map_err(error)?;
        let expires_at = workspace
            .created_at
            .saturating_add(engine.config().workspace_ttl_seconds);
        Ok(crate::WorkspaceWriteSession {
            workspace_id: workspace.workspace_id,
            branch_id: workspace.branch_id,
            principal: workspace.principal,
            fencing_token: workspace.fencing_token,
            expires_at,
            state: workspace.state,
        })
    }
    fn stage_namespace(
        &self,
        identity: &Identity,
        command: crate::StageNamespace,
    ) -> Result<storage_contract::Workspace, ApiError> {
        let mut engine = self.engine.lock().expect("service state lock poisoned");
        let current = current_protected_identity(&engine, identity)?;
        Application::new(&mut engine, &current).stage_namespace(command)
    }

    fn authorize_upload(
        &self,
        identity: &Identity,
        workspace: &str,
        logical_size: Option<u64>,
    ) -> Result<(), ApiError> {
        let engine = self.engine.lock().expect("service state lock poisoned");
        let current = current_protected_identity(&engine, identity)?;
        let workspace = workspace_authorize(&engine, &current, workspace)?;
        if workspace.state != storage_contract::WorkspaceState::Open {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::WorkspaceClosed,
                "workspace is no longer open for upload",
            ));
        }
        let now = upload_time(std::time::SystemTime::now())?;
        if now.saturating_sub(workspace.created_at) > engine.config().workspace_ttl_seconds {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::LeaseExpired,
                "workspace lease expired; retained data remains available",
            ));
        }
        if logical_size.is_some_and(|size| {
            size > engine.config().max_file_bytes || size > engine.config().max_workspace_bytes
        }) {
            return Err(ApiError::new(
                ::http::StatusCode::PAYLOAD_TOO_LARGE,
                storage_contract::ErrorCode::ResourceLimitExceeded,
                "declared upload exceeds the pool file or workspace limit",
            ));
        }
        Ok(())
    }

    fn read_directory(
        &self,
        identity: &Identity,
        request: crate::ReadDirectory<'_>,
    ) -> Result<crate::DirectorySnapshot, ApiError> {
        let engine = self.engine.lock().expect("service state lock poisoned");
        current_protected_identity(&engine, identity)?;
        let branch = request.branch;
        match read_projection::DirectoryRead::capture(&engine, request)? {
            read_projection::DirectoryRead::Captured(snapshot) => Ok(snapshot),
            read_projection::DirectoryRead::Page(plan) => {
                drop(engine);
                self.finish_directory_page(identity, branch, plan.finish())
            }
        }
    }

    fn authorize_workspace_selection(
        &self,
        identity: &Identity,
        workspace: &str,
        path: &name_policy::PortablePath,
    ) -> Result<(), ApiError> {
        let engine = self.engine.lock().expect("service state lock poisoned");
        let current = current_protected_identity(&engine, identity)?;
        workspace_authorize(&engine, &current, workspace)?;
        engine
            .validate_workspace_file_selection(workspace, path)
            .map_err(error)
    }

    fn authorize_current(&self, identity: &Identity, branch: &str) -> Result<(), ApiError> {
        let engine = self.engine.lock().expect("service state lock poisoned");
        current_protected_identity(&engine, identity)?;
        read_projection::authorize_current(engine.state(), branch)
    }

    fn lookup_current(
        &self,
        identity: &Identity,
        branch: &str,
        path: &name_policy::LookupPath,
    ) -> Result<Option<storage_contract::Entry>, ApiError> {
        let engine = self.engine.lock().expect("service state lock poisoned");
        current_protected_identity(&engine, identity)?;
        read_projection::lookup_current(&engine, branch, path)
    }

    fn lookup_at(
        &self,
        identity: &Identity,
        branch: &str,
        commit: &str,
        path: &name_policy::LookupPath,
    ) -> Result<Option<storage_contract::Entry>, ApiError> {
        let engine = self.engine.lock().expect("service state lock poisoned");
        current_protected_identity(&engine, identity)?;
        let (_, snapshot) = read_projection::branch_snapshot(engine.state(), branch, Some(commit))?;
        // One indexed point read replaces the former sorted-list search.
        engine
            .commit_namespace(&snapshot.commit_id)
            .expect("invariant: validated snapshot commit exists")
            .get_key(path.key())
            .map_err(error)
    }

    fn plan(&self, identity: &Identity, plan_id: &str) -> Result<storage_contract::Plan, ApiError> {
        let engine = self.engine.lock().expect("service state lock poisoned");
        let identity = current_protected_identity(&engine, identity)?;
        let plan = engine.plan(plan_id).map_err(error)?;
        if plan.principal != identity.principal {
            return Err(ApiError::forbidden());
        }
        repo_for_history(&engine, field(&plan.input, "history_tree_id")?)?;
        Ok(plan)
    }
    fn cancel_plan(
        &self,
        identity: &Identity,
        request: Result<crate::CancelPlan, crate::ApiError>,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        let mut engine = self.engine.lock().expect("service state lock poisoned");
        let identity = current_protected_identity(&engine, identity)?;
        let request = request?;
        let plan = engine.plan(&request.plan_id).map_err(error)?;
        if plan.principal != identity.principal {
            return Err(ApiError::forbidden());
        }
        repo_for_history(&engine, field(&plan.input, "history_tree_id")?)?;
        engine
            .cancel_plan(&request.plan_id, &identity.principal, request.operation_id?)
            .map_err(error)
    }

    fn register_disk(
        &self,
        identity: &crate::auth::Identity,
    ) -> Result<std::convert::Infallible, crate::ApiError> {
        let engine = self
            .engine
            .lock()
            .expect("invariant: service state lock poisoned");
        current_protected_identity(&engine, identity)?;
        Err(ApiError::new(
            ::http::StatusCode::CONFLICT,
            storage_contract::ErrorCode::OfflineConfigurationRequired,
            "disk registration requires an explicit offline configuration and topology migration",
        ))
    }
    fn commit_plan(
        &self,
        identity: &Identity,
        request: Result<crate::CommitPlan, ApiError>,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        let mut engine = self
            .engine
            .lock()
            .expect("invariant: service state lock poisoned");
        let identity = current_protected_identity(&engine, identity)?;
        Application::new(&mut engine, &identity).commit_plan(request?)
    }

    fn upload(
        &self,
        identity: &Identity,
        workspace: &str,
        item: &str,
        path: &name_policy::PortablePath,
        source: &crate::UploadSource,
    ) -> Result<storage_contract::StagedFile, ApiError> {
        let engine = self.engine.lock().expect("service state lock poisoned");
        let current = current_protected_identity(&engine, identity)?;
        let workspace_record = workspace_authorize(&engine, &current, workspace)?;
        let generation = workspace_record
            .files
            .get(item)
            .map(|file| file.dirty_generation.checked_add(1))
            .unwrap_or(Some(1))
            .ok_or_else(|| {
                error(storage_engine::Error::new(
                    storage_contract::ErrorCode::ResourceLimit,
                    "upload generation exhausted",
                ))
            })?;
        let plan = engine
            .prepare_upload_file(
                workspace,
                item,
                path,
                source.file.try_clone().map_err(ApiError::io)?,
                generation,
                Some((source.size, source.hash)),
            )
            .map_err(error)?;
        drop(engine);
        let prepared = plan.execute().map_err(error)?;
        let mut engine = self.engine.lock().expect("service state lock poisoned");
        finish_authorized_upload(&mut engine, identity, workspace, prepared)
    }
    fn upload_generation(
        &self,
        identity: &Identity,
        workspace: &str,
        item: &str,
        path: &name_policy::PortablePath,
        source: &crate::UploadSource,
        generation: u64,
    ) -> Result<storage_contract::StagedFile, ApiError> {
        let engine = self.engine.lock().expect("service state lock poisoned");
        let current = current_protected_identity(&engine, identity)?;
        workspace_authorize(&engine, &current, workspace)?;
        let plan = engine
            .prepare_upload_file(
                workspace,
                item,
                path,
                source.file.try_clone().map_err(ApiError::io)?,
                generation,
                Some((source.size, source.hash)),
            )
            .map_err(error)?;
        drop(engine);
        let prepared = plan.execute().map_err(error)?;
        let mut engine = self.engine.lock().expect("service state lock poisoned");
        // The session or SMB binding may have changed while bytes were copied.
        finish_authorized_upload(&mut engine, identity, workspace, prepared)
    }
    fn content(&self, identity: &Identity, version: &str) -> Result<Content, ApiError> {
        self.open_version_content(
            identity,
            crate::ReadContent {
                version,
                context: None,
            },
        )?
        .fully_verified()
    }
    fn range_content(
        &self,
        identity: &Identity,
        request: crate::ReadContent<'_>,
    ) -> Result<Content, ApiError> {
        self.open_version_content(identity, request)
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
    fn observe_disks(
        &self,
        _: &crate::InternalDiskObservation,
    ) -> Result<Vec<crate::DiskSample>, ApiError> {
        let observation = {
            let engine = self.engine.lock().expect("service state lock poisoned");
            engine.prepare_disk_observation()
        };
        Ok(disk_monitor::observe(observation))
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

fn upload_time(time: std::time::SystemTime) -> Result<u64, ApiError> {
    time.duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|source| {
            ApiError::new(
                ::http::StatusCode::INTERNAL_SERVER_ERROR,
                storage_contract::ErrorCode::IoError,
                "system clock precedes the Unix epoch",
            )
            .with_source(source)
        })
}
