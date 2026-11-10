//! Current files and directories share one captured request and namespace spool.
use super::*;
use super::{namespace::NamespaceAdmission, path_reservation::PathReservation};
use crate::{TopologySlot, auth::SmbBinding};
use data_serialization::{Deserialize, Deserializer, Serialize};
use storage_contract::file_relocation::{NamespaceEntryId, ObjectMoveRequest};
use storage_contract::history_window::{HistoryFileId, HistoryRepositoryId};

const FORMAT: &str = "smb-object-move";

#[derive(Clone, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Binding {
    repository_id: String,
    branch_id: String,
    #[serialization(with = "storage_contract::u64_string")]
    generation: u64,
}
impl Binding {
    fn capture(key: &SmbBinding) -> Self {
        Self {
            repository_id: key.repository_id.clone(),
            branch_id: key.branch_id.clone(),
            generation: key.generation,
        }
    }
    pub(super) fn matches(&self, key: &SmbBinding) -> bool {
        self.repository_id == key.repository_id
            && self.branch_id == key.branch_id
            && self.generation == key.generation
    }
    fn identity(&self) -> crate::auth::SmbBinding {
        crate::auth::SmbBinding {
            repository_id: self.repository_id.clone(),
            branch_id: self.branch_id.clone(),
            generation: self.generation,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct DiskIdentity {
    disk_id: String,
    #[serialization(with = "storage_contract::u64_string")]
    binding_generation: u64,
}
impl DiskIdentity {
    fn capture(slot: &TopologySlot) -> Self {
        Self {
            disk_id: slot.disk_id.clone(),
            binding_generation: slot.binding_generation,
        }
    }
    pub(super) fn matches(&self, slot: &TopologySlot) -> bool {
        self.disk_id == slot.disk_id
            && self.binding_generation == slot.binding_generation
            && slot.level == 1
    }
}

fn required<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    decoder: D,
) -> Result<Option<T>, D::Error> {
    Option::deserialize(decoder)
}

#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Intent {
    format: String,
    principal: String,
    operation_id: OperationId,
    publication_outcome: String,
    #[serialization(with = "storage_contract::u64_string")]
    account_generation: u64,
    pub(super) replace_requested: bool,
    pub(super) source_binding: Binding,
    #[serialization(deserialize_with = "required")]
    pub(super) target_binding: Option<Binding>,
    pub(super) source_slot_id: String,
    pub(super) source_disk: DiskIdentity,
    pub(super) target_disk: DiskIdentity,
    pub(super) movement: ObjectMoveRequest,
    #[serialization(deserialize_with = "required")]
    source_file_id: Option<HistoryFileId>,
    #[serialization(deserialize_with = "required")]
    target_file_id: Option<HistoryFileId>,
}
impl Intent {
    fn command(&self) -> crate::MoveCurrentObject {
        crate::MoveCurrentObject {
            source_binding: self.source_binding.identity(),
            source_slot_id: self.source_slot_id.clone(),
            source_disk_id: self.source_disk.disk_id.clone(),
            source_binding_generation: self.source_disk.binding_generation,
            target: match &self.target_binding {
                Some(binding) => crate::MoveObjectTarget::Repository {
                    binding: binding.identity(),
                    disk_id: self.target_disk.disk_id.clone(),
                    binding_generation: self.target_disk.binding_generation,
                },
                None => crate::MoveObjectTarget::EmptyDisk {
                    disk_id: self.target_disk.disk_id.clone(),
                    binding_generation: self.target_disk.binding_generation,
                },
            },
            movement: self.movement.clone(),
            expected_source_file_id: self.source_file_id.clone(),
            expected_target_file_id: self.target_file_id.clone(),
            operation_id: self.operation_id,
        }
    }
}

pub(super) fn changed() -> ApiError {
    ApiError::conflict(
        storage_contract::ErrorCode::IdempotencyKeyReused,
        "object move differs from its original actor, bindings, disks or paths",
    )
}
fn file_identity(entry: &Entry) -> Result<Option<HistoryFileId>, ApiError> {
    match entry.kind {
        storage_contract::NamespaceEntryKind::File => HistoryFileId::parse(&entry.file_id)
            .map(Some)
            .map_err(ApiError::contract_input),
        storage_contract::NamespaceEntryKind::Directory => Ok(None),
    }
}

/// One request actor and original account generation, borrowed from its session.
/// Routes select endpoints; the engine owns live authorization and publication.
pub(super) struct MoveContext<'a> {
    pub(super) shared: &'a Arc<Shared>,
    pub(super) identity: &'a Identity,
    pub(super) account_generation: u64,
}

pub(super) struct MoveSource {
    pub(super) binding: SmbBinding,
    pub(super) slot: TopologySlot,
    pub(super) path: name_policy::LookupPath,
}

pub(super) struct MoveTarget {
    pub(super) binding: Option<SmbBinding>,
    pub(super) slot: TopologySlot,
    pub(super) path: name_policy::PortablePath,
}

pub(super) struct RetainedMove {
    pub(super) intent: Intent,
    admission: NamespaceAdmission,
    bytes: Vec<u8>,
}

impl RetainedMove {
    pub(super) fn read(
        context: &MoveContext<'_>,
        operation: OperationId,
    ) -> Result<Option<Self>, ApiError> {
        let Some(admission) =
            NamespaceAdmission::retain_move(context.shared, context.identity, operation)?
        else {
            return Ok(None);
        };
        let bytes = admission.retained_move_bytes()?;
        let intent = storage_contract::decode_strict(&bytes).map_err(ApiError::contract_input)?;
        Ok(Some(Self {
            intent,
            admission,
            bytes,
        }))
    }

    pub(super) fn submit(self, context: &MoveContext<'_>) -> Result<Value, ApiError> {
        let paths = self.intent.reserve(context.shared)?;
        context.submit(self.intent, self.admission, &self.bytes, paths)
    }
}

impl Intent {
    fn reserve(&self, shared: &Arc<Shared>) -> Result<PathReservation, ApiError> {
        let target_scope = self.target_binding.as_ref().map_or_else(
            || {
                format!(
                    "@empty-disk:{}:{}",
                    self.movement.target_slot_id, self.target_disk.binding_generation
                )
            },
            |binding| binding.branch_id.clone(),
        );
        PathReservation::acquire(
            shared,
            [
                (
                    self.source_binding.branch_id.as_str(),
                    self.movement.source_path.key(),
                ),
                (target_scope.as_str(), self.movement.target_path.key()),
            ],
        )
    }
}

impl MoveContext<'_> {
    fn bound_identity(&self, binding: &SmbBinding) -> Identity {
        let mut identity = self.identity.clone();
        identity.smb_binding = Some(binding.clone());
        identity
    }

    pub(super) fn capture(
        &self,
        source: MoveSource,
        target: MoveTarget,
        replace: bool,
        admission: NamespaceAdmission,
    ) -> Result<Value, ApiError> {
        let source_identity = self.bound_identity(&source.binding);
        self.shared
            .backend
            .authorize_repository(&source_identity, &source.binding.repository_id)?;
        let target_identity = target
            .binding
            .as_ref()
            .map(|binding| self.bound_identity(binding));
        if let (Some(binding), Some(identity)) = (&target.binding, &target_identity) {
            self.shared
                .backend
                .authorize_repository(identity, &binding.repository_id)?;
        }
        let target_scope = target.binding.as_ref().map_or_else(
            || {
                format!(
                    "@empty-disk:{}:{}",
                    target.slot.slot_id, target.slot.binding_generation
                )
            },
            |binding| binding.branch_id.clone(),
        );
        let paths = PathReservation::acquire(
            self.shared,
            [
                (source.binding.branch_id.as_str(), source.path.key()),
                (target_scope.as_str(), target.path.key()),
            ],
        )?;
        let entry = self
            .shared
            .backend
            .lookup_current(&source_identity, &source.binding.branch_id, &source.path)?
            .ok_or_else(|| {
                ApiError::new(
                    http::StatusCode::NOT_FOUND,
                    storage_contract::ErrorCode::NotFound,
                    "move source is no longer visible",
                )
            })?;
        let displaced = match (&target.binding, &target_identity) {
            (Some(binding), Some(identity)) => self
                .shared
                .backend
                .lookup_current(
                    identity,
                    &binding.branch_id,
                    &name_policy::LookupPath::from(&target.path),
                )?
                .filter(|current| {
                    binding.repository_id != source.binding.repository_id
                        || current.entry_id != entry.entry_id
                        || current.lookup_key() != entry.lookup_key()
                }),
            _ => None,
        };
        if displaced.is_some() && !replace {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::NameCollision,
                "move destination exists without replacement intent",
            ));
        }
        let intent = Intent {
            format: FORMAT.into(),
            principal: self.identity.principal.clone(),
            operation_id: admission.operation_id(),
            publication_outcome: "unconfirmed".into(),
            account_generation: self.account_generation,
            replace_requested: replace,
            source_binding: Binding::capture(&source.binding),
            target_binding: target.binding.as_ref().map(Binding::capture),
            source_disk: DiskIdentity::capture(&source.slot),
            source_slot_id: source.slot.slot_id,
            target_disk: DiskIdentity::capture(&target.slot),
            source_file_id: file_identity(&entry)?,
            target_file_id: displaced.as_ref().map(file_identity).transpose()?.flatten(),
            movement: ObjectMoveRequest {
                source_repository_id: HistoryRepositoryId::parse(&source.binding.repository_id)
                    .map_err(ApiError::contract_input)?,
                source_entry_id: NamespaceEntryId::parse(&entry.entry_id)
                    .map_err(ApiError::contract_input)?,
                source_path: source.path,
                target_slot_id: target.slot.slot_id,
                target_repository_id: target
                    .binding
                    .as_ref()
                    .map(|binding| HistoryRepositoryId::parse(&binding.repository_id))
                    .transpose()
                    .map_err(ApiError::contract_input)?,
                target_path: target.path,
                replace: displaced.is_some(),
                target_entry_id: displaced
                    .as_ref()
                    .map(|entry| NamespaceEntryId::parse(&entry.entry_id))
                    .transpose()
                    .map_err(ApiError::contract_input)?,
            },
        };
        let bytes = admission.record_intent(&intent)?;
        self.submit(intent, admission, &bytes, paths)
    }

    fn submit(
        &self,
        intent: Intent,
        admission: NamespaceAdmission,
        bytes: &[u8],
        _paths: PathReservation,
    ) -> Result<Value, ApiError> {
        if intent.format != FORMAT
            || intent.operation_id != admission.operation_id()
            || intent.principal != self.identity.principal
            || intent.account_generation != self.account_generation
            || intent.publication_outcome != "unconfirmed"
            || intent.movement.source_repository_id.as_str() != intent.source_binding.repository_id
            || intent
                .movement
                .target_repository_id
                .as_ref()
                .map(|id| id.as_str())
                != intent
                    .target_binding
                    .as_ref()
                    .map(|binding| binding.repository_id.as_str())
            || (!intent.replace_requested && intent.movement.replace)
            || (intent.movement.target_entry_id.is_none() && intent.target_file_id.is_some())
        {
            return Err(changed());
        }
        intent
            .movement
            .require_target_intent()
            .map_err(ApiError::contract_input)?;
        if admission.retained_move_bytes()?.as_slice() != bytes {
            return Err(changed());
        }
        let command = intent.command();
        let receipt = MoveReceipt::object(&command)?;
        let result = self
            .shared
            .backend
            .move_current_object(self.identity, command)?;
        let reply = receipt.published_object_reply(result, &self.identity.principal)?;
        admission.finish_published();
        Ok(reply)
    }
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
    fn object(request: &crate::MoveCurrentObject) -> Result<Self, ApiError> {
        let same_entry = request.movement.target_repository_id.as_ref()
            == Some(&request.movement.source_repository_id)
            && request.movement.target_entry_id.as_ref() == Some(&request.movement.source_entry_id)
            && request.movement.target_path.key() == request.movement.source_path.key();
        if same_entry
            && request
                .expected_target_file_id
                .as_ref()
                .is_some_and(|target| request.expected_source_file_id.as_ref() != Some(target))
        {
            return Err(receipt_changed());
        }
        Ok(Self {
            operation_id: request.operation_id,
            movement: request.movement.clone(),
            source_file_id: request.expected_source_file_id.clone(),
            displaced_target_file_id: if same_entry {
                None
            } else {
                request.expected_target_file_id.clone()
            },
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
        let versions = result
            .result
            .get("version_mapping")
            .and_then(Value::as_object)
            .ok_or_else(receipt_changed)?;
        let protection = if versions.is_empty() {
            storage_contract::ProtectionAtPublish::NotApplicable
        } else {
            storage_contract::ProtectionAtPublish::TargetMet
        };
        if result.commit_state != storage_contract::CommitState::Published
            || result.protection_at_publish != protection
            || result.current_health != storage_contract::CurrentHealth::Healthy
        {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::RecoveryRequired,
                "native move has no complete publication result; query the original operation",
            ));
        }
        let reply = crate::operation_wire::operation_wire(result.into())?;
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

fn receipt_changed() -> ApiError {
    ApiError::new(
        http::StatusCode::INTERNAL_SERVER_ERROR,
        storage_contract::ErrorCode::StateInvalid,
        "native move returned a different operation receipt",
    )
}

#[cfg(test)]
#[path = "../../../../test/server/unit/smb_rpc/history_move_reply_tests.rs"]
mod tests;
