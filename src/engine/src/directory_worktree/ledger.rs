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
pub(crate) use record::{Checkpoint, EditRecord, Event};
pub(crate) use storage::{
    Binding, HeldWorktreeLedger, LedgerPreparationError, PendingPublication, RecoveredLedger,
};

use super::lifecycle::LifecycleError;
use crate::disk::{DirectoryIoError, NewEntryCopyError};

const MAX_FRAMES: u64 = 1024;
const MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;
const MAX_LEDGER_BYTES: usize = 64 * 1024 * 1024;

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
    #[error("worktree namespace changes require durable FILE publication obligations")]
    NamespacePublicationRequired,
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
