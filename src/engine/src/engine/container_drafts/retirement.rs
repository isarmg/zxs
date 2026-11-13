//! A fully published successor may retire a superseded draft's body, not its
//! original intent or permanent result. Local inventory never grants authority.
use super::{invalid, store};
use crate::{Engine, OperationResult, State};
use storage_contract::container_edit::{DraftRevision, EditCommand};
use storage_contract::FileId;

pub(in crate::engine) struct PublishedDraftSuccessors {
    state: State,
}

impl PublishedDraftSuccessors {
    /// The caller owns either live publication admission or a complete verified
    /// journal prefix, including every original container-edit transition.
    pub(in crate::engine) fn capture(state: &State) -> Self {
        Self { state: state.clone() }
    }

    pub(in crate::engine) fn requires_full_replay(&self) -> bool {
        self.state.operations.values().any(|receipt| receipt.kind == super::EDIT_KIND)
    }

    pub(in crate::engine) fn permits(
        &self,
        file: FileId,
        revision: &DraftRevision,
        original: Option<&OperationResult>,
    ) -> crate::Result<bool> {
        let Some(receipt) = self.state.operations.get(&revision.operation) else {
            return Ok(false);
        };
        if original.is_some_and(|old| old != receipt) {
            return Err(invalid());
        }
        if receipt.kind != super::EDIT_KIND {
            return Err(invalid());
        }
        let principal = self.state.operation_principals.get(&revision.operation)
            .ok_or_else(invalid)?;
        let decoded = crate::persist::container_edits::decode(receipt)?;
        if principal != revision.principal.as_str()
            || decoded.request != (EditCommand::Save { file, revision: revision.clone() })
            || *receipt != crate::persist::container_edits::expected_receipt(
                revision.operation, principal, receipt.sequence, &decoded,
            )?
        {
            return Err(invalid());
        }
        let Some(current) = self.state.container_edits.get(&file) else {
            return Ok(false);
        };
        Ok(self.state.sequence > receipt.sequence
            && current.generation() > decoded.replacement.generation()
            && current.draft().is_none_or(|draft| draft.operation != revision.operation))
    }
}

impl Engine {
    /// Validate every final selected body before recovery can use a later
    /// publication to justify absence of a historical, superseded body.
    pub(in crate::engine) fn verify_selected_draft_bodies(
        &self, state: &State,
    ) -> crate::Result<()> {
        for (file, edit) in state.container_edits.iter() {
            let Some(revision) = edit.draft() else { continue };
            let placement = state.placements.get(&revision.base.version.to_string())
                .ok_or_else(invalid)?;
            let disk = self.disk(&placement.primary_slot_id)?;
            let handle = self.disk_resources.get(disk)?;
            store::reopen(
                handle, *file, revision.expected_generation, revision, self.draft_limits(),
            ).map_err(|error| crate::Error::ContainerDrafts(super::DraftError::Store(error)))?;
        }
        Ok(())
    }

    fn retains_draft_workspace(&self, file: FileId, revision: &DraftRevision) -> bool {
        self.workspaces.values().any(|workspace| {
            // Successful abort persists terminal mirrors before updating this
            // admitted map. Its retained binding is evidence, not a consumer;
            // an outstanding preparation still keeps its original body pinned.
            let may_consume = match workspace.state {
                crate::WorkspaceState::Aborted => {
                    self.workspace_has_preparation(&workspace.workspace_id)
                }
                crate::WorkspaceState::Open
                | crate::WorkspaceState::Sealed
                | crate::WorkspaceState::Committed => true,
            };
            may_consume && workspace.container_draft_commit.as_ref().is_some_and(|binding| {
                binding.file == file && binding.revision.operation == revision.operation
            })
        })
    }

    /// Retry before allocation, not only after successful saves: readers may
    /// have released their last descriptor since the previous attempt.
    pub(super) fn retire_superseded_drafts(&self, file: FileId) -> crate::Result<()> {
        self.require_write()?;
        let proof = PublishedDraftSuccessors::capture(&self.state);
        for disk in self.config.io_disks() {
            let Some(handle) = self.disk_resources.available(disk) else { continue };
            store::retire_superseded(handle, file, |revision| {
                // A matching receipt does not authorize deleting an arbitrary
                // copy on another disk. Changed placement is retained until a
                // separate original-source proof can establish its ownership.
                let placed_here = self.state.placements
                    .get(&revision.base.version.to_string())
                    .is_some_and(|placement| placement.primary_slot_id == disk.slot_id);
                if !placed_here || self.retains_draft_workspace(file, revision) {
                    return Ok(false);
                }
                proof.permits(file, revision, None)
            }).map_err(|error| crate::Error::ContainerDrafts(super::DraftError::Store(error)))?;
        }
        Ok(())
    }

    pub(in crate::engine) fn verify_recovered_draft_material(
        &self,
        next: &State,
        proof: Option<&PublishedDraftSuccessors>,
    ) -> crate::Result<()> {
        for (file, edit) in next.container_edits.iter() {
            if self.state.container_edits.get(file) == Some(edit) {
                continue;
            }
            let Some(revision) = edit.draft() else { continue };
            if self.current_container_draft_base(
                *file, revision.base.repository, revision.base.entry,
            )? != revision.base {
                return Err(crate::Error::ContainerEdit(
                    storage_contract::container_edit::EditConflict::SourceChanged,
                ));
            }
            let permitted = match proof {
                Some(proof) if !self.retains_draft_workspace(*file, revision) => {
                    proof.permits(*file, revision, next.operations.get(&revision.operation))?
                }
                Some(_) | None => false,
            };
            if permitted {
                let disk = self.draft_disk(revision)?;
                store::verify_retired_or_present(
                    &disk, *file, revision, self.draft_limits(),
                ).map_err(|error| crate::Error::ContainerDrafts(super::DraftError::Store(error)))?;
            } else {
                let disk = self.draft_disk(revision)?;
                store::reopen(
                    &disk, *file, revision.expected_generation, revision, self.draft_limits(),
                ).map_err(|error| crate::Error::ContainerDrafts(super::DraftError::Store(error)))?;
            }
        }
        Ok(())
    }
}
