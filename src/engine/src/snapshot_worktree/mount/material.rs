//! The two original nodes, real sparse instance and original ledger remain
//! held independently of the mount name. This code cannot create a FILE/node.
use crate::directory_worktree::ledger::{MountFrame, install_mount_frame, observe_mount_frame};
use crate::directory_worktree::store::HeldSparseWorktree;
use crate::engine::{PlanningContext, RegisteredNamespaceRoot};
use crate::persist::snapshot_worktree::{Record, content_batch, mount_change};
use crate::snapshot_worktree::{RegistrationError, Result};
use crate::working_tree::snapshot::{HeldContentNode, HeldSnapshotNode, PinnedSnapshotRepository};
use crate::{OperationResult, State, ValidatedConfig};
use std::sync::Arc;
use storage_contract::Principal;
use storage_contract::working_tree::MaterialCommitment;

pub(crate) struct HeldMount {
    source: HeldSnapshotNode,
    repository: PinnedSnapshotRepository,
    committed: HeldContentNode,
    sparse: HeldSparseWorktree,
    original: content_batch::Reservation,
    completed: content_batch::Publication,
    pub(super) plan: mount_change::Reservation,
    pub(super) value: mount_change::Publication,
    pub(super) actor: Principal,
    _descriptors: crate::engine::PayloadProofReservation,
}

impl HeldMount {
    pub(super) fn open(
        host: &RegisteredNamespaceRoot<'_>,
        plan: &mount_change::Reservation,
        original: &content_batch::Reservation,
        completed: &content_batch::Publication,
        actor: &Principal,
        commitment: MaterialCommitment,
    ) -> Result<Self> {
        let grant = crate::engine::payload_proof_budget().reserve(11).ok_or_else(|| {
            RegistrationError::Publication(crate::engine::fresh_layout::FreshLayoutError::ContainerBudget.into())
        })?;
        let binding = host.binding();
        if binding.registration != plan.host.registration || binding.root_id != plan.root.root_id
            || binding.generation != plan.root.expected_root_generation
            || binding.root_entity != plan.host.root_entity || binding.roots.common_data != plan.host.common_data
            || binding.roots.common_data.directory_id != plan.root.directory_id
            || !plan.host.directories.is_empty() || plan.source != original.source
            || plan.committed != original.handle() || plan.content_reserve != original.request.operation_id
            || plan.ledger != original.ledger || plan.instance != original.instance
            || completed.reserve != plan.content_reserve
            || Record::ReserveCleanMount(plan.clone()).commitment().map_err(RegistrationError::Codec)? != commitment
        {
            return Err(RegistrationError::Source);
        }
        let common = host.common().map_err(RegistrationError::Publication)?;
        let source = HeldSnapshotNode::reopen_at_host(common, original.source_binding)
            .map_err(RegistrationError::NodeReadback)?;
        let repository = source.pin_repository(common).map_err(RegistrationError::NodeReadback)?;
        let sparse = HeldSparseWorktree::reopen(&source, plan.instance).map_err(RegistrationError::EditInstance)?;
        let committed = HeldContentNode::observe(&source, &repository, &sparse, original, completed, true)
            .map_err(RegistrationError::NodeReadback)?;
        committed.require_installed(&source, &repository, &sparse, original)
            .map_err(RegistrationError::NodeReadback)?;
        let checkpoint = crate::directory_worktree::ledger::mount_checkpoint(plan)
            .map_err(RegistrationError::Ledger)?;
        Ok(Self {
            source, repository, committed, sparse,
            original: original.clone(), completed: completed.clone(), plan: plan.clone(),
            value: mount_change::Publication { reserve: plan.request.operation_id, reservation: commitment, checkpoint },
            actor: actor.clone(), _descriptors: grant,
        })
    }

    pub(crate) fn plan(&self) -> &mount_change::Reservation { &self.plan }
    pub(crate) fn result(&self) -> mount_change::Publication { self.value }
    pub(crate) fn principal(&self) -> &Principal { &self.actor }

    pub(crate) fn frame(&self, allow_after: bool) -> Result<MountFrame> {
        self.committed.require_installed(&self.source, &self.repository, &self.sparse, &self.original)
            .map_err(RegistrationError::NodeReadback)?;
        let result = observe_mount_frame(
            &self.source, &self.plan, &self.original, &self.completed, allow_after,
        ).map_err(RegistrationError::Ledger)?;
        if result.after != self.value.checkpoint { return Err(RegistrationError::CommitScope); }
        Ok(result)
    }

    pub(crate) fn require_candidate(&self, candidate: &crate::journal::Candidate) -> Result<()> {
        if candidate.generation().is_some() || !candidate.versions.is_empty()
            || candidate.operation_id != self.plan.publish_operation
            || candidate.request_digest != Record::PublishCleanMount(self.value)
                .digest(&self.actor).map_err(RegistrationError::Codec)?
        {
            return Err(RegistrationError::CommitScope);
        }
        Ok(())
    }

    pub(crate) fn require_installed(&self) -> Result<()> {
        if self.frame(true)?.observed.is_none() { return Err(RegistrationError::CommitScope); }
        Ok(())
    }

    pub(crate) fn install(
        &self, proof: &crate::engine::FinalizedOriginalContent<'_>,
        cut: &mut impl FnMut(&str) -> crate::Result<()>,
    ) -> Result<()> {
        install_mount_frame(proof, &self.source, &self.plan, self.value, &self.original, &self.completed)
            .map_err(RegistrationError::Ledger)?;
        cut("after_worktree_clean_mount_frame").map_err(RegistrationError::Publication)?;
        self.require_installed()
    }
}

pub(crate) struct PreparedMount {
    pub(crate) held: HeldMount,
    state: Arc<State>,
    config: ValidatedConfig,
    resources: Arc<crate::disk::DiskResources>,
}

impl PreparedMount {
    pub(super) fn seal(ctx: &PlanningContext<'_>, held: HeldMount) -> Result<Self> {
        held.frame(false)?;
        Ok(Self { held, state: Arc::clone(ctx.state), config: ctx.config.clone(),
            resources: Arc::clone(ctx.disk_resources) })
    }

    pub(crate) fn require_before(&self, ctx: &PlanningContext<'_>) -> Result<()> {
        if !Arc::ptr_eq(&self.state, ctx.state) || !self.config.same_admission(ctx.config)
            || !Arc::ptr_eq(&self.resources, ctx.disk_resources) || ctx.current_namespaces.is_some()
            || ctx.config.generation().is_some()
        {
            return Err(RegistrationError::PublicationChanged);
        }
        self.held.frame(false)?;
        Ok(())
    }

    pub(crate) fn require_plan(
        &self, ctx: &PlanningContext<'_>, next: &State, prepared: &[crate::engine::Prepared],
        result: &OperationResult,
    ) -> Result<()> {
        self.require_before(ctx)?;
        if !prepared.is_empty() { return Err(RegistrationError::CommitScope); }
        let index = ctx.snapshot_receipts()?;
        let record = Record::PublishCleanMount(self.held.value);
        let (original, actor, commitment) = index.frozen_mount(self.held.value.reserve)?;
        if original != &self.held.plan || actor != &self.held.actor || commitment != self.held.value.reservation {
            return Err(RegistrationError::CommitScope);
        }
        index.require_planned(next, result, actor, &record)?;
        require_metadata(ctx.config, ctx.state, next, result, actor, &record)?;
        index.require_mount_namespace(next, ctx.config.max_entries)
    }
}

/// Neither occupancy nor Hide/Restore may change FILE, directory, auth or
/// other unrelated facts. Use the actual ordinary metadata planner's generations.
pub(super) fn require_metadata(
    config: &ValidatedConfig, previous: &State, next: &State, result: &OperationResult,
    actor: &Principal, record: &Record,
) -> Result<()> {
    let native = crate::persist::snapshot_worktree::decode(result).map_err(RegistrationError::Codec)?
        .ok_or(RegistrationError::Source)?;
    let sequence = storage_contract::Sequence::new(result.sequence);
    if native.record != *record || native.membership_epoch != config.membership_epoch()
        || !native.protection.is_empty() || next.sequence != sequence.get()
        || previous.operations.contains_key(&result.operation_id)
        || native.receipt(result.operation_id, sequence, actor).map_err(RegistrationError::Codec)? != *result
    {
        return Err(RegistrationError::PublicationChanged);
    }
    let mut projected = crate::business::file_draft::unborn::begin(previous, sequence)
        .map_err(RegistrationError::Publication)?;
    crate::container_storage::capture_changed_names(previous, &mut projected);
    let expected = crate::business::file_draft::unborn::finish_state(previous, projected, result, actor)
        .map_err(RegistrationError::Publication)?;
    if !crate::publication_shape::same_transition(previous, &expected, next) {
        return Err(RegistrationError::PublicationChanged);
    }
    Ok(())
}
