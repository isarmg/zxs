//! Explicitly register an ordinary root without creating a file or snapshot.
use crate::{OperationId, RepositoryId};
use data_serialization::{Deserialize, Serialize};
use name_policy::SlotId;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct RegisterSnapshotRoot {
    pub operation_id: OperationId,
}

/// The root is actually registered. This is not an operation receipt: the
/// existing creation kernel may return an already registered root unchanged.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct RegisteredSnapshotRoot {
    pub slot_id: SlotId,
    pub repository_id: RepositoryId,
}
