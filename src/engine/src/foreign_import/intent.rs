//! Explicitly confirmed, immutable execution intents on an admitted data disk.
//! An intent is neither a target reservation nor a namespace publication receipt.
use super::*;
use data_serialization::{Deserialize, Serialize};
use name_policy::PortablePath;
use std::os::unix::fs::FileExt;

mod location;
mod persist;
mod source;
use super::migration::PhysicalBinding;
pub use source::ForeignImportIntentSource;
use source::SourceRecord;

const FORMAT: &str = "foreign-import-intent-current-format";
const DIRECTORY: &str = "metadata/foreign-import-intents";
const MAX_INTENT_BYTES: usize = 2 * 1024 * 1024;
const MAX_RETAINED_INTENTS: usize = 128;
const MAX_RETAINED_BYTES: u64 = 256 * 1024 * 1024;

/// Validated target syntax, not proof of a currently writable or empty target.
pub struct ForeignImportIntentTarget {
    slot: String,
    parent: Option<PortablePath>,
    names: BTreeMap<String, String>,
}
impl ForeignImportIntentTarget {
    pub fn new(
        slot: String,
        parent: Option<String>,
        names: BTreeMap<String, String>,
    ) -> Result<Self> {
        name_policy::validate_slot(&slot)?;
        let parent = parent.as_deref().map(PortablePath::parse).transpose()?;
        Ok(Self {
            slot,
            parent,
            names,
        })
    }
}

/// Audit and review bindings supplied by the trusted authenticated caller.
/// A principal string is never a credential or an authorization capability.
pub struct ForeignImportIntentRequest {
    pub operation_id: OperationId,
    pub review_operation_id: OperationId,
    pub review_digest: ForeignEvidenceDigest,
    pub principal: String,
}

/// Nonserializable preparation. The source and exact target facts have been
/// observed; no disk has changed and current permissions are still required.
pub struct PreparedForeignImportIntent {
    source: ForeignImportIntentSource,
    target: ForeignImportIntentTarget,
    record: IntentRecord,
    digest: ForeignEvidenceDigest,
}
impl PreparedForeignImportIntent {
    pub fn digest(&self) -> &ForeignEvidenceDigest {
        &self.digest
    }
    pub fn operation_id(&self) -> OperationId {
        self.record.operation_id
    }
    pub fn source_review_digest(&self) -> &str {
        self.record.source.review_digest()
    }
    pub fn target_identity(&self) -> &DiskRegistrationIdentity {
        &self.record.target.identity
    }
    pub fn target_mapping_digest(&self) -> &str {
        &self.record.target.mapping_digest
    }
    pub fn history_count(&self) -> usize {
        self.record.mapping.len()
    }
    pub fn temporarily_unavailable_versions(&self) -> usize {
        self.record.source.unavailable_count()
    }
    /// Confirm only saving the exact reviewed intent. This is not an import or
    /// permission to consume a persisted record as a future write capability.
    pub fn confirm_save(
        self,
        reviewed: &ForeignEvidenceDigest,
    ) -> Result<ConfirmedForeignImportIntent> {
        if &self.digest != reviewed {
            return Err(stale(
                "foreign import intent confirmation differs from the complete review",
            ));
        }
        Ok(ConfirmedForeignImportIntent(self))
    }
}

/// Produced only from an exact in-process reviewed preparation, never JSON.
pub struct ConfirmedForeignImportIntent(PreparedForeignImportIntent);

/// Observation of retained intent bytes, not authorization, progress or success.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForeignImportIntentInfo {
    operation_id: OperationId,
    digest: ForeignEvidenceDigest,
    target: DiskRegistrationIdentity,
    review_operation_id: OperationId,
    review_digest: ForeignEvidenceDigest,
    histories: usize,
    unavailable: usize,
}
impl ForeignImportIntentInfo {
    pub fn operation_id(&self) -> OperationId {
        self.operation_id
    }
    pub fn digest(&self) -> &ForeignEvidenceDigest {
        &self.digest
    }
    pub fn target(&self) -> &DiskRegistrationIdentity {
        &self.target
    }
    pub fn review_operation_id(&self) -> OperationId {
        self.review_operation_id
    }
    pub fn review_digest(&self) -> &ForeignEvidenceDigest {
        &self.review_digest
    }
    pub fn history_count(&self) -> usize {
        self.histories
    }
    pub fn temporarily_unavailable_versions(&self) -> usize {
        self.unavailable
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct TargetRecord {
    identity: DiskRegistrationIdentity,
    physical: PhysicalBinding,
    sequence: u64,
    mapping_digest: String,
    #[serialization(deserialize_with = "required_nullable")]
    parent: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct MappingRecord {
    history_tree_id: String,
    path: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(rename_all = "snake_case")]
enum MissingBodyPolicy {
    PreserveUnavailable,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct IntentRecord {
    format: String,
    operation_id: OperationId,
    review_operation_id: OperationId,
    review_digest: String,
    principal: String,
    source: SourceRecord,
    target: TargetRecord,
    mapping: BTreeMap<String, MappingRecord>,
    missing_body_policy: MissingBodyPolicy,
}
impl IntentRecord {
    fn digest(&self) -> Result<ForeignEvidenceDigest> {
        digest_value(&object_hash("foreign-import-intent", self)?)
    }
    fn validate(&self) -> Result<()> {
        if self.format != FORMAT {
            return Err(Error::new(
                ErrorCode::FormatUnsupported,
                "unsupported foreign intent format",
            ));
        }
        Engine::require_actor(&self.principal)?;
        if self.operation_id == self.review_operation_id {
            return Err(invalid(
                "execution intent must not reuse its read-only review operation",
            ));
        }
        digest_value(&self.review_digest)?;
        self.source.validate()?;
        identity(&self.target.identity)?;
        digest_value(&self.target.mapping_digest)?;
        let parent = self
            .target
            .parent
            .as_deref()
            .map(PortablePath::parse)
            .transpose()?;
        let mut keys = std::collections::BTreeSet::new();
        if self.source.groups().count() != self.mapping.len() {
            return Err(invalid("intent mapping omits or adds a source history"));
        }
        for (group, history) in self.source.groups() {
            let mapped = self
                .mapping
                .get(group)
                .ok_or_else(|| invalid("intent mapping omits a source group"))?;
            let path = PortablePath::parse(&mapped.path)?;
            let child = match &parent {
                Some(parent) => path.as_str().strip_prefix(&format!("{}/", parent.as_str())),
                None => Some(path.as_str()),
            }
            .ok_or_else(|| invalid("intent mapping escapes the reviewed parent"))?;
            name_policy::validate_name(child)?;
            if mapped.history_tree_id != history || !keys.insert(path.key().to_owned()) {
                return Err(invalid(
                    "intent mapping changes history identity or repeats a destination",
                ));
            }
        }
        Ok(())
    }
    fn info(&self) -> Result<ForeignImportIntentInfo> {
        Ok(ForeignImportIntentInfo {
            operation_id: self.operation_id,
            digest: self.digest()?,
            target: self.target.identity.clone(),
            review_operation_id: self.review_operation_id,
            review_digest: digest_value(&self.review_digest)?,
            histories: self.mapping.len(),
            unavailable: self.source.unavailable_count(),
        })
    }
}

impl Engine {
    /// Capture one complete mapping for a trusted, already authenticated actor.
    /// The caller must revalidate current authentication before save as well.
    pub fn prepare_foreign_import_intent(
        &self,
        source: ForeignImportIntentSource,
        target: ForeignImportIntentTarget,
        request: ForeignImportIntentRequest,
    ) -> Result<PreparedForeignImportIntent> {
        let ForeignImportIntentRequest {
            operation_id: operation,
            review_operation_id,
            review_digest,
            principal,
        } = request;
        Self::require_actor(&principal)?;
        self.require_write()?;
        self.require_unfenced_membership()?;
        self.require_foreign_intent_operation(operation)?;
        let mapping = source.preview.map_to_primary(
            self,
            &target.slot,
            target.parent.as_ref(),
            &target.names,
        )?;
        let disk = self.disk(&mapping.target.slot_id)?;
        let root = &self.disk_resources.get(disk)?.root;
        location::reject_overlap(&source.preview.root, root)?;
        let record = IntentRecord {
            format: FORMAT.into(),
            operation_id: operation,
            review_operation_id,
            review_digest: review_digest.to_string(),
            principal,
            source: source.record.clone(),
            target: TargetRecord {
                identity: mapping.target.clone(),
                physical: PhysicalBinding::of(root)?,
                sequence: mapping.target_sequence,
                mapping_digest: mapping.digest.to_string(),
                parent: target
                    .parent
                    .as_ref()
                    .map(|parent| parent.as_str().to_owned()),
            },
            mapping: mapping
                .histories
                .iter()
                .map(|history| {
                    (
                        history.group_id.clone(),
                        MappingRecord {
                            history_tree_id: history.history_tree_id.clone(),
                            path: history.path.as_str().into(),
                        },
                    )
                })
                .collect(),
            missing_body_policy: MissingBodyPolicy::PreserveUnavailable,
        };
        record.validate()?;
        encode_metadata(&record, MAX_INTENT_BYTES)?;
        let digest = record.digest()?;
        Ok(PreparedForeignImportIntent {
            source,
            target,
            record,
            digest,
        })
    }

    /// Save only the confirmed immutable intent through this admitted Engine's
    /// exact target resource and writer fence. No namespace or receipt is created.
    pub fn save_foreign_import_intent(
        &mut self,
        confirmed: ConfirmedForeignImportIntent,
        principal: &str,
    ) -> Result<ForeignImportIntentInfo> {
        let prepared = confirmed.0;
        if prepared.record.principal != principal {
            return Err(Error::new(
                ErrorCode::PermissionDenied,
                "foreign intent actor differs",
            ));
        }
        self.require_write()?;
        self.require_unfenced_membership()?;
        self.require_foreign_intent_operation(prepared.record.operation_id)?;
        let mapping = prepared.source.preview.map_to_primary(
            self,
            &prepared.target.slot,
            prepared.target.parent.as_ref(),
            &prepared.target.names,
        )?;
        if mapping.digest.to_string() != prepared.record.target.mapping_digest
            || mapping.target != prepared.record.target.identity
        {
            return Err(stale("foreign intent target changed after confirmation"));
        }
        let disk = self.disk(&mapping.target.slot_id)?;
        let root = &self.disk_resources.get(disk)?.root;
        let _activity = root.retain_io_activity()?;
        let _locks = self.retain_disk_writer_locks(disk, root)?;
        if PhysicalBinding::of(root)? != prepared.record.target.physical {
            return Err(stale("foreign intent target physical binding changed"));
        }
        location::reject_overlap(&prepared.source.preview.root, root)?;
        persist::save(root, &prepared.record, self.config.metadata_reserve_bytes)?;
        prepared.record.info()
    }

    /// Read only the selected target's intent. Never reopens or probes the old
    /// source. A partial operation directory is RecoveryRequired, not absence.
    pub fn foreign_import_intent(
        &self,
        target: &DiskRegistrationIdentity,
        operation: OperationId,
        principal: &str,
    ) -> Result<Option<ForeignImportIntentInfo>> {
        Self::require_actor(principal)?;
        let registered = self
            .registered_disks()
            .find(|disk| disk.slot_id == target.slot_id)
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::DiskUnavailable,
                    "foreign intent target is not registered",
                )
            })?;
        if registered.identity() != *target
            || registered.registration_mode != RegistrationMode::Enabled
        {
            return Err(stale(
                "foreign intent target binding is not currently enabled",
            ));
        }
        let disk = self.disk(&target.slot_id)?;
        let root = &self.disk_resources.get(disk)?.root;
        let Some(observed) = persist::read(root, operation)? else {
            return Ok(None);
        };
        let record = &observed.record;
        if record.principal != principal {
            return Err(Error::new(
                ErrorCode::PermissionDenied,
                "foreign intent actor differs",
            ));
        }
        if record.target.identity != *target || record.target.physical != PhysicalBinding::of(root)?
        {
            return Err(stale("foreign intent record names another target"));
        }
        Ok(Some(record.info()?))
    }

    /// Retry only the exact saved intent, without opening its original source
    /// or assuming the target namespace is still reserved. Re-establish barriers
    /// after a prior uncertain response; never manufacture a publication receipt.
    pub fn reconcile_foreign_import_intent(
        &mut self,
        target: &DiskRegistrationIdentity,
        operation: OperationId,
        principal: &str,
        reviewed: &ForeignEvidenceDigest,
    ) -> Result<Option<ForeignImportIntentInfo>> {
        self.require_write()?;
        self.require_unfenced_membership()?;
        self.require_foreign_intent_operation(operation)?;
        Self::require_actor(principal)?;
        let registered = self
            .registered_disks()
            .find(|disk| disk.slot_id == target.slot_id)
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::DiskUnavailable,
                    "foreign intent target is not registered",
                )
            })?;
        if registered.identity() != *target
            || registered.registration_mode != RegistrationMode::Enabled
        {
            return Err(stale(
                "foreign intent target binding is not currently enabled",
            ));
        }
        let disk = self.disk(&target.slot_id)?;
        let root = &self.disk_resources.get(disk)?.root;
        let _activity = root.retain_io_activity()?;
        let _locks = self.retain_disk_writer_locks(disk, root)?;
        let Some(observed) = persist::read(root, operation)? else {
            return Ok(None);
        };
        if observed.record.principal != principal {
            return Err(Error::new(
                ErrorCode::PermissionDenied,
                "foreign intent actor differs",
            ));
        }
        if observed.record.target.identity != *target
            || observed.record.target.physical != PhysicalBinding::of(root)?
        {
            return Err(stale("foreign intent record names another target"));
        }
        let info = observed.record.info()?;
        if info.digest() != reviewed {
            return Err(stale(
                "saved foreign intent differs from the confirmed digest",
            ));
        }
        observed.synchronize(root)?;
        Ok(Some(info))
    }

    fn require_foreign_intent_operation(&self, operation: OperationId) -> Result<()> {
        if self.state.operations.get(&operation).is_some() {
            return Err(Error::new(
                ErrorCode::IdempotencyKeyReused,
                "execution intent operation already names a published business operation",
            ));
        }
        Ok(())
    }
}

/// Check an exact-object read descriptor without sharing or changing its cursor.
/// Callers retain the fsync object and prove that this reader names its inode;
/// inode equality alone does not bind an in-place rewrite to the reviewed plan.
fn verify_record_bytes(file: &ReadFile, expected: &[u8]) -> Result<()> {
    if file.metadata()?.len() != expected.len() as u64 {
        return Err(stale("retained foreign evidence bytes changed"));
    }
    let mut buffer = [0u8; 64 * 1024];
    let mut offset = 0;
    for chunk in expected.chunks(buffer.len()) {
        file.read_exact_at(&mut buffer[..chunk.len()], offset)?;
        if &buffer[..chunk.len()] != chunk {
            return Err(stale("retained foreign evidence bytes changed"));
        }
        offset += chunk.len() as u64;
    }
    if file.metadata()?.len() != offset {
        return Err(stale(
            "retained foreign evidence changed during its barrier",
        ));
    }
    file.check_running()
}

fn identity(identity: &DiskRegistrationIdentity) -> Result<()> {
    validate_id(&identity.pool_id, "pool")?;
    validate_id(&identity.disk_id, "disk")?;
    name_policy::validate_slot(&identity.slot_id)?;
    if identity.binding_generation == 0 {
        return Err(invalid("foreign identity has zero binding generation"));
    }
    Ok(())
}
fn digest_value(value: &str) -> Result<ForeignEvidenceDigest> {
    value
        .parse::<ForeignEvidenceDigest>()
        .map_err(|error| Error::from(std::io::Error::other(error)))
}
fn stale(message: &str) -> Error {
    Error::new(ErrorCode::PlanStale, message)
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/foreign_import/intent_tests.rs"]
mod tests;

fn required_nullable<'de, T: Deserialize<'de>, D: data_serialization::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<T>, D::Error> {
    Option::<T>::deserialize(deserializer)
}
