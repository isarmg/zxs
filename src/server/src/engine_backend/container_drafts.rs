//! Current-context container reads and policy changes under one credential lock.
use crate::{ApiError, Content, CurrentContainerContent, Identity, ReadContainerEdit};
use crate::engine_backend::{EngineBackend, error, repository};
use storage_contract::container_edit::{ContainerEdit, DraftBase, SetCommitMode};

fn context(
    engine: &storage_engine::Engine,
    identity: &Identity,
    request: &ReadContainerEdit,
    writable: bool,
) -> Result<DraftBase, ApiError> {
    let repository = repository(engine, &request.repository.to_string())?;
    if identity.smb_binding.as_ref().is_some_and(|binding| {
        binding.repository_id != repository.repository_id
            || binding.branch_id != repository.main_branch_id
    }) {
        return Err(ApiError::forbidden());
    }
    engine.require_container_draft_context(
        request.file,
        &storage_contract::file_container_tags::Context {
            repository_id: request.repository,
            path: request.path.clone(),
        },
        writable,
    ).map_err(error)?;
    let namespace = engine.namespace_tree(&repository.main_branch_id).map_err(error)?;
    let entry = namespace.get_key(request.path.key()).ok_or_else(|| {
        error(storage_engine::Error::ContainerTags(storage_engine::ContainerTagsError::Context))
    })?;
    let entry = entry.entry_id.parse::<storage_contract::EntryId>()
        .map_err(|source| error(storage_engine::Error::from(source)))?;
    engine.current_container_draft_base(request.file, request.repository, entry).map_err(error)
}

impl EngineBackend {
    pub(super) fn read_container_edit_admitted(
        &self, identity: &Identity, request: ReadContainerEdit,
    ) -> Result<ContainerEdit, ApiError> {
        let (mut engine, current) = self.lock_current(identity)?;
        super::application::Application::new(&mut engine, &current)
            .authorize_file(&request.file.to_string())?;
        context(&engine, &current, &request, false)?;
        engine.container_edit(request.file).map_err(error)
    }

    pub(super) fn set_container_commit_mode_admitted(
        &self, identity: &Identity, request: crate::SetContainerCommitMode,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        let (mut engine, current) = self.lock_current(identity)?;
        // A filesystem save is not permission to change the container policy.
        if current.smb_binding.is_some() {
            return Err(ApiError::forbidden());
        }
        super::application::Application::new(&mut engine, &current)
            .authorize_file(&request.context.file.to_string())?;
        context(&engine, &current, &request.context, true)?;
        let principal = current.principal.parse()
            .map_err(ApiError::invalid_authenticated_principal)?;
        engine.set_container_commit_mode(&SetCommitMode {
            file: request.context.file,
            expected_generation: request.expected_generation,
            mode: request.mode,
        }, &principal, request.operation_id).map_err(error)
    }

    pub(super) fn prepare_container_draft_commit_admitted(
        &self, identity: &Identity, request: crate::PrepareContainerDraftCommit,
    ) -> Result<storage_contract::SealedWorkspace, ApiError> {
        let (mut engine, current) = self.lock_current(identity)?;
        if current.smb_binding.is_some() { return Err(ApiError::forbidden()); }
        super::application::Application::new(&mut engine, &current)
            .authorize_file(&request.context.file.to_string())?;
        let principal = current.principal.parse()
            .map_err(ApiError::invalid_authenticated_principal)?;
        engine.prepare_shared_container_draft_commit(request.context.file,
            &storage_contract::file_container_tags::Context {
                repository_id: request.context.repository, path: request.context.path,
            }, request.expected_generation, request.draft_operation,
            &principal, request.operation_id).map_err(error)
    }

    pub(super) fn persist_container_draft_admitted(
        &self, identity: &Identity, request: crate::PersistContainerDraft,
        source: &std::fs::File,
    ) -> Result<crate::DraftPersisted, ApiError> {
        let (mut engine, current) = self.lock_current(identity)?;
        super::application::Application::new(&mut engine, &current)
            .authorize_file(&request.context.file.to_string())?;
        let base = context(&engine, &current, &request.context, true)?;
        if request.revision.principal.as_str() != current.principal {
            return Err(ApiError::forbidden());
        }
        let edit = engine.container_edit(request.context.file).map_err(error)?;
        let expected_base = edit.draft().map_or(&base, |draft| &draft.base);
        if expected_base != &request.revision.base {
            return Err(error(storage_engine::Error::ContainerEdit(
                storage_contract::container_edit::EditConflict::SourceChanged,
            )));
        }
        // A different occurrence may edit the shared draft, but cannot change
        // its original source or borrow authority from the caller's path alone.
        let original = repository(&engine, &request.revision.base.repository.to_string())?;
        let namespace = engine.namespace_tree(&original.main_branch_id).map_err(error)?;
        let source_entry = namespace.entries().iter().find(|entry| {
            entry.entry_id == request.revision.base.entry.to_string()
                && entry.file_id == request.context.file.to_string()
        }).ok_or_else(|| error(storage_engine::Error::ContainerTags(
            storage_engine::ContainerTagsError::Context,
        )))?;
        let source_context = ReadContainerEdit {
            file: request.context.file,
            repository: request.revision.base.repository,
            path: source_entry.path.clone(),
        };
        if context(&engine, &current, &source_context, true)? != request.revision.base {
            return Err(error(storage_engine::Error::ContainerEdit(
                storage_contract::container_edit::EditConflict::SourceChanged,
            )));
        }
        let receipt = engine.save_container_draft(request.context.file, &request.revision,
            source.try_clone().map_err(ApiError::io)?).map_err(error)?;
        Ok(crate::DraftPersisted { revision: request.revision, receipt })
    }

    pub(super) fn current_container_content_admitted(
        &self, identity: &Identity, request: ReadContainerEdit,
    ) -> Result<CurrentContainerContent, ApiError> {
        let (mut engine, current) = self.lock_current(identity)?;
        super::application::Application::new(&mut engine, &current)
            .authorize_file(&request.file.to_string())?;
        let base = context(&engine, &current, &request, false)?;
        let edit = engine.container_edit(request.file).map_err(error)?;
        let download_name = Some(request.path.file_name().to_owned());
        // Reserve before the store allocates its authenticated chunk proof.
        // The smallest supported chunk is an upper bound across stored formats.
        let draft_proof = edit.draft().map(|revision| {
            let chunks = revision.size.div_ceil(65_536);
            let bytes = chunks.checked_mul(32)
                .and_then(|n| n.checked_add(storage_contract::CHECKSUM_HEADER_BYTES as u64))
                .and_then(|n| usize::try_from(n).ok())
                .ok_or_else(|| ApiError::limit("draft checksum proof exceeds memory budget"))?;
            crate::content_proof_budget::reserve(bytes)
        }).transpose()?;
        if let Some(draft) = engine.open_container_draft(request.file).map_err(error)? {
            let content = Content {
                fallback_files: Vec::new(),
                checksum_manifest: Some(draft.checksums),
                file: draft.file.into(),
                size: draft.revision.size,
                hash: draft.revision.content_hash.to_string(),
                lease: Box::new(draft_proof),
                download_name,
            };
            return Ok(CurrentContainerContent { edit, base, content });
        }
        // Only an admitted clean state reaches historical content. Missing or
        // damaged selected draft material above is an error, never a fallback.
        let plan = engine.prepare_content(&base.version.to_string()).map_err(error)?;
        let proof_memory =
            crate::content_proof_budget::reserve(plan.checksum_manifest_bytes().map_err(error)?)?;
        drop(engine);
        let opened = plan.open().map_err(error)?;
        let content = Content {
            checksum_manifest: Some(opened.manifest),
            fallback_files: opened.fallback_files.into_iter().map(Into::into).collect(),
            file: opened.file.into(),
            size: opened.size,
            hash: opened.content_hash.to_string(),
            lease: Box::new((opened.lease, proof_memory)),
            download_name,
        };
        Ok(CurrentContainerContent { edit, base, content })
    }
}
