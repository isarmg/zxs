//! Permanent snapshot/worktree registration, separate from local store records.
//!
//! The receipt index does not authorize a host directory. The live publisher
//! must bind its current namespace and held root before physical preparation;
//! a completed index entry is only the exact independent anchor for reopening.
mod registry;
mod public;
pub use public::SnapshotWorktreeError;
mod reopen;
mod prepare;
mod initialize;
mod publication;
mod instance;
mod source;
mod edit;
mod commit;
pub use source::SnapshotSourceError;
pub(crate) use registry::{DirectoryRegistration, RegisteredNamespaceDirectories};
pub(crate) use prepare::prepare_namespace_host;
pub(crate) use initialize::initialize_root;
pub(crate) use instance::{create_worktree, with_worktree};
pub(crate) use edit::edit_worktree;
pub(crate) use commit::{commit_worktree, ReservedFileVersion, ReservedDirectoryTransfer};
pub(crate) use publication::PublishedNodeReservation;

#[derive(Debug, error_definition::Error)]
pub(crate) enum RegistrationError {
    #[error("snapshot initialization has no complete live Current publication mode")]
    InitialOnly,
    #[error("snapshot operation requires the explicitly registered root as its parent")]
    RootOnly,
    #[error("allocating a new snapshot identity")]
    Allocation(#[source] storage_contract::Error),
    #[error("publishing the exact snapshot reservation or completion")]
    Publication(#[source] crate::Error),
    #[error("a completed snapshot publication differs from its actual retained plan")]
    PublicationChanged,
    #[error("creating the exclusively reserved snapshot repository")]
    RepositoryPreparation(#[source] crate::disk::DirectoryIoError),
    #[error("preparing the reserved immutable snapshot node")]
    NodePreparation(#[source] crate::working_tree::snapshot::SnapshotPreparationError),
    #[error("reading back the exact prepared immutable snapshot node")]
    NodeReadback(#[source] crate::working_tree::snapshot::SnapshotStoreError),
    #[error("preparing the reserved sparse worktree and its original ledger")]
    WorktreePreparation(#[source] crate::directory_worktree::session::PreparationError),
    #[error("decoding the permanent snapshot/worktree registration")]
    Codec(#[source] crate::persist::snapshot_worktree::CodecError),
    #[error("snapshot/worktree receipt has a conflicting operation identity: {0}")]
    Operation(storage_contract::OperationId),
    #[error("snapshot/worktree receipt has no permanent author: {0}")]
    MissingPrincipal(storage_contract::OperationId),
    #[error("snapshot/worktree receipt has an invalid permanent author")]
    Principal(#[source] storage_contract::EmptyPrincipal),
    #[error("snapshot/worktree request digest differs: {0}")]
    Digest(storage_contract::OperationId),
    #[error("snapshot/worktree receipt has no exact single publication event: {0}")]
    Event(storage_contract::OperationId),
    #[error("snapshot/worktree receipt sequence is conflicting or outside its source")]
    Sequence,
    #[error("snapshot/worktree reservation reuses an allocated identity")]
    Identity,
    #[error("snapshot repository host already has a permanent reservation")]
    Host,
    #[error("worktree name is already reserved in its actual parent")]
    Mount,
    #[error("snapshot/worktree completion does not match its original reservation: {0}")]
    Completion(storage_contract::OperationId),
    #[error("snapshot/worktree registration has an absent or conflicting original source")]
    Source,
    #[error("snapshot/worktree reservation {0} is incomplete; its local material must be retained")]
    Pending(storage_contract::OperationId),
    #[error("resolving the snapshot's actual current namespace host")]
    NamespaceHost(#[source] crate::container_storage::namespace_host::NamespaceHostError),
    #[error("preparing the original ordinary namespace host")]
    NamespacePreparation(
        #[source] crate::container_storage::namespace_host::NamespacePreparationError,
    ),
    #[error("snapshot manifest differs from its reserved complete material")]
    Material,
    #[error("admitting the complete reserved snapshot manifest")]
    Manifest(#[source] crate::working_tree::snapshot::SnapshotManifestError),
    #[error("reopening the independently registered worktree ledger")]
    Ledger(#[source] crate::directory_worktree::ledger::LedgerError),
    #[error("this worktree write requires a pristine instance and one existing root-level file")]
    EditScope,
    #[error("worktree write differs from its declared complete content or exceeds its limit")]
    EditBody,
    #[error("preparing the independently reserved worktree edit")]
    EditPreparation(#[source] crate::disk::NewEntryCopyError),
    #[error("worktree edit name exceeds its actual held target budget")]
    EditName(#[source] crate::name_budget::NameBudgetError),
    #[error("opening the independently bound sparse worktree")]
    EditInstance(#[source] crate::directory_worktree::store::WorktreeStoreError),
    #[error("this directory commit requires one dirty file at its exact original FILE head")]
    CommitScope,
    #[error("reading the independently sealed worktree edit")]
    CommitRead(#[source] std::io::Error),
}

type Result<T> = std::result::Result<T, RegistrationError>;

#[cfg(test)]
#[path = "../../../test/engine/unit/snapshot_worktree_tests.rs"]
mod tests;
