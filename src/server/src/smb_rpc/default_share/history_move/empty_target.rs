//! Retained original file-move retry across destination-root initialization.
//! New moves use the shared object command and its same-operation empty root.
use super::*;
use data_serialization::{Deserialize, Serialize};

pub(super) const FORMAT: &str = "smb-file-history-move-empty-target";
pub(super) const REPLACE_FORMAT: &str = "smb-file-history-move-empty-target-replace";

fn intent_format(replace: bool) -> &'static str {
    if replace { REPLACE_FORMAT } else { FORMAT }
}

#[derive(Deserialize, Serialize)]
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

#[derive(Deserialize, Serialize)]
#[serialization(deny_unknown_fields)]
struct Intent {
    format: String,
    principal: String,
    operation_id: OperationId,
    publication_outcome: String,
    source_binding: Binding,
    source_path: String,
    source_entry_id: String,
    source_file_id: String,
    target_slot_id: String,
    target_disk_id: String,
    #[serialization(with = "storage_contract::u64_string")]
    target_binding_generation: u64,
    target_name: String,
    #[serialization(with = "storage_contract::u64_string")]
    account_generation: u64,
}

fn changed() -> ApiError {
    ApiError::conflict(
        storage_contract::ErrorCode::IdempotencyKeyReused,
        "empty-target move retry differs from its original actor, disk, source or paths",
    )
}

impl DefaultShare {
    pub(in crate::smb_rpc::default_share::history_move) fn resume_empty_disk_move(
        &mut self,
        source: &str,
        target: &str,
        replace: bool,
        admission: NamespaceAdmission,
        bytes: &[u8],
    ) -> Result<Value, ApiError> {
        let intent: Intent =
            storage_contract::decode_strict(bytes).map_err(ApiError::contract_input)?;
        if intent.format != intent_format(replace) {
            return Err(changed());
        }
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
        let source_repository = self
            .shared
            .backend
            .repository_snapshot(&self.identity, &intent.source_binding.repository_id)?;
        if source_slot.slot_id != source_repository.primary_slot_id
            || source_path.key() != name_policy::path_key(&intent.source_path)
            || target_slot.slot_id != intent.target_slot_id
            || target_slot.disk_id != intent.target_disk_id
            || target_slot.binding_generation != intent.target_binding_generation
            || target_path.as_str() != intent.target_name
        {
            return Err(changed());
        }
        self.submit_empty_disk_move(intent, admission, bytes)
    }

    fn submit_empty_disk_move(
        &mut self,
        intent: Intent,
        admission: NamespaceAdmission,
        bytes: &[u8],
    ) -> Result<Value, ApiError> {
        if !matches!(intent.format.as_str(), FORMAT | REPLACE_FORMAT)
            || intent.publication_outcome != "unconfirmed"
            || intent.operation_id != admission.operation_id()
            || intent.principal != self.identity.principal
            || intent.account_generation != self.account_generation
        {
            return Err(changed());
        }
        let repository = self
            .shared
            .backend
            .repository_snapshot(&self.identity, &intent.source_binding.repository_id)?;
        if !repository.file_manager_root || repository.primary_slot_id == intent.target_slot_id {
            return Err(changed());
        }
        let source = self.pin_binding(repository)?;
        if !intent.source_binding.matches(&source) {
            return Err(changed());
        }
        let source_identity = self.bound_identity(&source);
        self.shared
            .backend
            .authorize_repository(&source_identity, &source.repository)?;
        let target = self
            .shared
            .backend
            .topology_snapshot(&self.identity)?
            .slots
            .into_iter()
            .find(|disk| disk.slot_id == intent.target_slot_id)
            .ok_or_else(changed)?;
        if target.disk_id != intent.target_disk_id
            || target.binding_generation != intent.target_binding_generation
            || target.level != 1
        {
            return Err(changed());
        }
        let source_path = name_policy::LookupPath::parse(&intent.source_path)
            .map_err(ApiError::invalid_source)?;
        let target_name = crate::MoveTargetName::parse(&intent.target_name)?;
        storage_contract::validate_id(&intent.source_entry_id, "entry")
            .map_err(ApiError::contract_input)?;
        let file = storage_contract::history_window::HistoryFileId::parse(&intent.source_file_id)
            .map_err(ApiError::contract_input)?;
        // There is no target branch to impersonate. Reserve the original disk
        // address while the backend owner lock creates/admits the real binding;
        // native move separately rejects any live target write workspace.
        let target_key = format!(
            "@empty-disk:{}:{}",
            intent.target_slot_id, intent.target_binding_generation,
        );
        let _paths = MovePaths::acquire(
            &self.shared,
            [
                (&source.branch, source_path.as_str()),
                (&target_key, target_name.as_str()),
            ],
        )?;
        if admission.retained_move_bytes()?.as_slice() != bytes {
            return Err(changed());
        }
        let operation = intent.operation_id;
        // Allowing replacement does not require an existing destination. This
        // intent captured absence, so even a replace request must use the
        // non-replacing native move and refuse a subsequently occupied name.
        let request = crate::MoveCurrentFileToEmptyDisk {
            source_binding: source_identity
                .smb_binding
                .expect("invariant: pinned source binding"),
            target_slot_id: intent.target_slot_id,
            target_disk_id: intent.target_disk_id,
            target_binding_generation: intent.target_binding_generation,
            source_path,
            target_name,
            expected_source_entry_id: intent.source_entry_id,
            expected_source_file_id: file,
            operation_id: operation,
        };
        let receipt = MoveReceipt::empty(&request)?;
        let result = self
            .shared
            .backend
            .move_current_file_to_empty_disk(&self.identity, request)?;
        let reply = receipt.published_reply(result, &self.identity.principal)?;
        admission.finish_published();
        Ok(reply)
    }
}
