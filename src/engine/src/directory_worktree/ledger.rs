//! Immutable, bounded transition frames for one independently bound instance.
//!
//! The owning repository decision retains Binding and Checkpoint. Cold opening
//! accepts exactly that checkpoint, not a newer self-reported tail. FILE and
//! directory receipts are verified by their own authority on append and replay.
//! Namespace frames currently authorize prepublication reads only. Commit and
//! soft-delete remain fenced until durable FILE obligations cover renamed and
//! removed occurrences; no production SMB namespace is registered here.
mod leaf;
mod record;
mod state;
mod storage;

pub(crate) use leaf::EditLeaf;
pub(crate) use record::{AddedFile, AddedFileResult, Checkpoint, EditRecord, Event};
pub(crate) use state::{merge_original_edits, revision_commitment, revision_commitment_with_files};
pub(crate) use storage::{
    content_checkpoints, Binding, HeldWorktreeLedger, LedgerPreparationError, PendingPublication, RecoveredLedger,
};

use super::lifecycle::LifecycleError;
use crate::disk::{DirectoryIoError, NewEntryCopyError};

const MAX_FRAMES: u64 = 1024;
const MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;
const MAX_LEDGER_BYTES: usize = 64 * 1024 * 1024;

impl Checkpoint {
    pub(crate) fn next_frame_sequence(self) -> Result<storage_contract::Sequence, LedgerError> {
        frame_after(self.sequence)
    }

    /// Both the committing and completed frames must fit before publishing any
    /// part of this operation. This is not a reservation for later saves.
    pub(crate) fn require_publication_frames(self) -> Result<(), LedgerError> {
        frame_after(self.next_frame_sequence()?).map(|_| ())
    }
}

fn frame_after(before: storage_contract::Sequence) -> Result<storage_contract::Sequence, LedgerError> {
    let next = before.next().map_err(LedgerError::Sequence)?;
    if next.get() >= MAX_FRAMES { return Err(LedgerError::FrameLimit); }
    Ok(next)
}

#[derive(Debug, error_definition::Error)]
pub(crate) enum LedgerError {
    #[error("worktree ledger disk I/O failed")]
    Disk(#[source] crate::Error),
    #[error("worktree ledger directory access failed")]
    Directory(#[source] DirectoryIoError),
    #[error("worktree ledger inventory access failed")]
    Inventory(#[source] platform_io::fs_safety::Error),
    #[error("worktree ledger frame encoding or decoding failed")]
    Codec(#[source] storage_contract::Error),
    #[error("worktree ledger transition is invalid")]
    Lifecycle(#[source] LifecycleError),
    #[error("worktree ledger sparse view is invalid")]
    Projection(#[source] super::projection::ProjectionError),
    #[error("worktree ledger namespace transition is invalid")]
    Namespace(#[source] crate::directory_worktree::projection::namespace::NamespaceError),
    #[error("new worktree component exceeds the held target name budget")]
    NameBudget(#[source] crate::name_budget::NameBudgetError),
    #[error("worktree namespace changes require durable FILE publication obligations")]
    NamespacePublicationRequired,
    #[error("added files require their first FILE publication before commit or delete")]
    AddedPublicationRequired,
    #[error("added worktree file differs from its exact birth or working predecessor")]
    AddedFile,
    #[error("added worktree working manifest is invalid")]
    WorkingManifest(#[source] storage_contract::file_draft::manifest::ManifestError),
    #[error("worktree ledger differs from its independent identity or checkpoint")]
    Binding,
    #[error("worktree ledger instance reopening failed")]
    Instance(#[source] super::store::WorktreeStoreError),
    #[error("worktree ledger edit payload verification failed")]
    Payload(#[source] platform_io::payload::Error),
    #[error("worktree ledger edit synchronization failed")]
    Io(#[source] std::io::Error),
    #[error("worktree ledger sequence is exhausted")]
    Sequence(#[source] storage_contract::SequenceExhausted),
    #[error("worktree ledger chain, canonical encoding or complete inventory differs")]
    Chain,
    #[error("worktree ledger exceeds its bounded frame, byte or edit inventory")]
    Budget,
    #[error("worktree ledger has no remaining frame for this transition")]
    FrameLimit,
    #[error("worktree ledger operation identity was reused for different work")]
    OperationConflict,
    #[error("worktree ledger transition uses a stale worktree revision")]
    Stale,
    #[error("the completed directory view must be rebound before accepting further edits")]
    RebaseRequired,
    #[error("worktree completion does not exactly cover its dirty file versions")]
    FileResults,
    #[error("worktree ledger has an uncertain append; reopen the decided checkpoint")]
    Uncertain,
    #[error("worktree has a frozen publication that requires its original publisher to resume")]
    PublicationPending,
    #[error("worktree frame was persisted but its directory binding or durability tail failed")]
    Persisted {
        intended: Checkpoint,
        #[source]
        source: DirectoryIoError,
    },
    #[error("worktree ledger must retain its intended append outcome for recovery")]
    Append {
        intended: Checkpoint,
        #[source]
        source: NewEntryCopyError,
    },
    #[error("worktree publication receipt was not admitted by its authority")]
    Receipt(#[source] crate::Error),
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/directory_worktree/ledger.rs"]
mod tests;
