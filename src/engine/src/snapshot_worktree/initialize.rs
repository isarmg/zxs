//! Capture the admitted ordinary namespace within an actually registered Initial root.
use crate::persist::snapshot_worktree::NodeAllocation;
use crate::snapshot_worktree::{RegistrationError, Result, prepare_namespace_host, publication};
use crate::Engine;
use storage_contract::snapshot_worktree::{InitializeSnapshot, SnapshotHandle};
use storage_contract::working_tree::{SnapshotBranchId, SnapshotNodeId, SnapshotRepositoryId};
use storage_contract::{OperationId, Principal};

pub(crate) fn initialize_root(
    engine: &mut Engine,
    request: &InitializeSnapshot,
    principal: &Principal,
) -> Result<SnapshotHandle> {
    // Idempotent completed reads need no live writer. Pending always stops here,
    // before opening or adopting any unregistered .zxs/node material.
    let index = engine.snapshot_receipts()?;
    if let Some(handle) = index.retry_initialization(request, principal)? {
        index.reopen_node(engine, handle)?;
        return Ok(handle);
    }
    if engine.admitted_current_namespaces().map_err(RegistrationError::Publication)?.is_some()
        || engine.config.generation().is_some()
    {
        return Err(RegistrationError::InitialOnly);
    }
    let selected = request.source.root.as_ref().map(|child| {
        let ordinary = engine.repository_for_branch(&request.source.branch_id.to_string())
            .map_err(RegistrationError::Publication)?.repository_id.parse().map_err(RegistrationError::Allocation)?;
        let selected = super::direct_child::DirectChild::capture(engine, &index, ordinary, child)?;
        if selected.context.host != request.host || selected.context.source != request.source {
            return Err(RegistrationError::PublicationChanged);
        }
        Ok(selected)
    }).transpose()?;
    drop(index);
    let host = prepare_namespace_host(
        engine, &request.source, request.host.root_id,
        request.host.expected_root_generation, request.operation_id,
    )?;
    let (actual_host, evidence) = host.reservation_evidence(engine)
        .map_err(RegistrationError::NamespaceHost)?;
    if actual_host != request.host || selected.as_ref().is_some_and(|child|
        evidence.directories.as_slice() != std::slice::from_ref(&child.directory)) {
        return Err(RegistrationError::Host);
    }
    let allocated = NodeAllocation {
        repository: SnapshotRepositoryId::generate().map_err(RegistrationError::Allocation)?,
        branch: SnapshotBranchId::generate().map_err(RegistrationError::Allocation)?,
        node: SnapshotNodeId::generate().map_err(RegistrationError::Allocation)?,
        completion: OperationId::generate().map_err(RegistrationError::Allocation)?,
    };
    let (reserved, manifest) = publication::reserve(engine, &host, request, allocated, principal)?;
    let node = host.prepare_node(engine, reserved, manifest)?;
    publication::complete(engine, &node)
}
