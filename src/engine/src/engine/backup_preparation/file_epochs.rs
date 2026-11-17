//! The last ordinary FILE write selected by each original publication, per
//! registered occurrence. The held SQLite header is an input to check, never
//! a way to choose the publication. This fold carries no physical byte proof.
use crate::{
    Result, State,
    engine::{
        fresh_layout::FreshLayoutError as Failure,
        membership_transition::PreparedBackupFirstCandidate,
    },
    history_package::{AdmittedState, preparation_checkpoint},
    journal::Candidate,
    state_projection::ProjectionSnapshot,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::atomic::AtomicBool,
};
use storage_contract::{
    ContentHash, DiskId, FileId, HistoryTreeId, MapChanges, file_container::ContainerReplica,
    working_tree::MaterialCommitment,
};

fn row_root(value: &str) -> Result<MaterialCommitment> {
    let digest = value
        .parse::<ContentHash>()
        .map_err(|_| Failure::BackupLayoutEvidence)?;
    Ok(MaterialCommitment::from_digest(digest.digest()))
}

const MAX_OCCURRENCES: usize = 65_536;

struct Epoch {
    rows: MaterialCommitment,
    history: HistoryTreeId,
    sequence: u64,
    // Includes the actual original root, exclusive preparation, inode and
    // complete registered source-prefix chain, not a reusable slot or name.
    holder: blake3::Hash,
}

fn holder(replica: &ContainerReplica) -> Result<blake3::Hash> {
    Ok(storage_contract::canonical_object_digest(
        "backup-original-file-holder",
        replica,
    )?)
}

pub(super) struct FileEpochReplay {
    original_pool: String,
    previous: State,
    snapshot: Option<String>,
    epochs: BTreeMap<(FileId, DiskId), Epoch>,
}
impl FileEpochReplay {
    pub(super) fn new(first: &PreparedBackupFirstCandidate) -> Self {
        Self {
            original_pool: first.original_config().pool_id.clone(),
            previous: State::default(),
            snapshot: None,
            epochs: BTreeMap::new(),
        }
    }

    // Only archive/startup callers feed these steps, after native seal and
    // receipt verification. An incomplete iteration never produces a witness.
    pub(super) fn observe(
        &mut self,
        candidate: &Candidate,
        admitted: &AdmittedState,
        stop: &AtomicBool,
    ) -> Result<()> {
        preparation_checkpoint(stop)?;
        let next = admitted.state();
        if candidate.pool_id != self.original_pool
            || candidate.generation().is_some()
            || candidate.registry().is_some()
            || self.previous.sequence.checked_add(1) != Some(candidate.sequence)
            || candidate.previous_snapshot != self.snapshot
            || next.sequence != candidate.sequence
        {
            return Err(Failure::BackupLayoutEvidence.into());
        }
        // This consumer supports the normal single-history FILE store, not a
        // shared directory layout's extra image rewrites. Lose the witness,
        // not the material; a subsequent proved ordinary writer can regain it.
        if !next.directory_git_repositories.is_empty()
            || !self.previous.directory_git_repositories.is_empty()
        {
            self.epochs.clear();
        } else {
            self.observe_files(next, stop)?;
        }
        self.previous = next.clone();
        self.snapshot = Some(candidate.snapshot_hash.clone());
        Ok(())
    }

    fn observe_files(&mut self, next: &State, stop: &AtomicBool) -> Result<()> {
        let regular = crate::container_storage::affected_file_material(&self.previous, next)
            .into_iter()
            .map(|id| id.parse::<FileId>())
            .collect::<std::result::Result<BTreeSet<_>, _>>()?;
        let mut inspect = regular.clone();
        let changed = |id: &FileId| {
            self.previous
                .file_containers
                .get(id)
                .map(|file| &file.replicas)
                != next.file_containers.get(id).map(|file| &file.replicas)
        };
        match next
            .file_containers
            .changes_since(&self.previous.file_containers)
        {
            MapChanges::Keys(keys) => inspect.extend(keys.iter().filter(|id| changed(id)).copied()),
            MapChanges::All => inspect.extend(
                self.previous
                    .file_containers
                    .keys()
                    .chain(next.file_containers.keys())
                    .filter(|id| changed(id))
                    .copied(),
            ),
        }
        if inspect.is_empty() {
            return Ok(());
        }
        let projection = ProjectionSnapshot::new(next);
        for file in inspect {
            preparation_checkpoint(stop)?;
            let before = self.previous.file_containers.get(&file);
            let after = next.file_containers.get(&file);
            if let Some(before) = before {
                for disk in before.replicas.keys() {
                    if !after.is_some_and(|current| current.replicas.contains_key(disk)) {
                        // No current observation; specifically NOT proof of retirement.
                        self.epochs.remove(&(file, *disk));
                    }
                }
            }
            let Some(after) = after else { continue };
            let mut rows = None;
            for (disk, occurrence) in &after.replicas {
                if !regular.contains(&file)
                    && before.and_then(|old| old.replicas.get(disk)) == Some(occurrence)
                {
                    continue;
                }
                preparation_checkpoint(stop)?;
                if !self.epochs.contains_key(&(file, *disk)) && self.epochs.len() >= MAX_OCCURRENCES
                {
                    return Err(Failure::ContainerBudget.into());
                }
                let hash = match rows {
                    Some(hash) => hash,
                    None => {
                        let captured =
                            crate::container_storage::file_database::capture_projected_copy(
                                projection.view(),
                                &file.to_string(),
                                &after.history_tree_id.to_string(),
                            )?;
                        let value = row_root(&captured.records_hash)?;
                        rows = Some(value);
                        value
                    }
                };
                self.epochs.insert(
                    (file, *disk),
                    Epoch {
                        rows: hash,
                        history: after.history_tree_id,
                        sequence: next.sequence,
                        holder: holder(occurrence)?,
                    },
                );
            }
        }
        Ok(())
    }

    pub(super) fn finish(self, first: &PreparedBackupFirstCandidate) -> Result<OriginalFileEpochs> {
        if first.original_config().pool_id != self.original_pool
            || self.snapshot.is_none()
            || self.snapshot != first.candidate().previous_snapshot
            || self.previous.sequence >= first.sequence()
        {
            return Err(Failure::BackupLayoutEvidence.into());
        }
        Ok(OriginalFileEpochs {
            first: first.hash().to_owned(),
            through: self.previous.sequence,
            epochs: self.epochs,
        })
    }
}

// Constructible only after completing the original native archive fold.
// No installer authority, release flag or persistent cache is contained here.
pub(in crate::engine) struct OriginalFileEpochs {
    first: String,
    through: u64,
    epochs: BTreeMap<(FileId, DiskId), Epoch>,
}
impl OriginalFileEpochs {
    pub(in crate::engine) fn require_source(
        &self,
        first: &PreparedBackupFirstCandidate,
        original: &AdmittedState,
    ) -> Result<()> {
        if first.hash() != self.first || original.state().sequence != self.through {
            return Err(Failure::BackupLayoutEvidence.into());
        }
        Ok(())
    }

    pub(in crate::engine) fn require_file(
        &self,
        original: &AdmittedState,
        disk: DiskId,
        file: FileId,
        sequence: u64,
        rows: &str,
    ) -> Result<()> {
        let epoch = self
            .epochs
            .get(&(file, disk))
            .ok_or(Failure::BackupLayoutEvidence)?;
        let source = original
            .state()
            .file_containers
            .get(&file)
            .ok_or(Failure::BackupLayoutEvidence)?;
        let replica = source
            .replicas
            .get(&disk)
            .ok_or(Failure::BackupLayoutEvidence)?;
        if sequence != epoch.sequence
            || row_root(rows)? != epoch.rows
            || source.history_tree_id != epoch.history
            || holder(replica)? != epoch.holder
        {
            return Err(Failure::BackupLayoutEvidence.into());
        }
        Ok(())
    }
}
