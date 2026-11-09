//! Immutable joint recovery preparation, not an ACCEPT, decision or target proof.
use super::Engine;
use crate::{disk::*, *};
use data_serialization::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use storage_contract::history_window::HistoryFileId;

mod operations;
mod persist;
use operations::metadata_members;

const FORMAT: &str = "file-history-move-joint-recovery";
const MAX_BYTES: usize = 2 * 1024 * 1024;
const MAX_ITEMS: usize = 65_536;

/// Exact reviewed recovery bytes, not authorization to execute the stored move.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FileHistoryMoveRecoveryDigest(blake3::Hash);

impl FileHistoryMoveRecoveryDigest {
    pub(crate) fn from_native_receipt(value: &str) -> Result<Self> {
        let hash = blake3::Hash::from_hex(value).map_err(|_| {
            Error::new(
                ErrorCode::PublicationInvalid,
                "native move receipt has an invalid material digest",
            )
        })?;
        if hash.to_hex().as_str() != value {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "native move digest is not canonical",
            ));
        }
        Ok(Self(hash))
    }
}

impl std::fmt::Display for FileHistoryMoveRecoveryDigest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.0.to_hex().as_str())
    }
}

/// A retained joint preparation. No body copy, source D or decision is certified.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileHistoryMoveRecoveryInfo {
    operation_id: OperationId,
    digest: FileHistoryMoveRecoveryDigest,
    source_file_id: HistoryFileId,
    source_sequence: u64,
    decision_slot: u64,
    metadata_members: usize,
}

impl FileHistoryMoveRecoveryInfo {
    pub fn operation_id(&self) -> OperationId {
        self.operation_id
    }

    pub fn digest(&self) -> &FileHistoryMoveRecoveryDigest {
        &self.digest
    }

    pub fn source_file_id(&self) -> &HistoryFileId {
        &self.source_file_id
    }

    pub fn source_sequence(&self) -> u64 {
        self.source_sequence
    }

    /// Original proposed slot, not a reservation or decided sequence.
    pub fn decision_slot(&self) -> u64 {
        self.decision_slot
    }

    /// Expected envelope replicas, not an independently observed health count.
    pub fn metadata_members(&self) -> usize {
        self.metadata_members
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
enum ConfigurationGeneration {
    Initial,
    Activated {
        hash: String,
        #[serialization(with = "storage_contract::u64_string")]
        membership_epoch: u64,
    },
}

impl ConfigurationGeneration {
    fn current(engine: &Engine) -> Self {
        match engine.config.generation_hash() {
            None => Self::Initial,
            Some(hash) => Self::Activated {
                hash: hash.into(),
                membership_epoch: engine.config.membership_epoch(),
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct RecoveryRecord {
    format: String,
    operation_id: OperationId,
    principal: String,
    pool_id: String,
    snapshot: String,
    configuration_hash: String,
    generation: ConfigurationGeneration,
    namespace: FileHistoryCopyRequest,
    source_file_id: String,
    #[serialization(skip_serializing_if = "Option::is_none")]
    replacement: Option<FileHistoryMoveReplacement>,
    #[serialization(with = "storage_contract::u64_string")]
    source_sequence: u64,
    source_records_hash: String,
    branch_heads: BTreeMap<String, String>,
    parent_node_ids: Vec<String>,
    version_ids: Vec<String>,
    target_slots: Vec<String>,
    route: crate::business::history_copy::CopyRoute,
    #[serialization(with = "storage_contract::u64_string")]
    decision_slot: u64,
    projection_hash: String,
    metadata_members: Vec<DiskRegistrationIdentity>,
}

fn stale(message: &str) -> Error {
    Error::new(ErrorCode::PreparationStale, message)
}

fn corrupt(message: &str) -> Error {
    Error::new(ErrorCode::ObjectCorrupt, message)
}

fn parse_hash(value: &str) -> Result<blake3::Hash> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(corrupt(
            "move recovery envelope hash is not canonical lowercase hexadecimal",
        ));
    }
    Ok(blake3::Hash::from_hex(value)?)
}

impl RecoveryRecord {
    fn capture(engine: &Engine, projection: &FileHistoryMoveProjection) -> Result<Self> {
        let capture = projection.capture();
        Ok(Self {
            format: FORMAT.into(),
            operation_id: projection.operation_id(),
            principal: projection.principal().into(),
            pool_id: engine.config.pool_id.clone(),
            snapshot: engine.snapshot.clone().ok_or_else(|| {
                stale("history move recovery requires a published source snapshot")
            })?,
            configuration_hash: object_hash("file-history-move-config", &engine.config)?,
            generation: ConfigurationGeneration::current(engine),
            namespace: capture.namespace().clone(),
            source_file_id: capture.source_file_id().as_str().into(),
            replacement: capture.replacement().cloned(),
            source_sequence: capture.source_sequence(),
            source_records_hash: capture.source_records_digest().to_string(),
            branch_heads: capture.branch_heads().clone(),
            parent_node_ids: capture.parent_node_ids().to_vec(),
            version_ids: capture.version_ids().to_vec(),
            target_slots: capture.target_slots().to_vec(),
            route: projection.recovery_route().clone(),
            decision_slot: projection.proposed_sequence(),
            projection_hash: projection.recovery_records_hash()?,
            metadata_members: metadata_members(engine)?,
        })
    }

    fn digest(&self) -> Result<FileHistoryMoveRecoveryDigest> {
        Ok(FileHistoryMoveRecoveryDigest(blake3::Hash::from_hex(
            object_hash("file-history-move-recovery", self)?,
        )?))
    }

    fn info(&self) -> Result<FileHistoryMoveRecoveryInfo> {
        Ok(FileHistoryMoveRecoveryInfo {
            operation_id: self.operation_id,
            digest: self.digest()?,
            source_file_id: HistoryFileId::parse(&self.source_file_id)?,
            source_sequence: self.source_sequence,
            decision_slot: self.decision_slot,
            metadata_members: self.metadata_members.len(),
        })
    }

    fn validate(&self) -> Result<()> {
        if self.format != FORMAT {
            return Err(Error::new(
                ErrorCode::FormatUnsupported,
                "unsupported file history move recovery envelope format; retained bytes preserved",
            ));
        }
        Engine::require_actor(&self.principal)?;
        storage_contract::validate_id(&self.pool_id, "pool")?;
        HistoryFileId::parse(&self.source_file_id)?;
        storage_contract::validate_id(&self.namespace.source_repository_id, "repo")?;
        storage_contract::validate_id(&self.namespace.target_repository_id, "repo")?;
        storage_contract::validate_id(&self.namespace.expected_source_head, "commit")?;
        storage_contract::validate_id(&self.namespace.expected_target_head, "commit")?;
        name_policy::LookupPath::parse(&self.namespace.source_path)?;
        name_policy::PortablePath::parse(&self.namespace.target_name)?;
        for hash in [
            &self.snapshot,
            &self.configuration_hash,
            &self.source_records_hash,
            &self.projection_hash,
        ] {
            parse_hash(hash)?;
        }
        if let ConfigurationGeneration::Activated {
            hash,
            membership_epoch,
        } = &self.generation
        {
            parse_hash(hash)?;
            if *membership_epoch == 0 {
                return Err(corrupt(
                    "move recovery envelope has an invalid configuration generation",
                ));
            }
        }
        if self.source_sequence == 0
            || self.branch_heads.is_empty()
            || self.branch_heads.len() > MAX_ITEMS
            || self.version_ids.is_empty()
            || self.version_ids.len() > MAX_ITEMS
            || self.parent_node_ids.len() > MAX_ITEMS
            || self.target_slots.is_empty()
            || self.target_slots.len() > MAX_ITEMS
        {
            return Err(corrupt(
                "move recovery envelope has an invalid retained source cardinality",
            ));
        }
        for (branch, head) in &self.branch_heads {
            storage_contract::validate_id(branch, "br")?;
            match head.split_once('_').map(|(prefix, _)| prefix) {
                Some("ver") => storage_contract::validate_id(head, "ver")?,
                Some("fnode") => storage_contract::validate_id(head, "fnode")?,
                _ => {
                    return Err(corrupt(
                        "move recovery envelope frontier is not a retained file node",
                    ));
                }
            }
        }
        let parents: Vec<_> = self
            .branch_heads
            .values()
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        if self.parent_node_ids != parents
            || self.version_ids.iter().collect::<BTreeSet<_>>().len() != self.version_ids.len()
            || self.target_slots.iter().collect::<BTreeSet<_>>().len() != self.target_slots.len()
        {
            return Err(corrupt(
                "move recovery envelope frontier, versions or target route is not exact",
            ));
        }
        for version in &self.version_ids {
            storage_contract::validate_id(version, "ver")?;
        }
        for slot in &self.target_slots {
            name_policy::validate_slot(slot)?;
        }
        if self.decision_slot <= self.source_sequence
            || self.route.slots != self.target_slots
            || self.route.bindings.len() != self.target_slots.len()
            || self.target_slots.iter().any(|slot| {
                self.route
                    .bindings
                    .get(slot)
                    .is_none_or(|generation| *generation == 0)
            })
            || self.metadata_members.is_empty()
            || self.metadata_members.len() > MAX_ITEMS
        {
            return Err(corrupt(
                "move recovery envelope has an invalid joint slot, route or member set",
            ));
        }
        let mut disk_ids = BTreeSet::new();
        let mut slots = BTreeSet::new();
        let mut previous: Option<&str> = None;
        for member in &self.metadata_members {
            storage_contract::validate_id(&member.disk_id, "disk")?;
            name_policy::validate_slot(&member.slot_id)?;
            if member.pool_id != self.pool_id
                || member.binding_generation == 0
                || !disk_ids.insert(&member.disk_id)
                || !slots.insert(&member.slot_id)
                || previous.is_some_and(|id| id >= member.disk_id.as_str())
            {
                return Err(corrupt(
                    "move recovery member identities are not exact and ordered",
                ));
            }
            previous = Some(&member.disk_id);
        }
        Ok(())
    }

    fn require_source(&self, engine: &Engine) -> Result<()> {
        if self.pool_id != engine.config.pool_id
            || self.source_sequence != engine.state.sequence
            || engine.snapshot.as_deref() != Some(self.snapshot.as_str())
            || self.configuration_hash != object_hash("file-history-move-config", &engine.config)?
            || self.generation != ConfigurationGeneration::current(engine)
            || self.metadata_members != metadata_members(engine)?
        {
            return Err(stale(
                "the original move source snapshot or configuration changed; no replan was saved",
            ));
        }
        Ok(())
    }

    fn require_current(&self, engine: &Engine) -> Result<()> {
        self.require_source(engine)?;
        if self.decision_slot != engine.move_source_slot() {
            return Err(stale("the original move decision slot changed"));
        }
        Ok(())
    }
}
