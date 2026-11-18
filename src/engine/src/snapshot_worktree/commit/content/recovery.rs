//! Observe only the original Candidate's bound prospective State and the prior
//! admitted reservation. This is read-only even when a finalized cut is seen.
use super::material::{finish_moves, require_candidate};
use crate::directory_worktree::store::HeldSparseWorktree;
use crate::engine::PlanningContext;
use crate::journal::Candidate;
use crate::persist::snapshot_worktree::{self as codec, Record, content_batch::{Publication, Reservation}};
use crate::snapshot_worktree::{ReceiptIndex, RegistrationError, Result};
use crate::working_tree::snapshot::{HeldContentNode, HeldSnapshotNode, PinnedSnapshotRepository};
use crate::{Engine, State};
use std::sync::Arc;
use storage_contract::Principal;

pub(crate) struct ObservedOriginalContent {
    state: Arc<State>,
    config: crate::ValidatedConfig,
    resources: Arc<crate::disk::DiskResources>,
    candidate_hash: String,
    cut: crate::engine::ContentReplayCut,
    scope_entities: std::sync::OnceLock<Box<[storage_contract::working_tree::LocalEntityIdentity]>>,
    principal: Principal,
    plan: Reservation,
    value: Publication,
    original: HeldSnapshotNode,
    repository: PinnedSnapshotRepository,
    node: HeldContentNode,
    _descriptors: crate::engine::PayloadProofReservation,
}

impl ObservedOriginalContent {
    pub(crate) fn original(engine: &Engine, candidate: &Candidate,
        admitted: &crate::publication_evidence::AdmittedSnapshot, cut: crate::engine::ContentReplayCut) -> Result<Self> {
        let context = engine.planning_context();
        cut.require(context.config, context.disk_resources, candidate).map_err(RegistrationError::Publication)?;
        let index = engine.snapshot_receipts()?;
        let (plan, principal, value) = source_transition(&index, context.state, context.config, candidate, admitted)?;
        if candidate.previous_snapshot != *context.snapshot || context.current_namespaces.is_some() {
            return Err(RegistrationError::CommitScope);
        }
        let host = context.namespace_host_root(plan.host.namespace.repository_id, plan.host.namespace.branch_id)
            .map_err(RegistrationError::Publication)?;
        let (original, repository, node, descriptors) = observe(context.config, context.disk_resources,
            context.state, &host, plan, &value, cut.can_have_moved(), cut.must_be_installed(), cut.mount_history())?;
        let observed = Self { state: Arc::clone(context.state), config: context.config.clone(),
            resources: Arc::clone(context.disk_resources), candidate_hash: storage_contract::object_hash("candidate", candidate)
                .map_err(RegistrationError::Allocation)?, principal: principal.clone(), plan: plan.clone(), value,
            original, repository, node, cut, scope_entities: std::sync::OnceLock::new(), _descriptors: descriptors };
        Ok(observed)
    }

    /// Startup can advance a separate authenticated cursor while the installed
    /// Engine intentionally remains at another State (checkpoint fast path).
    /// Never replay this transition against that later or earlier installation.
    pub(crate) fn with_replay_scope<T>(source: &crate::engine::DirectoryReplayPrefix<'_>,
        candidate: &Candidate, admitted: &crate::publication_evidence::AdmittedSnapshot,
        cut: &crate::engine::ContentReplayCut, read: impl FnOnce(&super::scope::OriginalContentScope<'_>) -> crate::Result<T>) -> Result<T> {
        cut.require(source.config(), source.resources(), candidate).map_err(RegistrationError::Publication)?;
        if candidate.previous_snapshot != *source.snapshot() { return Err(RegistrationError::CommitScope); }
        let (plan, _, value) = source_transition(source.index(), source.previous(), source.config(), candidate, admitted)?;
        let host = source.namespace_host_root(plan.host.namespace.repository_id, plan.host.namespace.branch_id)
            .map_err(RegistrationError::Publication)?;
        let (original, _repository, node, _descriptors) = observe(source.config(), source.resources(), source.previous(),
            &host, plan, &value, cut.can_have_moved(), cut.must_be_installed(), cut.mount_history())?;
        let entities = std::sync::OnceLock::new();
        let scope = super::scope::OriginalContentScope::replay(plan, &value, &original, &node, cut, &entities)?;
        read(&scope).map_err(RegistrationError::Publication)
    }

    pub(crate) fn material_scope(&self, ctx: &PlanningContext<'_>, candidate: &Candidate)
        -> Result<super::scope::OriginalContentScope<'_>> {
        self.require_context(ctx, candidate)?;
        self.cut.require(ctx.config, ctx.disk_resources, candidate).map_err(RegistrationError::Publication)?;
        let sparse = HeldSparseWorktree::reopen(&self.original, self.plan.instance).map_err(RegistrationError::EditInstance)?;
        self.node.require_observed(&self.original, &self.repository, &sparse, &self.plan, &self.cut)
            .map_err(RegistrationError::NodeReadback)?;
        super::scope::OriginalContentScope::replay(&self.plan, &self.value, &self.original, &self.node, &self.cut, &self.scope_entities)
    }
    pub(crate) fn require_context(&self, context: &PlanningContext<'_>, candidate: &Candidate) -> Result<()> {
        if !Arc::ptr_eq(&self.state, context.state) || !self.config.same_admission(context.config)
            || !Arc::ptr_eq(&self.resources, context.disk_resources)
            || storage_contract::object_hash("candidate", candidate).map_err(RegistrationError::Allocation)? != self.candidate_hash
        { return Err(RegistrationError::PublicationChanged); }
        require_candidate(&self.plan, &self.value, &self.principal, candidate)?;
        let host = context.namespace_host_root(self.plan.host.namespace.repository_id, self.plan.host.namespace.branch_id)
            .map_err(RegistrationError::Publication)?;
        let original = HeldSnapshotNode::reopen_at_host(host.common().map_err(RegistrationError::Publication)?,
            self.plan.source_binding).map_err(RegistrationError::NodeReadback)?;
        self.repository.require_held_source(&original).map_err(RegistrationError::NodeReadback)?;
        self.repository.require_held_source(&self.original).map_err(RegistrationError::NodeReadback)
    }

    pub(crate) fn finish_moves(&self, decision: &crate::engine::FinalizedOriginalContent<'_>,
        cut: &mut impl FnMut(&str) -> crate::Result<()>) -> Result<()> {
        finish_moves(decision, &self.original, &self.repository, &self.node, &self.plan, &self.value, cut)
    }
    pub(crate) fn require_installed(&self) -> Result<()> {
        super::material::require_installed_at(&self.original, &self.repository, &self.node, &self.plan, &self.value, Some(self.cut.mount_history()))
    }
}

fn source_transition<'a>(index: &'a ReceiptIndex<'_>, previous: &State, config: &crate::ValidatedConfig,
    candidate: &Candidate, admitted: &crate::publication_evidence::AdmittedSnapshot)
    -> Result<(&'a Reservation, &'a Principal, Publication)> {
    let next = admitted.for_candidate(candidate).map_err(RegistrationError::Publication)?.state();
    let receipt = next.operations.get(&candidate.operation_id).ok_or(RegistrationError::Source)?;
    let decoded = codec::decode(receipt).map_err(RegistrationError::Codec)?.ok_or(RegistrationError::Source)?;
    let Record::PublishContentBatch(value) = decoded.record else { return Err(RegistrationError::Source); };
    let (plan, principal) = index.frozen_for_result(&value, candidate.operation_id)?;
    require_candidate(plan, &value, principal, candidate)?;
    index.require_planned(next, receipt, principal, &Record::PublishContentBatch(value.clone()))?;
    let projected = crate::business::file_draft::worktree_content::replay(config, previous, receipt, principal,
        plan, &value).map_err(RegistrationError::Publication)?;
    if !crate::publication_shape::same_transition(previous, &projected, next) {
        return Err(RegistrationError::PublicationChanged);
    }
    Ok((plan, principal, value))
}

fn observe(config: &crate::ValidatedConfig, resources: &crate::disk::DiskResources, previous: &State,
    host: &crate::engine::RegisteredNamespaceRoot<'_>, plan: &Reservation, value: &Publication,
    finalized: bool, published: bool, history: &crate::engine::NativeMountHistory)
    -> Result<(HeldSnapshotNode, PinnedSnapshotRepository, HeldContentNode, crate::engine::PayloadProofReservation)> {
    let descriptors = crate::engine::payload_proof_budget().reserve(9).ok_or_else(||
        RegistrationError::Publication(crate::engine::fresh_layout::FreshLayoutError::ContainerBudget.into()))?;
    let binding = host.binding();
    if binding.root_id != plan.root.root_id || binding.generation != plan.root.expected_root_generation
        || binding.root_entity != plan.host.root_entity || binding.registration != plan.host.registration
        || binding.roots.common_data != plan.host.common_data || !plan.host.directories.is_empty()
    { return Err(RegistrationError::Source); }
    let common = host.common().map_err(RegistrationError::Publication)?;
    let original = HeldSnapshotNode::reopen_at_host(common, plan.source_binding).map_err(RegistrationError::NodeReadback)?;
    let repository = original.pin_repository(common).map_err(RegistrationError::NodeReadback)?;
    let sparse = HeldSparseWorktree::reopen(&original, plan.instance).map_err(RegistrationError::EditInstance)?;
    let node = HeldContentNode::observe(&original, &repository, &sparse, plan, value, finalized || published)
        .map_err(RegistrationError::NodeReadback)?;
    // Once every member finalized, normal native FILE recovery admits its
    // original preparation or already-installed exact after image. Do not
    // require a moved FILE database to masquerade as the predecessor.
    if !published && !finalized {
        let disk = config.disks.iter().find(|d| d.disk_id == plan.host.registration.disk_id)
            .ok_or(RegistrationError::CommitScope)?;
        for row in &plan.rows {
            let held = crate::engine::fresh_layout::containers::observe_replica(config, resources,
                previous, row.original.file_id, disk).map_err(RegistrationError::Publication)?;
            let version = previous.versions.get(&row.original.version_id.to_string()).ok_or(RegistrationError::Source)?;
            held.verify_version_body(version).map_err(RegistrationError::Publication)?;
        }
    }
    if published { super::material::require_installed_at(&original, &repository, &node, plan, value, Some(history))?; }
    if !finalized && !published { super::material::require_original_ledger(&original, plan, value)?; }
    Ok((original, repository, node, descriptors))
}
