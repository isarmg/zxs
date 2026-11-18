//! The service's exclusive Engine writer chooses a name from the complete
//! current root and original reservation claims, then publishes that choice once.
use crate::Engine;
use crate::container_storage::namespace_host::NamespaceHost;
use crate::directory_worktree::names;
use crate::persist::snapshot_worktree::{AllocatedWorktreeReservation, Record};
use crate::snapshot_worktree::instance::{MountParent, prepare, publish, require_initial, with_worktree};
use crate::snapshot_worktree::registry::ReceiptIndex;
use crate::snapshot_worktree::{RegistrationError, Result};
use crate::working_tree::snapshot::HeldSnapshotNode;
use storage_contract::Principal;
use storage_contract::snapshot_worktree::creation::AllocateWorktree;
use storage_contract::snapshot_worktree::{CreateWorktree, WorktreeHandle};

pub(crate) fn allocate_worktree(
    engine: &mut Engine,
    request: &AllocateWorktree,
    principal: &Principal,
) -> Result<WorktreeHandle> {
    let receipts = engine.snapshot_receipt_source()?;
    let index = receipts.index()?;
    // Check the complete original intent before consulting current names. A
    // retry never chooses a successor after another operation occupies a name.
    if let Some(handle) = index.retry_allocation(request, principal)? {
        with_worktree(engine, handle, |_| Ok(()))?;
        return Ok(handle);
    }
    require_initial(engine)?;
    let (source, allocated) = allocation_plan(engine, &index, request)?;
    let plan = allocated.resolved.clone();
    publish(
        engine,
        principal,
        source,
        plan,
        Record::ReserveAllocatedWorktree(allocated),
    )
}

fn allocation_plan(
    engine: &Engine,
    index: &ReceiptIndex<'_>,
    request: &AllocateWorktree,
) -> Result<(HeldSnapshotNode, AllocatedWorktreeReservation)> {
    let registered = index.node(request.source)?;
    let parent = MountParent::original(registered.host())?;
    if request.parent_directory_id != parent.directory
        || request.expected_root_generation
            != registered.initialization().host.expected_root_generation
    {
        return Err(RegistrationError::RootOnly);
    }
    let host = NamespaceHost::registered(engine, parent.namespace)
        .map_err(RegistrationError::NamespaceHost)?;
    let (budget, mut occupied) = host
        .root_mount_allocation(
            request.parent_directory_id,
            request.expected_root_generation,
        )
        .map_err(RegistrationError::NamespaceHost)?;
    occupied.extend(index.reserved_mount_names(registered.host(), request.parent_directory_id));
    let selected = names::allocate(registered.branch_name(), occupied, budget.allowed())
        .map_err(RegistrationError::WorktreeName)?;
    let resolved = CreateWorktree {
        source: request.source,
        parent_directory_id: request.parent_directory_id,
        expected_root_generation: request.expected_root_generation,
        operation_id: request.operation_id,
        name: selected.name().clone(),
    };
    let (source, resolved) = prepare(engine, index, &resolved)?;
    Ok((
        source,
        AllocatedWorktreeReservation {
            request: request.clone(),
            resolved,
        },
    ))
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/snapshot_worktree/automatic_names.rs"]
mod tests;
