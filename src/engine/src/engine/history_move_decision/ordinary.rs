//! Install actual independent target bodies at ordinary addresses, retaining
//! original stage copies and candidate identity. No decision or source hiding.
use super::*;
use candidate::FrozenCandidate;
use data_serialization::Deserialize;
use std::{
    os::unix::fs::{FileExt, MetadataExt},
    path::{Path, PathBuf},
    sync::Arc,
};

mod copy;
mod databases;
mod installation;
mod store;

const DIRECTORY: &str = "file-history-move-ordinary";
const MANIFEST: &str = "ordinary-inputs.json";
const MAX_RECORD: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serialization(deny_unknown_fields)]
struct Inode {
    #[serialization(with = "storage_contract::u64_string")]
    device: u64,
    #[serialization(with = "storage_contract::u64_string")]
    inode: u64,
}
impl Inode {
    fn directory(root: &DiskRoot, path: &Path) -> Result<Self> {
        let identity = root.directory_identity(path)?;
        Ok(Self {
            device: identity.device,
            inode: identity.inode,
        })
    }
    fn file(file: &ReadFile, version: &FileVersion) -> Result<Self> {
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.nlink() != 1 || metadata.len() != version.logical_size {
            return Err(invalid(
                "ordinary move copy must be one independent complete regular file",
            ));
        }
        Ok(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
}

/// Ordinary payload observation, not a public operation or serving-ready proof.
#[derive(Debug)]
pub struct FileHistoryMoveOrdinaryPreparation {
    operation: OperationId,
    candidate_hash: String,
    target_disks: usize,
    copies: usize,
    bytes: u64,
    file_database_intents: usize,
    disk_database_intents: usize,
}
impl FileHistoryMoveOrdinaryPreparation {
    pub fn operation_id(&self) -> OperationId {
        self.operation
    }
    pub fn candidate_hash(&self) -> &str {
        &self.candidate_hash
    }
    pub fn target_disks(&self) -> usize {
        self.target_disks
    }
    pub fn copies(&self) -> usize {
        self.copies
    }
    pub fn bytes(&self) -> u64 {
        self.bytes
    }
    pub fn file_database_intents(&self) -> usize {
        self.file_database_intents
    }
    pub fn disk_database_intents(&self) -> usize {
        self.disk_database_intents
    }
    pub fn outstanding(&self) -> &[FileHistoryMoveDecisionObligation] {
        &OUTSTANDING
    }
}

impl Engine {
    /// Copy every original target FULL into real ordinary version/container
    /// addresses without links or replacing either the source or original stage.
    /// Preserve exact origin inodes through directory/body moves and retries.
    pub fn prepare_file_history_move_ordinary(
        &mut self,
        projection: &FileHistoryMoveProjection,
        recovery: &FileHistoryMoveRecoveryDigest,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<FileHistoryMoveOrdinaryPreparation> {
        let (materials, native, scopes, choice) =
            self.selected_move_inputs(projection, recovery, cancellation)?;
        let frozen = FrozenCandidate::capture(self, projection, &native, &choice, cancellation)?;
        self.verify_move_candidate(&materials, &frozen, cancellation)?;
        frozen.preflight(self, &materials, &native, &scopes, cancellation)?;
        let replicas = installation::Replicas::read(self, &materials, cancellation)?;
        if replicas.has_original() {
            // One surviving record is enough to forbid recreating any original
            // pair. Missing record replicas may only be filled after read-only
            // reconstruction verifies all original intents and their inodes.
            let bindings = databases::Databases::read_bindings(
                self,
                projection,
                &frozen,
                &materials,
                cancellation,
            )?;
            let bytes = installation::Replicas::encode(&frozen, &bindings)?;
            replicas.require_original(&bytes, cancellation)?;
        }
        let mut targets = Vec::new();
        for slot in projection.capture().target_slots() {
            checkpoint(cancellation)?;
            let target = store::Target::capture(self, projection, &materials, &frozen, slot)?;
            target.preflight(self, &materials, &native, &scopes, &frozen, cancellation)?;
            targets.push(target);
        }
        let mut finish = || -> Result<FileHistoryMoveOrdinaryPreparation> {
            for target in &mut targets {
                checkpoint(cancellation)?;
                target.install_manifest()?;
                target.prepare_container()?;
                let original = crate::engine::history_move_target::OriginalMoveTarget::open(
                    self,
                    projection,
                    recovery,
                    target.slot(),
                )?;
                for evidence in frozen.versions() {
                    checkpoint(cancellation)?;
                    target.prepare_version(&original, &evidence.version, cancellation)?;
                }
            }
            let mut report = verify_targets(self, projection, &frozen, &targets, cancellation)?;
            // These are pending preserving-install intents, not exchanges. The
            // currently published source FILE/root aliases remain untouched.
            let mut databases =
                databases::Databases::capture(self, projection, &frozen, &materials, cancellation)?;
            databases.preflight(
                self,
                projection,
                (&materials, &native, &scopes),
                &frozen,
                &targets,
            )?;
            (report.file_database_intents, report.disk_database_intents) =
                databases.stage(self, cancellation)?;
            let bindings = databases.bindings(self, cancellation)?;
            let bytes = installation::Replicas::encode(&frozen, &bindings)?;
            replicas.install(&bytes, cancellation)?;
            self.verify_move_candidate(&materials, &frozen, cancellation)?;
            self.read_file_history_move_joint_choice(projection, recovery, cancellation)?;
            verify_targets(self, projection, &frozen, &targets, cancellation)?;
            databases.verify(self, cancellation)?;
            replicas.verify(
                &installation::Replicas::encode(&frozen, &databases.bindings(self, cancellation)?)?,
                cancellation,
            )?;
            Ok(report)
        };
        finish().map_err(|error| error.context(ErrorCode::RecoveryRequired,
            "ordinary move preparation may be partially retained; source and journal remain unchanged, resume only the original candidate"))
    }

    /// Read-only full preparation observation: independently verify all ordinary
    /// bodies and original pending FILE/current pairs on every required disk.
    /// Missing records, directories or intents are never created here. This
    /// still does not authorize a journal decision or active database exchange.
    pub fn read_file_history_move_ordinary(
        &self,
        projection: &FileHistoryMoveProjection,
        recovery: &FileHistoryMoveRecoveryDigest,
        cancellation: &HistoryWindowCancellation,
    ) -> Result<FileHistoryMoveOrdinaryPreparation> {
        let (materials, native, _, choice) =
            self.selected_move_inputs(projection, recovery, cancellation)?;
        let frozen = FrozenCandidate::capture(self, projection, &native, &choice, cancellation)?;
        self.verify_move_candidate(&materials, &frozen, cancellation)?;
        let targets = projection
            .capture()
            .target_slots()
            .iter()
            .map(|slot| store::Target::capture(self, projection, &materials, &frozen, slot))
            .collect::<Result<Vec<_>>>()?;
        verify_targets(self, projection, &frozen, &targets, cancellation)?;
        let bindings = databases::Databases::read_bindings(
            self,
            projection,
            &frozen,
            &materials,
            cancellation,
        )?;
        let replicas = installation::Replicas::read(self, &materials, cancellation)?;
        replicas.verify(
            &installation::Replicas::encode(&frozen, &bindings)?,
            cancellation,
        )?;
        self.read_file_history_move_joint_choice(projection, recovery, cancellation)?;
        let mut report = verify_targets(self, projection, &frozen, &targets, cancellation)?;
        report.file_database_intents = bindings.files.len();
        report.disk_database_intents = bindings.disks.len();
        Ok(report)
    }
}

fn verify_targets(
    engine: &Engine,
    projection: &FileHistoryMoveProjection,
    frozen: &FrozenCandidate,
    targets: &[store::Target],
    cancellation: &HistoryWindowCancellation,
) -> Result<FileHistoryMoveOrdinaryPreparation> {
    let mut copies = 0;
    let mut bytes = 0u64;
    for target in targets {
        target.verify_manifest()?;
        target.verify_container()?;
        for evidence in frozen.versions() {
            checkpoint(cancellation)?;
            target.verify_version(&evidence.version, cancellation)?;
            copies += 1;
            bytes = bytes
                .checked_add(evidence.version.logical_size)
                .ok_or_else(limit)?;
        }
        target.check_owner()?;
    }
    engine.recheck_file_history_move_projection(projection, cancellation)?;
    Ok(FileHistoryMoveOrdinaryPreparation {
        operation: projection.operation_id(),
        candidate_hash: frozen.hash().into(),
        target_disks: targets.len(),
        copies,
        bytes,
        file_database_intents: 0,
        disk_database_intents: 0,
    })
}
