//! Original publication admission, separate from any mutable ledger or name.
use super::{HeldMount, ReceiptIndex, RegistrationError, Result, material::require_metadata};
use crate::engine::{ContentReplayCut, DirectoryReplayPrefix, NativeMountHistory, PlanningContext, RegisteredNamespaceRoot};
use crate::journal::Candidate;
use crate::persist::snapshot_worktree::{self as codec, Record, mount_change};
use crate::{Engine, State, ValidatedConfig};
use storage_contract::Principal;

pub(crate) struct ObservedMount {
    held: HeldMount,
    cut: ContentReplayCut,
}

fn original<'a>(
    index: &'a ReceiptIndex<'_>, previous: &State, config: &ValidatedConfig,
    candidate: &Candidate, next: &State,
) -> Result<(mount_change::Publication, &'a mount_change::Reservation, &'a Principal)> {
    let receipt = next.operations.get(&candidate.operation_id).ok_or(RegistrationError::Source)?;
    let decoded = codec::decode(receipt).map_err(RegistrationError::Codec)?.ok_or(RegistrationError::Source)?;
    let Record::PublishCleanMount(result) = decoded.record else { return Err(RegistrationError::Source); };
    let (plan,actor,commitment) = index.frozen_mount(result.reserve)?;
    if result.reservation != commitment || plan.publish_operation != candidate.operation_id {
        return Err(RegistrationError::CommitScope);
    }
    let record = Record::PublishCleanMount(result);
    index.require_planned(next, receipt, actor, &record)?;
    index.require_mount_namespace(next, config.max_entries)?;
    require_metadata(config, previous, next, receipt, actor, &record)?;
    Ok((result,plan,actor))
}

impl ObservedMount {
    fn at(
        index: &ReceiptIndex<'_>, host: &RegisteredNamespaceRoot<'_>, candidate: &Candidate,
        plan: &mount_change::Reservation, result: mount_change::Publication, actor: &Principal, cut: ContentReplayCut,
    ) -> Result<Self> {
        let base = index.clean_base(&plan.request, Some(result.reserve))?;
        let held = HeldMount::open(host,plan,base.plan,base.result,actor,result.reservation)?;
        if result != held.result() { return Err(RegistrationError::CommitScope); }
        held.require_candidate(candidate)?;
        let frame = held.frame_at(cut.can_have_moved(), Some(cut.mount_history()))?;
        if cut.must_be_installed() && frame.observed.is_none() { return Err(RegistrationError::CommitScope); }
        Ok(Self { held, cut })
    }

    pub(crate) fn from_replay(
        source: &DirectoryReplayPrefix<'_>, candidate: &Candidate, next: &State, cut: ContentReplayCut,
    ) -> Result<Self> {
        cut.require(source.config(), source.resources(), candidate).map_err(RegistrationError::Publication)?;
        if candidate.previous_snapshot != *source.snapshot() { return Err(RegistrationError::CommitScope); }
        let (result,plan,actor) = original(source.index(), source.previous(),source.config(),candidate,next)?;
        let host = source.namespace_host_root(plan.host.namespace.repository_id, plan.host.namespace.branch_id)
            .map_err(RegistrationError::Publication)?;
        Self::at(source.index(), &host, candidate, plan, result, actor, cut)
    }

    pub(crate) fn from_engine(engine: &Engine, candidate: &Candidate, next: &State, cut: ContentReplayCut) -> Result<Self> {
        let ctx = engine.planning_context();
        cut.require(ctx.config,ctx.disk_resources,candidate).map_err(RegistrationError::Publication)?;
        if ctx.current_namespaces.is_some() || candidate.previous_snapshot != *ctx.snapshot {
            return Err(RegistrationError::CommitScope);
        }
        let index = ctx.snapshot_receipts()?;
        let (result,plan,actor) = original(&index,ctx.state,ctx.config,candidate,next)?;
        let host = ctx.namespace_host_root(plan.host.namespace.repository_id,plan.host.namespace.branch_id)
            .map_err(RegistrationError::Publication)?;
        Self::at(&index,&host,candidate,plan,result,actor,cut)
    }

    pub(crate) fn require_context(&self, ctx: &PlanningContext<'_>, candidate: &Candidate) -> Result<()> {
        self.cut.require(ctx.config,ctx.disk_resources,candidate).map_err(RegistrationError::Publication)?;
        if candidate.previous_snapshot != *ctx.snapshot || ctx.current_namespaces.is_some() { return Err(RegistrationError::CommitScope); }
        let index = ctx.snapshot_receipts()?;
        let (plan,actor,hash) = index.frozen_mount(self.held.value.reserve)?;
        if plan != self.held.plan() || actor != self.held.principal() || hash != self.held.value.reservation {
            return Err(RegistrationError::CommitScope);
        }
        let host = ctx.namespace_host_root(plan.host.namespace.repository_id,plan.host.namespace.branch_id)
            .map_err(RegistrationError::Publication)?;
        self.held.require_original_host(&host)?;
        if !self.cut.must_be_installed() { self.held.require_vacant_name(&host,ctx.config.max_entries)?; }
        self.held.require_candidate(candidate)
    }

    pub(crate) fn require_installed(&self) -> Result<()> {
        if self.held.frame_at(true, Some(self.cut.mount_history()))?.observed.is_none() {
            return Err(RegistrationError::CommitScope);
        }
        Ok(())
    }
    pub(crate) fn install(&self, proof: &crate::engine::FinalizedOriginalContent<'_>,
        cut: &mut impl FnMut(&str) -> crate::Result<()>) -> Result<()> {
        self.held.install(proof,cut)
    }
}

/// Even a historical reserve cannot bless a pathname or an unrelated frame.
/// Verify its precise predecessor, or its exact now-decided historical successor.
pub(crate) fn reserve(
    source: &DirectoryReplayPrefix<'_>, candidate: &Candidate, next: &State, history: &NativeMountHistory,
) -> Result<()> {
    let op = candidate.operation_id;
    let receipt = next.operations.get(&op).ok_or(RegistrationError::Source)?;
    let actor: Principal = next.operation_principals.get(&op).ok_or(RegistrationError::Source)?.parse()
        .map_err(RegistrationError::Allocation)?;
    let value = codec::decode(receipt).map_err(RegistrationError::Codec)?.ok_or(RegistrationError::Source)?;
    let Record::ReserveCleanMount(plan) = &value.record else { return Err(RegistrationError::Source); };
    if candidate.generation().is_some() || !candidate.versions.is_empty() || candidate.previous_snapshot != *source.snapshot()
        || plan.request.operation_id != op || candidate.request_digest != receipt.request_digest || receipt.sequence != candidate.sequence
    { return Err(RegistrationError::CommitScope); }
    let index = source.index();
    index.require_planned(next, receipt, &actor, &value.record)?;
    index.require_new_mount_namespace(next, source.config().max_entries, &value.record)?;
    index.require_original_restore_name(plan,source.config().max_entries)?;
    require_metadata(source.config(), source.previous(), next, receipt, &actor, &value.record)?;
    let base = index.clean_base(&plan.request,None)?;
    let host = source.namespace_host_root(plan.host.namespace.repository_id,plan.host.namespace.branch_id)
        .map_err(RegistrationError::Publication)?;
    let commitment = value.record.commitment().map_err(RegistrationError::Codec)?;
    let held = HeldMount::open(&host,plan,base.plan,base.result,&actor,commitment)?;
    let decision = history.rows(plan.request.target.worktree_id).iter().find(|r| r.plan.request.operation_id == op);
    if decision.is_some_and(|row| row.plan != *plan || row.result != held.result()) { return Err(RegistrationError::CommitScope); }
    let frame = held.frame_at(decision.is_some(), Some(history))?;
    if decision.is_some_and(|d| d.published) && frame.observed.is_none() { return Err(RegistrationError::CommitScope); }
    Ok(())
}
