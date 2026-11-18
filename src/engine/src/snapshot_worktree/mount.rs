//! A clean visibility change has one native reserve and one native decision.
//! It never publishes contents or creates another physical instance.
mod material;
pub(crate) mod recovery;
pub(crate) use material::{HeldMount, PreparedMount};
use super::registry::{CleanBase, Presence};
use super::{ReceiptIndex, RegistrationError, Result};
use crate::Engine;
use crate::container_storage::namespace_host::NamespaceHost;
use crate::directory_worktree::lifecycle::Mount;
use crate::directory_worktree::names;
use crate::persist::snapshot_worktree::{Record, mount_change::{Reservation, Resolved}};
use name_policy::{LookupName, Name};
use storage_contract::{OperationId, Principal};
use storage_contract::snapshot_worktree::mount_change::{MountAction, MountChange, MountChangeReceipt, MountOutcome, RestoreNaming};

fn normal(name: &LookupName) -> Result<Name> {
    Name::try_from(name.clone()).map_err(|_| RegistrationError::Mount)
}

fn initial(engine: &Engine) -> Result<()> {
    if engine.config.generation().is_some()
        || engine.admitted_current_namespaces().map_err(RegistrationError::Publication)?.is_some()
    {
        return Err(RegistrationError::InitialOnly);
    }
    Ok(())
}

fn resolve(
    engine: &Engine, index: &ReceiptIndex<'_>, request: &MountChange, base: &CleanBase<'_>,
) -> Result<Resolved> {
    let parent = base.mount.mount.parent;
    let generation = base.plan.root.expected_root_generation;
    let host = NamespaceHost::registered(engine, base.plan.host.namespace)
        .map_err(RegistrationError::NamespaceHost)?;
    let (budget, mut occupied) = host.root_mount_allocation(parent, generation)
        .map_err(RegistrationError::NamespaceHost)?;
    match (&request.action, base.mount.presence) {
        (MountAction::Hide { committed, visible }, Presence::Visible(found))
            if *committed == base.plan.handle() && *visible == found => {
            host.require_root_mount(parent, generation, &normal(&base.mount.mount.name)?)
                .map_err(RegistrationError::NamespaceHost)?;
            Ok(Resolved::Hide {})
        }
        (MountAction::Restore { deletion, naming }, Presence::Hidden(found)) if *deletion == found => {
            occupied.extend(index.reserved_mount_names(&base.plan.host, parent));
            let old = &base.mount.mount.name;
            let name = match naming {
                RestoreNaming::Explicit { name } => name.clone(),
                RestoreNaming::OriginalOrAutomatic {} if occupied.iter().all(|n| n.key() != old.key()) =>
                    normal(old)?,
                RestoreNaming::OriginalOrAutomatic {} => {
                    let source = index.node(base.plan.source)?;
                    names::allocate(source.branch_name(), occupied.iter().cloned(), budget.allowed())
                        .map_err(RegistrationError::WorktreeName)?.name().clone()
                }
            };
            if occupied.iter().any(|n| n.key() == name.key()) {
                return Err(RegistrationError::Mount);
            }
            host.require_root_mount(parent, generation, &name).map_err(RegistrationError::NamespaceHost)?;
            names::require_mounted_paths(&name, base.result.prepared_node.entries.iter().map(|e| &e.path))
                .map_err(RegistrationError::WorktreeName)?;
            Ok(Resolved::Restore { mount: Mount { parent, name: name.into() } })
        }
        _ => Err(RegistrationError::CommitScope),
    }
}

fn open(
    engine: &Engine, base: &CleanBase<'_>, plan: &Reservation, actor: &Principal,
    commitment: storage_contract::working_tree::MaterialCommitment,
) -> Result<HeldMount> {
    let host = engine.namespace_host_root(plan.host.namespace.repository_id, plan.host.namespace.branch_id)
        .map_err(RegistrationError::Publication)?;
    HeldMount::open(&host, plan, base.plan, base.result, actor, commitment)
}

fn fresh(
    engine: &mut Engine, request: &MountChange, actor: &Principal,
) -> Result<()> {
    let selected = engine.snapshot_receipt_source()?;
    let index = selected.index()?;
    let base = index.clean_base(request, None)?;
    let resolved = resolve(engine, &index, request, &base)?;
    let plan = Reservation {
        request: request.clone(),
        content_reserve: base.plan.request.operation_id,
        source: base.plan.source,
        committed: base.plan.handle(),
        host: base.plan.host.clone(),
        root: base.plan.root,
        instance: base.plan.instance,
        ledger: base.plan.ledger,
        before: base.before,
        prior_mount: base.mount.mount.clone(),
        resolved,
        publish_operation: OperationId::generate().map_err(RegistrationError::Allocation)?,
    };
    let record = Record::ReserveCleanMount(plan.clone());
    let commitment = record.commitment().map_err(RegistrationError::Codec)?;
    open(engine, &base, &plan, actor, commitment)?.frame(false)?;
    super::publication::publish_new(engine, request.operation_id, actor, &record)?;
    #[cfg(feature = "fault-injection")]
    engine.failpoint("after_worktree_clean_mount_reserve").map_err(RegistrationError::Publication)?;
    Ok(())
}

pub(super) fn reserve(
    engine: &mut Engine, request: &MountChange, actor: &Principal,
) -> Result<MountChangeReceipt> {
    if let Some(prior) = engine.snapshot_receipts()?.mount_retry(request, actor)? { return Ok(prior); }
    initial(engine)?;
    engine.require_unstarted_mount_slot(request, actor, None).map_err(RegistrationError::Publication)?;
    fresh(engine, request, actor)?;
    let index = engine.snapshot_receipts()?;
    index.clean_mount_receipt(index.mount_owner(request.target)?, request.operation_id, actor)
}

pub(super) fn change(
    engine: &mut Engine, request: &MountChange, actor: &Principal,
) -> Result<MountChangeReceipt> {
    let prior = reserve(engine, request, actor)?;
    if !matches!(prior.outcome, MountOutcome::Pending {}) { return Ok(prior); }
    initial(engine)?;
    let source = engine.snapshot_receipt_source()?;
    let index = source.index()?;
    let (plan, principal, commitment) = index.frozen_mount(request.operation_id)?;
    if &plan.request != request || principal != actor {
        return Err(RegistrationError::Digest(request.operation_id));
    }
    engine.require_unstarted_mount_slot(request, actor, Some(plan.publish_operation))
        .map_err(RegistrationError::Publication)?;
    let base = index.clean_base(request, Some(request.operation_id))?;
    // A reserved restore never reallocates because an unrelated name was freed.
    let host = NamespaceHost::registered(engine, plan.host.namespace).map_err(RegistrationError::NamespaceHost)?;
    let chosen = match &plan.resolved { Resolved::Hide {} => &plan.prior_mount,
        Resolved::Restore { mount } => mount };
    host.require_root_mount(chosen.parent, plan.root.expected_root_generation, &normal(&chosen.name)?)
        .map_err(RegistrationError::NamespaceHost)?;
    let held = open(engine, &base, plan, actor, commitment)?;
    let owner = plan.host.namespace.repository_id;
    let prepared = PreparedMount::seal(&engine.planning_context(), held)?;
    engine.publish_clean_mount(prepared).map_err(RegistrationError::Publication)?;
    #[cfg(feature = "fault-injection")]
    engine.failpoint("after_worktree_clean_mount_response").map_err(RegistrationError::Publication)?;
    let receipt = engine.snapshot_receipts()?.clean_mount_receipt(owner, request.operation_id, actor)?;
    if matches!(receipt.outcome, MountOutcome::Pending {}) { return Err(RegistrationError::Pending(request.operation_id)); }
    Ok(receipt)
}
