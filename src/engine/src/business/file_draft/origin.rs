//! A FILE node's original publication reason and ordinary namespace ownership.
//! An address returned by a read or a later successful save cannot issue it.
use super::{PublishedContent, invalid, ordinary::OrdinaryOccurrence};
use crate::{Result, State};
use data_serialization::{Deserialize, Serialize};
use storage_contract::file_draft::{DraftLocation, automatic::SaveDecision};
use storage_contract::{DiskId, EntryId, FileNodeId, Generation, OperationId, Sequence, Timestamp};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Reason {
    ManualFile {},
    ManualDirectory {},
    AutomaticSave { epoch: Generation, save: SaveDecision },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct NodeOrigin {
    pub(crate) reason: Reason,
    pub(crate) target: DraftLocation,
    pub(crate) owner: DiskId,
    pub(crate) ancestors: Vec<EntryId>,
    pub(crate) node: FileNodeId,
    pub(crate) operation: OperationId,
    pub(crate) sequence: Sequence,
    pub(crate) published_at: Timestamp,
}

impl NodeOrigin {
    pub(crate) fn from_content(state: &State, target: &DraftLocation, content: &PublishedContent,
        reason: Reason, operation: OperationId, sequence: Sequence, published_at: Timestamp)
        -> Result<Self>
    {
        let record = state.file_containers.get(&target.file).ok_or_else(invalid)?;
        let occurrence = OrdinaryOccurrence::capture(state, target.file, record)?;
        occurrence.require_target(target)?;
        if content.version.file_id != target.file.to_string()
            || record.replicas.len() != 1 || !record.replicas.contains_key(&record.owner_disk)
            || state.versions.contains_key(&content.version.version_id)
            || sequence.get() <= state.sequence
        { return Err(invalid()); }
        if let Reason::AutomaticSave { epoch, save } = reason {
            let controller = record.working.automatic_controller().ok_or_else(invalid)?;
            if controller.epoch() != epoch || controller.protection().is_some()
                || controller.pending_save() != Some(&save) || save.revision != record.working.revision()
                || operation == save.operation || sequence <= save.sequence
            { return Err(invalid()); }
        }
        Ok(Self { reason, target: target.clone(), owner: record.owner_disk,
            ancestors: occurrence.parents.into_iter().map(|(id, _)| id).collect(),
            node: FileNodeId::Version(content.version.version_id.parse()?),
            operation, sequence, published_at })
    }
}
