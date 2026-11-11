//! One captured current-object move in the displayed source-disk namespace.
use name_policy::{LookupPath, PortablePath, SlotId};
use storage_contract::{
    OperationId,
    file_relocation::{NamespaceEntryId, ObjectMoveRequest},
    history_window::HistoryRepositoryId,
};

pub struct MoveDiskObject {
    pub source_slot_id: SlotId,
    pub source_repository_id: HistoryRepositoryId,
    pub source_entry_id: NamespaceEntryId,
    pub source_path: LookupPath,
    pub target_slot_id: SlotId,
    pub target_repository_id: Option<HistoryRepositoryId>,
    pub target_path: PortablePath,
    pub replace: bool,
    pub target_entry_id: Option<NamespaceEntryId>,
    pub operation_id: OperationId,
}

impl MoveDiskObject {
    /// Admit the target slot after the complete HTTP body has been decoded.
    pub fn new(
        source_slot_id: SlotId,
        movement: ObjectMoveRequest,
        operation_id: OperationId,
    ) -> name_policy::Result<Self> {
        let target_slot_id = SlotId::parse(&movement.target_slot_id)?;
        Ok(Self {
            source_slot_id,
            source_repository_id: movement.source_repository_id,
            source_entry_id: movement.source_entry_id,
            source_path: movement.source_path,
            target_slot_id,
            target_repository_id: movement.target_repository_id,
            target_path: movement.target_path,
            replace: movement.replace,
            target_entry_id: movement.target_entry_id,
            operation_id,
        })
    }

    /// Encode the admitted command into the engine's existing request representation.
    pub(crate) fn into_request(self) -> ObjectMoveRequest {
        ObjectMoveRequest {
            source_repository_id: self.source_repository_id,
            source_entry_id: self.source_entry_id,
            source_path: self.source_path,
            target_slot_id: self.target_slot_id.to_string(),
            target_repository_id: self.target_repository_id,
            target_path: self.target_path,
            replace: self.replace,
            target_entry_id: self.target_entry_id,
        }
    }
}

#[cfg(test)]
#[path = "../../../test/server/unit/backend_object_move.rs"]
mod tests;
