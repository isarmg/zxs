//! One independent instance publisher for explicit and automatically allocated
//! mounts. Both retain the exact original before any physical preparation.
mod allocation;
mod mount_parent;
pub(in crate::snapshot_worktree) use mount_parent::MountParent;
pub(crate) use allocation::allocate_worktree;

use crate::container_storage::namespace_host::NamespaceHost;
use crate::directory_worktree::ledger::LedgerError;
use crate::directory_worktree::lifecycle::Mount;
use crate::directory_worktree::session::{PreparedWorktree, ReopenedWorktree, WorktreeSession};
use crate::disk::DiskResources;
use crate::persist::snapshot_worktree::{
    Record, WorktreeAllocation, WorktreeCompletion, WorktreeReservation,
};
use crate::snapshot_worktree::{RegistrationError, Result, publication};
use crate::snapshot_worktree::registry::ReceiptIndex;
use crate::working_tree::snapshot::HeldSnapshotNode;
use crate::{Engine, State, ValidatedConfig};
use std::sync::Arc;
use storage_contract::snapshot_worktree::{CreateWorktree, WorktreeHandle, WorktreeId};
use storage_contract::working_tree::MaterialCommitment;
use storage_contract::{OperationId, Principal};

/// This can only be constructed after the actual reserve publication succeeds.
/// It is consumed once while retaining the original node and source environment.
struct ReservedWorktree {
    state: Arc<State>,
    config: ValidatedConfig,
    resources: Arc<DiskResources>,
    source: HeldSnapshotNode,
    plan: WorktreeReservation,
    principal: Principal,
    commitment: MaterialCommitment,
}

pub(crate) fn create_worktree(
    engine: &mut Engine,
    request: &CreateWorktree,
    principal: &Principal,
) -> Result<WorktreeHandle> {
    let receipts = engine.snapshot_receipt_source()?;
    let index = receipts.index()?;
    if let Some(handle) = index.retry_worktree(request, principal)? {
        with_worktree(engine, handle, |_| Ok(()))?;
        return Ok(handle);
    }
    require_initial(engine)?;
    let (source, plan) = prepare(engine, &index, request)?;
    let record = Record::ReserveWorktree(plan.clone());
    publish(engine, principal, source, plan, record)
}

fn require_initial(engine: &Engine) -> Result<()> {
    if engine.admitted_current_namespaces().map_err(RegistrationError::Publication)?.is_some()
        || engine.config.generation().is_some()
    {
        return Err(RegistrationError::InitialOnly);
    }
    Ok(())
}

fn prepare(
    engine: &Engine, index: &ReceiptIndex<'_>, request: &CreateWorktree,
) -> Result<(HeldSnapshotNode, WorktreeReservation)> {
    let registered = index.node(request.source)?;
    let parent = MountParent::original(registered.host())?;
    if request.parent_directory_id != parent.directory
        || request.expected_root_generation
            != registered.initialization().host.expected_root_generation
    {
        return Err(RegistrationError::RootOnly);
    }
    let source = index.reopen_node(engine, request.source)?;
    NamespaceHost::registered(engine, parent.namespace)
        .map_err(RegistrationError::NamespaceHost)?
        .require_root_mount(
            request.parent_directory_id, request.expected_root_generation, &request.name,
        ).map_err(RegistrationError::NamespaceHost)?;
    crate::directory_worktree::names::require_mounted_paths(
        &request.name, source.manifest().entries().iter().map(|entry| &entry.path),
    ).map_err(RegistrationError::WorktreeName)?;
    crate::directory_worktree::io::require_configured_manifest(engine.state(),source.manifest(),Some(&request.name))
        .map_err(RegistrationError::ReadView)?;
    let plan = WorktreeReservation {
        request: request.clone(), host: registered.host().clone(),
        allocated: WorktreeAllocation {
            worktree: WorktreeId::generate().map_err(RegistrationError::Allocation)?,
            completion: OperationId::generate().map_err(RegistrationError::Allocation)?,
        },
        material: source.manifest().reference(),
    };
    Ok((source, plan))
}

fn publish(
    engine: &mut Engine, principal: &Principal, source: HeldSnapshotNode,
    plan: WorktreeReservation, record: Record,
) -> Result<WorktreeHandle> {
    let commitment = record.commitment().map_err(RegistrationError::Codec)?;
    let config = engine.config.clone();
    let resources = Arc::clone(&engine.disk_resources);
    publication::publish_new(engine, plan.request.operation_id, principal, &record)?;
    ReservedWorktree {
        state: Arc::clone(&engine.state), config, resources, source, plan,
        principal: principal.clone(), commitment,
    }.complete(engine)
}

impl ReservedWorktree {
    fn require_engine(&self, engine: &Engine) -> Result<()> {
        if !Arc::ptr_eq(&self.state, &engine.state)
            || !self.config.same_admission(&engine.config)
            || !Arc::ptr_eq(&self.resources, &engine.disk_resources)
            || engine.current_namespaces.is_some()
        {
            return Err(RegistrationError::PublicationChanged);
        }
        let fresh = engine.snapshot_receipts()?
            .reopen_node(engine, self.plan.request.source)?;
        if fresh.binding() != self.source.binding() {
            return Err(RegistrationError::Source);
        }
        let parent = MountParent::original(&self.plan.host)?;
        NamespaceHost::registered(engine, parent.namespace)
            .map_err(RegistrationError::NamespaceHost)?
            .require_root_mount(
                self.plan.request.parent_directory_id,
                self.plan.request.expected_root_generation, &self.plan.request.name,
            ).map_err(RegistrationError::NamespaceHost)
    }

    fn complete(self, engine: &mut Engine) -> Result<WorktreeHandle> {
        self.require_engine(engine)?;
        let request = &self.plan.request;
        let prepared = PreparedWorktree::prepare(
            &self.source, self.plan.allocated.worktree, request.operation_id,
            request.source.branch_id,
            Mount {
                parent: request.parent_directory_id,
                name: name_policy::LookupName::from(&request.name),
            },
        ).map_err(RegistrationError::WorktreePreparation)?;
        self.require_engine(engine)?;
        let reopened = WorktreeSession::reopen(
            &self.source, prepared.binding(), prepared.checkpoint(),
            |_, _| Err(LedgerError::PublicationPending),
        ).map_err(RegistrationError::Ledger)?;
        let ReopenedWorktree::Ready(session) = reopened else {
            return Err(RegistrationError::Source);
        };
        if session.instance().operation() != request.operation_id
            || session.worktree().source().branch != request.source.branch_id
            || session.worktree().mount().parent != request.parent_directory_id
            || session.worktree().mount().name.as_str() != request.name.as_str()
        {
            return Err(RegistrationError::Source);
        }
        let completion = Record::CompleteWorktree(WorktreeCompletion {
            reserve: request.operation_id, reservation: self.commitment,
            binding: prepared.binding(), checkpoint: prepared.checkpoint(),
        });
        publication::publish_new(
            engine, self.plan.allocated.completion, &self.principal, &completion,
        )?;
        let handle = WorktreeHandle {
            publication: self.plan.allocated.completion,
            worktree_id: self.plan.allocated.worktree,
        };
        with_worktree(engine, handle, |_| Ok(()))?;
        Ok(handle)
    }
}

/// The caller holds the actual Engine throughout this read. The borrowed
/// session cannot be moved into a writer or outlive its source node/index.
pub(crate) fn with_worktree<R>(
    engine: &Engine,
    handle: WorktreeHandle,
    read: impl for<'node, 'session> FnOnce(&'session WorktreeSession<'node>) -> Result<R>,
) -> Result<R> {
    engine.snapshot_receipts()?.with_worktree(engine, handle, |_, session| {
        read(session)
    })
}
