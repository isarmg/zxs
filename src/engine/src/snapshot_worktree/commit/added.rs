//! One Added file, the shared real FULL producer and the original node publisher.
use super::ReservedDirectoryTransfer;
use crate::Engine;
use crate::directory_worktree::ledger::{Event, HeldWorktreeLedger, LedgerError};
use crate::directory_worktree::session::WorktreeSession;
use crate::persist::snapshot_worktree::added_commit::AddedCommit;
use crate::persist::snapshot_worktree::{CommitCompletion, CommitNode, Record};
use crate::snapshot_worktree::registry::{AddedCommitRetry, PublishedWorktree, ReceiptIndex};
use crate::snapshot_worktree::{RegistrationError, Result, publication, with_worktree};
use crate::working_tree::snapshot::HeldSnapshotNode;
use storage_contract::snapshot_worktree::{CommitWorktree, SnapshotHandle};
use storage_contract::working_tree::{MaterialCommitment, SnapshotBranchId, SnapshotNodeId};
use storage_contract::{BranchId, BranchName, CommitId, OperationId, Principal, TreeId};

pub(super) fn retry(
    engine: &mut Engine,
    index: &ReceiptIndex<'_>,
    retry: AddedCommitRetry<'_>,
    target: storage_contract::snapshot_worktree::WorktreeHandle,
    principal: &Principal,
) -> Result<SnapshotHandle> {
    match retry {
        AddedCommitRetry::Complete(handle) => {
            index.reopen_node(engine, handle)?;
            with_worktree(engine, target, |_| Ok(()))?;
            Ok(handle)
        }
        AddedCommitRetry::Published(tail) => {
            engine
                .require_write()
                .map_err(RegistrationError::Publication)?;
            if engine
                .admitted_current_namespaces()
                .map_err(RegistrationError::Publication)?
                .is_some()
                || engine.config.generation().is_some()
            {
                return Err(RegistrationError::InitialOnly);
            }
            let plan = tail.plan();
            let original = index.reopen_node(engine, plan.source)?;
            let node = index.reopen_node(engine, plan.handle())?;
            let ledger = HeldWorktreeLedger::finish_published_added_commit(&original, &node, &tail)
                .map_err(RegistrationError::Ledger)?;
            let handle = complete(
                engine,
                plan,
                tail.commitment(),
                principal,
                ledger.checkpoint(),
            )?;
            ledger
                .admit_read_view()
                .map_err(RegistrationError::Ledger)?;
            Ok(handle)
        }
    }
}

pub(super) fn commit(
    engine: &mut Engine,
    request: &CommitWorktree,
    principal: &Principal,
    registered: &PublishedWorktree<'_>,
    original: &HeldSnapshotNode,
    session: WorktreeSession<'_>,
    observed_head: SnapshotHandle,
) -> Result<SnapshotHandle> {
    let [added] = session.added_files() else {
        return Err(RegistrationError::CommitScope);
    };
    let added = added.clone();
    let mut ledger = session.into_ledger();
    ledger
        .check_single_added(&added)
        .map_err(RegistrationError::Ledger)?;
    if request.expected != registered.revision() || registered.published_node().is_some() {
        return Err(RegistrationError::CommitScope);
    }
    crate::snapshot_worktree::edit::require_mount(engine, registered.reservation())?;
    let record = crate::business::file_draft::worktree::selected(engine.state(), &added.target)
        .map_err(RegistrationError::Publication)?;
    if record.working.unborn() != Some(&added.working) || record.created_by != added.birth {
        return Err(RegistrationError::CommitScope);
    }
    let source = registered.source().handle();
    let (destination, branch_name) = if source == observed_head {
        (source.branch_id, registered.source().branch_name().clone())
    } else {
        (
            SnapshotBranchId::generate().map_err(RegistrationError::Allocation)?,
            BranchName::parse(registered.reservation().request.name.as_str())
                .map_err(RegistrationError::Allocation)?,
        )
    };
    let plan = AddedCommit {
        request: request.clone(),
        source,
        observed_head,
        destination,
        branch_name,
        binding: registered.binding(),
        before: registered.checkpoint(),
        history: record.history_tree_id,
        added,
        file_branch: BranchId::generate().map_err(RegistrationError::Allocation)?,
        file_commit: CommitId::generate().map_err(RegistrationError::Allocation)?,
        file_tree: TreeId::generate().map_err(RegistrationError::Allocation)?,
        file_operation: OperationId::generate().map_err(RegistrationError::Allocation)?,
        node: SnapshotNodeId::generate().map_err(RegistrationError::Allocation)?,
        node_publication: OperationId::generate().map_err(RegistrationError::Allocation)?,
        completion: OperationId::generate().map_err(RegistrationError::Allocation)?,
    };
    let reserved = Record::ReserveAddedCommit(plan.clone());
    let commitment = reserved.commitment().map_err(RegistrationError::Codec)?;
    publication::publish_new(engine, request.operation_id, principal, &reserved)?;
    ledger
        .append_with_added(
            plan.commit_event(),
            |_, _| Err(LedgerError::PublicationPending),
            |_, _| Err(LedgerError::PublicationPending),
        )
        .map_err(RegistrationError::Ledger)?;
    // No metadata reserve may be inserted between the shared producer and its
    // native first-file decision: it retains this exact State, resources and slot.
    let (full, first, _) = engine
        .publish_added_first(&plan, principal, &ledger)
        .map_err(RegistrationError::Publication)?;
    let index = engine.snapshot_receipts()?;
    let (manifest, admitted_first, result) = index.added_material(request.operation_id)?;
    if first != admitted_first {
        return Err(RegistrationError::PublicationChanged);
    }
    let host = index.node(source)?.host().clone();
    let root = engine
        .namespace_host_root(host.namespace.repository_id, host.namespace.branch_id)
        .map_err(RegistrationError::Publication)?;
    let transfer = ReservedDirectoryTransfer::added(
        original,
        &plan,
        &first,
        &result,
        full,
        manifest.reference(),
    )
    .map_err(RegistrationError::NodeReadback)?;
    let node = HeldSnapshotNode::prepare_worktree_commit(
        root.common().map_err(RegistrationError::Publication)?,
        manifest,
        &transfer,
    )
    .map_err(RegistrationError::NodePreparation)?;
    HeldSnapshotNode::reopen_at_host(
        root.common().map_err(RegistrationError::Publication)?,
        node.binding(),
    )
    .map_err(RegistrationError::NodeReadback)?;
    publication::publish_new(
        engine,
        plan.node_publication,
        principal,
        &Record::PublishAddedNode(CommitNode {
            reserve: request.operation_id,
            reservation: commitment,
            binding: node.binding(),
        }),
    )?;
    let index = engine.snapshot_receipts()?;
    let checkpoint = ledger
        .append_with_added(
            Event::AddedCompleted {
                publication: plan.publication(),
                file: result,
            },
            |_, _| Err(LedgerError::FileResults),
            |publication, file| {
                index.require_added_publication(request.operation_id, publication, file)
            },
        )
        .map_err(RegistrationError::Ledger)?;
    complete(engine, &plan, commitment, principal, checkpoint)
}

fn complete(
    engine: &mut Engine,
    plan: &AddedCommit,
    commitment: MaterialCommitment,
    principal: &Principal,
    checkpoint: crate::directory_worktree::ledger::Checkpoint,
) -> Result<SnapshotHandle> {
    publication::publish_new(
        engine,
        plan.completion,
        principal,
        &Record::CompleteAddedCommit(CommitCompletion {
            reserve: plan.request.operation_id,
            reservation: commitment,
            checkpoint,
        }),
    )?;
    let handle = plan.handle();
    engine.snapshot_receipts()?.reopen_node(engine, handle)?;
    with_worktree(engine, plan.request.target, |_| Ok(()))?;
    Ok(handle)
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/snapshot_worktree/added_first.rs"]
mod tests;
