//! Authorization and wire adaptation for the storage engine.
use crate::{ApiError, Backend, Content, auth::Identity};
use json_codec::Value;
#[cfg(test)]
use json_codec::json;
use std::{
    path::Path,
    sync::{Arc, Mutex},
};
use storage_contract::{ContentState, FileVersion, VersionId};
use storage_engine::{Engine, NamespaceOperation, Repository};
mod admission;
use admission::AdmissionEngine;
pub mod application;
use application::Application;
mod backup_transaction;
mod branch;
mod content;
mod container_drafts;
mod directory_deletion;
mod directory_snapshot_source;
mod directory_scopes;
mod directory_selection;
mod disk_monitor;
mod error_registry;
mod file_deletion;
mod file_history_move;
mod file_manager;
mod foreign_identity;
mod foreign_intent;
pub(crate) mod foreign_plan;
mod graph;
mod integrity_scrub;
mod node_retirement;
mod operations;
mod ordinary_conversion;
mod read_projection;
mod storage_addition;
pub(crate) mod storage_lifecycle;
pub(crate) mod storage_readmission;
mod wire;
use wire::{current_entry_source, field};
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
            "recovery seal verification failed; recovery evidence remains restricted"
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
    terminal_exit: Mutex<Option<storage_engine::TerminalDiskExit>>,
    #[cfg(test)]
    commit_preparation_hook: Mutex<Option<std::sync::Arc<dyn Fn() + Send + Sync>>>,
    #[cfg(test)]
    operation_health_hook: Mutex<Option<std::sync::Arc<dyn Fn() + Send + Sync>>>,
}
impl EngineBackend {
    /// Keep the current credential and SMB-binding proof inside the same engine
    /// ownership interval as the caller's business checks and publication.
    fn lock_current(
        &self,
        identity: &Identity,
    ) -> Result<(std::sync::MutexGuard<'_, AdmissionEngine>, Identity), ApiError> {
        let engine = self
            .engine
            .lock()
            .expect("invariant: service state lock poisoned");
        let current = current_protected_identity(&engine, identity)?;
        Ok((engine, current))
    }

    pub fn new(mut engine: Engine) -> Self {
        report_recovery_warnings(&mut engine);
        Self {
            engine: Mutex::new(AdmissionEngine::new(engine)),
            terminal_exit: Mutex::new(None),
            #[cfg(test)]
            commit_preparation_hook: Mutex::new(None),
            #[cfg(test)]
            operation_health_hook: Mutex::new(None),
        }
    }
    /// Admit one current registration while holding its configuration identity.
    /// Disk-card reads clone the small registration and observe outside this lock;
    /// note writes retain it only through their system-disk atomic replacement.
    pub(crate) fn with_disk_registration<T>(
        &self,
        identity: &Identity,
        slot: &str,
        action: impl FnOnce(
            &storage_contract::DiskConfig,
            &storage_engine::Engine,
        ) -> Result<T, ApiError>,
    ) -> Result<T, ApiError> {
        let (engine, _) = self.lock_current(identity)?;
        let registration = engine
            .registered_disks()
            .find(|disk| disk.slot_id.eq_ignore_ascii_case(slot))
            .ok_or_else(|| {
                ApiError::new(
                    http::StatusCode::NOT_FOUND,
                    storage_contract::ErrorCode::NotFound,
                    "registered disk slot not found",
                )
            })?;
        let disk = engine
            .config()
            .disks
            .iter()
            .find(|disk| disk.disk_id == registration.disk_id)
            .expect("invariant: current registration retains its historical definition");
        action(disk, &engine)
    }
    #[cfg(test)]
    pub(crate) fn test_with_engine_locked<T>(&self, f: impl FnOnce(&mut Engine) -> T) -> T {
        let mut engine = self.engine.lock().unwrap();
        f(&mut engine)
    }
    /// Managed dispatch linearizes admission after acquiring the engine mutex.
    /// An operation already admitted may finish; queued requests see revocation.
    /// Bind the existing primary credential authority for native dispatch. This
    /// does not authenticate a network caller or create a management session.
    /// The caller retains the AuthStore/engine owners for the full operation.
    pub fn set_management_auth(&self, auth: crate::auth::AuthReader) -> Result<(), ApiError> {
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
                storage_contract::OperationId::generate().map_err(ApiError::contract_input)?,
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
            backend.replicate_auth(snapshot)
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
            storage_contract::OperationId::generate().unwrap(),
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
pub(crate) fn capture_event_page<'a>(
    events: impl Iterator<Item = &'a storage_contract::Event>,
    published: u64,
    after: u64,
) -> Result<crate::EventPage, ApiError> {
    if after > published {
        return Err(ApiError::new(
            ::http::StatusCode::BAD_REQUEST,
            storage_contract::ErrorCode::InvalidCursor,
            "event cursor is ahead of the published sequence",
        ));
    }
    let events: Vec<_> = events.take(1000).cloned().collect();
    let next_cursor = events.last().map_or(after, |event| event.sequence);
    Ok(crate::EventPage {
        events,
        next_cursor,
    })
}
fn repo_for_history<'a>(engine: &'a Engine, id: &str) -> Result<&'a Repository, ApiError> {
    repo_for_history_in_state(engine.state(), id)
}
pub(crate) fn repo_for_history_in_state<'a>(
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
) -> Result<(&'a FileVersion, Option<ContentState>), ApiError> {
    // This is the single Backend text -> Engine ID boundary, after
    // lock_current. Noncanonical text remains indistinguishable from absence.
    let (version, disposition) = id
        .parse::<VersionId>()
        .ok()
        .and_then(|id| engine.version_metadata(id))
        .filter(|(_, state)| *state != Some(ContentState::Retired))
        .ok_or_else(|| {
            ApiError::new(
                ::http::StatusCode::NOT_FOUND,
                storage_contract::ErrorCode::NotFound,
                "version not found",
            )
        })?;
    repo_for_history(engine, &version.history_tree_id)?;
    Ok((version, disposition))
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
        let (engine, current) = self.lock_current(identity)?;
        operations::authorize_record(&engine, &current, &record)?;
        drop(engine);
        Ok(crate::OperationObservation {
            outcome: record.into_result().into(),
            health,
        })
    }
}

impl Backend for EngineBackend {
    fn directory_snapshot_source(
        &self,
        identity: &Identity,
        request: crate::ReadDirectorySnapshotSource,
        cancellation: storage_contract::history_window::HistoryWindowCancellation,
    ) -> Result<crate::DirectorySnapshotSourcePreview, ApiError> {
        directory_snapshot_source::read(self, identity, request, cancellation)
    }

    fn operation(
        &self,
        identity: &Identity,
        operation_id: storage_contract::OperationId,
    ) -> Result<crate::OperationObservation, ApiError> {
        let (engine, current) = self.lock_current(identity)?;
        let identity = &current;
        self.operation_admitted(engine, identity, operation_id)
    }

    fn operation_outcome(
        &self,
        identity: &Identity,
        operation_id: storage_contract::OperationId,
    ) -> Result<storage_contract::OperationOutcome, ApiError> {
        let (engine, current) = self.lock_current(identity)?;
        let record = operations::authorized_record(&engine, &current, operation_id)?;
        Ok(record.into_result().into())
    }

    fn commit_workspace(
        &self,
        identity: &Identity,
        command: crate::CommitWorkspace,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        let (mut engine, current) = self.lock_current(identity)?;
        let identity = &current;
        let workspace_id = command.workspace_id.clone();
        let plan = Application::new(&mut engine, identity).prepare_commit_workspace(command)?;
        drop(engine);
        #[cfg(test)]
        if let Some(hook) = self.commit_preparation_hook.lock().unwrap().clone() {
            hook();
        }
        let prepared = plan.execute().map_err(error)?;
        let (mut engine, current) = self.lock_current(identity)?;
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
        let (engine, _) = self.lock_current(identity)?;
        let (commit, _) = read_projection::branch_snapshot(engine.state(), branch, Some(commit))?;
        Ok(engine
            .state()
            .commits
            .get_shared(commit)
            .expect("invariant: admitted commit exists under the engine lock"))
    }

    fn create_workspace(
        &self,
        identity: &Identity,
        command: crate::CreateWorkspace,
    ) -> Result<storage_contract::Workspace, ApiError> {
        let (mut engine, current) = self.lock_current(identity)?;
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
        let (mut engine, current) = self.lock_current(identity)?;
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
        let (engine, current) = self.lock_current(identity)?;
        workspace_authorize(&engine, &current, workspace).map(std::borrow::Cow::into_owned)
    }

    fn require_native_workspace_commit_completion(
        &self,
        identity: &Identity,
        command: crate::CommitWorkspace,
    ) -> Result<(), ApiError> {
        let (engine, current) = self.lock_current(identity)?;
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
        let (engine, current) = self.lock_current(identity)?;
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
        let (mut engine, current) = self.lock_current(identity)?;
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
        let (mut engine, current) = self.lock_current(identity)?;
        let workspace_id = command.workspace_id().to_owned();
        let plan =
            Application::new(&mut engine, &current).prepare_commit_workspace_public(command)?;
        drop(engine);
        #[cfg(test)]
        if let Some(hook) = self.commit_preparation_hook.lock().unwrap().clone() {
            hook();
        }
        let prepared = plan.execute().map_err(error)?;
        let (mut engine, current) = self.lock_current(identity)?;
        Application::new(&mut engine, &current).finish_workspace_commit(&workspace_id, prepared)
    }

    fn version_snapshot(
        &self,
        identity: &crate::auth::Identity,
        version: &str,
    ) -> Result<crate::VersionSnapshot, crate::ApiError> {
        let (engine, _) = self.lock_current(identity)?;
        let (version, disposition) = authorized_version(&engine, version)?;
        let content_state = wire::stored_version_content_state(disposition.as_ref());
        Ok(crate::VersionSnapshot {
            version: version.clone(),
            content_state,
        })
    }
    fn authorize_version(
        &self,
        identity: &crate::auth::Identity,
        version: &str,
    ) -> Result<(), crate::ApiError> {
        let (engine, current) = self.lock_current(identity)?;
        let (version, _) = authorized_version(&engine, version)?;
        if current.smb_binding.is_some() && engine.file_deletion(&version.file_id).is_some() {
            return Err(ApiError::new(
                http::StatusCode::NOT_FOUND,
                storage_contract::ErrorCode::FileNotFound,
                "file is deleted",
            ));
        }
        Ok(())
    }
    fn repository_snapshot(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<storage_contract::Repository, crate::ApiError> {
        let (engine, _) = self.lock_current(identity)?;
        Ok(repository(&engine, id)?.clone())
    }
    fn authorize_repository(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<(), crate::ApiError> {
        let (engine, _) = self.lock_current(identity)?;
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
        let sequence = engine.read().sequence();
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
        let (engine, current) = self.lock_current(identity)?;
        Ok(engine.workspace_capacity(&current.principal))
    }
    fn resolve_smb_path(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ResolveSmbPath,
    ) -> Result<crate::SmbPathLocation, crate::ApiError> {
        let (mut engine, current) = self.lock_current(identity)?;
        file_manager::resolve_smb_path(&mut engine, &current, request)
    }
    fn smb_binding_status(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<crate::SmbBindingStatus, crate::ApiError> {
        let (engine, _) = self.lock_current(identity)?;
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
        let (mut engine, current) = self.lock_current(identity)?;
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
        let (engine, _) = self.lock_current(identity)?;
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
        let (engine, _) = self.lock_current(identity)?;
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
        let (mut engine, current) = self.lock_current(identity)?;
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
        let (mut engine, current) = self.lock_current(identity)?;
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
        let (engine, _) = self.lock_current(identity)?;
        let (left, right) = {
            let capture = |id: &str| {
                let commit = engine.state().commits.get_shared(id).ok_or_else(|| {
                    ApiError::new(
                        ::http::StatusCode::NOT_FOUND,
                        storage_contract::ErrorCode::NotFound,
                        "comparison commit not found",
                    )
                })?;
                repo_for_history(&engine, &commit.history_tree_id)?;
                Ok::<_, ApiError>(commit)
            };
            (capture(request.left)?, capture(request.right)?)
        };
        drop(engine);
        Ok(storage_contract::CommitComparison::between(left, right))
    }
    fn disk_status(
        &self,
        identity: &crate::auth::Identity,
    ) -> Result<Vec<storage_contract::DiskStatus>, crate::ApiError> {
        let (engine, _) = self.lock_current(identity)?;
        let observation = engine.prepare_disk_observation();
        drop(engine);
        Ok(disk_monitor::diagnostics(observation))
    }
    fn integrity_check(
        &self,
        identity: &crate::auth::Identity,
    ) -> Result<storage_contract::CheckReport, crate::ApiError> {
        let (engine, _) = self.lock_current(identity)?;
        engine.check().map_err(error)
    }
    fn create_merge_link(
        &self,
        identity: &crate::auth::Identity,
        request: crate::CreateMergeLink,
    ) -> Result<storage_contract::MergeLink, crate::ApiError> {
        let (mut engine, current) = self.lock_current(identity)?;
        Application::new(&mut engine, &current).merge_link(request)
    }
    fn create_repository(
        &self,
        identity: &crate::auth::Identity,
        request: crate::CreateRepository,
    ) -> Result<storage_contract::Repository, crate::ApiError> {
        let (mut engine, current) = self.lock_current(identity)?;
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
        let (mut engine, current) = self.lock_current(identity)?;
        let branch = Application::new(&mut engine, &current).create_branch(request)?;
        branch::capture_current(&engine, &branch)
    }
    fn create_file_branch(
        &self,
        identity: &Identity,
        request: crate::CreateFileBranch,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        let (mut engine, current) = self.lock_current(identity)?;
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
        let (mut engine, current) = self.lock_current(identity)?;
        match request.target {
            crate::NoteTarget::File(file) => {
                let file = file
                    .into_id()
                    .map_err(|source| {
                        ApiError::new(
                            ::http::StatusCode::NOT_FOUND,
                            storage_contract::ErrorCode::FileNotFound,
                            "file not found",
                        )
                        .with_source(source)
                    })?
                    .to_string();
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
                let missing = || {
                    ApiError::new(
                        ::http::StatusCode::NOT_FOUND,
                        storage_contract::ErrorCode::VersionNotFound,
                        "version not found",
                    )
                };
                let version = version
                    .into_id()
                    .map_err(|source| missing().with_source(source))?
                    .to_string();
                let file = engine
                    .state()
                    .versions
                    .get(&version)
                    .ok_or_else(missing)?
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
                let file = file
                    .into_id()
                    .map_err(|source| {
                        ApiError::new(
                            ::http::StatusCode::NOT_FOUND,
                            storage_contract::ErrorCode::FileNotFound,
                            "file not found",
                        )
                        .with_source(source)
                    })?
                    .to_string();
                file_manager::file_repo(&engine, &file)?;
                engine.set_file_node_note(
                    &file,
                    &node.request_text(),
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
        let (mut engine, current) = self.lock_current(identity)?;
        application::Application::new(&mut engine, &current).merge_file_branches(request)
    }
    fn merge_file_history(
        &self,
        identity: &Identity,
        request: crate::MergeFileHistory,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        let (mut engine, current) = self.lock_current(identity)?;
        application::Application::new(&mut engine, &current).merge_file_history(request)
    }
    fn read_container_edit(
        &self, identity: &Identity, request: crate::ReadContainerEdit,
    ) -> Result<storage_contract::container_edit::ContainerEdit, ApiError> {
        self.read_container_edit_admitted(identity, request)
    }
    fn set_container_commit_mode(
        &self, identity: &Identity, request: crate::SetContainerCommitMode,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        self.set_container_commit_mode_admitted(identity, request)
    }
    fn current_container_content(
        &self, identity: &Identity, request: crate::ReadContainerEdit,
    ) -> Result<crate::CurrentContainerContent, ApiError> {
        self.current_container_content_admitted(identity, request)
    }
    fn replace_container_tags(
        &self,
        identity: &Identity,
        request: crate::ReplaceContainerTags,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        let (mut engine, current) = self.lock_current(identity)?;
        application::Application::new(&mut engine, &current)
            .authorize_file(&request.command.file_id.to_string())?;
        let principal = current
            .principal
            .parse::<storage_contract::Principal>()
            .map_err(ApiError::invalid_authenticated_principal)?;
        engine
            .replace_container_tags(&request.command, &principal, request.operation_id)
            .map_err(error)
    }
    fn file_container_tags(
        &self,
        identity: &Identity,
        request: storage_contract::file_container_tags::ReadRequest,
    ) -> Result<storage_contract::file_container_tags::Snapshot, ApiError> {
        let (mut engine, current) = self.lock_current(identity)?;
        application::Application::new(&mut engine, &current)
            .authorize_file(&request.file_id.to_string())?;
        engine.file_container_tags(&request).map_err(error)
    }
    fn create_disk_directory(
        &self,
        identity: &Identity,
        request: crate::CreateDiskDirectory,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        let (mut engine, current) = self.lock_current(identity)?;
        file_manager::mkdir(&mut engine, &current, request)
    }
    fn move_disk_file(
        &self,
        identity: &Identity,
        request: crate::MoveDiskFile,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        let (mut engine, current) = self.lock_current(identity)?;
        file_manager::move_file(&mut engine, &current, request)
    }
    fn move_disk_object(
        &self,
        identity: &Identity,
        request: crate::MoveDiskObject,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        let (mut engine, current) = self.lock_current(identity)?;
        file_manager::move_object(&mut engine, &current, request)
    }
    fn create_disk_write_session(
        &self,
        identity: &Identity,
        request: crate::CreateDiskWriteSession,
    ) -> Result<crate::DiskWriteSession, ApiError> {
        let (mut engine, current) = self.lock_current(identity)?;
        file_manager::write_session(&mut engine, &current, request)
    }
    fn prepare_historical_save(
        &self,
        identity: &Identity,
        request: Result<crate::PrepareHistoricalSave, crate::ApiError>,
    ) -> Result<storage_contract::Plan, ApiError> {
        let (mut engine, current) = self.lock_current(identity)?;
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
        let (mut engine, current) = self.lock_current(identity)?;
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
        let (mut engine, current) = self.lock_current(identity)?;
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
    fn placement_configuration(
        &self,
        identity: &Identity,
        request: crate::ReadPlacementConfiguration,
    ) -> Result<storage_contract::PlacementConfigurationSnapshot, ApiError> {
        let (engine, _) = self.lock_current(identity)?;
        engine
            .placement_configuration(&request.repository_id, &request.path)
            .map_err(error)
    }
    fn configure_placement(
        &self,
        identity: &Identity,
        request: crate::ConfigurePlacement,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        let (mut engine, current) = self.lock_current(identity)?;
        engine
            .configure_placement(storage_engine::PlacementConfigurationRequest {
                repository_id: &request.repository_id,
                path: &request.path,
                primary_slot_id: &request.primary_slot_id,
                expected_scope_id: &request.expected_scope_id,
                expected_rule_id: request.expected_rule_id.as_deref(),
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
    fn backup_configuration(
        &self,
        identity: &Identity,
        request: crate::ReadBackupConfiguration,
    ) -> Result<storage_contract::BackupConfigurationSnapshot, ApiError> {
        let (engine, _) = self.lock_current(identity)?;
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
        let (mut engine, current) = self.lock_current(identity)?;
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
    fn recursive_backup_configuration(
        &self,
        identity: &Identity,
        request: crate::ReadBackupConfiguration,
    ) -> Result<crate::RecursiveBackupConfiguration, ApiError> {
        let (engine, _) = self.lock_current(identity)?;
        repository(&engine, &request.repository_id)?;
        let configuration = engine
            .backup_configuration(&request.repository_id, &request.path)
            .map_err(error)?;
        let capture = engine.recursive_backup_capture().map_err(error)?;
        Ok(crate::RecursiveBackupConfiguration {
            configuration,
            capture,
        })
    }
    fn configure_recursive_backups(
        &self,
        identity: &Identity,
        request: crate::ConfigureRecursiveBackups,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        let (mut engine, current) = self.lock_current(identity)?;
        repository(&engine, &request.repository_id)?;
        engine
            .configure_recursive_backups(storage_engine::RecursiveBackupConfigurationRequest {
                repository_id: &request.repository_id,
                path: &request.path,
                expected_scope_id: &request.expected_scope_id,
                target_copies: request.target_copies,
                expected_generation: request.expected_generation,
                capture: &request.capture,
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
    fn configure_recursive_backup_tree(
        &self,
        identity: &Identity,
        request: crate::ConfigureRecursiveBackups,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        let (mut engine, current) = self.lock_current(identity)?;
        repository(&engine, &request.repository_id)?;
        engine
            .configure_recursive_backup_tree(storage_engine::RecursiveBackupConfigurationRequest {
                repository_id: &request.repository_id,
                path: &request.path,
                expected_scope_id: &request.expected_scope_id,
                target_copies: request.target_copies,
                expected_generation: request.expected_generation,
                capture: &request.capture,
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
        let (mut engine, current) = self.lock_current(identity)?;
        application::Application::new(&mut engine, &current).namespace_batch(request)
    }
    fn select_file_branch(
        &self,
        identity: &Identity,
        request: crate::SelectFileBranch,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        let (mut engine, current) = self.lock_current(identity)?;
        let repository = file_manager::file_repo(&engine, &request.file_id)?;
        if let Some(baseline) = &request.default_baseline {
            let location = request.location.as_ref().ok_or_else(|| {
                ApiError::invalid("setting a file default requires its logical path")
            })?;
            return engine
                .set_file_default(storage_engine::FileDefaultSelection {
                    file_id: &request.file_id,
                    branch_id: &request.branch_id,
                    repository_id: &location.repository_id,
                    path: &location.path,
                    expected_node_id: &baseline.expected_node_id,
                    expected_head: &baseline.expected_head,
                    principal: &current.principal,
                    operation_id: request.operation_id,
                })
                .map_err(error);
        }
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
        let (mut engine, current) = self.lock_current(identity)?;
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
    fn soft_delete_file(
        &self,
        identity: &Identity,
        command: crate::DeleteFile,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        self.soft_delete_file_admitted(identity, command)
    }
    fn move_current_file(
        &self,
        identity: &Identity,
        command: crate::MoveCurrentFile,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        self.move_current_file_admitted(identity, command)
    }
    fn move_current_object(
        &self,
        identity: &Identity,
        command: crate::MoveCurrentObject,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        self.move_current_object_admitted(identity, command)
    }
    fn replace_current_file(
        &self,
        identity: &Identity,
        command: crate::ReplaceCurrentFile,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        self.replace_current_file_admitted(identity, command)
    }
    fn move_current_file_to_empty_disk(
        &self,
        identity: &Identity,
        command: crate::MoveCurrentFileToEmptyDisk,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        self.move_current_file_to_empty_disk_admitted(identity, command)
    }
    fn delete_current_file(
        &self,
        identity: &Identity,
        command: crate::DeleteCurrentFile,
    ) -> Result<Option<storage_contract::OperationResult>, ApiError> {
        self.delete_current_file_admitted(identity, command)
    }
    fn prepare_current_directory_deletion(
        &self,
        identity: &Identity,
        command: crate::PrepareCurrentDirectoryDeletion,
    ) -> Result<Option<crate::CurrentDirectoryDeletion>, ApiError> {
        self.prepare_current_directory_deletion_admitted(identity, command)
    }
    fn delete_current_directory(
        &self,
        identity: &Identity,
        command: crate::DeleteCurrentDirectory,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        self.delete_current_directory_admitted(identity, command)
    }
    fn directory_deletions(
        &self,
        identity: &Identity,
        repository: &str,
        owner: &str,
        request: crate::ReadDirectoryDeletions,
    ) -> Result<storage_contract::directory_git::DirectoryDeletionsPage, ApiError> {
        self.directory_deletions_admitted(identity, repository, owner, request)
    }
    fn current_directory_deletion(
        &self,
        identity: &Identity,
        repository: &str,
        owner: &str,
        directory: &str,
    ) -> Result<Option<storage_contract::directory_git::DirectoryDeletionNode>, ApiError> {
        self.current_directory_deletion_admitted(identity, repository, owner, directory)
    }
    fn restore_deleted_directory(
        &self,
        identity: &Identity,
        repository: &str,
        request: &storage_contract::directory_git::UndoDirectoryDeletionBody,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        self.restore_deleted_directory_admitted(identity, repository, request)
    }
    fn file_deletion_summary(
        &self,
        identity: &Identity,
        file: storage_contract::FileId,
        deletion: storage_contract::FileDeletionId,
    ) -> Result<storage_contract::file_deletion_review::Summary, ApiError> {
        self.file_deletion_summary_admitted(identity, file, deletion)
    }

    fn file_deletion_branches(
        &self,
        identity: &Identity,
        file: storage_contract::FileId,
        deletion: storage_contract::FileDeletionId,
        after: Option<storage_contract::BranchId>,
    ) -> Result<storage_contract::file_deletion_review::BranchPage, ApiError> {
        self.file_deletion_branches_admitted(identity, file, deletion, after)
    }

    fn file_deletion_parents(
        &self,
        identity: &Identity,
        file: storage_contract::FileId,
        deletion: storage_contract::FileDeletionId,
        offset: u64,
    ) -> Result<storage_contract::file_deletion_review::ParentPage, ApiError> {
        self.file_deletion_parents_admitted(identity, file, deletion, offset)
    }

    fn current_file_deletion(
        &self,
        identity: &Identity,
        file: &storage_contract::history_window::HistoryFileId,
    ) -> Result<Option<storage_contract::FileDeletionNode>, ApiError> {
        self.current_file_deletion_admitted(identity, file)
    }
    fn file_deletions(
        &self,
        identity: &Identity,
        request: crate::ReadFileDeletions,
    ) -> Result<crate::FileDeletionsPage, ApiError> {
        self.file_deletions_admitted(identity, request)
    }
    fn file_deletion_batch_undo_preview(
        &self,
        identity: &Identity,
        operation: storage_contract::OperationId,
    ) -> Result<storage_contract::file_deletion_batch::FileDeletionBatchUndoPreview, ApiError> {
        self.file_deletion_batch_undo_preview_admitted(identity, operation)
    }
    fn restore_deleted_file_batch(
        &self,
        identity: &Identity,
        request: &storage_contract::file_deletion_batch::UndoFileDeletionBatchBody,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        self.restore_deleted_file_batch_admitted(identity, request)
    }
    fn restore_deleted_file(
        &self,
        identity: &Identity,
        command: crate::RestoreDeletedFile,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        self.restore_deleted_file_admitted(identity, command)
    }
    fn authorize_listing_snapshot(
        &self,
        identity: &Identity,
        snapshot: &str,
    ) -> Result<(), ApiError> {
        let (engine, _) = self.lock_current(identity)?;
        if engine.listing_snapshot() != snapshot {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::ListingChanged,
                "directory changed; restart from its first page",
            ));
        }
        Ok(())
    }
    fn listing(
        &self,
        identity: &Identity,
        request: crate::ReadListing,
    ) -> Result<crate::ListingSnapshot, ApiError> {
        let read = {
            let (engine, _) = self.lock_current(identity)?;
            file_manager::listing::capture(&engine, request)?
        };
        match read {
            file_manager::listing::ListingRead::Backup(read) => {
                let selected = read.select();
                let (engine, _) = self.lock_current(identity)?;
                let selected = selected?;
                let page = selected.project(&engine);
                drop(engine);
                Ok(selected.finish(page?))
            }
            file_manager::listing::ListingRead::Primary(read) => {
                let selected = read.select();
                let (engine, _) = self.lock_current(identity)?;
                let selected = selected?;
                let page = selected.project(&engine);
                drop(engine);
                Ok(selected.finish(page?))
            }
        }
    }
    fn mutate_disk_directory(
        &self,
        identity: &Identity,
        request: crate::MutateDiskDirectory,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        let (mut engine, current) = self.lock_current(identity)?;
        file_manager::mutate_directory(&mut engine, &current, request)
    }
    fn initialize_directory_scope(
        &self,
        identity: &Identity,
        request: crate::InitializeDirectoryScope,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        let (mut engine, current) = self.lock_current(identity)?;
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
        let (mut engine, current) = self.lock_current(identity)?;
        let branch = request
            .branch_id
            .into_id()
            .map_err(|source| {
                ApiError::new(
                    ::http::StatusCode::NOT_FOUND,
                    storage_contract::ErrorCode::NotFound,
                    "branch not found",
                )
                .with_source(source)
            })?
            .to_string();
        repo_for_branch(&engine, &branch)?;
        engine
            .archive_branch(&branch, &current.principal, request.operation_id)
            .map_err(error)
    }
    fn rename_directory_branch(
        &self,
        identity: &crate::auth::Identity,
        request: crate::RenameDirectoryBranch,
    ) -> Result<storage_contract::OperationResult, crate::ApiError> {
        let (mut engine, current) = self.lock_current(identity)?;
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
        let (mut engine, current) = self.lock_current(identity)?;
        repository(&engine, &request.repository_id)?;
        engine
            .set_directory_default(storage_engine::DirectoryDefaultSelection {
                repository_id: &request.repository_id,
                directory_branch_id: &request.directory_branch_id,
                expected_tree_id: &request.expected_tree_id,
                expected_head: &request.expected_head,
                principal: &current.principal,
                operation_id: request.operation_id,
            })
            .map_err(error)
    }
    fn directory_selection(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadDirectorySelection,
    ) -> Result<crate::DirectorySelectionSnapshot, crate::ApiError> {
        let (engine, _) = self.lock_current(identity)?;
        directory_selection::read(&engine, &request)
    }
    fn directory_scopes(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadDirectoryScopes<'_>,
    ) -> Result<crate::DirectoryScopesSnapshot, crate::ApiError> {
        let (engine, _) = self.lock_current(identity)?;
        directory_scopes::capture(&engine, request)
    }
    fn default_directory_branch(
        &self,
        identity: &crate::auth::Identity,
        repository_id: &str,
    ) -> Result<crate::DefaultDirectoryBranchSnapshot, crate::ApiError> {
        let (engine, _) = self.lock_current(identity)?;
        let repo = repository(&engine, repository_id)?;
        let state = engine.state();
        let git = state.directory_git_repositories.get(repository_id);
        let selected = git.map(|git| {
            let branch = &state.directory_branches[&git.current_branch_id];
            crate::SelectedDirectoryBranch {
                directory_branch_id: git.current_branch_id.clone(),
                name: branch.name.clone(),
            }
        });
        Ok(crate::DefaultDirectoryBranchSnapshot {
            repository_id: repo.repository_id.clone(),
            namespace_branch_id: repo.main_branch_id.clone(),
            selected,
            directory_branches: match git {
                None => Vec::new(),
                Some(git) => storage_contract::directory_branch_summaries(
                    state,
                    storage_contract::DirectoryBranchScope {
                        repository_id,
                        directory_id: repository_id,
                        path: "",
                    },
                )
                .into_iter()
                .filter(|branch| git.branches.contains_key(&branch.directory_branch_id))
                .collect(),
            },
        })
    }
    fn preview_node_retirement(
        &self,
        identity: &Identity,
        request: storage_contract::node_retirement::Selection,
        cancellation: storage_contract::history_window::HistoryWindowCancellation,
    ) -> Result<storage_contract::node_retirement::Preview, ApiError> {
        node_retirement::preview(self, identity, request, cancellation)
    }
    fn retire_file_nodes(
        &self,
        identity: &Identity,
        request: storage_contract::node_retirement::Selection,
        operation: storage_contract::OperationId,
    ) -> Result<storage_contract::node_retirement::Outcome, ApiError> {
        node_retirement::submit(self, identity, request, operation)
    }
    fn resume_node_retirement(
        &self,
        identity: &Identity,
        file: storage_contract::history_window::HistoryFileId,
        operation: storage_contract::OperationId,
    ) -> Result<storage_contract::node_retirement::Outcome, ApiError> {
        node_retirement::resume(self, identity, file, operation)
    }
    fn detached_history(
        &self,
        identity: &Identity,
        request: storage_contract::detached_history::Request,
        cancellation: storage_contract::history_window::HistoryWindowCancellation,
    ) -> Result<storage_contract::detached_history::Page, ApiError> {
        node_retirement::detached_history(self, identity, request, cancellation)
    }
    fn shared_history(
        &self,
        identity: &Identity,
        request: storage_contract::shared_history::SharedHistoryRequest,
        cancellation: storage_contract::history_window::HistoryWindowCancellation,
    ) -> Result<storage_contract::shared_history::SharedHistoryGraph, ApiError> {
        let read = {
            let (engine, _) = self.lock_current(identity)?;
            engine
                .capture_shared_history(request, cancellation)
                .map_err(error)?
        };
        let page = read.graph().map_err(error);
        let (engine, _) = self.lock_current(identity)?;
        engine.recheck_shared_history(&read).map_err(error)?;
        page
    }
    fn file_lifecycle(
        &self,
        identity: &Identity,
        request: storage_contract::file_lifecycle::Request,
        cancellation: storage_contract::history_window::HistoryWindowCancellation,
    ) -> Result<storage_contract::file_lifecycle::Page, ApiError> {
        let read = {
            let (engine, _) = self.lock_current(identity)?;
            engine
                .capture_file_lifecycle(request, cancellation)
                .map_err(error)?
        };
        let page = read.page().map_err(error);
        let (engine, _) = self.lock_current(identity)?;
        engine.recheck_file_lifecycle(&read).map_err(error)?;
        page
    }
    fn content_matches(
        &self,
        identity: &Identity,
        request: storage_contract::content_matches::ContentMatchRequest,
        cancellation: storage_contract::history_window::HistoryWindowCancellation,
    ) -> Result<storage_contract::content_matches::ContentMatchPage, ApiError> {
        let read = {
            let (engine, _) = self.lock_current(identity)?;
            engine
                .capture_content_matches(request, cancellation)
                .map_err(error)?
        };
        let page = read.page().map_err(error);
        let (engine, _) = self.lock_current(identity)?;
        engine.recheck_content_matches(&read).map_err(error)?;
        page
    }
    fn file_history_source(
        &self,
        identity: &Identity,
        request: storage_contract::file_history_merge::FileHistorySourceRequest,
        cancellation: storage_contract::history_window::HistoryWindowCancellation,
    ) -> Result<storage_contract::file_history_merge::FileHistorySource, ApiError> {
        let read = {
            let (engine, _) = self.lock_current(identity)?;
            engine
                .capture_file_history_source(request, cancellation)
                .map_err(error)?
        };
        let result = read.source().map_err(error);
        let (engine, _) = self.lock_current(identity)?;
        engine.recheck_file_history_source(&read).map_err(error)?;
        result
    }
    fn directory_git_status(
        &self,
        identity: &Identity,
        repository_id: &str,
        path: &str,
        cancellation: storage_contract::history_window::HistoryWindowCancellation,
    ) -> Result<storage_contract::directory_git::DirectoryGitStatus, ApiError> {
        let read = {
            let (engine, _) = self.lock_current(identity)?;
            repository(&engine, repository_id)?;
            engine
                .capture_directory_git_status(repository_id, path, cancellation)
                .map_err(error)?
        };
        let result = read.status().map_err(error);
        let (engine, _) = self.lock_current(identity)?;
        engine.recheck_directory_git_status(&read).map_err(error)?;
        result
    }

    fn reconcile_directory_layout(
        &self,
        identity: &Identity,
        repository_id: &str,
        request: storage_contract::directory_git::DirectoryLayoutReconcileBody,
    ) -> Result<storage_contract::directory_git::DirectoryLayoutResult, ApiError> {
        let (mut engine, _) = self.lock_current(identity)?;
        let repository = repository(&engine, repository_id)?;
        let main = engine
            .state()
            .branches
            .get(&repository.main_branch_id)
            .expect("invariant: admitted repository has its main branch");
        if main.head_commit_id != request.expected_namespace_head {
            return Err(ApiError::new(
                ::http::StatusCode::CONFLICT,
                storage_contract::ErrorCode::HeadConflict,
                "repository namespace changed; refresh before reconciling layout",
            ));
        }
        // Keep authentication, the location guard and every physical step in
        // one writer-lock interval. Independent mains are read at execution.
        let receipt = engine
            .reconcile_directory_layout(repository_id)
            .map_err(error)?;
        Ok(storage_contract::directory_git::DirectoryLayoutResult {
            repository_id: repository_id.to_owned(),
            state_sequence: receipt.state_sequence,
            directories: receipt.directories as u64,
            containers: receipt.containers as u64,
            unavailable_containers: receipt.unavailable_containers.len() as u64,
            unavailable_histories: receipt.unavailable_histories.len() as u64,
        })
    }
    fn initialize_directory_git(
        &self,
        identity: &Identity,
        repository_id: &str,
        request: storage_contract::directory_git::DirectoryGitInitializeBody,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        let (mut engine, current) = self.lock_current(identity)?;
        repository(&engine, repository_id)?;
        engine
            .initialize_directory_git(repository_id, &request, &current.principal)
            .map_err(error)
    }
    fn stage_directory_git_child(
        &self,
        identity: &Identity,
        repository_id: &str,
        request: storage_contract::directory_git::DirectoryGitStageChildBody,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        let (mut engine, current) = self.lock_current(identity)?;
        repository(&engine, repository_id)?;
        engine
            .stage_directory_git_child(repository_id, &request, &current.principal)
            .map_err(error)
    }
    fn stage_directory_git_owned(
        &self,
        identity: &Identity,
        repository_id: &str,
        request: storage_contract::directory_git::DirectoryGitStageOwnedBody,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        let (mut engine, current) = self.lock_current(identity)?;
        repository(&engine, repository_id)?;
        engine
            .stage_directory_git_owned(repository_id, &request, &current.principal)
            .map_err(error)
    }
    fn directory_staging(
        &self,
        identity: &Identity,
        repository_id: &str,
        branch: &str,
    ) -> Result<storage_contract::directory_staging::DirectoryStagingStatus, ApiError> {
        let (engine, _) = self.lock_current(identity)?;
        repository(&engine, repository_id)?;
        engine
            .directory_staging_status(repository_id, branch)
            .map_err(error)
    }
    fn commit_directory_changes(
        &self,
        identity: &Identity,
        repository_id: &str,
        branch: &str,
        request: storage_contract::directory_staging::DirectoryCommitBody,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        let (mut engine, current) = self.lock_current(identity)?;
        repository(&engine, repository_id)?;
        engine
            .commit_directory_changes(storage_engine::DirectoryCommitRequest {
                repository_id,
                directory_branch_id: branch,
                expected_tree_id: &request.expected_tree_id,
                expected_index_generation: request.expected_index_generation,
                message: &request.message,
                principal: &current.principal,
                operation_id: request.operation_id,
            })
            .map_err(error)
    }
    fn directory_history(
        &self,
        identity: &Identity,
        request: storage_contract::directory_history::DirectoryHistoryRequest,
        cancellation: storage_contract::history_window::HistoryWindowCancellation,
    ) -> Result<storage_contract::directory_history::DirectoryHistoryPage, ApiError> {
        let read = {
            let (engine, _) = self.lock_current(identity)?;
            engine
                .capture_directory_history(request, cancellation)
                .map_err(error)?
        };
        let page = read.page().map_err(error);
        let (engine, _) = self.lock_current(identity)?;
        engine.recheck_directory_history(&read).map_err(error)?;
        page
    }
    fn history_window(
        &self,
        identity: &Identity,
        request: storage_contract::history_window::HistoryWindowRequest,
        cancellation: storage_contract::history_window::HistoryWindowCancellation,
    ) -> Result<storage_contract::history_window::HistoryWindowPage, ApiError> {
        let read = {
            let (engine, _) = self.lock_current(identity)?;
            engine
                .capture_history_window(request, cancellation)
                .map_err(error)?
        };
        let page = read.page().map_err(error);
        let (engine, _) = self.lock_current(identity)?;
        engine.recheck_history_window(&read).map_err(error)?;
        page
    }
    fn file_history_snapshot(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadFileHistory,
    ) -> Result<crate::FileHistorySnapshot, crate::ApiError> {
        let read = {
            let (engine, _) = self.lock_current(identity)?;
            file_manager::FileHistoryRead::capture(&engine, request)?
        };
        #[cfg(test)]
        graph::tests::after_capture(read.view.business_projection().sequence);
        let snapshot = read.project();
        let (engine, _) = self.lock_current(identity)?;
        read.authorize_current(&engine)?;
        snapshot
    }
    fn file_history(
        &self,
        identity: &crate::auth::Identity,
        file: &str,
    ) -> Result<Vec<storage_contract::FileVersion>, crate::ApiError> {
        let (engine, _) = self.lock_current(identity)?;
        let versions = engine.file_history(file).map_err(error)?;
        if versions.is_empty() {
            return Err(ApiError::new(
                ::http::StatusCode::NOT_FOUND,
                storage_contract::ErrorCode::NotFound,
                "file history not found",
            ));
        }
        Ok(versions)
    }
    fn topology_snapshot(
        &self,
        identity: &crate::auth::Identity,
    ) -> Result<crate::TopologySnapshot, crate::ApiError> {
        let (engine, _) = self.lock_current(identity)?;
        let names = crate::backend_storage_metadata::disk_directory_names(&engine.config().disks)?;
        let slots = engine
            .config()
            .disks
            .iter()
            .zip(names)
            .map(|(disk, name)| {
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
                    name: Some(name),
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
            generation: engine.read().topology_generation(),
            slots,
        })
    }
    fn placement_snapshot(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<storage_contract::Placement, crate::ApiError> {
        let (engine, _) = self.lock_current(identity)?;
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
        let (engine, _) = self.lock_current(identity)?;
        capture_event_page(engine.events_after(after), engine.read().sequence(), after)
    }
    fn activity_history(
        &self,
        identity: &Identity,
    ) -> Result<crate::ActivityHistorySnapshot, ApiError> {
        let (engine, _) = self.lock_current(identity)?;
        file_manager::activity::read(&engine)
    }
    fn history_graph(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<crate::HistoryGraphSnapshot, crate::ApiError> {
        let view = {
            let (engine, _) = self.lock_current(identity)?;
            repo_for_history(&engine, id)?;
            graph::GraphMetadata::capture(engine.state())
        };
        #[cfg(test)]
        graph::tests::after_capture(view.sequence);
        let mut snapshot = view.project(id);
        snapshot.commits.sort_by_key(|commit| commit.sequence);
        let (engine, _) = self.lock_current(identity)?;
        graph::authorize_current(engine.state(), &snapshot.history)?;
        Ok(snapshot)
    }
    fn commit_snapshot(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<std::sync::Arc<storage_contract::Commit>, crate::ApiError> {
        let (engine, _) = self.lock_current(identity)?;
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
        let (engine, _) = self.lock_current(identity)?;
        read_projection::resolve_entry(&engine, request)
    }
    fn branch_snapshot(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<crate::BranchSnapshot, crate::ApiError> {
        let (engine, _) = self.lock_current(identity)?;
        repo_for_branch(&engine, id)?;
        branch::capture_current(&engine, &engine.state().branches[id])
    }
    fn branches_snapshot(
        &self,
        identity: &crate::auth::Identity,
        id: &str,
    ) -> Result<Vec<crate::BranchSnapshot>, crate::ApiError> {
        let (engine, _) = self.lock_current(identity)?;
        let repo = repository(&engine, id)?;
        engine
            .state()
            .branches
            .values()
            .filter(|branch| branch.history_tree_id == repo.history_tree_id)
            .map(|branch| branch::capture_current(&engine, branch))
            .collect()
    }
    fn repositories_snapshot(
        &self,
        identity: &crate::auth::Identity,
    ) -> Result<Vec<storage_contract::Repository>, crate::ApiError> {
        let (engine, _) = self.lock_current(identity)?;
        Ok(engine.state().repositories.values().cloned().collect())
    }
    fn tree_snapshot(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadTree<'_>,
    ) -> Result<crate::TreeSnapshot, crate::ApiError> {
        let (engine, _) = self.lock_current(identity)?;
        crate::TreeSnapshot::capture(&engine, request)
    }
    fn create_write_session(
        &self,
        identity: &crate::auth::Identity,
        branch: &str,
    ) -> Result<crate::WorkspaceWriteSession, crate::ApiError> {
        let (mut engine, current) = self.lock_current(identity)?;
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
        let (mut engine, current) = self.lock_current(identity)?;
        Application::new(&mut engine, &current).stage_namespace(command)
    }

    fn authorize_workspace_write(
        &self,
        identity: &Identity,
        workspace: &str,
        access: crate::WorkspaceWrite<'_>,
    ) -> Result<(), ApiError> {
        let (engine, current) = self.lock_current(identity)?;
        let file = match access {
            crate::WorkspaceWrite::Upload { .. } => None,
            crate::WorkspaceWrite::ActiveFile { branch, path }
            | crate::WorkspaceWrite::SavedFile { branch, path } => {
                read_projection::authorize_current(engine.state(), branch)?;
                Some(path)
            }
        };
        let workspace = workspace_authorize(&engine, &current, workspace)?;
        if let Some(path) = file {
            engine
                .validate_workspace_file_selection(&workspace, path)
                .map_err(error)?;
        }
        let logical_size = match access {
            crate::WorkspaceWrite::Upload { logical_size } => logical_size,
            crate::WorkspaceWrite::ActiveFile { .. } => None,
            crate::WorkspaceWrite::SavedFile { .. } => return Ok(()),
        };
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
        let (engine, _) = self.lock_current(identity)?;
        crate::DirectorySnapshot::capture(&engine, request)
    }

    fn authorize_current(&self, identity: &Identity, branch: &str) -> Result<(), ApiError> {
        let (engine, _) = self.lock_current(identity)?;
        read_projection::authorize_current(engine.state(), branch)
    }

    fn lookup_current(
        &self,
        identity: &Identity,
        branch: &str,
        path: &name_policy::LookupPath,
    ) -> Result<Option<storage_contract::Entry>, ApiError> {
        let (engine, _) = self.lock_current(identity)?;
        read_projection::lookup_current(&engine, branch, path)
    }

    fn lookup_at(
        &self,
        identity: &Identity,
        branch: &str,
        commit: &str,
        path: &name_policy::LookupPath,
    ) -> Result<Option<storage_contract::Entry>, ApiError> {
        let (engine, _) = self.lock_current(identity)?;
        let (_, snapshot) = read_projection::branch_snapshot(engine.state(), branch, Some(commit))?;
        Ok(snapshot
            .entries
            .binary_search_by(|entry| entry.lookup_key().cmp(path.key()))
            .ok()
            .map(|index| snapshot.entries[index].clone()))
    }

    fn plan(&self, identity: &Identity, plan_id: &str) -> Result<storage_contract::Plan, ApiError> {
        let (engine, identity) = self.lock_current(identity)?;
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
        let (mut engine, identity) = self.lock_current(identity)?;
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
        let (_engine, _) = self.lock_current(identity)?;
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
        let (mut engine, identity) = self.lock_current(identity)?;
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
        let (engine, current) = self.lock_current(identity)?;
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
        let (engine, current) = self.lock_current(identity)?;
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
#[path = "../../../test/server/unit/engine_backend/locked_admission_tests.rs"]
mod locked_admission_tests;

#[cfg(test)]
#[path = "../../../test/server/unit/engine_backend/authorization_tests.rs"]
mod authorization_tests;
#[cfg(test)]
#[path = "../../../test/server/unit/engine_backend/event_tests.rs"]
mod event_tests;
#[cfg(test)]
#[path = "../../../test/server/unit/engine_backend/version_read_tests.rs"]
mod version_read_tests;
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
