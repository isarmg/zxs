//! One preparation and reopening path for a sparse worktree and its ledger.
//!
//! The repository owner must reserve the identity and mount before preparation,
//! then retain the returned ledger binding and checkpoint in its own decision.
//! Preparation does not open a session. Reopening consumes that independently
//! retained anchor, never a checkpoint discovered in the local directory.
//! This module does not register an HTTP/SMB mount or decide a publication.

use crate::directory_worktree::io::{HeldReadView, ViewError};
use crate::directory_worktree::ledger::{
    Binding, Checkpoint, HeldWorktreeLedger, LedgerError, LedgerPreparationError,
    PendingPublication, RecoveredLedger,
};
use crate::directory_worktree::lifecycle::{Mount, Publication, Worktree, WorktreeId};
use crate::directory_worktree::projection::FileResult;
use crate::directory_worktree::store::{
    HeldSparseWorktree, SparseWorktreeBinding, WorktreePreparationError,
};
use crate::working_tree::snapshot::HeldSnapshotNode;
use storage_contract::working_tree::SnapshotBranchId;
use storage_contract::OperationId;

/// Material to retain in the repository's original creation decision. It has no
/// read or append shortcut: the owner must reopen its decided exact checkpoint.
pub(crate) struct PreparedWorktree {
    binding: Binding,
    checkpoint: Checkpoint,
}

#[derive(Debug, error_definition::Error)]
pub(crate) enum PreparationError {
    #[error("sparse worktree preparation failed")]
    Instance(#[source] WorktreePreparationError),
    #[error("worktree instance exists but its ledger was not completely prepared")]
    Ledger {
        instance: SparseWorktreeBinding,
        #[source]
        source: LedgerPreparationError,
    },
}

impl PreparedWorktree {
    /// The same repository writer fence covers both no-replace creations. On a
    /// second-stage failure retain the exact first-stage instance; do not retry
    /// with a new identity, remove it, or adopt a ledger already at that name.
    pub(crate) fn prepare(
        source: &HeldSnapshotNode,
        worktree: WorktreeId,
        operation: OperationId,
        branch: SnapshotBranchId,
        mount: Mount,
    ) -> Result<Self, PreparationError> {
        let instance = HeldSparseWorktree::prepare(source, worktree, operation)
            .map_err(PreparationError::Instance)?;
        let instance = instance.binding();
        let ledger = HeldWorktreeLedger::prepare(source, instance, branch, mount)
            .map_err(|source| PreparationError::Ledger { instance, source })?;
        Ok(Self {
            binding: ledger.binding(),
            checkpoint: ledger.checkpoint(),
        })
    }

    pub(crate) fn binding(&self) -> Binding {
        self.binding
    }

    pub(crate) fn checkpoint(&self) -> Checkpoint {
        self.checkpoint
    }
}

/// A pending cross-domain publication is kept out of the ordinary session. Its
/// original publisher must reconcile FILE/directory receipts before reopening.
pub(crate) enum ReopenedWorktree<'a> {
    Ready(WorktreeSession<'a>),
    Publishing(PendingPublication<'a>),
}

/// The node is held by the snapshot owner throughout this scoped session. A
/// completed directory publication needs its separately admitted new node;
/// choosing the original backing cannot silently reread obsolete snapshot data.
pub(crate) enum ReadMaterial<'a> {
    Original,
    Published(&'a HeldSnapshotNode),
}

pub(crate) struct WorktreeSession<'a> {
    source: &'a HeldSnapshotNode,
    ledger: HeldWorktreeLedger<'a>,
}

impl<'a> WorktreeSession<'a> {
    /// The caller supplies the repository's admitted anchor and its existing
    /// permanent-publication verifier. Local frames never supply either one.
    pub(crate) fn reopen(
        source: &'a HeldSnapshotNode,
        binding: Binding,
        checkpoint: Checkpoint,
        verify: impl FnMut(&Publication, &[FileResult]) -> Result<(), LedgerError>,
    ) -> Result<ReopenedWorktree<'a>, LedgerError> {
        match HeldWorktreeLedger::recover(source, binding, checkpoint, verify)? {
            RecoveredLedger::Ready(ledger) => {
                Ok(ReopenedWorktree::Ready(Self { source, ledger }))
            }
            RecoveredLedger::Publishing(pending) => {
                Ok(ReopenedWorktree::Publishing(pending))
            }
        }
    }

    pub(crate) fn worktree(&self) -> &Worktree {
        self.ledger.worktree()
    }

    pub(crate) fn added_files(&self) -> &[crate::directory_worktree::ledger::AddedFile] {
        self.ledger.added_files()
    }

    pub(crate) fn instance(&self) -> SparseWorktreeBinding {
        self.ledger.instance()
    }

    /// File reads and bounded directory pages use the same admitted logical
    /// view, including namespace edits. Deleted/publishing instances cannot
    /// acquire a view. Every actual read retains the existing live ledger check.
    pub(crate) fn read<'s>(
        &'s self,
        material: ReadMaterial<'s>,
    ) -> Result<HeldReadView<'s>, ViewError> {
        match material {
            ReadMaterial::Original => HeldReadView::sparse(self.source, &self.ledger),
            ReadMaterial::Published(node) => HeldReadView::published(node, &self.ledger),
        }
    }

    /// The actual writable owner consumes this session under its writer fence.
    /// It appends through the existing ledger, then independently decides the
    /// returned checkpoint before exposing another read session.
    pub(crate) fn into_ledger(self) -> HeldWorktreeLedger<'a> {
        self.ledger
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/directory_worktree/session.rs"]
mod tests;
