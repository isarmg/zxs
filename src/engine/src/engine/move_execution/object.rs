//! The one path-facing operation for current files, folders and repositories.
//! Transport adapters supply intent and captured identity, never a move plan.
use super::*;
use storage_contract::file_relocation::{ObjectMoveFact, ObjectMoveRequest, OBJECT_MOVE_KIND};
use crate::{CapturedFileMoveSource, FileHistoryMovePathRequest, FileHistoryMoveReplacement};
use storage_contract::history_window::HistoryFileId;

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

fn require_identity(original: &ObjectMoveFact, files: Option<&CapturedFiles<'_>>) -> Result<()> {
    if let Some(files) = files {
        if original.source_file_id.as_ref() != files.source
            || original.displaced_target_file_id.as_ref() != files.target
        { return Err(conflict()); }
    }
    Ok(())
}

impl Engine {
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
        let target = replacement.map(|target| HistoryFileId::parse(target.file_id())).transpose()?;
        self.move_object_captured(request, Some(CapturedFiles {
            source: Some(&source.file_id), target: target.as_ref(),
        }), principal, operation)
    }

    fn move_object_captured(&mut self, request: &ObjectMoveRequest,
        files: Option<CapturedFiles<'_>>, principal: &str, operation: OperationId) -> Result<OperationResult>
    {
        Self::require_actor(principal)?;
        request.require_target_intent()?;
        let digest = request.digest(principal)?;
        if let Some(original) = self.state.operations.get(&operation) {
            if original.kind != OBJECT_MOVE_KIND || original.request_digest != digest
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
        let plan = crate::business::object_move::plan(self, request, principal, operation)?;
        let (next, prepared, result, retained) = plan.into_parts();
        require_identity(&fact(&result)?, files.as_ref())?;
        self.publish_move(next, prepared, result, retained)
    }
}
