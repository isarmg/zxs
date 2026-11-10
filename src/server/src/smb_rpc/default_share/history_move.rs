//! Captured object moves and original file receipts share namespace admission.
use super::*;
use crate::smb_rpc::{namespace::NamespaceAdmission, path_reservation::MovePaths};

mod empty_target;
mod object;
mod resume;

#[derive(Clone, data_serialization::Serialize, data_serialization::Deserialize)]
#[serialization(deny_unknown_fields)]
struct ReplacementIntent {
    entry_id: String,
    file_id: String,
}

/// The protocol reply binds to the same captured command as the durable intent.
/// This proves a result; it never resolves a new path or submits another move.
struct MoveReceipt {
    operation_id: OperationId,
    movement: storage_contract::file_relocation::ObjectMoveRequest,
    source_file_id: Option<storage_contract::history_window::HistoryFileId>,
    displaced_target_file_id: Option<storage_contract::history_window::HistoryFileId>,
}

impl MoveReceipt {
    fn current(
        request: &crate::MoveCurrentFile,
        target_slot_id: &str,
        replacement: Option<&ReplacementIntent>,
    ) -> Result<Self, ApiError> {
        Ok(Self {
            operation_id: request.operation_id,
            movement: request.movement(target_slot_id, replacement.map(|target| target.entry_id.as_str()))?,
            source_file_id: Some(request.expected_source_file_id.clone()),
            displaced_target_file_id: replacement.map(|target| {
                storage_contract::history_window::HistoryFileId::parse(&target.file_id)
                    .map_err(ApiError::contract_input)
            }).transpose()?,
        })
    }

    fn empty(request: &crate::MoveCurrentFileToEmptyDisk) -> Result<Self, ApiError> {
        Ok(Self {
            operation_id: request.operation_id,
            movement: request.movement()?,
            source_file_id: Some(request.expected_source_file_id.clone()),
            displaced_target_file_id: None,
        })
    }

    fn published_reply(
        self,
        result: storage_contract::OperationResult,
        principal: &str,
    ) -> Result<Value, ApiError> {
        if result.kind != storage_contract::file_relocation::OBJECT_MOVE_KIND {
            return published_reply(result, self.operation_id);
        }
        self.published_object_reply(result, principal)
    }

    fn object(request: &crate::MoveCurrentObject) -> Result<Self, ApiError> {
        let same_entry = request.movement.target_repository_id.as_ref()
            == Some(&request.movement.source_repository_id)
            && request.movement.target_entry_id.as_ref() == Some(&request.movement.source_entry_id)
            && request.movement.target_path.key() == request.movement.source_path.key();
        if same_entry && request.expected_target_file_id.as_ref().is_some_and(|target| {
            request.expected_source_file_id.as_ref() != Some(target)
        }) {
            return Err(receipt_changed());
        }
        Ok(Self {
            operation_id: request.operation_id,
            movement: request.movement.clone(),
            source_file_id: request.expected_source_file_id.clone(),
            displaced_target_file_id: if same_entry { None } else { request.expected_target_file_id.clone() },
        })
    }

    fn published_object_reply(
        self,
        result: storage_contract::OperationResult,
        principal: &str,
    ) -> Result<Value, ApiError> {
        if result.operation_id != self.operation_id
            || result.kind != storage_contract::file_relocation::OBJECT_MOVE_KIND
        {
            return Err(receipt_changed());
        }
        let versions = result.result.get("version_mapping")
            .and_then(Value::as_object).ok_or_else(receipt_changed)?;
        let protection = if versions.is_empty() {
            storage_contract::ProtectionAtPublish::NotApplicable
        } else {
            storage_contract::ProtectionAtPublish::TargetMet
        };
        let reply = complete_publication_reply(result, protection)?;
        let fact = &reply["result"]["movement"];
        if !self.movement.matches_receipt(principal, &reply)
            || fact["source_file_id"] != json!(self.source_file_id)
            || fact["displaced_target_file_id"] != json!(self.displaced_target_file_id)
        {
            return Err(receipt_changed());
        }
        Ok(reply)
    }
}

impl DefaultShare {
    fn submit_move(
        &self,
        request: crate::MoveCurrentFile,
        replacement: Option<ReplacementIntent>,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        match replacement {
            Some(target) => self.shared.backend.replace_current_file(
                &self.identity,
                crate::ReplaceCurrentFile {
                    request,
                    expected_target_entry_id: target.entry_id,
                    expected_target_file_id:
                        storage_contract::history_window::HistoryFileId::parse(&target.file_id)
                            .map_err(ApiError::contract_input)?,
                },
            ),
            None => self
                .shared
                .backend
                .move_current_file(&self.identity, request),
        }
    }
}

fn published_reply(
    result: storage_contract::OperationResult,
    operation_id: OperationId,
) -> Result<Value, ApiError> {
    if result.operation_id != operation_id || result.kind != "file.history.move" {
        return Err(receipt_changed());
    }
    complete_publication_reply(result, storage_contract::ProtectionAtPublish::TargetMet)
}

fn receipt_changed() -> ApiError {
    ApiError::new(
        http::StatusCode::INTERNAL_SERVER_ERROR,
        storage_contract::ErrorCode::StateInvalid,
        "native move returned a different operation receipt",
    )
}

fn complete_publication_reply(
    result: storage_contract::OperationResult,
    protection: storage_contract::ProtectionAtPublish,
) -> Result<Value, ApiError> {
    if result.commit_state != storage_contract::CommitState::Published
        || result.protection_at_publish != protection
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
