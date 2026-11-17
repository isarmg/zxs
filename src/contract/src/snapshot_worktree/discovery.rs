//! Read descriptions only. Discovery does not change a persistent selection,
//! and every subsequent command revalidates the discovered handle.
use crate::snapshot_worktree::{
    NamespaceHostAddress, SnapshotHandle, SnapshotHost, SnapshotRootContext,
};
use crate::working_tree::{SnapshotBranchId, SnapshotRepositoryId};
use crate::{BranchName, OperationId, RepositoryId};
use data_serialization::{Deserialize, Serialize};

/// Initial names the branch allocated by the host's actual initialization.
/// There is no implicit current/default branch selection in this API.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SnapshotBranchSelection {
    Initial,
    Exact { repository: SnapshotRepositoryId, branch: SnapshotBranchId },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SnapshotHeadRequest {
    pub namespace: NamespaceHostAddress,
    pub host: SnapshotHost,
    pub branch: SnapshotBranchSelection,
}

/// Pending occupancy is not absence, and must never invite initialization or
/// adoption of local material. Head names the indexed completed branch head.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum SnapshotHeadDiscovery {
    NotInitialized,
    Pending { operation: OperationId },
    Head { snapshot: SnapshotHandle, branch_name: BranchName },
}

/// The namespace context and its actual initialization branch head are read
/// under one engine lock. The repository identity binds a delayed Web response.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct DirectorySnapshotHead {
    pub repository_id: RepositoryId,
    pub context: SnapshotRootContext,
    pub head: SnapshotHeadDiscovery,
}

#[cfg(test)]
#[path = "../../../../test/contract/unit/snapshot_discovery.rs"]
mod tests;
