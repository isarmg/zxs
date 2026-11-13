//! Current container editing is separate from immutable version reads.
use data_serialization::{Deserialize, Serialize};
use storage_contract::{FileId, Generation, OperationId, RepositoryId};
use storage_contract::container_edit::{CommitMode, ContainerEdit, DraftBase};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct ReadContainerEdit {
    pub file: FileId,
    pub repository: RepositoryId,
    pub path: name_policy::PortablePath,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct SetContainerCommitMode {
    pub context: ReadContainerEdit,
    pub expected_generation: Generation,
    pub mode: CommitMode,
    pub operation_id: OperationId,
}

/// Current reads own their admitted body handle. A draft never acquires a VersionId.
pub struct CurrentContainerContent {
    pub edit: ContainerEdit,
    pub base: DraftBase,
    pub content: crate::Content,
}

/// An ordinary filesystem save, not authorization to create a historical node.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct PersistContainerDraft {
    pub context: ReadContainerEdit,
    pub revision: storage_contract::container_edit::DraftRevision,
}

/// A durable draft acknowledgement cannot be mistaken for a published version.
pub struct DraftPersisted {
    pub revision: storage_contract::container_edit::DraftRevision,
    pub receipt: storage_contract::OperationResult,
}
