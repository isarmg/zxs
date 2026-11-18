//! Shared outer inventory row; each owner supplies only its strict typed object.
use data_serialization::Serialize;
use storage_contract::{OperationId, working_tree::{DirectoryId, DiskRegistrationIdentity, LocalEntityIdentity, RootId}};

#[derive(Serialize)]
#[serialization(tag = "kind", rename_all = "snake_case")]
pub(in crate::engine::scope_closure) enum Ownership<'a> { Original { registration: &'a DiskRegistrationIdentity } }
#[derive(Serialize)]
#[serialization(tag = "kind", rename_all = "snake_case")]
pub(in crate::engine::scope_closure) enum Location<'a> { Anchor { root_id: RootId, directory_id: DirectoryId },
    Entry { root_id: RootId, parent_directory_id: DirectoryId, name: &'a str } }
#[derive(Serialize)]
#[serialization(tag = "kind", rename_all = "snake_case")]
pub(in crate::engine::scope_closure) enum Lifecycle { Pending { reservation: OperationId }, Current {} }
#[derive(Serialize)]
#[serialization(deny_unknown_fields)]
pub(in crate::engine::scope_closure) struct InventoryRow<'a, O> {
    pub object: O, pub ownership: Ownership<'a>, pub location: Location<'a>, pub lifecycle: Lifecycle,
    pub entity: LocalEntityIdentity, pub operation_id: OperationId,
}
