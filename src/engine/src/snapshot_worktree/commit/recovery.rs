//! The node publication is the durable recovery decision. This path never
//! reconstructs an upload workspace, prepares a new node, or repeats FILE work.
use crate::directory_worktree::ledger::HeldWorktreeLedger;
use crate::snapshot_worktree::commit::publish_completion;
use crate::snapshot_worktree::registry::ReceiptIndex;
use crate::snapshot_worktree::{PublishedCommitTail, RegistrationError, Result};
use crate::Engine;
use storage_contract::Principal;
use storage_contract::snapshot_worktree::SnapshotHandle;

pub(super) fn resume(
    engine: &mut Engine,
    index: &ReceiptIndex<'_>,
    tail: PublishedCommitTail<'_>,
    principal: &Principal,
) -> Result<SnapshotHandle> {
    // A completed exact retry remains read-only. This unfinished tail writes
    // the ledger, so it must pass the ordinary runtime fence before that I/O.
    engine.require_write().map_err(RegistrationError::Publication)?;
    if engine.admitted_current_namespaces().map_err(RegistrationError::Publication)?.is_some()
        || engine.config.generation().is_some()
    {
        return Err(RegistrationError::InitialOnly);
    }
    let plan = tail.plan();
    // Both reopens demand this exact Engine State and its admitted namespace
    // host. The permanent bindings, not local names, select all held entities.
    let original = index.reopen_node(engine, plan.source)?;
    let routing = tail.routing();
    let node = index.reopen_node(engine, plan.handle_at(routing.directory.branch()))?;
    let ledger = HeldWorktreeLedger::finish_published_commit(&original, &node, &tail)
        .map_err(RegistrationError::Ledger)?;
    let handle = publish_completion(
        engine, principal, plan, routing, tail.commitment(), ledger.checkpoint(),
    )?;
    ledger.admit_read_view().map_err(RegistrationError::Ledger)?;
    Ok(handle)
}
