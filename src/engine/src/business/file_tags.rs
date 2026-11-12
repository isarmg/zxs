//! Mutable labels belong to one stable container, not its content graph.
use crate::engine::CurrentNamespaces;
use crate::{BranchState, Engine, Error, OperationId, OperationResult, Result, State};
use storage_contract::file_container_tags::{
    ContainerTags, Context, ReadRequest, ReplaceCommand, Replacement, Snapshot,
};
use storage_contract::{FileId, NamespaceEntryKind, Principal};

pub(crate) fn invalid() -> Error {
    Error::ContainerTags(crate::ContainerTagsError::Publication)
}

pub(crate) fn context(
    state: &State,
    current: Option<&CurrentNamespaces>,
    file: FileId,
    context: &Context,
    writable: bool,
) -> Result<()> {
    let missing = || Error::ContainerTags(crate::ContainerTagsError::Context);
    let repository = state
        .repositories
        .get(&context.repository_id.to_string())
        .ok_or_else(missing)?;
    let branch = state
        .branches
        .get(&repository.main_branch_id)
        .ok_or_else(invalid)?;
    if writable && branch.state != BranchState::Active {
        return Err(missing());
    }
    let entries = match current {
        Some(view) => {
            if writable {
                view.require_branch_write(state, &repository.main_branch_id)?;
            }
            view.branch_entries(&repository.main_branch_id, &branch.head_commit_id)?
        }
        None => {
            &state
                .commits
                .get(&branch.head_commit_id)
                .ok_or_else(invalid)?
                .entries
        }
    };
    let namespace = crate::business::NamespaceTree::from_current_state(
        state,
        &repository.repository_id,
        entries,
    );
    let entry = namespace
        .get_key(context.path.key())
        .ok_or_else(missing)?;
    if entry.kind != NamespaceEntryKind::File || entry.file_id != file.to_string() {
        return Err(missing());
    }
    let version = state.versions.get(&entry.version_id).ok_or_else(invalid)?;
    if version.file_id != entry.file_id || version.history_tree_id != repository.history_tree_id {
        return Err(invalid());
    }
    Ok(())
}

pub(crate) fn plan(
    state: &State,
    current: Option<&CurrentNamespaces>,
    request: &ReplaceCommand,
) -> Result<Replacement> {
    context(state, current, request.file_id, &request.context, true)?;
    let old = state
        .container_tags
        .get(&request.file_id)
        .ok_or_else(invalid)?;
    if old.generation() != request.expected_generation {
        return Err(Error::ContainerTags(crate::ContainerTagsError::Conflict));
    }
    let changed = old.tags() != &request.tags;
    let generation = if changed {
        old.generation().next().map_err(|source| {
            Error::ContainerTags(crate::ContainerTagsError::Exhausted(source))
        })?
    } else {
        old.generation()
    };
    Ok(Replacement {
        request: request.clone(),
        tags: request.tags.clone(),
        generation,
        changed,
    })
}

/// Other operations may create independent identities or retire unreferenced ones,
/// but cannot replace the CAS state of an identity that survives the transition.
pub(crate) fn preserve_existing(previous: &State, next: &State) -> Result<()> {
    let check = |id: &FileId| {
        match (previous.container_tags.get(id), next.container_tags.get(id)) {
            (Some(before), Some(after)) if before != after => Err(invalid()),
            _ => Ok(()),
        }
    };
    match next.container_tags.changes_since(&previous.container_tags) {
        storage_contract::MapChanges::Keys(ids) => ids.iter().try_for_each(check),
        storage_contract::MapChanges::All => previous.container_tags.keys().try_for_each(check),
    }
}

pub(crate) fn apply(state: &mut State, replacement: &Replacement) {
    if replacement.changed {
        state.container_tags.insert(
            replacement.request.file_id,
            ContainerTags::new(replacement.tags.clone(), replacement.generation),
        );
    }
}

impl Engine {
    /// Business response only; the caller must authorize access to this operation.
    pub fn container_tags_outcome(&self, operation: OperationId) -> Result<Option<Replacement>> {
        let Some(receipt) = self.state().operations.get(&operation) else {
            return Ok(None);
        };
        if receipt.kind != storage_contract::file_container_tags::KIND {
            return Ok(None);
        }
        let (replacement, _, _) = crate::persist::container_tags::decode(receipt)?;
        replacement
            .validate(&replacement.request)
            .map_err(|source| {
                Error::ContainerTags(crate::ContainerTagsError::Response(source))
            })?;
        Ok(Some(replacement))
    }

    pub fn file_container_tags(&self, request: &ReadRequest) -> Result<Snapshot> {
        let current = self.admitted_current_namespaces()?;
        context(self.state(), current, request.file_id, &request.context, false)?;
        let value = self
            .state()
            .container_tags
            .get(&request.file_id)
            .ok_or_else(invalid)?;
        Ok(Snapshot {
            file_id: request.file_id,
            context: request.context.clone(),
            tags: value.tags().clone(),
            generation: value.generation(),
        })
    }

    pub fn replace_container_tags(
        &mut self,
        request: &ReplaceCommand,
        principal: &Principal,
        operation: OperationId,
    ) -> Result<OperationResult> {
        crate::persist::container_tags::replace(self, request, principal, operation)
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/business/file_tags/tests.rs"]
mod tests;
