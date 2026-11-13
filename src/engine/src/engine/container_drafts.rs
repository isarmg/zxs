//! Shared edit authority is selected by SYSTEM publication, never local bodies.
pub(crate) mod store;
pub(crate) mod commit;
pub(crate) mod format;

use crate::{Engine, Error, OperationId, OperationResult, State};
use storage_contract::container_edit::{
    ContainerEdit, DraftBase, DraftRevision, EditCommand, EditConflict, SetCommitMode,
};
use storage_contract::{FileId, Generation, Principal};

pub(crate) const EDIT_KIND: &str = "file.container-edit";

#[cfg(test)]
#[path = "../../../../test/engine/unit/container_drafts_tests.rs"]
mod tests;

#[derive(Debug, error_definition::Error)]
pub enum DraftError {
    #[error(transparent)]
    Format(crate::authority_selection::manual::ManualFormatError),
    #[error("container {0} has no retained content identity")]
    FileMissing(FileId),
    #[error("container edit publication differs from its original request")]
    Publication,
    #[error("container edit receipt encoding failed")]
    Encoding(#[source] json_codec::Error),
    #[error("container edit publication generation exhausted")]
    Generation(#[source] storage_contract::GenerationExhausted),
    #[error(transparent)]
    Store(store::StoreError),
}

impl DraftError {
    pub fn code(&self) -> crate::ErrorCode {
        match self {
            Self::Format(error) => error.code(),
            Self::FileMissing(_) => crate::ErrorCode::FileNotFound,
            Self::Publication | Self::Encoding(_) => crate::ErrorCode::PublicationInvalid,
            Self::Generation(_) => crate::ErrorCode::ResourceLimit,
            Self::Store(store::StoreError::Engine(error)) => error.code(),
            Self::Store(store::StoreError::Budget) => crate::ErrorCode::ResourceLimit,
            Self::Store(store::StoreError::Codec(_))
            | Self::Store(store::StoreError::Payload(_))
            | Self::Store(store::StoreError::Conflict)
            | Self::Store(store::StoreError::Incomplete) => crate::ErrorCode::RecoveryRequired,
        }
    }
}

pub(crate) fn format_error(error: crate::authority_selection::manual::ManualFormatError) -> Error {
    Error::ContainerDrafts(DraftError::Format(error))
}

pub(crate) fn invalid() -> Error {
    Error::ContainerDrafts(DraftError::Publication)
}

pub(crate) fn current(state: &State, file: FileId) -> crate::Result<ContainerEdit> {
    if !state.container_tags.contains_key(&file) {
        return Err(Error::ContainerDrafts(DraftError::FileMissing(file)));
    }
    // Absence in the required edit table is the specified initial automatic
    // mode, not a fallback for a missing disk field or unreadable draft body.
    Ok(state.container_edits.get(&file).cloned().unwrap_or_else(ContainerEdit::initial))
}

pub(crate) fn plan_edit(state: &State, request: &EditCommand) -> crate::Result<ContainerEdit> {
    let previous = current(state, request.file())?;
    match request {
        EditCommand::SetMode { request } => {
            previous.set_mode(request.expected_generation, request.mode)
        }
        EditCommand::Save { revision, .. } => {
            previous.save(revision.expected_generation, revision.clone())
        }
        EditCommand::Discard { expected_generation, draft, .. } => {
            previous.discard(*expected_generation, *draft)
        }
    }.map_err(Error::ContainerEdit)
}

pub struct ContainerDraftRead {
    pub file: crate::ReadFile,
    pub revision: DraftRevision,
    pub checksums: storage_contract::ChecksumManifest,
}

impl Engine {
    /// Namespace admission only. The caller still authorizes the authenticated principal.
    pub fn require_container_draft_context(
        &self,
        file: FileId,
        context: &storage_contract::file_container_tags::Context,
        writable: bool,
    ) -> crate::Result<()> {
        let current = self.admitted_current_namespaces()?;
        crate::business::file_tags::context(&self.state, current, file, context, writable)
    }

    pub fn container_edit(&self, file: FileId) -> crate::Result<ContainerEdit> {
        current(&self.state, file)
    }

    pub fn set_container_commit_mode(
        &mut self,
        request: &SetCommitMode,
        principal: &Principal,
        operation: OperationId,
    ) -> crate::Result<OperationResult> {
        crate::persist::container_edits::publish(
            self, &EditCommand::SetMode { request: request.clone() }, principal, operation,
        )
    }

    pub fn current_container_draft_base(
        &self,
        file: FileId,
        repository: storage_contract::RepositoryId,
        occurrence: storage_contract::EntryId,
    ) -> crate::Result<DraftBase> {
        let repository = self.state.repositories.get(&repository.to_string())
            .ok_or_else(invalid)?;
        let snapshot = self.namespace_snapshot(&repository.main_branch_id)?;
        let entry = snapshot.entries().iter()
            .find(|entry| entry.entry_id == occurrence.to_string()
                && entry.file_id == file.to_string()
                && entry.kind == crate::NamespaceEntryKind::File)
            .ok_or_else(invalid)?;
        let context = self.file_context_for_current_entry(&repository.repository_id, entry)
            .ok_or_else(invalid)?;
        let version = entry.version_id.parse::<storage_contract::VersionId>()?;
        Ok(DraftBase {
            entry: occurrence,
            repository: repository.repository_id.parse()?,
            branch: context.branch_id.parse()?,
            node: match context.node_id.as_deref() {
                Some(node) => node.parse()?,
                None => storage_contract::FileNodeId::Version(version),
            },
            version,
            selection_generation: Generation::new(context.selection_generation),
        })
    }

    fn require_draft_source_write(&self, file: FileId, revision: &DraftRevision) -> crate::Result<()> {
        let repository = self.state.repositories.get(&revision.base.repository.to_string())
            .ok_or_else(invalid)?;
        let snapshot = self.namespace_snapshot(&repository.main_branch_id)?;
        let entry = snapshot.entries().iter().find(|entry| {
            entry.entry_id == revision.base.entry.to_string()
                && entry.file_id == file.to_string()
        }).ok_or_else(invalid)?;
        self.require_container_draft_context(file, &storage_contract::file_container_tags::Context {
            repository_id: revision.base.repository,
            path: entry.location().clone(),
        }, true)
    }

    /// The source is a completed upload, not a mutable version or an SMB path.
    /// A failed predecision preparation retains its original body and intent.
    pub fn save_container_draft(
        &mut self,
        file: FileId,
        revision: &DraftRevision,
        source: std::fs::File,
    ) -> crate::Result<OperationResult> {
        self.require_write()?;
        let request = EditCommand::Save { file, revision: revision.clone() };
        if self.state.operations.contains_key(&revision.operation) {
            return crate::persist::container_edits::publish(
                self, &request, &revision.principal, revision.operation,
            );
        }
        plan_edit(&self.state, &request)?;
        self.require_draft_source_write(file, revision)?;
        if self.current_container_draft_base(file, revision.base.repository, revision.base.entry)?
            != revision.base
        {
            return Err(Error::ContainerEdit(EditConflict::SourceChanged));
        }
        let handle = self.draft_disk(revision)?;
        let prepared = store::prepare(
            &handle, file, revision.expected_generation, revision, source, self.draft_limits(),
        ).map_err(|error| Error::ContainerDrafts(DraftError::Store(error)))?;
        let result = crate::persist::container_edits::publish(
            self, &request, &revision.principal, revision.operation,
        );
        drop(prepared);
        result
    }

    pub fn discard_container_draft(
        &mut self,
        file: FileId,
        expected_generation: Generation,
        draft: OperationId,
        principal: &Principal,
        operation: OperationId,
    ) -> crate::Result<OperationResult> {
        crate::persist::container_edits::publish(
            self,
            &EditCommand::Discard { file, expected_generation, draft },
            principal,
            operation,
        )
    }

    /// None means an admitted clean state. Missing or corrupt selected bodies
    /// return an error and must never fall back to immutable history content.
    pub fn open_container_draft(&self, file: FileId) -> crate::Result<Option<ContainerDraftRead>> {
        let edit = self.container_edit(file)?;
        let Some(revision) = edit.draft() else {
            return Ok(None);
        };
        if self.current_container_draft_base(file, revision.base.repository, revision.base.entry)?
            != revision.base
        {
            return Err(Error::ContainerEdit(EditConflict::SourceChanged));
        }
        let handle = self.draft_disk(revision)?;
        let prepared = store::reopen(
            &handle, file, revision.expected_generation, revision, self.draft_limits(),
        ).map_err(|error| Error::ContainerDrafts(DraftError::Store(error)))?;
        let (body, checksums) = prepared.into_read_parts();
        Ok(Some(ContainerDraftRead {
            file: body.into(),
            revision: revision.clone(),
            checksums,
        }))
    }

    fn draft_limits(&self) -> store::DraftLimits {
        store::DraftLimits {
            max_file_bytes: self.config.max_file_bytes,
            checksum_chunk_bytes: self.config.checksum_chunk_bytes,
            max_entries: self.config.max_retained_workspaces,
            max_retained_bytes: self.config.max_workspace_bytes,
        }
    }

    fn draft_disk(
        &self,
        revision: &DraftRevision,
    ) -> crate::Result<std::sync::Arc<crate::disk::DiskHandle>> {
        let placement = self.state.placements.get(&revision.base.version.to_string())
            .ok_or_else(invalid)?;
        let disk = self.disk(&placement.primary_slot_id)?;
        Ok(std::sync::Arc::clone(self.disk_resources.get(disk)?))
    }

    pub(in crate::engine) fn verify_draft_material(&self, next: &State) -> crate::Result<()> {
        for (file, edit) in next.container_edits.iter() {
            if self.state.container_edits.get(file) == Some(edit) {
                continue;
            }
            if let Some(revision) = edit.draft() {
                let current = self.current_container_draft_base(
                    *file, revision.base.repository, revision.base.entry,
                )?;
                if current != revision.base {
                    return Err(Error::ContainerEdit(EditConflict::SourceChanged));
                }
                let handle = self.draft_disk(revision)?;
                store::reopen(
                    &handle, *file, revision.expected_generation, revision, self.draft_limits(),
                ).map_err(|error| Error::ContainerDrafts(DraftError::Store(error)))?;
            }
        }
        Ok(())
    }
}

/// Verify the complete state delta before either live publication or cold repair.
/// An arbitrary mutation cannot add, remove or rewrite shared edit authority.
pub(crate) fn verify_transition(previous: &State, next: &State) -> crate::Result<()> {
    let consumed = commit::consumption(previous, next)?;
    // The interaction with explicit directory commits remains a separately
    // reserved contract. This file-container feature does not redefine it.
    let directory_commit = next.events.last().is_some_and(|event| {
        event.sequence == next.sequence && event.kind == "directory.changes.commit"
    });
    for (file, edit) in previous.container_edits.iter() {
        // A clean manual container also cannot be auto-published by a legacy
        // content-save path that forgot to route through shared draft storage.
        if !directory_commit && consumed != Some(*file)
            && edit.mode() == storage_contract::container_edit::CommitMode::Manual
        {
            let owns_new_version = |id: &String| {
                !previous.versions.contains_key(id)
                    && next.versions.get(id).is_some_and(|version| {
                        version.file_id == file.to_string()
                    })
            };
            let creates_version = match next.versions.changes_since(&previous.versions) {
                storage_contract::MapChanges::Keys(keys) => keys.iter().any(owns_new_version),
                storage_contract::MapChanges::All => next.versions.keys().any(owns_new_version),
            };
            if creates_version {
                return Err(Error::ContainerEdit(EditConflict::UncommittedDraft));
            }
        }
        if consumed == Some(*file) {
            continue;
        }
        let Some(draft) = edit.draft() else {
            continue;
        };
        let key = file.to_string();
        let version = draft.base.version.to_string();
        let selected = draft.base.branch.to_string();
        let branch_source_changed = match (
            previous.branches.get(&selected), next.branches.get(&selected),
        ) {
            (Some(before), Some(after)) => {
                let selected_entry = |state: &State, head: &str| {
                    state.commits.get(head).and_then(|commit| {
                        commit.entries.iter().find(|entry| {
                            entry.entry_id == draft.base.entry.to_string()
                        }).map(|entry| (entry.file_id.clone(), entry.version_id.clone()))
                    })
                };
                (before.file_id.as_deref() == Some(key.as_str())
                    && before.head_commit_id != after.head_commit_id)
                    || selected_entry(previous, &before.head_commit_id)
                        != selected_entry(next, &after.head_commit_id)
                    || before.file_node_id != after.file_node_id
                    || before.pending_file_versions.get(&key)
                        != after.pending_file_versions.get(&key)
                    || before.state != after.state
            }
            (None, None) => false,
            (Some(_), None) | (None, Some(_)) => true,
        };
        let selection_or_source_changed = branch_source_changed
            || previous.file_selections.get(&key) != next.file_selections.get(&key)
            || previous.file_selection_generations.get(&key)
                != next.file_selection_generations.get(&key)
            || previous.placements.get(&version) != next.placements.get(&version)
            || previous.versions.get(&version) != next.versions.get(&version)
            || previous.content_states.get(&version) != next.content_states.get(&version);
        if !directory_commit && selection_or_source_changed {
            return Err(Error::ContainerEdit(EditConflict::UncommittedDraft));
        }
        if !directory_commit {
            let changes = next.versions.changes_since(&previous.versions);
            let creates_version = match changes {
                storage_contract::MapChanges::Keys(keys) => keys.iter().any(|id| {
                    !previous.versions.contains_key(id)
                        && next.versions.get(id).is_some_and(|version| version.file_id == key)
                }),
                storage_contract::MapChanges::All => next.versions.iter().any(|(id, version)| {
                    !previous.versions.contains_key(id) && version.file_id == key
                }),
            };
            if creates_version {
                return Err(Error::ContainerEdit(EditConflict::UncommittedDraft));
            }
        }
    }
    let mut changed = std::collections::BTreeSet::new();
    match next.operations.changes_since(&previous.operations) {
        storage_contract::MapChanges::Keys(keys) => changed.extend(keys.iter().copied()),
        storage_contract::MapChanges::All => {
            changed.extend(previous.operations.keys().copied());
            changed.extend(next.operations.keys().copied());
        }
    }
    match next.operation_principals.changes_since(&previous.operation_principals) {
        storage_contract::MapChanges::Keys(keys) => changed.extend(keys.iter().copied()),
        storage_contract::MapChanges::All => {
            changed.extend(previous.operation_principals.keys().copied());
            changed.extend(next.operation_principals.keys().copied());
        }
    }
    let mut receipt = None;
    for id in changed {
        let before = previous.operations.get(&id);
        let after = next.operations.get(&id);
        if let Some(old) = before.filter(|old| old.kind == EDIT_KIND) {
            if Some(old) != after
                || previous.operation_principals.get(&id) != next.operation_principals.get(&id)
            {
                return Err(invalid());
            }
        } else if let Some(value) = after.filter(|value| value.kind == EDIT_KIND) {
            if before.is_some() || value.operation_id != id || receipt.replace(value).is_some() {
                return Err(invalid());
            }
        }
    }
    let Some(receipt) = receipt else {
        if consumed.is_none() && previous.container_edits != next.container_edits {
            return Err(invalid());
        }
        return Ok(());
    };
    let principal = next.operation_principals.get(&receipt.operation_id).ok_or_else(invalid)?;
    let decoded = crate::persist::container_edits::decode(receipt)?;
    if let EditCommand::Save { revision, .. } = &decoded.request {
        if revision.operation != receipt.operation_id || revision.principal.as_str() != principal {
            return Err(invalid());
        }
    }
    let replacement = plan_edit(previous, &decoded.request)?;
    if replacement != decoded.replacement || principal.is_empty() {
        return Err(invalid());
    }
    let expected_receipt = crate::persist::container_edits::expected_receipt(
        receipt.operation_id, principal, next.sequence, &decoded,
    )?;
    if *receipt != expected_receipt {
        return Err(invalid());
    }
    let mut expected = previous.clone();
    expected.begin_tracking();
    expected.container_edits.insert(decoded.request.file(), replacement);
    expected.sequence = next.sequence;
    expected.business_generation = Generation::new(previous.business_generation)
        .next()
        .map_err(|error| Error::ContainerDrafts(DraftError::Generation(error)))?
        .get();
    expected.operations.insert(receipt.operation_id, expected_receipt);
    expected.operation_principals.insert(receipt.operation_id, principal.clone());
    expected.events.push(storage_contract::Event {
        sequence: next.sequence,
        operation_id: receipt.operation_id,
        kind: EDIT_KIND.to_owned(),
        object_id: receipt.operation_id.to_string(),
    });
    if next.sequence <= previous.sequence
        || !crate::publication_shape::same_transition(previous, &expected, next)
    {
        return Err(invalid());
    }
    Ok(())
}
