//! A bounded-scope physical read projection for the verified publication cursor.
//! It never supplies a replacement State or install authority. A separately
//! verified finalized tail can select only its exact two recovery locations.
use crate::engine::fresh_layout::{FreshLayoutError as Failure, containers};
use crate::journal::{Candidate, Publication, PublicationSeal, verify_publication_seal};
use crate::publication_evidence::AdmittedSnapshot;
use crate::{Result, State};
use name_policy::LookupName;
use std::collections::BTreeMap;
use storage_contract::{DiskId, FileId, HistoryTreeId, OperationId};
use storage_contract::file_container::{ContainerNamespace, ContainerReplica, FileContainerRecord};

#[derive(Default)]
pub(in crate::engine) struct ReplayContainerLocations {
    pending: BTreeMap<FileId, finalized::PendingName>,
    files: BTreeMap<FileId, FileLineage>,
}

mod finalized;

pub(in crate::engine) enum ReplayName<'a> {
    Published(&'a LookupName),
    Finalized {
        source: &'a LookupName,
        target: &'a LookupName,
        sequence: storage_contract::Sequence,
    },
}

struct FileLineage {
    history: HistoryTreeId,
    owner: DiskId,
    created_by: OperationId,
    namespace: ContainerNamespace,
    replicas: BTreeMap<DiskId, ReplicaLineage>,
}

struct ReplicaLineage {
    occurrence: ContainerReplica,
    original_name: LookupName,
    names: BTreeMap<u64, LookupName>,
}

impl FileLineage {
    fn require_occurrence(&self, record: &FileContainerRecord) -> Result<()> {
        let namespace = &record.namespace;
        if record.history_tree_id != self.history
            || record.owner_disk != self.owner
            || record.created_by != self.created_by
            || namespace.repository != self.namespace.repository
            || namespace.branch != self.namespace.branch
            || namespace.entry != self.namespace.entry
            || namespace.parent != self.namespace.parent
        {
            return Err(Failure::ContainerScope.into());
        }
        Ok(())
    }
}

fn has_name_change(previous: &State, next: &State) -> bool {
    let changed = |file: &FileId| {
        previous.file_containers.get(file).zip(next.file_containers.get(file))
            .is_some_and(|(old, new)| old.namespace != new.namespace)
    };
    match next.file_containers.changes_since(&previous.file_containers) {
        storage_contract::MapChanges::Keys(keys) => keys.iter().any(changed),
        storage_contract::MapChanges::All => next.file_containers.keys().any(changed),
    }
}

impl ReplayContainerLocations {
    /// Only call on the cursor's next original candidate after its actual
    /// held publication barrier succeeds. The snapshot and seal do not grant
    /// permission to select a different predecessor or to install material.
    pub(super) fn admit_published(
        &mut self,
        config: &crate::ValidatedConfig,
        previous: &State,
        original: &AdmittedSnapshot,
        candidate: &Candidate,
        publication: &Publication,
        seal: &PublicationSeal,
    ) -> Result<()> {
        let next = original.for_candidate(candidate)?.state();
        let result = next.operations.get(&candidate.operation_id).ok_or(Failure::ContainerScope)?;
        let transition = has_name_change(previous, next)
            .then(|| containers::verify_transition(config, previous, next, candidate)).transpose()?;
        let empty = BTreeMap::new();
        let changes = transition.as_ref().map_or(&empty, |proof| proof.renames());

        // A later publication cannot detach a tracked inode or silently change
        // its namespace while the scanner is projecting historical reads.
        let require_carry = |file: &FileId, lineage: &FileLineage| -> Result<()> {
            let before = previous.file_containers.get(file).ok_or(Failure::ContainerScope)?;
            let after = next.file_containers.get(file).ok_or(Failure::ContainerScope)?;
            if before == after {
                return Ok(());
            }
            lineage.require_occurrence(before)?;
            lineage.require_occurrence(after)?;
            let destination = match changes.get(file) {
                Some(change) => change.target(),
                None => &lineage.namespace,
            };
            if before.namespace != lineage.namespace || &after.namespace != destination {
                return Err(Failure::ContainerScope.into());
            }
            for (disk, replica) in &lineage.replicas {
                if before.replicas.get(disk) != Some(&replica.occurrence)
                    || after.replicas.get(disk) != Some(&replica.occurrence)
                {
                    return Err(Failure::ContainerIdentity.into());
                }
            }
            Ok(())
        };
        match next.file_containers.changes_since(&previous.file_containers) {
            storage_contract::MapChanges::Keys(keys) => {
                for file in keys.iter() {
                    if let Some(lineage) = self.files.get(file) {
                        require_carry(file, lineage)?;
                    }
                }
            }
            storage_contract::MapChanges::All => {
                // Independently decoded maps cannot claim a trusted empty delta.
                for (file, lineage) in &self.files {
                    require_carry(file, lineage)?;
                }
            }
        }

        if changes.is_empty() {
            return Ok(());
        }
        if publication.recovery.is_some()
            || &publication.result != result
            || !candidate.versions.is_empty()
            || candidate.sequence != next.sequence
            || previous.sequence >= next.sequence
        {
            return Err(Failure::ContainerScope.into());
        }
        verify_publication_seal(candidate, publication, seal)?;
        for (file, change) in changes {
            let before = previous.file_containers.get(file).ok_or(Failure::ContainerScope)?;
            let after = next.file_containers.get(file).ok_or(Failure::ContainerScope)?;
            let mut expected = before.clone();
            expected.namespace = change.target().clone();
            if expected != *after || &before.namespace.name != change.source() {
                return Err(Failure::ContainerScope.into());
            }
        }

        for (file, change) in changes {
            let before = &previous.file_containers[file];
            let lineage = self.files.entry(*file).or_insert_with(|| FileLineage {
                history: before.history_tree_id,
                owner: before.owner_disk,
                created_by: before.created_by,
                namespace: before.namespace.clone(),
                replicas: BTreeMap::new(),
            });
            for (disk, occurrence) in &before.replicas {
                let replica = lineage.replicas.entry(*disk).or_insert_with(|| ReplicaLineage {
                    occurrence: occurrence.clone(),
                    original_name: before.namespace.name.clone(),
                    names: BTreeMap::new(),
                });
                replica.names.insert(next.sequence, change.target().name.clone());
            }
            lineage.namespace = change.target().clone();
        }
        Ok(())
    }

    pub(in crate::engine) fn historical_location(
        &self, state: &State, file: FileId, disk: DiskId,
    ) -> Result<Option<ReplayName<'_>>> {
        let Some(pending) = self.pending.get(&file) else {
            return self.historical_name(state, file, disk)
                .map(|name| name.map(ReplayName::Published));
        };
        let record = state.file_containers.get(&file).ok_or(Failure::ContainerScope)?;
        let mut expected = pending.before.clone();
        // Working/material data can legitimately differ in an older prefix.
        // Only the location identity and that original replica select a path.
        expected.working = record.working.clone();
        expected.replicas = record.replicas.clone();
        if state.sequence == pending.sequence {
            expected.namespace = pending.after.clone();
        } else if state.sequence < pending.sequence {
            if let Some(name) = self.historical_name(state, file, disk)? {
                expected.namespace.name = record.namespace.name.clone();
                if name != &pending.before.namespace.name {
                    return Err(Failure::ContainerScope.into());
                }
            }
        } else {
            return Err(Failure::ContainerScope.into());
        }
        if expected != *record
            || record.replicas.get(&disk) != pending.before.replicas.get(&disk)
            || !record.replicas.contains_key(&disk)
        {
            return Err(Failure::ContainerIdentity.into());
        }
        if pending.published && state.sequence == pending.sequence {
            return Ok(Some(ReplayName::Published(&pending.after.name)));
        }
        Ok(Some(ReplayName::Finalized {
            source: &pending.before.namespace.name,
            target: &pending.after.name,
            sequence: storage_contract::Sequence::new(pending.sequence),
        }))
    }

    pub(in crate::engine) fn requires_complete_history(&self, state: &State, file: FileId) -> bool {
        self.pending.get(&file).is_some_and(|pending| {
            pending.published && pending.sequence == state.sequence
        })
    }

    /// Resolve only an originally recorded occurrence of this exact replay
    /// state. Later replicas cannot satisfy publication evidence on an earlier
    /// disk. The caller must still open the selected name under the held parent
    /// and match the original container inode and all durable parent bindings.
    pub(in crate::engine) fn historical_name(
        &self,
        state: &State,
        file: FileId,
        disk: DiskId,
    ) -> Result<Option<&LookupName>> {
        let Some(lineage) = self.files.get(&file) else {
            return Ok(None);
        };
        let Some(replica) = lineage.replicas.get(&disk) else {
            return Ok(None);
        };
        let record = state.file_containers.get(&file).ok_or(Failure::ContainerScope)?;
        lineage.require_occurrence(record)?;
        if record.replicas.get(&disk) != Some(&replica.occurrence) {
            return Err(Failure::ContainerIdentity.into());
        }
        let original_name = match replica.names.range(..=state.sequence).next_back() {
            Some((_, observed)) => observed,
            None => &replica.original_name,
        };
        if &record.namespace.name != original_name {
            return Err(Failure::ContainerScope.into());
        }
        Ok((original_name != &lineage.namespace.name).then_some(&lineage.namespace.name))
    }
}
