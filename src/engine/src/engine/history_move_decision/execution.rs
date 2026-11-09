//! One vertical native move: exact intent, complete preparation, ordinary joint
//! decision, required database installation, then one visible State replacement.
use super::*;
use crate::business::NativeMoveCandidate;
use crate::journal::{Candidate, Publication, RecordType};
use crate::publication_evidence::verified_recovery_snapshot_at;
use storage_contract::history_window::HistoryFileId;

mod archive;
mod scopes;

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
impl FileHistoryMovePathRequest {
    fn validate(&self) -> Result<()> {
        storage_contract::validate_id(&self.source_repository_id, "repo")?;
        storage_contract::validate_id(&self.target_repository_id, "repo")?;
        name_policy::PortablePath::parse(&self.source_path)?;
        name_policy::validate_name(&self.target_name)?;
        Ok(())
    }
    pub(crate) fn matches(&self, original: &FileHistoryCopyRequest) -> bool {
        self.source_repository_id == original.source_repository_id
            && self.source_path == original.source_path
            && self.target_repository_id == original.target_repository_id
            && self.target_name == original.target_name
    }
}

/// Only successful ordinary completion plus all-member journal/seal observation
/// constructs this capability. Installation cannot be invoked with a bare State.
pub(crate) struct PublishedMove {
    candidate: Candidate,
    outcome: NativeMoveCandidate,
    hash: String,
    files: BTreeSet<(String, String, String)>,
}
impl PublishedMove {
    pub(crate) fn state(&self) -> &State {
        self.outcome.state()
    }
    pub(crate) fn files(&self) -> &BTreeSet<(String, String, String)> {
        &self.files
    }
}

fn conflict() -> Error {
    Error::new(
        ErrorCode::IdempotencyKeyReused,
        "move operation belongs to another actor or original path request",
    )
}

impl Engine {
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
                || !request.matches(&crate::business::original_move_namespace(original)?)
            {
                return Err(conflict());
            }
            return Ok(original.clone());
        }
        self.require_move_source_read()?;
        self.require_unfenced_membership()?;
        if self.config.generation().is_some() || self.current_namespaces.is_some() {
            return Err(Error::new(
                ErrorCode::UnsupportedOperation,
                "native move currently requires original fixed genesis bindings",
            ));
        }
        let cancellation = HistoryWindowCancellation::default();
        let projection = if let Some(original) =
            self.original_move_path_intent(request, principal, operation)?
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
            let capture = self.capture_file_history_move(
                FileHistoryMoveRequest {
                    namespace: &namespace,
                    source_file_id: &file,
                },
                &cancellation,
            )?;
            self.project_file_history_move(capture, principal, operation, &cancellation)?
        };
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
        choice.require_prepare_candidate(&candidate)?;
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
        self.require_move_choice_baseline()?;
        let files = scopes::file_scopes(self, projection, &candidate);
        scopes::require_resources(self, &candidate)?;
        scopes::verify_full(self, projection, &candidate)?;
        let mut databases = scopes::Databases::prepare(self, &candidate, &hash, decided)?;
        if !decided {
            databases.stage(self)?;
        }
        let transition =
            crate::state_projection::ProjectionTransition::new(&self.state, outcome.state(), None)?;
        let names = if decided {
            crate::container_storage::prepare_recovered_names(
                &self.config,
                &self.disk_resources,
                &transition,
                MAX_METADATA,
            )?
        } else {
            let names = crate::container_storage::prepare_names(
                &self.config,
                &self.disk_resources,
                &transition,
                MAX_METADATA,
            )?;
            names.unpublished_file_database_reservations(&files)?;
            names.verify_unpublished_file_databases()?;
            names
        };
        self.require_move_choice_baseline()?;
        scopes::require_resources(self, &candidate)?;
        // All preflight is finished. After this point errors are indeterminate,
        // never an abort or permission to release/replan the original operation.
        self.blocked = true;
        self.history_move_prepare
            .as_mut()
            .expect("invariant: every move starts with its PREPARE")
            .mark_decided();
        let finish = (|| -> Result<OperationResult> {
            if decided {
                for writer in self.journals.values_mut() {
                    writer.verify_retained_name()?;
                    writer.repair_torn_tail()?;
                    writer.verify_retained_name()?;
                }
            }
            self.complete_move_candidate(
                &candidate,
                &hash,
                outcome.state(),
                existing,
                names,
                decided,
            )?;
            self.observe_move_publication(&candidate, &hash, outcome.result())?;
            let published = PublishedMove {
                candidate,
                hash,
                outcome,
                files,
            };
            databases.install(self, &published)?;
            crate::container_storage::verify_move_files(
                &self.config,
                &self.disk_resources,
                &published,
            )?;
            self.install_move_system_index(&published)?;
            #[cfg(feature = "fault-injection")]
            self.failpoint("move_after_system_database")?;
            scopes::verify_published_full(self, &published)?;
            let roots = self.verify_move_member_closure(&published)?;
            scopes::require_resources(self, &published.candidate)?;
            archive::retire(self, &published)?;
            let result = published.outcome.result().clone();
            let snapshot = published.candidate.snapshot_hash.clone();
            self.state = std::sync::Arc::new(published.outcome.into_state());
            self.bind_recovered_history_roots(roots);
            self.snapshot = Some(snapshot);
            self.published_state_base = None;
            self.history_move_intent = None;
            self.history_move_choice = None;
            self.history_move_prepare = None;
            self.blocked = false;
            Ok(result)
        })();
        finish.map_err(|error| {
            self.metadata_integrity.invalidate();
            error.context(ErrorCode::InDoubt,
                "original move may be decided; keep both ends and reopen to finish this same operation").retryable()
        })
    }

    fn verify_move_member_closure(
        &self,
        published: &PublishedMove,
    ) -> Result<BTreeMap<String, String>> {
        let mut roots = None;
        for disk in self.members() {
            let members = std::slice::from_ref(&disk);
            let stored: Candidate = crate::journal::load_object_at(
                &self.disk_resources.get(&disk)?.root,
                &disk,
                "candidate",
                &published.hash,
            )?;
            self.verify_candidate_membership(&stored)?;
            let snapshot = verified_recovery_snapshot_at(
                &self.disk_resources,
                members,
                &stored.snapshot_hash,
            )?;
            let admitted = snapshot.for_candidate(&stored)?;
            if admitted.state() != published.state() {
                return Err(invalid(
                    "move member lost its exact complete candidate State",
                ));
            }
            let (_, current) = crate::history_package::bound_history_objects_with_roots_at(
                &self.disk_resources,
                members,
                &stored,
                Some(admitted),
            )?;
            if roots.as_ref().is_some_and(|original| original != &current) {
                return Err(invalid("move members disagree on complete history roots"));
            }
            roots = Some(current);
        }
        roots.ok_or_else(|| invalid("move publication has no original metadata membership"))
    }

    fn observe_move_publication(
        &self,
        candidate: &Candidate,
        hash: &str,
        result: &OperationResult,
    ) -> Result<()> {
        for disk in self.members() {
            let writer = self
                .journals
                .get(&disk.disk_id)
                .ok_or_else(|| invalid("move journal owner absent"))?;
            writer.verify_retained_name()?;
            let root = &self.disk_resources.get(&disk)?.root;
            let scan = crate::journal::scan_configured_at(root, &disk, &self.config)?;
            if scan.torn_tail {
                return Err(invalid("move publication journal has a torn tail"));
            }
            let frames: Vec<_> = scan
                .records
                .iter()
                .filter(|(record, _)| record.sequence >= candidate.sequence)
                .collect();
            if frames.len() != 4
                || frames
                    .iter()
                    .zip([
                        RecordType::Prepare,
                        RecordType::Accept,
                        RecordType::Finalize,
                        RecordType::Publication,
                    ])
                    .any(|((record, _), phase)| {
                        record.sequence != candidate.sequence
                            || record.candidate_hash != hash
                            || record.record_type != phase
                    })
                || frames[0].0.object_hash != candidate.snapshot_hash
                || frames[1].0.object_hash != hash
            {
                return Err(invalid(
                    "move does not retain every original ordered journal phase",
                ));
            }
            let receipt = frames[3];
            let publication: Publication =
                crate::journal::load_object_at(root, &disk, "publication", &receipt.0.object_hash)?;
            if publication.recovery.is_some() || &publication.result != result {
                return Err(invalid("move publication substituted its original outcome"));
            }
            if !publication.finalize.accepts.iter().any(|item| {
                item.disk_id == disk.disk_id
                    && item.record == frames[1].0
                    && item.frame_hash == frames[1].1
            }) || !publication.finalize_receipts.iter().any(|item| {
                item.disk_id == disk.disk_id
                    && item.record == frames[2].0
                    && item.frame_hash == frames[2].1
            }) {
                return Err(invalid(
                    "move publication lacks the actual original ACCEPT/FINALIZE frames",
                ));
            }
            let seal: crate::journal::PublicationSeal = root.read_json_observed(
                &crate::journal::publication_seal_path(&disk, hash),
                &mut |_| Ok(()),
            )?;
            crate::journal::verify_publication_seal(candidate, &publication, &seal)?;
            if !seal
                .receipts
                .iter()
                .any(|item| item.disk_id == disk.disk_id && item.frame_hash == receipt.1)
            {
                return Err(invalid(
                    "move seal does not retain the actual original journal frame",
                ));
            }
            writer.verify_retained_name()?;
        }
        Ok(())
    }
}
