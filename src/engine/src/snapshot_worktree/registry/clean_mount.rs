//! Read-only projection of the same original native decisions. Hidden and
//! pending instances remain retained; a name is never their identity.
use super::{Allocations, CompletionOwner, NameKey, Publication, Receipt, ReceiptIndex, WorktreeRecord};
use crate::directory_worktree::ledger::mount_checkpoint;
use crate::directory_worktree::lifecycle::Mount;
use crate::persist::snapshot_worktree::mount_change::{Publication as Outcome, Reservation, Resolved};
use crate::snapshot_worktree::{RegistrationError, Result};
use name_policy::{LookupName, Name};
use storage_contract::{OperationId, Principal, RepositoryId};
use storage_contract::snapshot_worktree::{WorktreeHandle, WorktreeId};
use storage_contract::snapshot_worktree::mount_change::{
    MountAction, MountChange, MountChangeReceipt, MountOutcome, RestoreNaming,
};
use storage_contract::working_tree::MaterialCommitment;

pub(super) struct Change<'a> {
    pub(super) receipt: Receipt<'a>,
    pub(super) plan: Reservation,
    pub(super) commitment: MaterialCommitment,
    pub(super) outcome: Option<Outcome>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(in crate::snapshot_worktree) enum Presence {
    Visible(OperationId),
    Hidden(OperationId),
}

#[derive(Clone)]
pub(in crate::snapshot_worktree) struct MountState {
    pub(in crate::snapshot_worktree) mount: Mount,
    pub(in crate::snapshot_worktree) presence: Presence,
}

fn portable(name: &LookupName) -> Result<Name> {
    Name::parse(name.as_str()).map_err(|_| RegistrationError::Mount)
}

impl ReceiptIndex<'_> {
    pub(super) fn mount_state_for(&self, tree: &WorktreeRecord<'_>) -> MountState {
        self.mount_states.get(&tree.plan.allocated.worktree).cloned().unwrap_or_else(|| {
            MountState {
                mount: Mount {
                    parent: tree.plan.request.parent_directory_id,
                    name: LookupName::from(&tree.plan.request.name),
                },
                presence: Presence::Visible(tree.plan.allocated.completion),
            }
        })
    }

    pub(super) fn clean_change_pending(&self, worktree: WorktreeId) -> bool {
        self.pending_edits.get(&worktree).is_some_and(|op| self.mount_changes.contains_key(op))
    }

    /// Includes the old visible name until Hide publishes, or the reserved
    /// destination of Restore; an already hidden instance itself occupies none.
    pub(super) fn mount_claim(&self, tree: &WorktreeRecord<'_>) -> Option<Mount> {
        let state = self.mount_state_for(tree);
        if matches!(state.presence, Presence::Visible(_)) {
            return Some(state.mount);
        }
        let op = self.pending_edits.get(&tree.plan.allocated.worktree)?;
        let change = self.mount_changes.get(op)?;
        match &change.plan.resolved {
            Resolved::Restore { mount } => Some(mount.clone()),
            Resolved::Hide {} => None,
        }
    }

    pub(in crate::snapshot_worktree) fn clean_mount_receipt(
        &self, repository: RepositoryId, operation: OperationId, actor: &Principal,
    ) -> Result<MountChangeReceipt> {
        let change = self.mount_changes.get(&operation).ok_or(RegistrationError::Operation(operation))?;
        if &change.receipt.principal != actor || change.plan.host.namespace.repository_id != repository {
            return Err(RegistrationError::Digest(operation));
        }
        let outcome = match (&change.plan.resolved, change.outcome) {
            (_, None) => MountOutcome::Pending {},
            (Resolved::Hide {}, Some(_)) => MountOutcome::Hidden {},
            (Resolved::Restore { mount }, Some(_)) => MountOutcome::Restored { name: portable(&mount.name)? },
        };
        Ok(MountChangeReceipt { request: change.plan.request.clone(), outcome })
    }

    pub(in crate::snapshot_worktree) fn mount_owner(
        &self, target: WorktreeHandle,
    ) -> Result<RepositoryId> {
        Ok(self.retained_tree(target)?.plan.host.namespace.repository_id)
    }

    pub(in crate::snapshot_worktree) fn mount_retry(
        &self, request: &MountChange, actor: &Principal,
    ) -> Result<Option<MountChangeReceipt>> {
        if !self.source.operations.contains_key(&request.operation_id) { return Ok(None); }
        let owner = self.mount_owner(request.target)?;
        let original = self.clean_mount_receipt(owner, request.operation_id, actor)?;
        if original.request != *request { return Err(RegistrationError::Digest(request.operation_id)); }
        Ok(Some(original))
    }
}

impl<'a> ReceiptIndex<'a> {
    pub(super) fn reserve_clean_mount(
        &mut self, receipt: Receipt<'a>, plan: Reservation, commitment: MaterialCommitment,
        allocations: &mut Allocations,
    ) -> Result<()> {
        let operation = receipt.original.operation_id;
        let worktree = plan.request.target.worktree_id;
        let tree = self.retained_tree(plan.request.target)?;
        let before = self.mount_state_for(tree);
        let head = self.heads.get(&worktree).ok_or(RegistrationError::Source)?;
        let content = self.content_batches.get(&plan.content_reserve).ok_or(RegistrationError::Source)?;
        let complete = content.complete().ok_or(RegistrationError::Source)?;
        let frozen = &content.plan;
        if plan.request.operation_id != operation || self.content_instances.get(&worktree) != Some(&plan.content_reserve)
            || self.pending_edits.contains_key(&worktree) || !self.added(plan.request.target).is_empty()
            || frozen.request.target != plan.request.target || frozen.request.expected != plan.request.expected
            || plan.request.expected != head.revision || plan.before != head.checkpoint
            || plan.before.sequence < complete.value.completed.sequence
            || plan.source != frozen.source || plan.committed != frozen.handle()
            || plan.instance != frozen.instance || plan.ledger != frozen.ledger
            || plan.root != frozen.root || plan.host != frozen.host
            || plan.host != tree.plan.host || plan.prior_mount != before.mount
            || plan.root.directory_id != before.mount.parent
        {
            return Err(RegistrationError::CommitScope);
        }
        let claim_key = |mount: &Mount| (plan.root.root_id, mount.parent, NameKey(mount.name.key()));
        match (&plan.request.action, before.presence, &plan.resolved) {
            (MountAction::Hide { committed, visible }, Presence::Visible(current), Resolved::Hide {})
                if *committed == plan.committed && *visible == current
                    && allocations.mounts.contains(&claim_key(&before.mount)) => {}
            (MountAction::Restore { deletion, naming }, Presence::Hidden(last), Resolved::Restore { mount })
                if *deletion == last && mount.parent == before.mount.parent => {
                let name = portable(&mount.name)?;
                let source = self.node(plan.source)?;
                match naming {
                    RestoreNaming::Explicit { name: exact } if *exact == name => {}
                    RestoreNaming::OriginalOrAutomatic {} if mount.name.as_str() == before.mount.name.as_str()
                        || crate::directory_worktree::names::is_allocated_name(source.branch_name(), &name) => {}
                    _ => return Err(RegistrationError::Mount),
                }
                crate::directory_worktree::names::require_mounted_paths(
                    &name, complete.value.prepared_node.entries.iter().map(|e| &e.path),
                ).map_err(RegistrationError::WorktreeName)?;
                if !allocations.mounts.insert(claim_key(mount)) { return Err(RegistrationError::Mount); }
            }
            _ => return Err(RegistrationError::Mount),
        }
        mount_checkpoint(&plan).map_err(RegistrationError::Ledger)?;
        self.reserve_completion(operation, plan.publish_operation, CompletionOwner::MountChange(operation))?;
        self.pending_edits.insert(worktree, operation);
        self.mount_changes.insert(operation, Change { receipt, plan, commitment, outcome: None });
        Ok(())
    }

    pub(super) fn publish_clean_mount(
        &mut self, receipt: Receipt<'a>, result: Outcome, allocations: &mut Allocations,
    ) -> Result<()> {
        let change = self.mount_changes.get(&result.reserve).ok_or(RegistrationError::Source)?;
        let plan = &change.plan;
        let worktree = plan.request.target.worktree_id;
        let head = self.heads.get_mut(&worktree).ok_or(RegistrationError::Source)?;
        super::require_completion(&change.receipt, &receipt, plan.publish_operation,
            change.commitment, result.reservation)?;
        if change.outcome.is_some() || self.pending_edits.get(&worktree) != Some(&result.reserve)
            || !matches!(self.completions.get(&plan.publish_operation),
                Some(CompletionOwner::MountChange(op)) if *op == result.reserve)
            || head.checkpoint != plan.before || head.revision != plan.request.expected
            || result.checkpoint != mount_checkpoint(plan).map_err(RegistrationError::Ledger)?
        {
            return Err(RegistrationError::Completion(receipt.original.operation_id));
        }
        let next = match &plan.resolved {
            Resolved::Hide {} => {
                let key = (plan.root.root_id, plan.prior_mount.parent, NameKey(plan.prior_mount.name.key()));
                if !allocations.mounts.remove(&key) { return Err(RegistrationError::Mount); }
                MountState { mount: plan.prior_mount.clone(), presence: Presence::Hidden(result.reserve) }
            }
            Resolved::Restore { mount } => {
                let key = (plan.root.root_id, mount.parent, NameKey(mount.name.key()));
                if !allocations.mounts.contains(&key) { return Err(RegistrationError::Mount); }
                MountState { mount: mount.clone(), presence: Presence::Visible(result.reserve) }
            }
        };
        self.mount_states.insert(worktree, next);
        head.checkpoint = result.checkpoint;
        self.pending_edits.remove(&worktree);
        self.mount_changes.get_mut(&result.reserve).ok_or(RegistrationError::Source)?.outcome = Some(result);
        Ok(())
    }
}
