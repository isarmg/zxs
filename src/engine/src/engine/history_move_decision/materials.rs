//! Exact scope requirements and bounded, independently rebuilt proposal bytes.
use super::*;
use crate::container_storage::file_database::PreparedMoveFile;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serialization(rename_all = "kebab-case")]
pub(super) enum Role {
    Metadata,
    Source,
    Target,
}

#[derive(Serialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct DiskScope {
    pub(super) identity: DiskRegistrationIdentity,
    pub(super) roles: BTreeSet<Role>,
}
impl DiskScope {
    pub(super) fn carries_state(&self) -> bool {
        self.roles.contains(&Role::Metadata)
    }
}

#[derive(Serialize)]
#[serialization(deny_unknown_fields)]
struct SystemScope {
    previous_published_snapshot: String,
    proposed_state_hash: String,
    #[serialization(with = "storage_contract::u64_string")]
    proposed_state_bytes: u64,
    metadata_members: Vec<DiskRegistrationIdentity>,
}

#[derive(Serialize)]
#[serialization(deny_unknown_fields)]
struct FileScope {
    file_id: String,
    history_tree_id: String,
    records_hash: String,
}

#[derive(Serialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Record {
    format: &'static str,
    operation_id: OperationId,
    recovery_digest: String,
    projection_hash: String,
    #[serialization(with = "storage_contract::u64_string")]
    proposed_sequence: u64,
    source_deletion: FileDeletionNode,
    system: SystemScope,
    pub(super) disks: Vec<DiskScope>,
    source_file: FileScope,
    target_file: FileScope,
    source_placements: BTreeMap<String, Placement>,
    target_route: crate::business::history_copy::CopyRoute,
    pub(super) target_copy_identities: super::super::history_move_target::CopyIdentities,
    version_mapping: BTreeMap<String, String>,
    branch_mapping: BTreeMap<String, String>,
    outstanding: [FileHistoryMoveDecisionObligation; 5],
}

pub(super) struct Materials {
    pub(super) record: Record,
    pub(super) manifest: Vec<u8>,
    pub(super) digest: FileHistoryMoveDecisionDigest,
    pub(super) logical: Vec<u8>,
    pub(super) state: Vec<u8>,
    pub(super) source: PreparedMoveFile,
    pub(super) target: PreparedMoveFile,
    pub(super) target_preparation_bytes: u64,
}

fn participants(
    engine: &Engine,
    projection: &FileHistoryMoveProjection,
    placements: &BTreeMap<String, Placement>,
) -> Result<Vec<DiskScope>> {
    let registered: BTreeMap<_, _> = engine
        .registered_disks()
        .filter(|disk| disk.registration_mode == RegistrationMode::Enabled)
        .map(|disk| (disk.slot_id.to_owned(), disk.identity()))
        .collect();
    let mut members: BTreeMap<String, DiskScope> = BTreeMap::new();
    let mut add = |slot: &str, generation: u64, role: Role| -> Result<()> {
        let identity = registered
            .get(slot)
            .ok_or_else(|| invalid("every original move participant must remain enabled"))?;
        let disk = engine.disk(slot)?;
        if identity.pool_id != engine.config.pool_id
            || identity.disk_id != disk.disk_id
            || identity.binding_generation != generation
            || disk.binding_generation != generation
        {
            return Err(invalid(
                "move participant differs from its exact original binding",
            ));
        }
        members
            .entry(identity.disk_id.clone())
            .or_insert_with(|| DiskScope {
                identity: identity.clone(),
                roles: BTreeSet::new(),
            })
            .roles
            .insert(role);
        Ok(())
    };
    for disk in engine
        .config
        .disks
        .iter()
        .filter(|disk| disk.metadata_member)
    {
        add(&disk.slot_id, disk.binding_generation, Role::Metadata)?;
    }
    for placement in placements.values() {
        for slot in &placement.route_slot_ids {
            let generation = placement
                .binding_generations
                .get(slot)
                .ok_or_else(|| invalid("source placement lacks its original binding generation"))?;
            add(slot, *generation, Role::Source)?;
        }
    }
    for slot in projection.capture().target_slots() {
        add(
            slot,
            projection.target_binding_generations()[slot],
            Role::Target,
        )?;
    }
    Ok(members.into_values().collect())
}

impl Materials {
    pub(super) fn capture(
        engine: &Engine,
        projection: &FileHistoryMoveProjection,
        recovery: &FileHistoryMoveRecoveryDigest,
        targets: &FileHistoryMoveTargetReport,
    ) -> Result<Self> {
        let source = projection.source_file_database(engine.state())?;
        let target = projection.target_file_database()?;
        let metadata = projection.target_metadata()?;
        let logical = encode_metadata(&metadata, STATE_BYTES)?;
        let mut target_preparation_bytes =
            (logical.len() + target.bytes().len() + 64 * 1024) as u64;
        for version in metadata.target.versions.values() {
            let checksums = checksum_manifest_length(version)? as u64;
            target_preparation_bytes = target_preparation_bytes
                .checked_add(version.logical_size)
                .and_then(|bytes| bytes.checked_add(checksums))
                .and_then(|bytes| bytes.checked_add(64 * 1024))
                .ok_or_else(limit)?;
        }
        let state = projection.unaccepted_state_bytes()?;
        let proposed_state_hash = hash(b"file-history-move-unaccepted-state", &state)
            .to_hex()
            .to_string();
        let source_placements = projection
            .capture()
            .version_ids()
            .iter()
            .map(|id| (id.clone(), engine.state().placements[id].clone()))
            .collect();
        let disks = participants(engine, projection, &source_placements)?;
        let metadata_members: Vec<_> = disks
            .iter()
            .filter(|disk| disk.carries_state())
            .map(|disk| disk.identity.clone())
            .collect();
        if metadata_members.is_empty() {
            return Err(invalid(
                "move decision inputs require the original metadata membership",
            ));
        }
        let target_repository =
            &engine.state().repositories[&projection.capture().namespace().target_repository_id];
        let record = Record {
            format: "file-history-move-unaccepted-decision-inputs",
            operation_id: projection.operation_id(),
            recovery_digest: recovery.to_string(),
            projection_hash: projection.recovery_records_hash()?,
            proposed_sequence: projection.proposed_sequence(),
            source_deletion: projection.proposed_source_deletion().clone(),
            system: SystemScope {
                previous_published_snapshot: engine.snapshot.clone().ok_or_else(|| {
                    invalid("move decision inputs require a published source snapshot")
                })?,
                proposed_state_hash,
                proposed_state_bytes: state.len() as u64,
                metadata_members,
            },
            disks,
            source_file: FileScope {
                file_id: projection.capture().source_file_id().as_str().into(),
                history_tree_id: projection
                    .proposed_source_deletion()
                    .history_tree_id
                    .clone(),
                records_hash: source.records_hash().into(),
            },
            target_file: FileScope {
                file_id: projection.target_file_id().as_str().into(),
                history_tree_id: target_repository.history_tree_id.clone(),
                records_hash: target.records_hash().into(),
            },
            source_placements,
            target_route: projection.recovery_route().clone(),
            target_copy_identities: targets.copy_identities().clone(),
            version_mapping: projection.version_mapping().clone(),
            branch_mapping: projection.branch_mapping().clone(),
            outstanding: OUTSTANDING,
        };
        let manifest = encode_metadata(&record, MANIFEST_BYTES)?;
        let digest =
            FileHistoryMoveDecisionDigest(hash(b"file-history-move-decision-inputs", &manifest));
        Ok(Self {
            record,
            manifest,
            digest,
            logical,
            state,
            source,
            target,
            target_preparation_bytes,
        })
    }

    pub(super) fn report(&self) -> FileHistoryMoveDecisionInputs {
        FileHistoryMoveDecisionInputs {
            operation: self.record.operation_id,
            digest: self.digest,
            metadata_members: self.record.system.metadata_members.len(),
            participant_disks: self.record.disks.len(),
            source_database_hash: self.source.records_hash().into(),
            target_database_hash: self.target.records_hash().into(),
            proposed_state_hash: self.record.system.proposed_state_hash.clone(),
        }
    }
}
