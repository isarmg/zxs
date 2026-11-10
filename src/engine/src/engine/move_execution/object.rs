//! The one path-facing operation for current files, folders and repositories.
//! Transport adapters supply intent and captured identity, never a move plan.
use super::*;
use storage_contract::file_relocation::{ObjectMoveFact, ObjectMoveRequest, OBJECT_MOVE_KIND};
use crate::{CapturedFileMoveSource, FileHistoryMovePathRequest, FileHistoryMoveReplacement};
use storage_contract::history_window::{HistoryFileId, HistoryRepositoryId};
use storage_contract::file_relocation::NamespaceEntryId;

fn conflict() -> Error {
    Error::new(ErrorCode::IdempotencyKeyReused,
        "move operation differs from its original actor, request or captured identities")
}

fn fact(receipt: &OperationResult) -> Result<ObjectMoveFact> {
    Ok(crate::business::object_move::read_result(receipt)?.movement)
}

struct CapturedFiles<'a> {
    source: Option<&'a HistoryFileId>,
    target: Option<&'a HistoryFileId>,
}

fn same_entry_target(request: &ObjectMoveRequest) -> bool {
    request.target_repository_id.as_ref() == Some(&request.source_repository_id)
        && request.target_entry_id.as_ref() == Some(&request.source_entry_id)
        && request.target_path.key() == request.source_path.key()
}

fn require_identity(original: &ObjectMoveFact, files: Option<&CapturedFiles<'_>>) -> Result<()> {
    if let Some(files) = files {
        if original.source_file_id.as_ref() != files.source
            || original.displaced_target_file_id.as_ref() != files.target
        { return Err(conflict()); }
    }
    Ok(())
}

impl Engine {
    /// Old path-facing APIs are adapters to the same new-operation planner.
    /// Only a retained original file receipt/intent resumes the old projection.
    pub(in crate::engine) fn move_file_path_as_object(&mut self,
        request: &FileHistoryMovePathRequest, captured: Option<&CapturedFileMoveSource>,
        replacement: Option<&FileHistoryMoveReplacement>, principal: &str,
        operation: OperationId) -> Result<OperationResult>
    {
        Self::require_actor(principal)?;
        request.validate()?;
        if let Some(original) = self.resume_file_move_path_if_original(
            request, captured, replacement, principal, operation,
        )? { return Ok(original); }
        if let Some(original) = self.state.operations.get(&operation) {
            if original.kind != OBJECT_MOVE_KIND { return Err(conflict()); }
            let original = fact(original)?;
            if original.request.source_repository_id.as_str() != request.source_repository_id
                || original.request.source_path.as_str() != request.source_path
                || original.request.target_repository_id.as_ref().map(|id| id.as_str())
                    != Some(request.target_repository_id.as_str())
                || original.request.target_path.as_str() != request.target_name
            { return Err(conflict()); }
            let source = CapturedFileMoveSource {
                entry_id: original.request.source_entry_id.clone(),
                file_id: original.source_file_id.ok_or_else(conflict)?,
            };
            return self.move_object_from_file(&original.request,
                captured.unwrap_or(&source), replacement, principal, operation);
        }
        self.require_write()?;
        let path = name_policy::LookupPath::parse(&request.source_path)?;
        let current = self.locate_current_object(&request.source_repository_id, &path)?;
        let entry = current.entry();
        if entry.kind != NamespaceEntryKind::File {
            return Err(Error::new(ErrorCode::FileNotFound, "move source is not a current file"));
        }
        let source = CapturedFileMoveSource {
            entry_id: NamespaceEntryId::parse(&entry.entry_id)?,
            file_id: HistoryFileId::parse(&entry.file_id)?,
        };
        let target = self.state.repositories.get(&request.target_repository_id)
            .ok_or_else(|| Error::new(ErrorCode::RepositoryNotFound, "move target repository is absent"))?;
        let movement = ObjectMoveRequest {
            source_repository_id: HistoryRepositoryId::parse(&request.source_repository_id)?,
            source_entry_id: source.entry_id.clone(), source_path: path,
            target_slot_id: target.primary_slot_id.clone(),
            target_repository_id: Some(HistoryRepositoryId::parse(&request.target_repository_id)?),
            target_path: name_policy::PortablePath::parse(&request.target_name)?,
            replace: replacement.is_some(),
            target_entry_id: replacement.map(|target| NamespaceEntryId::parse(target.entry_id())).transpose()?,
        };
        self.move_object_from_file(&movement, captured.unwrap_or(&source), replacement, principal, operation)
    }

    pub fn move_object(&mut self, request: &ObjectMoveRequest, principal: &str,
        operation: OperationId) -> Result<OperationResult>
    {
        self.move_object_captured(request, None, principal, operation)
    }

    /// A protocol open captures an exact object identity. None means a captured
    /// directory, not permission to ignore a later file at the original path.
    pub fn move_captured_object(&mut self, request: &ObjectMoveRequest,
        source_file: Option<&HistoryFileId>, target_file: Option<&HistoryFileId>,
        principal: &str, operation: OperationId) -> Result<OperationResult>
    {
        let target_file = if same_entry_target(request) {
            if target_file.is_some() && target_file != source_file { return Err(conflict()); }
            None
        } else { target_file };
        self.move_object_captured(request, Some(CapturedFiles {
            source: source_file, target: target_file,
        }), principal, operation)
    }

    /// Existing SMB records retain their complete source/replacement identity.
    /// An original file operation is resumed before any current object capture.
    pub fn move_object_from_file(&mut self, request: &ObjectMoveRequest,
        source: &CapturedFileMoveSource, replacement: Option<&FileHistoryMoveReplacement>,
        principal: &str, operation: OperationId) -> Result<OperationResult>
    {
        Self::require_actor(principal)?;
        request.require_target_intent()?;
        if request.source_entry_id != source.entry_id
            || request.replace != replacement.is_some()
            || request.target_entry_id.as_ref().map(|id| id.as_str()) != replacement.map(|target| target.entry_id())
        { return Err(conflict()); }
        if same_entry_target(request)
            && replacement.is_some_and(|target| target.file_id() != source.file_id.as_str())
        { return Err(conflict()); }
        if let Some(target) = &request.target_repository_id {
            let original = FileHistoryMovePathRequest {
                source_repository_id: request.source_repository_id.as_str().into(),
                source_path: request.source_path.as_str().into(),
                target_repository_id: target.as_str().into(),
                target_name: request.target_path.as_str().into(),
            };
            if let Some(receipt) = self.resume_file_move_if_original(
                &original, source, replacement, principal, operation,
            )? { return Ok(receipt); }
        }
        let target = if same_entry_target(request) { None }
            else { replacement.map(|target| HistoryFileId::parse(target.file_id())).transpose()? };
        self.move_object_captured(request, Some(CapturedFiles {
            source: Some(&source.file_id), target: target.as_ref(),
        }), principal, operation)
    }

    fn move_object_captured(&mut self, request: &ObjectMoveRequest,
        files: Option<CapturedFiles<'_>>, principal: &str, operation: OperationId) -> Result<OperationResult>
    {
        let input = crate::business::object_move::MoveRequest::new(request, principal)?;
        if let Some(original) = self.state.operations.get(&operation) {
            if original.kind != OBJECT_MOVE_KIND || original.request_digest != input.digest()
                || self.state.operation_principals.get(&operation).map(String::as_str) != Some(principal)
            { return Err(conflict()); }
            let original_fact = fact(original)?;
            if original_fact.request != *request || original_fact.principal != principal {
                return Err(conflict());
            }
            require_identity(&original_fact, files.as_ref())?;
            return Ok(original.clone());
        }
        // require_write observes the same retained completion binding that cold
        // recovery discovers. Never recapture a selected but unpublished move.
        self.require_write()?;
        self.require_unfenced_membership()?;
        self.require_native_move_authority()?;
        let plan = crate::business::object_move::plan(self, &input, operation)?;
        require_identity(plan.movement(), files.as_ref())?;
        self.publish_move(plan)
    }
}
