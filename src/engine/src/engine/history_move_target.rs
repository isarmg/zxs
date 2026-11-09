//! Real target-file material preparation. No namespace, receipt or decision
//! becomes visible; the system/disk historical closure is still not proven.
use super::Engine;
use crate::{disk::*, *};
use data_serialization::Serialize;
use std::{collections::BTreeMap, path::Path, sync::Arc};

mod payloads;
mod store;

const METADATA_BYTES: usize = 12 * 1024 * 1024;
const INTENT_BYTES: usize = 64 * 1024;
const DIRECTORY: &str = "file-history-move-target";
const LOGICAL: &str = "joint-metadata.json";
const DATABASE: &str = ".zxs-file.sqlite";

pub(super) type CopyIdentities = BTreeMap<String, BTreeMap<String, String>>;

/// An observation after complete target-file-material verification. This is
/// deliberately not an OperationResult, a publication permit or a three-scope
/// database receipt. Later consumers must read and verify the material again.
#[derive(Debug)]
pub struct FileHistoryMoveTargetReport {
    operation: OperationId,
    recovery_digest: FileHistoryMoveRecoveryDigest,
    target_file: String,
    target_slots: Vec<String>,
    versions_per_slot: usize,
    body_bytes_per_slot: u64,
    file_database_records_hash: String,
    copy_identities: CopyIdentities,
}

impl FileHistoryMoveTargetReport {
    pub fn operation_id(&self) -> OperationId {
        self.operation
    }
    pub fn recovery_digest(&self) -> &FileHistoryMoveRecoveryDigest {
        &self.recovery_digest
    }
    pub fn target_file_id(&self) -> &str {
        &self.target_file
    }
    pub fn target_slots(&self) -> &[String] {
        &self.target_slots
    }
    pub fn versions_per_slot(&self) -> usize {
        self.versions_per_slot
    }
    pub fn body_bytes_per_slot(&self) -> u64 {
        self.body_bytes_per_slot
    }
    pub fn file_database_records_hash(&self) -> &str {
        &self.file_database_records_hash
    }
    pub(super) fn copy_identities(&self) -> &CopyIdentities {
        &self.copy_identities
    }
}

#[derive(Serialize)]
#[serialization(deny_unknown_fields)]
struct Intent<'a> {
    format: &'static str,
    operation_id: OperationId,
    recovery_digest: String,
    projection_hash: String,
    identity: &'a DiskRegistrationIdentity,
    target_file_id: &'a str,
    file_database_records_hash: &'a str,
}

struct Materials {
    logical: Vec<u8>,
    projection_hash: String,
    versions: BTreeMap<String, FileVersion>,
    database: crate::container_storage::file_database::PreparedMoveFile,
    body_bytes: u64,
    storage_bytes: u64,
}

fn limit() -> Error {
    Error::new(
        ErrorCode::ResourceLimit,
        "complete move target material size overflow",
    )
}

fn checkpoint(cancellation: &HistoryWindowCancellation) -> Result<()> {
    if cancellation.is_cancelled() {
        return Err(Error::new(
            ErrorCode::OperationAborted,
            "move target preparation cancelled",
        ));
    }
    Ok(())
}

impl Materials {
    fn capture(projection: &FileHistoryMoveProjection) -> Result<Self> {
        let metadata = projection.target_metadata()?;
        let projection_hash = object_hash("file-history-move-joint-projection", &metadata)?;
        let logical = encode_metadata(&metadata, METADATA_BYTES)?;
        let database = projection.target_file_database()?;
        let mut body_bytes = 0u64;
        let mut storage_bytes = (logical.len() + database.bytes().len() + INTENT_BYTES) as u64;
        for version in metadata.target.versions.values() {
            body_bytes = body_bytes
                .checked_add(version.logical_size)
                .ok_or_else(limit)?;
            let checksum_bytes = checksum_manifest_length(version)? as u64;
            storage_bytes = storage_bytes
                .checked_add(version.logical_size)
                .and_then(|value| value.checked_add(checksum_bytes))
                .and_then(|value| value.checked_add(64 * 1024))
                .ok_or_else(limit)?;
        }
        if metadata.target.versions.len() != projection.version_mapping().len() {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "move target version set is not exact",
            ));
        }
        Ok(Self {
            logical,
            projection_hash,
            versions: metadata.target.versions,
            database,
            body_bytes,
            storage_bytes,
        })
    }

    fn report(
        &self,
        projection: &FileHistoryMoveProjection,
        reviewed: &FileHistoryMoveRecoveryDigest,
        copy_identities: CopyIdentities,
    ) -> FileHistoryMoveTargetReport {
        FileHistoryMoveTargetReport {
            operation: projection.operation_id(),
            recovery_digest: *reviewed,
            target_file: projection.target_file_id().as_str().into(),
            target_slots: projection.capture().target_slots().to_vec(),
            versions_per_slot: self.versions.len(),
            body_bytes_per_slot: self.body_bytes,
            file_database_records_hash: self.database.records_hash().into(),
            copy_identities,
        }
    }
}

impl Engine {
    /// Create real independent FULL files plus complete target logical graph,
    /// imported audit and file-local SQLite on EVERY frozen target route member.
    /// Requires the already saved original recovery envelope. No source hiding,
    /// target namespace publication, ACCEPT or successful move is possible here.
    pub fn prepare_file_history_move_target(
        &mut self,
        projection: &FileHistoryMoveProjection,
        reviewed: &FileHistoryMoveRecoveryDigest,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<FileHistoryMoveTargetReport> {
        self.verify_file_history_move_recovery(projection, reviewed, cancellation)?;
        let materials = Materials::capture(projection)?;
        let prepare = || -> Result<CopyIdentities> {
            for slot in projection.capture().target_slots() {
                checkpoint(cancellation)?;
                let target = store::Target::open(
                    self,
                    projection,
                    reviewed,
                    &materials,
                    slot,
                    store::Mode::Prepare,
                )?;
                target.preflight(self, &materials)?;
                target.install_metadata(&materials)?;
                for (source, id) in projection.version_mapping() {
                    payloads::prepare(
                        self,
                        &target,
                        source,
                        &materials.versions[id],
                        cancellation,
                    )?;
                }
                target.synchronize()?;
            }
            // Do not use copy-loop progress as the final independent proof.
            self.verify_move_target_materials(projection, reviewed, &materials, cancellation)
        };
        let identities = prepare().map_err(|error| error.context(
            ErrorCode::RecoveryRequired,
            "move target materials may be partially retained; source and target namespaces are unchanged, resume the same original operation",
        ))?;
        Ok(materials.report(projection, reviewed, identities))
    }

    /// Read-only, complete target-file verification. A cold caller first resumes
    /// the same recovery envelope; no stored completion bit or self hash is a
    /// substitute for independent source-bound metadata and target body checks.
    pub fn verify_file_history_move_target(
        &self,
        projection: &FileHistoryMoveProjection,
        reviewed: &FileHistoryMoveRecoveryDigest,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<FileHistoryMoveTargetReport> {
        self.verify_file_history_move_recovery(projection, reviewed, cancellation)?;
        let materials = Materials::capture(projection)?;
        let identities =
            self.verify_move_target_materials(projection, reviewed, &materials, cancellation)?;
        Ok(materials.report(projection, reviewed, identities))
    }

    fn verify_move_target_materials(
        &self,
        projection: &FileHistoryMoveProjection,
        reviewed: &FileHistoryMoveRecoveryDigest,
        materials: &Materials,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<CopyIdentities> {
        let mut identities = BTreeMap::new();
        for slot in projection.capture().target_slots() {
            checkpoint(cancellation)?;
            let target = store::Target::open(
                self,
                projection,
                reviewed,
                materials,
                slot,
                store::Mode::Read,
            )?;
            target.verify_metadata(materials)?;
            let mut versions = BTreeMap::new();
            for version in materials.versions.values() {
                versions.insert(
                    version.version_id.clone(),
                    payloads::verify(&target, version, cancellation)?,
                );
            }
            target.check_names()?;
            identities.insert(slot.clone(), versions);
        }
        self.verify_file_history_move_recovery(projection, reviewed, cancellation)?;
        Ok(identities)
    }
}
