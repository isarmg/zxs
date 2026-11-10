//! Current files and directories share one captured request and namespace spool.
use super::*;
use data_serialization::{Deserialize, Deserializer, Serialize};
use storage_contract::file_relocation::{NamespaceEntryId, ObjectMoveRequest};
use storage_contract::history_window::{HistoryFileId, HistoryRepositoryId};

const FORMAT: &str = "smb-object-move";

#[derive(Clone, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Binding {
    repository_id: String,
    branch_id: String,
    #[serialization(with = "storage_contract::u64_string")]
    generation: u64,
}
impl Binding {
    fn capture(key: &SessionKey) -> Self {
        Self {
            repository_id: key.repository.clone(),
            branch_id: key.branch.clone(),
            generation: key.generation,
        }
    }
    fn matches(&self, key: &SessionKey) -> bool {
        self.repository_id == key.repository
            && self.branch_id == key.branch
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
struct DiskIdentity {
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
    fn matches(&self, slot: &TopologySlot) -> bool {
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
struct Intent {
    format: String,
    principal: String,
    operation_id: OperationId,
    publication_outcome: String,
    #[serialization(with = "storage_contract::u64_string")]
    account_generation: u64,
    replace_requested: bool,
    source_binding: Binding,
    #[serialization(deserialize_with = "required")]
    target_binding: Option<Binding>,
    source_slot_id: String,
    source_disk: DiskIdentity,
    target_disk: DiskIdentity,
    movement: ObjectMoveRequest,
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

fn changed() -> ApiError {
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

impl DefaultShare {
    pub(in crate::smb_rpc::default_share) fn move_object(
        &mut self,
        source: SmbPathLocation,
        destination: &str,
        admission: NamespaceAdmission,
        replace: bool,
    ) -> Result<Value, ApiError> {
        let SmbRelativePath::Entry(source_path) = source.relative_path else {
            return Err(ApiError::unsupported());
        };
        let Location::Path {
            slot: target_slot,
            path: destination_path,
        } = self.locate(destination)?
        else {
            return Err(ApiError::unsupported());
        };
        let source_slot_id = source.repository.primary_slot_id.clone();
        let source_slot = self
            .shared
            .backend
            .topology_snapshot(&self.identity)?
            .slots
            .into_iter()
            .find(|slot| slot.slot_id == source_slot_id)
            .ok_or_else(changed)?;
        let source = self.pin_binding(source.repository)?;
        let source_identity = self.bound_identity(&source);
        self.shared
            .backend
            .authorize_repository(&source_identity, &source.repository)?;
        let (target, target_path) = match self.resolve_at(
            &target_slot,
            destination_path.clone(),
            SmbPathIntent::Lookup,
        ) {
            Ok(location) => {
                let SmbRelativePath::Entry(path) = location.relative_path else {
                    return Err(ApiError::unsupported());
                };
                let target_path =
                    name_policy::PortablePath::try_from(path).map_err(ApiError::invalid_source)?;
                let key = self.pin_binding(location.repository)?;
                let identity = self.bound_identity(&key);
                self.shared
                    .backend
                    .authorize_repository(&identity, &key.repository)?;
                (Some(key), target_path)
            }
            Err(error)
                if error.code == storage_contract::ErrorCode::DirectoryNotFound
                    && !destination_path.as_str().contains('/') =>
            {
                let target_path = name_policy::PortablePath::try_from(destination_path)
                    .map_err(ApiError::invalid_source)?;
                (None, target_path)
            }
            Err(error) => return Err(error),
        };
        let target_identity = target.as_ref().map(|key| self.bound_identity(key));
        let target_scope = target.as_ref().map_or_else(
            || {
                format!(
                    "@empty-disk:{}:{}",
                    target_slot.slot_id, target_slot.binding_generation
                )
            },
            |key| key.branch.clone(),
        );
        let _paths = MovePaths::acquire(
            &self.shared,
            [
                (&source.branch, source_path.as_str()),
                (&target_scope, target_path.as_str()),
            ],
        )?;
        let entry = self
            .shared
            .backend
            .lookup_current(&source_identity, &source.branch, &source_path)?
            .ok_or_else(|| {
                ApiError::new(
                    http::StatusCode::NOT_FOUND,
                    storage_contract::ErrorCode::NotFound,
                    "move source is no longer visible",
                )
            })?;
        let displaced = match (&target, &target_identity) {
            (Some(target), Some(identity)) => self
                .shared
                .backend
                .lookup_current(
                    identity,
                    &target.branch,
                    &name_policy::LookupPath::from(&target_path),
                )?
                .filter(|current| {
                    target.repository != source.repository
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
            source_binding: Binding::capture(&source),
            target_binding: target.as_ref().map(Binding::capture),
            source_slot_id,
            source_disk: DiskIdentity::capture(&source_slot),
            target_disk: DiskIdentity::capture(&target_slot),
            source_file_id: file_identity(&entry)?,
            target_file_id: displaced.as_ref().map(file_identity).transpose()?.flatten(),
            movement: ObjectMoveRequest {
                source_repository_id: HistoryRepositoryId::parse(&source.repository)
                    .map_err(ApiError::contract_input)?,
                source_entry_id: NamespaceEntryId::parse(&entry.entry_id)
                    .map_err(ApiError::contract_input)?,
                source_path,
                target_slot_id: target_slot.slot_id,
                target_repository_id: target
                    .as_ref()
                    .map(|key| HistoryRepositoryId::parse(&key.repository))
                    .transpose()
                    .map_err(ApiError::contract_input)?,
                target_path,
                replace: displaced.is_some(),
                target_entry_id: displaced
                    .as_ref()
                    .map(|entry| NamespaceEntryId::parse(&entry.entry_id))
                    .transpose()
                    .map_err(ApiError::contract_input)?,
            },
        };
        admission
            .record_intent(&json_codec::to_value(&intent).map_err(ApiError::invalid_source)?)?;
        let bytes = admission.retained_move_bytes()?;
        self.submit_object_move(intent, admission, &bytes, Some(_paths))
    }

    pub(in crate::smb_rpc::default_share) fn resume_object_move(
        &mut self,
        source: &str,
        target: &str,
        replace: bool,
        operation: OperationId,
    ) -> Result<Option<Value>, ApiError> {
        let Some(admission) =
            NamespaceAdmission::retain_move(&self.shared, &self.identity, operation)?
        else {
            return Ok(None);
        };
        let bytes = admission.retained_move_bytes()?;
        let intent: Intent =
            storage_contract::decode_strict(&bytes).map_err(ApiError::contract_input)?;
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
        if intent.replace_requested != replace
            || source_slot.slot_id != intent.source_slot_id
            || target_slot.slot_id != intent.movement.target_slot_id
            || !intent.source_disk.matches(&source_slot)
            || !intent.target_disk.matches(&target_slot)
        {
            return Err(changed());
        }
        // Resolve only the displayed repository binding. A completed move can
        // remove the source occurrence; it must never capture new members here.
        let source = self.resolve_at(&source_slot, source_path, SmbPathIntent::Lookup)?;
        let SmbRelativePath::Entry(source_path) = source.relative_path else {
            return Err(changed());
        };
        if !intent
            .source_binding
            .matches(&self.pin_binding(source.repository)?)
            || source_path.key() != intent.movement.source_path.key()
        {
            return Err(changed());
        }
        match &intent.target_binding {
            Some(binding) => {
                let target = self.resolve_at(&target_slot, target_path, SmbPathIntent::Lookup)?;
                let SmbRelativePath::Entry(path) = target.relative_path else {
                    return Err(changed());
                };
                if !binding.matches(&self.pin_binding(target.repository)?)
                    || path.as_str() != intent.movement.target_path.as_str()
                {
                    return Err(changed());
                }
            }
            // An originally empty disk may now own the published root. Keep
            // the captured None intent rather than resolving it as a new target.
            None if target_path.as_str() != intent.movement.target_path.as_str() => {
                return Err(changed());
            }
            None => {}
        }
        self.submit_object_move(intent, admission, &bytes, None)
            .map(Some)
    }

    fn require_object_binding(&mut self, binding: &Binding, slot: &str) -> Result<(), ApiError> {
        let repository = self
            .shared
            .backend
            .repository_snapshot(&self.identity, &binding.repository_id)?;
        if repository.primary_slot_id != slot {
            return Err(changed());
        }
        let key = self.pin_binding(repository)?;
        if !binding.matches(&key) {
            return Err(changed());
        }
        self.shared
            .backend
            .authorize_repository(&self.bound_identity(&key), &key.repository)
    }

    fn submit_object_move(
        &mut self,
        intent: Intent,
        admission: NamespaceAdmission,
        bytes: &[u8],
        captured_paths: Option<MovePaths>,
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
        let slots = self.shared.backend.topology_snapshot(&self.identity)?.slots;
        let source = slots
            .iter()
            .find(|slot| slot.slot_id == intent.source_slot_id)
            .ok_or_else(changed)?;
        let target = slots
            .iter()
            .find(|slot| slot.slot_id == intent.movement.target_slot_id)
            .ok_or_else(changed)?;
        if !intent.source_disk.matches(source) || !intent.target_disk.matches(target) {
            return Err(changed());
        }
        self.require_object_binding(&intent.source_binding, &intent.source_slot_id)?;
        if let Some(binding) = &intent.target_binding {
            self.require_object_binding(binding, &intent.movement.target_slot_id)?;
        }
        let target_scope = intent.target_binding.as_ref().map_or_else(
            || {
                format!(
                    "@empty-disk:{}:{}",
                    intent.movement.target_slot_id, intent.target_disk.binding_generation
                )
            },
            |binding| binding.branch_id.clone(),
        );
        let _paths = match captured_paths {
            Some(paths) => paths,
            None => MovePaths::acquire(
                &self.shared,
                [
                    (
                        &intent.source_binding.branch_id,
                        intent.movement.source_path.as_str(),
                    ),
                    (&target_scope, intent.movement.target_path.as_str()),
                ],
            )?,
        };
        if admission.retained_move_bytes()?.as_slice() != bytes {
            return Err(changed());
        }
        let command = intent.command();
        let receipt = MoveReceipt::object(&command)?;
        let result = self
            .shared
            .backend
            .move_current_object(&self.identity, command)?;
        let reply = receipt.published_object_reply(result, &self.identity.principal)?;
        admission.finish_published();
        Ok(reply)
    }
}
