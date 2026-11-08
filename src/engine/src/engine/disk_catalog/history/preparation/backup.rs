//! Append one authenticated Backup publication without relabelling old history.
use super::*;
use crate::engine::membership_transition::PreparedBackupFirstCandidate;

impl PreparedDiskScopes {
    /// The source prefix has already been independently replayed on this target.
    /// New bindings have no predecessor rows: their first arrivals belong only
    /// to the actual Backup publication, never to the source disk's past.
    pub(in crate::engine) fn extend_backup(
        &mut self,
        first: &PreparedBackupFirstCandidate,
        config: &crate::ValidatedConfig,
        source: &AdmittedState,
        published: &AdmittedState,
        stop: &AtomicBool,
    ) -> Result<()> {
        let original = first.original_config();
        let retained = original.disks.len();
        if self.disks.len() != retained
            || config.pool_id != original.pool_id
            || config.disks.len() <= retained
            || config.disks.len() > retained + 8
            || storage_contract::canonical_json(&config.disks[..retained])?
                != storage_contract::canonical_json(&original.disks)?
        {
            return Err(invalid(
                "published disk scopes change the original fixed bindings",
            ));
        }
        let candidate = first.candidate();
        for (scope, disk) in self.disks.iter().zip(&original.disks) {
            preparation_checkpoint(stop)?;
            if scope.header.identity != identity(original, disk)
                || scope.header.through_sequence != source.state().sequence
                || scope.header.through_snapshot != candidate.previous_snapshot
            {
                return Err(invalid(
                    "published disk scopes lost their exact original prefix",
                ));
            }
        }
        let boundary = self
            .disks
            .first()
            .ok_or_else(|| invalid("Backup disk scopes have no genesis binding"))?
            .header
            .clone();
        for disk in &config.disks[retained..] {
            preparation_checkpoint(stop)?;
            let identity = identity(config, disk);
            // The effective Config is append-only and validated. Still require
            // the exact source State to have no data for any fresh binding.
            if disk.binding_generation != 1
                || original
                    .disks
                    .iter()
                    .any(|old| old.slot_id == disk.slot_id || old.disk_id == disk.disk_id)
                || !snapshot(source.state(), identity.clone())?.is_empty()
            {
                return Err(invalid(
                    "new Backup binding already owns source data or history",
                ));
            }
            self.disks.push(DiskScope {
                header: DiskHistoryHeader {
                    identity: identity.clone(),
                    through_sequence: boundary.through_sequence,
                    through_snapshot: boundary.through_snapshot.clone(),
                    publications: boundary.publications,
                },
                previous: DiskCatalogSnapshot::empty(identity),
                changes: Changes {
                    rows: Vec::new(),
                    bytes: 0,
                },
                record_bytes: 0,
            });
        }
        // One common actual Candidate extends every scope, including the old
        // terminal's departures and each new target's first independent copy.
        self.observe(candidate, first.hash(), published, stop)
    }
}

fn identity(config: &crate::ValidatedConfig, disk: &DiskConfig) -> DiskRegistrationIdentity {
    DiskRegistrationIdentity {
        pool_id: config.pool_id.clone(),
        disk_id: disk.disk_id.clone(),
        slot_id: disk.slot_id.clone(),
        binding_generation: disk.binding_generation,
    }
}
