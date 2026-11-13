//! Explicit sealed-file commit consumes a draft in the same permanent decision.
use crate::engine::container_drafts::{current, invalid};
use crate::{CommitRequest, Engine, Error, State, Workspace, WorkspacePurpose, WorkspaceState};
use storage_contract::container_edit::{DraftConsumption, EditConflict};
use storage_contract::{FileId, Generation, OperationId};

impl Engine {
    /// Prepare explicit submission directly from the admitted shared body. The
    /// browser never downloads/reuploads the draft to manufacture a workspace.
    pub fn prepare_shared_container_draft_commit(
        &mut self,
        file: FileId,
        context: &storage_contract::file_container_tags::Context,
        generation: Generation,
        draft: OperationId,
        principal: &storage_contract::Principal,
        operation: OperationId,
    ) -> crate::Result<crate::SealedWorkspace> {
        self.require_write()?;
        self.require_container_draft_context(file, context, true)?;
        let edit = current(&self.state, file)?;
        let revision = edit.draft().ok_or_else(invalid)?.clone();
        if edit.generation() != generation || revision.operation != draft {
            return Err(Error::ContainerEdit(EditConflict::GenerationChanged));
        }
        self.require_draft_source_write(file, &revision)?;
        if self.current_container_draft_base(
            file, revision.base.repository, revision.base.entry,
        )? != revision.base {
            return Err(Error::ContainerEdit(EditConflict::SourceChanged));
        }
        let repository = self.state.repositories.get(&revision.base.repository.to_string())
            .ok_or_else(invalid)?.clone();
        let entries = self.namespace_snapshot(&repository.main_branch_id)?;
        let entry = entries.entries().iter().find(|entry| {
            entry.entry_id == revision.base.entry.to_string()
                && entry.file_id == file.to_string()
        }).ok_or_else(invalid)?.clone();
        let binding = DraftConsumption { file, generation, revision: revision.clone(), operation };
        let retained = self.workspaces.values().filter(|workspace| {
            workspace.container_draft_commit.as_ref()
                .is_some_and(|value| value.operation == operation)
        }).cloned().collect::<Vec<_>>();
        if retained.len() > 1 {
            return Err(invalid());
        }
        let workspace = if let Some(workspace) = retained.into_iter().next() {
            if workspace.container_draft_commit.as_ref() != Some(&binding)
                || workspace.principal != principal.as_str()
            {
                return Err(invalid());
            }
            workspace
        } else {
            let mut workspace = self.create_workspace(
                &repository.main_branch_id, principal.as_str(),
            )?;
            // This is only preparation intent. Neither it nor local body bytes
            // grants permission to clear the SYSTEM draft without a decision.
            workspace.container_draft_commit = Some(binding);
            self.persist_workspace(&workspace)?;
            self.workspaces.insert(workspace.workspace_id.clone(), workspace.clone());
            workspace
        };
        if workspace.state == WorkspaceState::Open {
            let disk = self.draft_disk(&revision)?;
            let body = crate::engine::container_drafts::store::reopen(
                &disk, file, revision.expected_generation, &revision, self.draft_limits(),
            ).map_err(|error| Error::ContainerDrafts(
                crate::engine::container_drafts::DraftError::Store(error),
            ))?.into_body();
            let upload = self.prepare_upload_managed(
                &workspace.workspace_id, &entry.entry_id, entry.location(), body, 1,
                (revision.size, revision.content_hash.clone()),
            )?.execute()?;
            self.finish_upload(upload)?;
        }
        self.seal_container_draft_workspace(
            &workspace.workspace_id, principal.as_str(), file, generation, draft, operation,
        )
    }

    /// An explicit file-submit preparation, distinct from ordinary upload sealing.
    /// Callers retain the returned operation and complete frozen public envelope.
    pub fn seal_container_draft_workspace(
        &mut self,
        workspace: &str,
        principal: &str,
        file: FileId,
        generation: Generation,
        draft: OperationId,
        operation: OperationId,
    ) -> crate::Result<crate::SealedWorkspace> {
        let mut sealed = self.seal_workspace(workspace, principal)?;
        let request = CommitRequest {
            workspace_id: sealed.workspace_id.clone(),
            operation_id: operation,
            principal: principal.to_owned(),
            expected_head: sealed.expected_head.clone(),
            fencing_token: sealed.fencing_token,
            message: String::new(),
            request_digest: None,
        };
        self.prepare_container_draft_commit(&request, file, generation, draft)?;
        let workspace = self.workspace(workspace)?;
        sealed.operation_id = operation;
        sealed.container_draft_commit = workspace.container_draft_commit.clone();
        sealed.sealed_manifest_hash = crate::wire::manifest_hash(&workspace)?;
        Ok(sealed)
    }

    /// Bind an already sealed, single-file workspace to an explicit commit.
    /// This does not publish content and must not be called for automatic flush.
    pub fn prepare_container_draft_commit(
        &mut self,
        request: &CommitRequest,
        file: FileId,
        generation: Generation,
        draft: OperationId,
    ) -> crate::Result<()> {
        self.require_write()?;
        let mut workspace = self.workspace(&request.workspace_id)?;
        if let Some(binding) = &workspace.container_draft_commit {
            if binding.file != file || binding.generation != generation
                || binding.revision.operation != draft || binding.operation != request.operation_id
            {
                return Err(invalid());
            }
            if self.state.operations.contains_key(&request.operation_id) {
                // The normal commit replay checks the persisted exact request/digest.
                return Ok(());
            }
        }
        let edit = current(&self.state, file)?;
        let revision = edit.draft().ok_or_else(invalid)?.clone();
        if edit.generation() != generation || revision.operation != draft {
            return Err(Error::ContainerEdit(EditConflict::GenerationChanged));
        }
        let binding = DraftConsumption {
            file, generation, revision, operation: request.operation_id,
        };
        self.require_draft_source_write(file, &binding.revision)?;
        if self.current_container_draft_base(
            file, binding.revision.base.repository, binding.revision.base.entry,
        )? != binding.revision.base {
            return Err(Error::ContainerEdit(EditConflict::SourceChanged));
        }
        // Reopen the selected, hash-verified body before authorizing consumption.
        let _body = self.open_container_draft(file)?.ok_or_else(invalid)?;
        let current_entries = self.namespace_snapshot(&workspace.branch_id)?;
        validate_workspace(
            &self.state,
            self.admitted_current_namespaces()?,
            current_entries.entries(),
            &workspace,
            request,
            &binding,
        )?;
        workspace.container_draft_commit = Some(binding);
        self.persist_workspace(&workspace)?;
        self.workspaces.insert(workspace.workspace_id.clone(), workspace);
        Ok(())
    }
}

pub(crate) fn validate_workspace(
    state: &State,
    view: Option<&crate::engine::CurrentNamespaces>,
    current_entries: &[crate::Entry],
    workspace: &Workspace,
    request: &CommitRequest,
    binding: &DraftConsumption,
) -> crate::Result<()> {
    let edit = current(state, binding.file)?;
    if binding.operation != request.operation_id
        || edit.generation() != binding.generation
        || edit.draft() != Some(&binding.revision)
        || workspace.principal != request.principal
        || workspace.purpose != WorkspacePurpose::Normal
        || workspace.state != WorkspaceState::Sealed
        || workspace.fencing_token != request.fencing_token
        || workspace.base_head != request.expected_head
        || workspace.files.len() != 1
        || !workspace.namespace_operations.is_empty()
    {
        return Err(invalid());
    }
    let repository = state.repositories.get(&binding.revision.base.repository.to_string())
        .ok_or_else(invalid)?;
    if workspace.branch_id != repository.main_branch_id {
        // Historical branch creation needs its own explicit historical-save proof.
        return Err(Error::ContainerEdit(EditConflict::SourceChanged));
    }
    let staged = workspace.files.values().next().ok_or_else(invalid)?;
    if let Some(current) = view {
        current.require_branch_write(state, &workspace.branch_id)?;
    }
    // Callers supply the same admitted, current-name projection used by ordinary
    // namespace reads. Initial admission has no registry yet; Pending is rejected
    // by that boundary rather than treated as an empty or historical fallback.
    let entry = current_entries.iter().find(|entry| {
        entry.entry_id == binding.revision.base.entry.to_string()
            && entry.file_id == binding.file.to_string()
    }).ok_or_else(invalid)?;
    if staged.path.key() != entry.lookup_key()
        || staged.content_hash != binding.revision.content_hash
        || staged.size != binding.revision.size
        || entry.version_id != binding.revision.base.version.to_string()
    {
        return Err(Error::ContainerEdit(EditConflict::SourceChanged));
    }
    Ok(())
}

pub(crate) fn apply(state: &mut State, binding: &DraftConsumption) -> crate::Result<()> {
    let edit = current(state, binding.file)?;
    if edit.draft() != Some(&binding.revision) {
        return Err(invalid());
    }
    let clean = edit.discard(binding.generation, binding.revision.operation)
        .map_err(Error::ContainerEdit)?;
    state.container_edits.insert(binding.file, clean);
    Ok(())
}

/// Validate the typed receipt against both sides of the permanent decision.
/// The returned identity is the sole exception to the dirty-source fence.
pub(crate) fn consumption(previous: &State, next: &State) -> crate::Result<Option<FileId>> {
    let mut accepted = None;
    let operations = match next.operations.changes_since(&previous.operations) {
        storage_contract::MapChanges::Keys(keys) => (*keys).clone(),
        storage_contract::MapChanges::All => next.operations.keys().copied().collect(),
    };
    for operation in &operations {
        let Some(receipt) = next.operations.get(operation) else {
            continue;
        };
        if previous.operations.contains_key(operation) {
            continue;
        }
        let Some(binding) = crate::persist::container_edits::consumption(receipt)? else {
            continue;
        };
        if accepted.replace(binding.file).is_some()
            || binding.operation != *operation
            || receipt.kind != "workspace.commit"
            || receipt.sequence != next.sequence
        {
            return Err(invalid());
        }
        let edit = current(previous, binding.file)?;
        if edit.generation() != binding.generation || edit.draft() != Some(&binding.revision) {
            return Err(invalid());
        }
        let mut expected = previous.container_edits.clone();
        expected.insert(binding.file, edit.discard(binding.generation, binding.revision.operation)
            .map_err(Error::ContainerEdit)?);
        if expected != next.container_edits {
            return Err(invalid());
        }
        let source = binding.revision.base.version.to_string();
        if previous.versions.get(&source) != next.versions.get(&source)
            || previous.placements.get(&source) != next.placements.get(&source)
            || previous.content_states.get(&source) != next.content_states.get(&source)
        {
            return Err(invalid());
        }
        let repository = previous.repositories
            .get(&binding.revision.base.repository.to_string()).ok_or_else(invalid)?;
        if receipt.branch_id.as_deref() != Some(repository.main_branch_id.as_str())
            || !next.operation_principals.get(operation).is_some_and(|owner| !owner.is_empty())
        {
            return Err(invalid());
        }
        let commit = next.commits.get(receipt.commit_id.as_deref().ok_or_else(invalid)?)
            .ok_or_else(invalid)?;
        if commit.history_tree_id != repository.history_tree_id {
            return Err(invalid());
        }
        let entry = commit.entries.iter().find(|entry| {
            entry.entry_id == binding.revision.base.entry.to_string()
                && entry.file_id == binding.file.to_string()
        }).ok_or_else(invalid)?;
        let version = next.versions.get(&entry.version_id).ok_or_else(invalid)?;
        if version.file_id != binding.file.to_string()
            || version.content_hash != binding.revision.content_hash
            || version.logical_size != binding.revision.size
        {
            return Err(invalid());
        }
    }
    Ok(accepted)
}
