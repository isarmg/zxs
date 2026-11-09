//! Cross-disk rename is one native history move, not a namespace copy/delete pair.
use super::*;
use crate::smb_rpc::{namespace::NamespaceAdmission, path_reservation::MovePaths};

mod resume;
mod empty_target;

impl DefaultShare {
    pub(super) fn move_file(
        &mut self,
        source: SmbPathLocation,
        target: SmbPathLocation,
        admission: NamespaceAdmission,
    ) -> Result<Value, ApiError> {
        if !source.repository.file_manager_root
            || !target.repository.file_manager_root
            || source.repository.primary_slot_id == target.repository.primary_slot_id
            || source.relative_path.is_empty()
            || target.relative_path.is_empty()
        {
            return Err(ApiError::unsupported());
        }
        let source_path = name_policy::LookupPath::parse(&source.relative_path)
            .map_err(ApiError::invalid_source)?;
        let target_name = crate::MoveTargetName::parse(&target.relative_path)?;
        let source = self.pin_binding(source.repository)?;
        let target = self.pin_binding(target.repository)?;
        let source_identity = self.bound_identity(&source);
        let target_identity = self.bound_identity(&target);
        self.shared
            .backend
            .authorize_repository(&source_identity, &source.repository)?;
        self.shared
            .backend
            .authorize_repository(&target_identity, &target.repository)?;
        // Both locks span the capture, durable intent, backend submission and
        // result classification. Core choice/authority fences outlive this RPC.
        let _paths = MovePaths::acquire(
            &self.shared,
            [
                (&source.branch, source_path.as_str()),
                (&target.branch, target_name.as_str()),
            ],
        )?;
        let entry = self
            .shared
            .backend
            .lookup_current(&source_identity, &source.branch, &source_path)?
            .ok_or_else(|| {
                ApiError::new(
                    http::StatusCode::NOT_FOUND,
                    storage_contract::ErrorCode::FileNotFound,
                    "move source is no longer visible",
                )
            })?;
        if entry.kind != storage_contract::NamespaceEntryKind::File {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::IsDirectory,
                "cross-disk move requires a single file",
            ));
        }
        if self
            .shared
            .backend
            .lookup_current(&target_identity, &target.branch, &target_name.lookup())?
            .is_some()
        {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::NameCollision,
                "cross-disk move never replaces a target entry",
            ));
        }
        let file_id = storage_contract::history_window::HistoryFileId::parse(&entry.file_id)
            .map_err(ApiError::contract_input)?;
        let operation_id = admission.operation_id();
        // Preserve the exact routed request before the first core call. The
        // record is not a decision, a replay command or a grant to recapture a
        // changed source. Any uncertain result retains this original identity.
        admission.record_intent(&json!({
            "format":"smb-file-history-move",
            "principal":self.identity.principal,
            "operation_id":operation_id,
            "publication_outcome":"unconfirmed",
            "move_request":{
                "source_repository_id":source.repository,
                "source_path":source_path.as_str(),
                "target_repository_id":target.repository,
                "target_name":target_name.as_str()
            },
            "source_binding":{
                "repository_id":source.repository,
                "branch_id":source.branch,
                "generation":source.generation.to_string()
            },
            "target_binding":{
                "repository_id":target.repository,
                "branch_id":target.branch,
                "generation":target.generation.to_string()
            },
            "source_entry_id":entry.entry_id,
            "source_file_id":file_id.as_str(),
            "account_generation":self.account_generation.to_string()
        }))?;
        let result = self.shared.backend.move_current_file(
            &self.identity,
            crate::MoveCurrentFile {
                source_binding: source_identity
                    .smb_binding
                    .expect("invariant: routed source identity has a pinned binding"),
                target_binding: target_identity
                    .smb_binding
                    .expect("invariant: routed target identity has a pinned binding"),
                source_path,
                target_name,
                expected_source_entry_id: entry.entry_id,
                expected_source_file_id: file_id,
                operation_id,
            },
        )?;
        let reply = published_reply(result, operation_id)?;
        admission.finish_published();
        Ok(reply)
    }
}

fn published_reply(
    result: storage_contract::OperationResult,
    operation_id: OperationId,
) -> Result<Value, ApiError> {
    if result.operation_id != operation_id || result.kind != "file.history.move" {
        return Err(ApiError::new(
            http::StatusCode::INTERNAL_SERVER_ERROR,
            storage_contract::ErrorCode::StateInvalid,
            "native move returned a different operation receipt",
        ));
    }
    if result.commit_state != storage_contract::CommitState::Published
        || result.protection_at_publish != storage_contract::ProtectionAtPublish::TargetMet
        || result.current_health != storage_contract::CurrentHealth::Healthy
    {
        return Err(ApiError::conflict(
            storage_contract::ErrorCode::RecoveryRequired,
            "native move has no complete publication result; query the original operation",
        ));
    }
    crate::operation_wire::operation_wire(result.into())
}

#[cfg(test)]
#[path = "../../../../../test/server/unit/smb_rpc/history_move_reply_tests.rs"]
mod tests;
