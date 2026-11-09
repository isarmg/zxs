//! Explicit original-operation retry. A retained spool is input, never authority:
//! current account and both binding checks still surround the same core call.
use super::*;
use data_serialization::Deserialize;

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct Request {
    source_repository_id: String,
    source_path: String,
    target_repository_id: String,
    target_name: String,
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct Binding {
    repository_id: String,
    branch_id: String,
    #[serialization(with = "storage_contract::u64_string")]
    generation: u64,
}
impl Binding {
    fn matches(&self, key: &SessionKey) -> bool {
        self.repository_id == key.repository
            && self.branch_id == key.branch
            && self.generation == key.generation
    }
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct Intent {
    format: String,
    principal: String,
    operation_id: OperationId,
    publication_outcome: String,
    move_request: Request,
    source_binding: Binding,
    target_binding: Binding,
    source_entry_id: String,
    source_file_id: String,
    #[serialization(with = "storage_contract::u64_string")]
    account_generation: u64,
}
fn changed() -> ApiError {
    ApiError::conflict(
        storage_contract::ErrorCode::IdempotencyKeyReused,
        "native move retry differs from its original actor, bindings or paths",
    )
}

impl DefaultShare {
    pub(in crate::smb_rpc::default_share) fn resume_move(
        &mut self,
        source: &str,
        target: &str,
        operation: OperationId,
    ) -> Result<Option<Value>, ApiError> {
        let Some(admission) =
            NamespaceAdmission::retain_move(&self.shared, &self.identity, operation)?
        else {
            return Ok(None);
        };
        let bytes = admission.retained_move_bytes()?;
        let format: Value =
            storage_contract::decode_strict(&bytes).map_err(ApiError::contract_input)?;
        if format.get("format").and_then(Value::as_str)
            == Some("smb-file-history-move-empty-target")
        {
            return self
                .resume_empty_disk_move(source, target, admission, &bytes)
                .map(Some);
        }
        let original: Intent =
            storage_contract::decode_strict(&bytes).map_err(ApiError::contract_input)?;
        let request = &original.move_request;
        if original.format != "smb-file-history-move"
            || original.publication_outcome != "unconfirmed"
            || original.operation_id != operation
            || original.principal != self.identity.principal
            || original.account_generation != self.account_generation
            || original.source_binding.repository_id != request.source_repository_id
            || original.target_binding.repository_id != request.target_repository_id
        {
            return Err(changed());
        }
        let source_repository = self
            .shared
            .backend
            .repository_snapshot(&self.identity, &request.source_repository_id)?;
        let target_repository = self
            .shared
            .backend
            .repository_snapshot(&self.identity, &request.target_repository_id)?;
        let Location::Path {
            slot: source_slot,
            path: source_path,
        } = self.locate(source)?
        else {
            return Err(changed());
        };
        let Location::Path {
            slot: target_slot,
            path: target_path,
        } = self.locate(target)?
        else {
            return Err(changed());
        };
        let original_source = name_policy::LookupPath::parse(&request.source_path)
            .map_err(ApiError::invalid_source)?;
        let original_target = crate::MoveTargetName::parse(&request.target_name)?;
        if !source_repository.file_manager_root
            || !target_repository.file_manager_root
            || source_repository.primary_slot_id == target_repository.primary_slot_id
            || source_slot.slot_id != source_repository.primary_slot_id
            || target_slot.slot_id != target_repository.primary_slot_id
            || source_path.key() != original_source.key()
            || target_path.as_str() != original_target.as_str()
        {
            return Err(changed());
        }
        let source = self.pin_binding(source_repository)?;
        let target = self.pin_binding(target_repository)?;
        if !original.source_binding.matches(&source) || !original.target_binding.matches(&target) {
            return Err(changed());
        }
        let source_identity = self.bound_identity(&source);
        let target_identity = self.bound_identity(&target);
        self.shared
            .backend
            .authorize_repository(&source_identity, &source.repository)?;
        self.shared
            .backend
            .authorize_repository(&target_identity, &target.repository)?;
        let _paths = MovePaths::acquire(
            &self.shared,
            [
                (&source.branch, original_source.as_str()),
                (&target.branch, original_target.as_str()),
            ],
        )?;
        storage_contract::validate_id(&original.source_entry_id, "entry")
            .map_err(ApiError::contract_input)?;
        let file = storage_contract::history_window::HistoryFileId::parse(&original.source_file_id)
            .map_err(ApiError::contract_input)?;
        if admission.retained_move_bytes()? != bytes {
            return Err(changed());
        }
        let result = self.shared.backend.move_current_file(
            &self.identity,
            crate::MoveCurrentFile {
                source_binding: source_identity
                    .smb_binding
                    .expect("invariant: pinned source binding"),
                target_binding: target_identity
                    .smb_binding
                    .expect("invariant: pinned target binding"),
                source_path: original_source,
                target_name: original_target,
                expected_source_entry_id: original.source_entry_id,
                expected_source_file_id: file,
                operation_id: operation,
            },
        )?;
        let reply = published_reply(result, operation)?;
        admission.finish_published();
        Ok(Some(reply))
    }
}
