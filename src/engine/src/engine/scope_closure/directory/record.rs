//! Closed canonical owners and keys of the approved per-host directory root.
//! Native material is never opened as SQLite, and no local header elects a
//! registration. Other row families enter only with their real owner proofs.
use crate::{
    metadata_map::Reference,
    persist::snapshot_worktree::{NodeAllocation, NamespaceDirectoryBinding},
    working_tree::snapshot::{SnapshotEntry, SnapshotReference},
};
use data_serialization::{Deserialize, Serialize};
use storage_contract::{
    BranchName, OperationId, Sequence,
    snapshot_worktree::NamespaceHostAddress,
    working_tree::{DirectoryId, DiskRegistrationIdentity, LocalEntityIdentity, MaterialCommitment,
        RootId, SnapshotBranchId, SnapshotRepositoryId},
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
pub(super) enum BranchState { Pending { reservation: OperationId } }
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
pub(super) enum Target { InitialNode { allocated: NodeAllocation, material: SnapshotReference } }
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
pub(super) enum Location { Anchor { root_id: RootId, directory_id: DirectoryId } }
#[derive(Serialize)]
#[serialization(tag = "kind", rename_all = "snake_case")]
pub(super) enum Lifecycle { Pending { reservation: OperationId } }
#[derive(Serialize)]
#[serialization(tag = "kind", rename_all = "snake_case")]
pub(super) enum Object<'a> {
    DirectoryRepositoryReservation { repository_id: SnapshotRepositoryId, directory_id: DirectoryId,
        reserve_operation: OperationId, material: &'a super::super::schema::Material<RepositoryIdentity> },
}
#[derive(Serialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct InventoryRow<'a> {
    pub object: Object<'a>, pub ownership: Ownership<'a>, pub location: Location, pub lifecycle: Lifecycle,
    pub entity: LocalEntityIdentity, pub operation_id: OperationId,
}
