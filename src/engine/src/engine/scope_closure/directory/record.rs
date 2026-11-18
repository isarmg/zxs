//! Closed canonical owners and keys of the approved per-host directory root.
//! Native material is never opened as SQLite, and no local header elects a
//! registration. Other row families enter only with their real owner proofs.
use crate::{
    metadata_map::Reference,
    persist::snapshot_worktree::{NodeAllocation, NamespaceDirectoryBinding, WorktreeAllocation},
    working_tree::snapshot::{SnapshotEntry, SnapshotNodeBinding, SnapshotReference},
};
use data_serialization::{Deserialize, Serialize};
use storage_contract::{
    BranchName, OperationId, Sequence,
    snapshot_worktree::{NamespaceHostAddress, SnapshotHandle, WorktreeHandle, WorktreeId},
    working_tree::{DirectoryId, DiskRegistrationIdentity, LocalEntityIdentity, MaterialCommitment,
        RootId, SnapshotBranchId, SnapshotNodeId, SnapshotRepositoryId},
};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Host {
    pub registration: DiskRegistrationIdentity, pub root_id: RootId, pub directory_id: DirectoryId,
    pub host_entity: LocalEntityIdentity, pub namespace: NamespaceHostAddress,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct RepositoryIdentity {
    pub repository_id: SnapshotRepositoryId, pub directory_id: DirectoryId,
}
#[derive(Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Header {
    pub repository_id: SnapshotRepositoryId, pub host: Host, pub initialization_operation_id: OperationId,
    pub rows: Reference,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Origin {
    pub sequence: Sequence, pub operation_id: OperationId, pub request_digest: MaterialCommitment,
}
#[derive(Serialize)]
#[serialization(tag = "kind", rename_all = "snake_case")]
pub(super) enum Key {
    HostStep { ordinal: Sequence },
    Branch { branch_id: SnapshotBranchId },
    Node { node_id: SnapshotNodeId },
    Instance { worktree_id: WorktreeId },
    Pending { reserve_operation: OperationId },
    PendingItem { reserve_operation: OperationId, role: ItemRole, source_ordinal: Sequence, item_ordinal: Sequence },
}
#[derive(Clone, Copy, Serialize)]
#[serialization(rename_all = "snake_case")]
pub(super) enum ItemRole { NodeEntry }
#[derive(Serialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Row<T> { pub source: Origin, pub value: T }
#[derive(Serialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct HostStep<'a> { pub original: &'a NamespaceDirectoryBinding }
#[derive(Serialize)]
#[serialization(tag = "kind", rename_all = "snake_case")]
pub(super) enum BranchState { Pending { reservation: OperationId }, Head { handle: SnapshotHandle } }
#[derive(Serialize)]
#[serialization(tag = "kind", rename_all = "snake_case")]
pub(super) enum Parent { Initial {}, Node { handle: SnapshotHandle } }
#[derive(Serialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Node {
    pub handle: SnapshotHandle, pub parent: Parent, pub reservation: OperationId, pub binding: SnapshotNodeBinding,
}
#[derive(Serialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Branch<'a> {
    pub branch_id: SnapshotBranchId, pub name: &'a BranchName, pub creation_operation: OperationId,
    pub state: BranchState,
}
#[derive(Serialize)]
#[serialization(tag = "kind", rename_all = "snake_case")]
pub(super) enum LocalBinding { NotBound {} }
#[derive(Serialize)]
#[serialization(tag = "kind", rename_all = "snake_case")]
pub(super) enum Target { InitialNode { allocated: NodeAllocation, material: SnapshotReference },
    Worktree { allocated: WorktreeAllocation, source: SnapshotHandle, material: SnapshotReference,
        mount: crate::directory_worktree::lifecycle::Mount } }
#[derive(Serialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Instance<'a> {
    pub handle: WorktreeHandle, pub reservation: OperationId, pub source: SnapshotHandle,
    pub mount: &'a crate::directory_worktree::lifecycle::Mount,
    pub instance: crate::directory_worktree::store::SparseWorktreeBinding,
    pub ledger: crate::directory_worktree::ledger::Binding, pub checkpoint: crate::directory_worktree::ledger::Checkpoint,
}
#[derive(Serialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Pending {
    pub record_kind: &'static str, pub commitment: MaterialCommitment, pub target: Target,
    pub binding: LocalBinding,
}
#[derive(Serialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct PlannedEntry<'a> { pub entry: &'a SnapshotEntry }

#[derive(Serialize)]
#[serialization(tag = "kind", rename_all = "snake_case")]
pub(super) enum Ownership<'a> { Original { registration: &'a DiskRegistrationIdentity } }
#[derive(Serialize)]
#[serialization(tag = "kind", rename_all = "snake_case")]
pub(super) enum Location<'a> { Anchor { root_id: RootId, directory_id: DirectoryId },
    Entry { root_id: RootId, parent_directory_id: DirectoryId, name: &'a str } }
#[derive(Serialize)]
#[serialization(tag = "kind", rename_all = "snake_case")]
pub(super) enum Lifecycle { Pending { reservation: OperationId }, Current {} }
#[derive(Serialize)]
#[serialization(tag = "kind", rename_all = "snake_case")]
pub(super) enum Object<'a> {
    Worktree { worktree_id: WorktreeId, material: &'a super::super::schema::Material<WorktreeId> },
    DirectoryRepository { repository_id: SnapshotRepositoryId, directory_id: DirectoryId,
        material: &'a super::super::schema::Material<RepositoryIdentity> },
    DirectoryRepositoryReservation { repository_id: SnapshotRepositoryId, directory_id: DirectoryId,
        reserve_operation: OperationId, material: &'a super::super::schema::Material<RepositoryIdentity> },
}
#[derive(Serialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct InventoryRow<'a> {
    pub object: Object<'a>, pub ownership: Ownership<'a>, pub location: Location<'a>, pub lifecycle: Lifecycle,
    pub entity: LocalEntityIdentity, pub operation_id: OperationId,
}
