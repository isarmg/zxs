//! Explicit, occurrence-bound mount changes. These requests do not select a
//! directory, delete a body or create a new WorktreeId.
use super::{SnapshotHandle, WorktreeHandle, WorktreeRevision};
use crate::OperationId;
use data_serialization::{Deserialize, Serialize};
use name_policy::Name;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RestoreNaming {
    OriginalOrAutomatic {},
    Explicit { name: Name },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MountAction {
    Hide { committed: SnapshotHandle, visible: OperationId },
    Restore { deletion: OperationId, naming: RestoreNaming },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct MountChange {
    pub target: WorktreeHandle,
    pub expected: WorktreeRevision,
    pub operation_id: OperationId,
    pub action: MountAction,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MountOutcome {
    Pending {},
    Hidden {},
    Restored { name: Name },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct MountChangeReceipt {
    pub request: MountChange,
    pub outcome: MountOutcome,
}
