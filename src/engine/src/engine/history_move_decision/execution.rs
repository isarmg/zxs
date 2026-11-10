//! One vertical native move: exact intent, complete preparation, ordinary joint
//! decision, required database installation, then one visible State replacement.
use super::*;
use crate::business::NativeMoveCandidate;
use crate::journal::{Candidate, Publication};
use crate::engine::move_execution::PublishedMove;
use storage_contract::{file_relocation::NamespaceEntryId, history_window::HistoryFileId};

mod archive;

/// A path-facing request for a single saved file. The engine captures the exact
/// source identity and both heads once; retries use the original durable intent.
#[derive(Clone, Debug, PartialEq, Eq, data_serialization::Deserialize, Serialize)]
#[serialization(deny_unknown_fields)]
pub struct FileHistoryMovePathRequest {
    pub source_repository_id: String,
    pub source_path: String,
    pub target_repository_id: String,
    pub target_name: String,
}
/// Captured by an external handle. Both identities are checked against the
/// original operation before consulting the current namespace on a retry.
pub struct CapturedFileMoveSource {
    pub entry_id: NamespaceEntryId,
    pub file_id: HistoryFileId,
}

impl CapturedFileMoveSource {
    fn matches(&self, state: &State, original: &FileHistoryCopyRequest) -> Result<bool> {
        let commit = state.commits.get(&original.expected_source_head)
            .ok_or_else(|| invalid("move original source namespace is missing"))?;
        let path = name_policy::LookupPath::parse(&original.source_path)?;
        Ok(crate::business::find_unordered_key(&commit.entries, path.key())
            .is_some_and(|entry| entry.kind == NamespaceEntryKind::File
                && entry.entry_id == self.entry_id.as_str()
                && entry.file_id == self.file_id.as_str()))
    }
}

impl FileHistoryMovePathRequest {
    fn validate(&self) -> Result<()> {
        storage_contract::validate_id(&self.source_repository_id, "repo")?;
        storage_contract::validate_id(&self.target_repository_id, "repo")?;
        name_policy::LookupPath::parse(&self.source_path)?;
        name_policy::PortablePath::parse(&self.target_name)?;
        Ok(())
    }
    pub(crate) fn matches(&self, original: &FileHistoryCopyRequest) -> bool {
        self.source_repository_id == original.source_repository_id
            && self.source_path == original.source_path
            && self.target_repository_id == original.target_repository_id
            && self.target_name == original.target_name
    }
}

impl crate::engine::move_execution::MoveOutcome for NativeMoveCandidate {
    fn state(&self) -> &State { self.state() }
    fn result(&self) -> &OperationResult { self.result() }
    fn into_state(self) -> State { self.into_state() }
}

struct OriginalFileSelection;
impl crate::engine::move_execution::MoveSelection<NativeMoveCandidate> for OriginalFileSelection {
    fn verify(&self, engine: &Engine, _: &Candidate, _: &str) -> Result<()> {
        engine.require_move_choice_baseline()
    }
    fn prepare(&self, _: &mut Engine, _: &Candidate, _: &str) -> Result<()> {
        // The existing file input preparation has already retained PREPARE.
        Ok(())
    }
    fn mark_decided(&self, engine: &mut Engine) {
        engine.history_move_prepare.as_mut()
            .expect("invariant: every move starts with its PREPARE").mark_decided();
    }
    fn retire(&self, engine: &mut Engine, published: &PublishedMove) -> Result<()> {
        archive::retire(engine, published)
    }
    fn clear(self, engine: &mut Engine) {
        engine.history_move_intent = None;
        engine.history_move_choice = None;
        engine.history_move_prepare = None;
    }
}

fn conflict() -> Error {
    Error::new(
        ErrorCode::IdempotencyKeyReused,
        "move operation belongs to another actor or original path request",
    )
}

impl Engine {
    pub(in crate::engine) fn require_native_move_authority(&self) -> Result<()> {
        let view = self.admitted_current_namespaces()?;
        match (self.config.generation(), view) {
            (None, None) => Ok(()),
            (Some(generation), Some(view))
                if !generation.portable.is_backup()
                    && !generation.portable.is_foreign()
                    && generation.registry() == Some(view.registry())
                    && self.generation_activation_published()
                    && view.matches_source(&self.state) =>
            {
                // Disabled/detached historical definitions are not I/O grants.
                // Every remaining enabled identity must still have its exact
                // held definition. The payload/full checks enforce the route.
                for registered in self
                    .registered_disks()
                    .filter(|disk| disk.registration_mode == RegistrationMode::Enabled)
                {
                    let disk = self.disk(registered.slot_id)?;
                    if registered.pool_id != self.config.pool_id
                        || registered.disk_id != disk.disk_id
                        || registered.binding_generation != disk.binding_generation
                        || registered.metadata_member != disk.metadata_member
                    {
                        return Err(invalid(
                            "move Current differs from the held original bindings",
                        ));
                    }
                }
                Ok(())
            }
            _ => Err(Error::new(
                ErrorCode::UnsupportedOperation,
                "native move requires admitted original bindings and a replayable Current generation; Backup/Foreign cutovers remain fenced",
            )),
        }
    }

    /// Execute one native cross-disk move. The only returned success is the
    /// original Published receipt after target FULL, both FILEs, disk current /
    /// complete history and SYSTEM have joined the same ordinary decision.
    /// Authentication and both namespace writer reservations belong to callers.
    pub fn move_current_file(
        &mut self,
        request: &FileHistoryMovePathRequest,
        principal: &str,
        operation: OperationId,
    ) -> Result<OperationResult> {
        self.move_current_file_target(request, None, None, principal, operation)
    }

    /// Replace the exact existing ordinary target in the SAME move decision.
    /// The displaced file retains its graph, FULLs, original occurrence and D.
    pub fn replace_current_file(
        &mut self,
        request: &FileHistoryMovePathRequest,
        replacement: &FileHistoryMoveReplacement,
        principal: &str,
        operation: OperationId,
    ) -> Result<OperationResult> {
        self.move_current_file_target(request, None, Some(replacement), principal, operation)
    }

    /// Keep externally captured source and displaced-target identity within
    /// the same original core operation. Adapters must not re-resolve a source
    /// that an earlier attempt has already moved or fenced.
    pub fn move_current_file_captured(
        &mut self,
        request: &FileHistoryMovePathRequest,
        source: &CapturedFileMoveSource,
        replacement: Option<&FileHistoryMoveReplacement>,
        principal: &str,
        operation: OperationId,
    ) -> Result<OperationResult> {
        self.move_current_file_target(request, Some(source), replacement, principal, operation)
    }

    /// An already retained file operation continues with its own exact inputs.
    /// A new object request is not permission to replan an existing operation.
    pub(crate) fn resume_file_move_if_original(
        &mut self,
        request: &FileHistoryMovePathRequest,
        source: &CapturedFileMoveSource,
        replacement: Option<&FileHistoryMoveReplacement>,
        principal: &str,
        operation: OperationId,
    ) -> Result<Option<OperationResult>> {
        let selected = match self.state.operations.get(&operation) {
            Some(original) => original.kind == crate::business::FILE_HISTORY_MOVE_KIND,
            None => self.original_move_path_intent(request, replacement, principal, operation)?.is_some(),
        };
        if selected {
            self.move_current_file_captured(request, source, replacement, principal, operation).map(Some)
        } else {
            Ok(None)
        }
    }

    fn move_current_file_target(
        &mut self,
        request: &FileHistoryMovePathRequest,
        captured_source: Option<&CapturedFileMoveSource>,
        replacement: Option<&FileHistoryMoveReplacement>,
        principal: &str,
        operation: OperationId,
    ) -> Result<OperationResult> {
        Self::require_actor(principal)?;
        request.validate()?;
        // Replay before resolving source visibility or today's namespace heads.
        if let Some(original) = self.state.operations.get(&operation) {
            if original.kind != crate::business::FILE_HISTORY_MOVE_KIND
                || self
                    .state
                    .operation_principals
                    .get(&operation)
                    .map(String::as_str)
                    != Some(principal)
            {
                return Err(conflict());
            }
            let (namespace, original_target) = crate::business::original_move_intent(original)?;
            if !request.matches(&namespace) || original_target.as_ref() != replacement {
                return Err(conflict());
            }
            if let Some(source) = captured_source {
                if !source.matches(&self.state, &namespace)? { return Err(conflict()); }
            }
            return Ok(original.clone());
        }
        self.require_move_source_read()?;
        self.require_unfenced_membership()?;
        self.require_native_move_authority()?;
        let cancellation = HistoryWindowCancellation::default();
        let projection = if let Some(original) =
            self.original_move_path_intent(request, replacement, principal, operation)?
        {
            self.resume_file_history_move_recovery(
                operation,
                original.digest(),
                principal,
                &cancellation,
            )?
        } else {
            self.require_write()?;
            let namespace = self.capture_move_path(request)?;
            let main = &self.state.repositories[&request.source_repository_id].main_branch_id;
            let view = self.namespace_tree(main)?;
            let path = name_policy::LookupPath::parse(&request.source_path)?;
            let entry = crate::business::find_unordered_key(view.entries(), path.key())
                .filter(|entry| entry.kind == NamespaceEntryKind::File)
                .ok_or_else(|| Error::new(ErrorCode::FileNotFound, "move source file is absent"))?;
            let file = HistoryFileId::parse(&entry.file_id)?;
            let capture = self.capture_retained_file_history_move_target(
                FileHistoryMoveRequest {
                    namespace: &namespace,
                    source_file_id: &file,
                },
                replacement,
                &cancellation,
            )?;
            self.project_file_history_move(capture, principal, operation, &cancellation)?
        };
        if let Some(source) = captured_source {
            if source.file_id != *projection.capture().source_file_id()
                || !source.matches(&self.state, projection.capture().namespace())?
            {
                return Err(Error::new(ErrorCode::WorkspaceChanged,
                    "move source differs from the originally captured entry and file"));
            }
        }
        let original = self.save_file_history_move_recovery(&projection, &cancellation)?;
        #[cfg(feature = "fault-injection")]
        self.failpoint("move_after_original_intent")?;
        let recovery = original.digest();
        self.prepare_file_history_move_target(&projection, recovery, &cancellation)?;
        self.prepare_file_history_move_decision_inputs(&projection, recovery, &cancellation)?;
        self.prepare_file_history_move_native_operation(&projection, recovery, &cancellation)?;
        self.prepare_file_history_move_scoped_databases(&projection, recovery, &cancellation)?;
        self.select_file_history_move_joint_choice(&projection, recovery, &cancellation)?;
        self.prepare_file_history_move_candidate(&projection, recovery, &cancellation)?;
        self.prepare_file_history_move_ordinary(&projection, recovery, &cancellation)?;
        self.prepare_file_history_move_journal(&projection, recovery, &cancellation)?;
        let (materials, native, scoped, choice) =
            self.selected_move_inputs(&projection, recovery, &cancellation)?;
        let frozen = candidate::FrozenCandidate::capture(
            self,
            &projection,
            &native,
            &choice,
            &cancellation,
        )?;
        self.verify_move_candidate(&materials, &frozen, &cancellation)?;
        ordinary::require_execution_capacity(
            self,
            &projection,
            (&materials, &native, &scoped),
            &frozen,
            &cancellation,
        )?;
        let (candidate, outcome) = frozen.into_decision();
        let hash = object_hash("candidate", &candidate)?;
        self.finish_native_move(candidate, hash, outcome, &projection, None, false)
    }

    fn capture_move_path(
        &self,
        request: &FileHistoryMovePathRequest,
    ) -> Result<FileHistoryCopyRequest> {
        let head = |id: &str| -> Result<String> {
            let repository = self.state.repositories.get(id).ok_or_else(|| {
                Error::new(ErrorCode::RepositoryNotFound, "move repository is absent")
            })?;
            Ok(self.state.branches[&repository.main_branch_id]
                .head_commit_id
                .clone())
        };
        Ok(FileHistoryCopyRequest {
            source_repository_id: request.source_repository_id.clone(),
            source_path: request.source_path.clone(),
            expected_source_head: head(&request.source_repository_id)?,
            target_repository_id: request.target_repository_id.clone(),
            target_name: request.target_name.clone(),
            expected_target_head: head(&request.target_repository_id)?,
        })
    }

    /// Invoked only for the selected accepted slot after authenticating its old
    /// source prefix and snapshot. Recompute its entire native write set first.
    pub(in crate::engine) fn recover_selected_move(
        &mut self,
        candidate: Candidate,
        hash: String,
        next: &State,
        existing: Option<Publication>,
    ) -> Result<()> {
        let choice = self
            .history_move_choice
            .as_ref()
            .expect("invariant: selected move recovery retains its exact choice");
        choice.require_prepare_candidate(&self.config, &candidate)?;
        let (projection, native) =
            crate::business::replay_selected_move(self, next, candidate.operation_id)?;
        choice.require_native(&native)?;
        let outcome = native.candidate_draft()?;
        let versions: Vec<_> = projection
            .version_mapping()
            .values()
            .map(|id| crate::journal::VersionEvidence {
                version: next.versions[id].clone(),
                placement: next.placements[id].clone(),
            })
            .collect();
        if encode_metadata(&versions, MAX_METADATA)?
            != encode_metadata(&candidate.versions, MAX_METADATA)?
        {
            return Err(invalid(
                "selected move candidate changes its complete FULL obligations",
            ));
        }
        self.finish_native_move(candidate, hash, outcome, &projection, existing, true)?;
        Ok(())
    }

    fn finish_native_move(
        &mut self,
        candidate: Candidate,
        hash: String,
        outcome: NativeMoveCandidate,
        projection: &FileHistoryMoveProjection,
        existing: Option<Publication>,
        decided: bool,
    ) -> Result<OperationResult> {
        self.finish_move(candidate, hash, outcome,
            projection.capture().retained_version_ids().cloned().collect(),
            existing, decided, OriginalFileSelection)
    }
}
