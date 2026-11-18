//! Combined physical preparation without a provisional node receipt.
//! The old ordinary decision path remains closed to this private native mode.
use super::*;
mod content;
pub(crate) use content::{BudgetedOriginalContent, FinalizedOriginalContent};
use crate::engine::file_draft::WorkingBatchHandoff;
use crate::engine::fresh_layout::{FreshLayoutError as Failure, containers::ContainerPreparation};
use crate::snapshot_worktree::drafts::{
    DirectoryDecisionInput, DirectoryPhysicalInput, DirectoryPublicationError,
    DirectoryPublicationInput, PreparedDirectoryMaterial, PreparedDirectoryProposal,
};
use crate::working_tree::snapshot::{SnapshotNodeBinding, SnapshotReference};

/// The constructor below binds one actually admitted aggregate preparation to
/// its exact directory input. This is not a persistent allocation reservation.
pub(crate) struct BudgetedDirectoryPreparation {
    input: Arc<DirectoryPhysicalInput>,
    // The node owns this permit until all three retained directories close.
    _descriptors: PayloadProofReservation,
}

impl BudgetedDirectoryPreparation {
    pub(crate) fn require_input(
        &self,
        context: &crate::engine::PlanningContext<'_>,
        input: &DirectoryPhysicalInput,
    ) -> Result<()> {
        if !std::ptr::eq(self.input.as_ref(), input) {
            return Err(Failure::ContainerScope.into());
        }
        input.require_context(context).map_err(physical_error)
    }

    /// Store code consumes the same already-pinned repository, not a path or a
    /// caller-created node identity. No write can use this for a second node.
    pub(crate) fn require_node(
        &self, reference: SnapshotReference, original: SnapshotNodeBinding,
    ) -> Result<()> {
        if self.input.reference() != reference || self.input.original_binding() != original {
            return Err(Failure::ContainerScope.into());
        }
        Ok(())
    }
}

/// Complete physical products, still unpublished. These fields stay together
/// until the real node feedback has passed full reconstruction and final native
/// metadata admission. No caller can substitute FILE or directory occurrences.
pub(crate) struct PreparedDirectoryFiles {
    source: WorkingBatchHandoff,
    directory: PreparedDirectoryMaterial,
    verified: VerifiedPayloadSet,
    containers: Arc<crate::container_storage::PreparedContainerEntities>,
    metadata_epoch: super::super::provenance::MetadataEpoch,
}

impl PreparedDirectoryFiles {
    pub(crate) fn files(&self) -> &WorkingBatchHandoff { &self.source }
    pub(crate) fn directory(&self) -> &PreparedDirectoryMaterial { &self.directory }

    pub(crate) fn require_context(
        &self, context: &crate::engine::PlanningContext<'_>,
    ) -> Result<()> {
        self.source.require_context(context)?;
        self.directory.require_context(context).map_err(physical_error)?;
        self.verified.validate_containers()
    }

    /// The final native envelope is issued only after actual node feedback.
    /// Until then this checks neither a request digest nor a snapshot hash.
    pub(crate) fn require_candidate(
        &self, context: &crate::engine::PlanningContext<'_>, candidate: &Candidate,
    ) -> Result<()> {
        self.require_context(context)?;
        self.source.require_candidate(candidate)?;
        self.verified.validate(context.config, candidate)
    }

    pub(crate) fn require_material(&self) -> Result<()> {
        self.source.require_material()?;
        self.directory.require_material().map_err(physical_error)?;
        self.verified.validate_containers()
    }
}

fn physical_error(source: crate::snapshot_worktree::drafts::DirectoryPhysicalError) -> Error {
    DirectoryPublicationError::Physical(source).into_engine()
}

impl crate::engine::PlanningContext<'_> {
    /// The complete FILE set and independent directory copies are charged in
    /// one calculation before the first new physical object. It does not infer
    /// cumulative disk occupancy from State.used_bytes or release orphan bytes.
    pub(crate) fn prepare_directory_material(
        &self, input: DirectoryPublicationInput, maximum: usize,
    ) -> Result<(DirectoryDecisionInput, PreparedDirectoryFiles)> {
        let (decision, source, prepared, directory, envelope) = input.into_material(self)
            .map_err(DirectoryPublicationError::into_engine)?;
        self.require_worktree_mount_reservations(source.state())?;
        let containers = ContainerPreparation::for_working_batch(self, &source, &prepared)?;
        let metadata_epoch = self.metadata_integrity.capture();
        let mut required = PhysicalRequirements::new(self);
        let existing = self.observe_existing_payloads(&prepared, &mut required)?;
        let owner = self.config.disks.iter()
            .find(|disk| disk.disk_id == directory.owner().to_string())
            .ok_or(Failure::Registration)?;
        required.add(owner, directory.cost().total_bytes())?;

        // This is real prospective FILE metadata and an encode-only upper
        // envelope, not a fake OperationResult/Binding. The final radix graph
        // can differ after feedback and must pass the full preflight again.
        let snapshot = prepare_snapshot(source.state(), maximum)?;
        let (_, histories) = crate::history_package::prepare_history_manifests(
            source.state(), maximum,
        )?;
        if envelope.maximum_bytes() > maximum {
            return Err(Failure::ContainerBudget.into());
        }
        let mut metadata_bytes = envelope.maximum_bytes() as u64;
        snapshot.visit_prepared_objects(&mut |_, _, bytes| {
            metadata_bytes = metadata_bytes.checked_add(bytes.len() as u64)
                .ok_or(Failure::PhysicalBytesOverflow)?;
            Ok(())
        })?;
        for history in histories {
            metadata_bytes = metadata_bytes.checked_add(history.bytes().len() as u64)
                .ok_or(Failure::PhysicalBytesOverflow)?;
        }
        for disk in self.members() {
            required.add(&disk, metadata_bytes)?;
        }
        required.check()?;
        source.require_context(self)?;
        directory.require_context(self).map_err(physical_error)?;
        let descriptors = payload_proof_budget().reserve(directory.cost().retained_descriptors())
            .ok_or(Failure::ContainerBudget)?;
        let permit = BudgetedDirectoryPreparation {
            input: Arc::clone(&directory), _descriptors: descriptors,
        };
        let versions = prepared.iter().map(|payload| VersionEvidence {
            version: payload.version.clone(), placement: payload.placement.clone(),
        }).collect::<Vec<_>>();
        let payloads = PayloadPreparation::new(
            self.config, self.disk_resources, &versions, Arc::clone(self.metadata_integrity),
        )?;
        // This restricted plan binds existing containers only. All retained
        // parent/container descriptor reservations happen before BODY/node I/O.
        let entities = Arc::new(crate::container_storage::PreparedContainerEntities::prepare(
            self, &containers,
        )?);
        let actual = containers.finish(&entities)?;
        if !crate::publication_shape::same_transition(self.state, source.state(), &actual) {
            return Err(Failure::ContainerScope.into());
        }
        source.require_context(self)?;
        source.require_prepared(&prepared)?;
        self.prepare_payloads(&prepared, existing, &entities)?;
        let verified = payloads.verify_with_containers(self.config, Arc::clone(&entities))?;
        source.require_context(self)?;
        let directory = directory.prepare(self, permit).map_err(physical_error)?;
        let result = PreparedDirectoryFiles {
            source, directory, verified, containers: entities, metadata_epoch,
        };
        result.require_context(self)?;
        Ok((decision, result))
    }
}

/// This product outlives all metadata writes and the actual native decision.
/// It is created only by consuming both prepared physical domains and the
/// complete reconstruction with the real directory feedback.
pub(super) struct WorkingDirectoryGuard {
    proposal: PreparedDirectoryProposal,
    files: WorkingBatchHandoff,
    material: PreparedDirectoryMaterial,
}

impl WorkingDirectoryGuard {
    pub(super) fn require_context(
        &self, context: &crate::engine::PlanningContext<'_>, candidate: &Candidate,
    ) -> Result<()> {
        self.proposal.require_context(context, &self.files, &self.material)
            .map_err(DirectoryPublicationError::into_engine)?;
        self.require_candidate(candidate)
    }

    /// Called after the original PREPARE advances its own slot, and again
    /// before installation. It does not accept a new State/config occurrence.
    pub(super) fn require_candidate(&self, candidate: &Candidate) -> Result<()> {
        self.proposal.require_candidate(candidate, &self.files, &self.material)
            .map_err(DirectoryPublicationError::into_engine)?;
        self.material.require_material().map_err(physical_error)
    }
}

impl PreparedDirectoryFiles {
    /// No BODY is prepared a second time here. All variable-size native
    /// metadata is admitted from the actual feedback before any metadata store
    /// or FILE database/name preparation. Failure leaves unpublished material,
    /// never a partial working clear or a synthetic successful receipt.
    fn finish(
        self, context: &crate::engine::PlanningContext<'_>,
        proposal: PreparedDirectoryProposal, maximum: usize, cache_maximum: usize,
    ) -> Result<DetachedPublication> {
        self.require_context(context)?;
        proposal.require_context(context, &self.source, &self.directory)
            .map_err(DirectoryPublicationError::into_engine)?;
        let next = proposal.state().clone();
        let result = proposal.result().clone();
        let members = context.members();
        let config_hash = object_hash("publication-config", &context.config)?;
        let mut snapshot = prepare_snapshot(&next, maximum)?;
        snapshot.metadata_integrity = Some(Arc::clone(context.metadata_integrity));
        let (history_basis, manifests) = crate::history_package::prepare_history_manifests(
            &next, maximum,
        )?;
        let versions = self.source.items().iter().filter_map(|item| match item.result() {
            super::super::file_draft::WorkingBatchResult::Retained(_) => None,
            super::super::file_draft::WorkingBatchResult::Published(content) => Some(
                VersionEvidence { version: content.version.clone(),
                    placement: content.placement.clone() },
            ),
        }).collect();
        let candidate = bind_candidate(context, CandidateData {
            pool_id: context.config.pool_id.clone(), sequence: next.sequence,
            transaction_id: new_id("tx")?, operation_id: result.operation_id,
            request_digest: result.request_digest.clone(),
            previous_snapshot: context.snapshot.clone(),
            snapshot_hash: snapshot.hash.clone(), versions,
            metadata_disk_ids: members.iter().map(|disk| disk.disk_id.clone()).collect(),
            history_roots: history_basis.reference().clone(),
        });
        crate::metadata_budget::preflight_publication_prepared(
            context, &candidate, &result, &next, None, maximum, Some((&snapshot, &history_basis)),
        )?;
        self.require_candidate(context, &candidate)?;
        proposal.require_candidate(&candidate, &self.source, &self.directory)
            .map_err(DirectoryPublicationError::into_engine)?;
        let projection = crate::state_projection::ProjectionTransition::new(
            context.state, &next, None,
        )?;
        crate::container_storage::validate_new_projection_paths(context.config, &projection)?;
        crate::protection::preflight_backup_directories(context, &result)?;
        let names = crate::container_storage::prepare_ordinary_names(
            context.config, context.disk_resources, &projection, cache_maximum,
            Arc::clone(&self.containers),
        )?;
        self.require_context(context)?;
        self.verified.validate(context.config, &candidate)?;
        context.metadata_integrity.observe(snapshot.store_at(
            context.disk_resources, &members, &next,
        ))?;
        let snapshot_basis = snapshot.reuse_basis();
        for manifest in manifests {
            context.metadata_integrity.observe(
                manifest.store_at(context.disk_resources, &members),
            )?;
        }
        self.require_context(context)?;
        let guard = DirectoryPublicationGuard::Working(Box::new(WorkingDirectoryGuard {
            proposal, files: self.source, material: self.directory,
        }));
        guard.require_context(context, &candidate)?;
        let history_owners = projection.into_owners();
        Ok(DetachedPublication {
            next, next_current_namespaces: None, candidate, result, verified: self.verified, names,
            directory: Some(guard), metadata_epoch: self.metadata_epoch,
            prepared_epoch: context.metadata_integrity.capture(), reused_dependencies: None,
            base_config: context.config.clone(), disk_resources: Arc::clone(context.disk_resources),
            base_root: context.root.to_path_buf(), base_snapshot: context.snapshot.clone(),
            base_state: Arc::downgrade(context.state), base_current: None,
            base_sequence: context.state.sequence, base_next_slot: context.next_slot,
            config_hash, snapshot_basis, history_basis, history_owners,
        })
    }
}

impl crate::engine::PlanningContext<'_> {
    /// Private native preparation. The eventual command dispatcher must first
    /// handle permanent Existing and use the independent exact cold kernel.
    pub(crate) fn prepare_directory_publication(
        &self, input: DirectoryPublicationInput, maximum: usize, cache_maximum: usize,
    ) -> Result<DetachedPublication> {
        let (decision, physical) = self.prepare_directory_material(input, maximum)?;
        let proposal = decision.finish(self, physical.files(), physical.directory())
            .map_err(DirectoryPublicationError::into_engine)?;
        physical.finish(self, proposal, maximum, cache_maximum)
    }
}

/// Recovery retains the original decision's directory material independently
/// of the FILE payload proof. Neither a visible result nor current HEAD can
/// rebuild this product; its only constructor takes the admitted original.
pub(crate) struct RecoveredDirectoryPayloads<'original> {
    ordinary: super::RecoveredOrdinaryPayloads<'original>,
    directory: crate::snapshot_worktree::drafts::ObservedOriginalDirectory<'original>,
}

impl RecoveredDirectoryPayloads<'_> {
    pub(in crate::engine) fn require_candidate(&self, candidate: &Candidate) -> Result<()> {
        self.directory.require_candidate(candidate)
            .map_err(DirectoryPublicationError::into_engine)?;
        self.ordinary.verified.validate(&self.ordinary.config, candidate)
    }
}

impl Engine {
    /// Verify the original FILE payloads before deciding which retained
    /// publication evidence this read-only startup probe is allowed to use.
    pub(in crate::engine) fn verify_directory_replay_payloads(
        &self, candidate: &Candidate,
        original: &crate::publication_evidence::AdmittedSnapshot,
        observed: Option<Publication>, hash: &str,
        containers: &Arc<crate::container_storage::PreparedContainerEntities>,
    ) -> Result<Option<Publication>> {
        let verified = PayloadPreparation::new(
            &self.config, &self.disk_resources, &candidate.versions,
            Arc::clone(&self.metadata_integrity),
        )?.verify_with_containers(&self.config, Arc::clone(containers))?;
        let publication = match observed {
            Some(publication) => Some(publication),
            None => crate::degraded_recovery::discover_directory_publication(
                self, candidate, hash, containers,
            )?,
        };
        if let Some(publication) = &publication {
            original.require_finalized_outcome(candidate, publication)?;
        }
        verified.validate(&self.config, candidate)?;
        Ok(publication)
    }

    pub(crate) fn observe_directory_pending<'original>(
        &self, candidate: &Candidate,
        original: &'original crate::publication_evidence::AdmittedSnapshot,
        maximum: usize,
    ) -> Result<RecoveredDirectoryPayloads<'original>> {
        let context = self.planning_context();
        let directory = crate::snapshot_worktree::drafts::observe_original(
            &context, candidate, original, maximum,
        ).map_err(DirectoryPublicationError::into_engine)?;
        let containers = Arc::new(
            crate::container_storage::PreparedContainerEntities::observe_directory_candidate(
                &context, candidate, original, &directory.proof(),
            )?,
        );
        let verified = PayloadPreparation::new(
            &self.config, &self.disk_resources, &candidate.versions,
            Arc::clone(&self.metadata_integrity),
        )?.verify_with_containers(&self.config, Arc::clone(&containers))?;
        directory.require_context(&context).map_err(DirectoryPublicationError::into_engine)?;
        let ordinary = super::RecoveredOrdinaryPayloads {
            candidate: candidate.clone(), original: original.for_candidate(candidate)?.state(),
            previous: Arc::downgrade(&self.state), current: self.current_namespaces.clone(),
            config: self.config.clone(), resources: Arc::clone(&self.disk_resources),
            snapshot: self.snapshot.clone(), verified, containers,
        };
        Ok(RecoveredDirectoryPayloads { ordinary, directory })
    }

    /// Consume the current explicitly selected working set in one decision.
    /// An earlier exact result never reopens a draft or allocates a new node.
    pub(crate) fn commit_directory_working_set(
        &mut self,
        intent: crate::snapshot_worktree::drafts::DirectoryDraftIntent<'_>,
    ) -> Result<OperationResult> {
        use crate::engine::file_draft::PreparedWorkingBatch;
        use crate::snapshot_worktree::drafts::{PendingDirectoryDrafts, retry};

        if let Some(result) = retry(self, &intent)
            .map_err(DirectoryPublicationError::Source)
            .map_err(DirectoryPublicationError::into_engine)?
        {
            return Ok(result);
        }
        let pending = PendingDirectoryDrafts::capture(self, intent)
            .map_err(DirectoryPublicationError::Source)
            .map_err(DirectoryPublicationError::into_engine)?;
        let context = self.planning_context();
        context.require_manual_directory_pool()?;
        let directory = pending.directory();
        let batch = PreparedWorkingBatch::prepare(
            &context,
            Arc::clone(pending.selected()),
            directory.operation(),
            directory.principal(),
            directory.message(),
        )?;
        let node = storage_contract::working_tree::SnapshotNodeId::generate()?;
        let input = pending
            .bind_working_batch(&context, batch, node)
            .map_err(DirectoryPublicationError::Working)
            .map_err(DirectoryPublicationError::into_engine)?
            .into_publication(&context, MAX_METADATA)
            .map_err(DirectoryPublicationError::into_engine)?;
        let publication = context.prepare_directory_publication(input, MAX_METADATA, MAX_METADATA)?;
        self.decide_publication(publication)
    }

    /// The caller must already own the real retained native decision. The same
    /// ordinary completion verifies its original Candidate, whole State loan,
    /// root/source occurrence and actual payloads before emitting any receipts.
    pub(crate) fn complete_observed_directory_candidate(
        &mut self, candidate: &Candidate, hash: &str, next: &State,
        observed: RecoveredDirectoryPayloads<'_>,
    ) -> Result<super::CompletedPublication> {
        observed.directory.require_context(&self.planning_context())
            .map_err(DirectoryPublicationError::into_engine)?;
        observed.directory.require_candidate(candidate)
            .map_err(DirectoryPublicationError::into_engine)?;
        let completed = self.complete_observed_ordinary_candidate(
            candidate, hash, next, observed.ordinary,
        )?;
        observed.directory.require_candidate(candidate)
            .map_err(DirectoryPublicationError::into_engine)?;
        Ok(completed)
    }
}


/// Different original producers, one native publication guard and linearization.
pub(super) enum DirectoryPublicationGuard {
    Working(Box<WorkingDirectoryGuard>),
    Content(Box<crate::snapshot_worktree::PreparedOriginalContent>),
}
impl DirectoryPublicationGuard {
    pub(super) fn require_context(&self, ctx: &crate::engine::PlanningContext<'_>, candidate: &Candidate) -> Result<()> {
        match self {
            Self::Working(value) => value.require_context(ctx, candidate),
            Self::Content(value) => {
                value.require_before(ctx).map_err(crate::business::file_draft::worktree::failure)?;
                value.require_candidate(candidate).map_err(crate::business::file_draft::worktree::failure)
            }
        }
    }
    pub(super) fn require_candidate(&self, candidate: &Candidate) -> Result<()> {
        match self {
            Self::Working(value) => value.require_candidate(candidate),
            Self::Content(value) => value.require_candidate(candidate).map_err(crate::business::file_draft::worktree::failure),
        }
    }
    pub(super) fn before_accept(&self, ctx: &crate::engine::PlanningContext<'_>, candidate: &Candidate) -> Result<()> {
        // Shared directory drafts have their own pre-PREPARE slot token; do
        // not ask that independent producer to re-plan after next_slot moved.
        match self { Self::Working(value) => value.require_candidate(candidate),
            Self::Content(_) => self.require_context(ctx, candidate) }
    }
    pub(super) fn after_decision(&self, candidate: &Candidate) -> Result<()> {
        self.require_candidate(candidate)?;
        if let Self::Content(value) = self { value.require_installed().map_err(crate::business::file_draft::worktree::failure)?; }
        Ok(())
    }
    pub(super) fn installer(&self) -> Option<ContentInstaller<'_>> {
        match self { Self::Working(_) => None, Self::Content(value) => Some(ContentInstaller::Live(value)) }
    }
}
pub(super) enum ContentInstaller<'a> {
    Live(&'a crate::snapshot_worktree::PreparedOriginalContent),
    Recovery(&'a crate::snapshot_worktree::ObservedOriginalContent),
}
impl ContentInstaller<'_> {
    pub(super) fn require_candidate(&self, ctx: &crate::engine::PlanningContext<'_>, candidate: &Candidate) -> Result<()> {
        match self {
            Self::Live(value) => { value.require_before(ctx).map_err(crate::business::file_draft::worktree::failure)?;
                value.require_candidate(candidate) },
            Self::Recovery(value) => value.require_context(ctx, candidate),
        }.map_err(crate::business::file_draft::worktree::failure)
    }
    pub(super) fn install(&self, engine: &mut Engine, proof: &FinalizedOriginalContent<'_>) -> Result<()> {
        #[cfg(feature = "fault-injection")]
        let mut cut = |point: &str| engine.failpoint(point);
        #[cfg(not(feature = "fault-injection"))]
        let mut cut = |_: &str| { let _ = &engine; Ok(()) };
        match self { Self::Live(value) => value.finish_moves(proof, &mut cut),
            Self::Recovery(value) => value.finish_moves(proof, &mut cut) }
            .map_err(crate::business::file_draft::worktree::failure)
    }
}
