//! New Backup bindings begin at their one selected first publication. A
//! transient earlier arrival followed by a departure is not an empty past.
use super::*;
use crate::ValidatedConfig;
use crate::engine::backup_preparation::BackupPreparedTopologyNode;
use crate::engine::membership_transition::PreparedBackupFirstCandidate;

pub(in crate::engine) struct FreshBackupBindings(Vec<DiskRegistrationIdentity>);

impl FreshBackupBindings {
    pub(in crate::engine) fn selected(
        original: &ValidatedConfig,
        proposed: &[BackupPreparedTopologyNode],
    ) -> Result<Self> {
        if proposed.is_empty() || proposed.len() > 8 {
            return Err(invalid("Backup needs its whole selected batch of fresh bindings"));
        }
        if original.disks.len().saturating_add(proposed.len()) > MAX_DISKS {
            return Err(budget());
        }
        let mut fresh = Vec::<DiskRegistrationIdentity>::with_capacity(proposed.len());
        for node in proposed {
            let id = &node.identity;
            if id.pool_id != original.pool_id
                || id.binding_generation != 1
                || original.disks.iter()
                    .any(|old| old.disk_id == id.disk_id || old.slot_id == id.slot_id)
                || fresh.iter()
                    .any(|old| old.disk_id == id.disk_id || old.slot_id == id.slot_id)
            {
                return Err(invalid("selected Backup registrations repeat an original or one another"));
            }
            fresh.push(id.clone());
        }
        Ok(Self(fresh))
    }

    /// Only about the two DISK row families. This does not admit a FILE
    /// replica, an actual empty disk, an independent directory or a voter.
    pub(in crate::engine) fn require_absent(
        &self,
        step: &AdmittedState,
        stop: &AtomicBool,
    ) -> Result<()> {
        let state = step.state();
        let conflict = || invalid("a selected fresh Backup binding already has original DISK history");
        for placement in state.placements.values() {
            preparation_checkpoint(stop)?;
            if self.0.iter()
                .any(|id| crate::engine::disk_catalog::placement_at(placement, id))
            {
                return Err(conflict());
            }
        }
        if crate::protection::has_published_directories_at_any(
            state, &self.0, &|| preparation_checkpoint(stop),
        )? {
            return Err(conflict());
        }
        Ok(())
    }
}

/// Built by a target's *own* archive visitor. The original fixed registrations
/// and every selected fresh target are checked on each authenticated step.
pub(in crate::engine) struct BackupDiskScopes<'a> {
    prefix: PreparedDiskScopes,
    added: FreshBackupBindings,
    first: &'a PreparedBackupFirstCandidate,
}

impl<'a> BackupDiskScopes<'a> {
    pub(in crate::engine) fn new(
        archived: &ValidatedConfig,
        first: &'a PreparedBackupFirstCandidate,
        selected: &ValidatedConfig,
    ) -> Result<Self> {
        let original = first.original_config();
        let count = original.disks.len();
        let added = FreshBackupBindings::selected(original, first.targets())?;
        if storage_contract::canonical_json(&**archived)?
                != storage_contract::canonical_json(&**original)?
            || selected.pool_id != original.pool_id
            || selected.disks.len() != count + added.0.len()
            || storage_contract::canonical_json(&selected.disks[..count])?
                != storage_contract::canonical_json(&original.disks)?
            || selected.disks[count..].iter().zip(&added.0).any(|(disk, expected)| {
                crate::engine::fresh_layout::registration(selected, disk) != *expected
            })
        {
            return Err(invalid("published Backup disk scopes differ from the complete selected bindings"));
        }
        // Do not initialize from the portable Config on the decision: only
        // the original target archive carries the fixed genesis predecessor.
        archived.fixed_genesis()?;
        Ok(Self {
            prefix: PreparedDiskScopes::new(archived)?,
            added,
            first,
        })
    }

    pub(in crate::engine) fn observe(
        &mut self,
        candidate: &Candidate,
        hash: &str,
        step: &AdmittedState,
        stop: &AtomicBool,
    ) -> Result<()> {
        self.added.require_absent(step, stop)?;
        self.prefix.observe(candidate, hash, step, stop)
    }

    /// Reconstruct the original recipe after D3, using the actual source
    /// prefix, stable first request and predecessor only. The enclosing
    /// first Candidate/Snapshot digest is never an input to these bytes.
    pub(in crate::engine) fn finish(
        self,
        source: &AdmittedState,
        published: &AdmittedState,
        stop: &AtomicBool,
        mut encode: impl FnMut(&DiskCatalogSnapshot, &StoredHistory) -> Result<(Vec<u8>, Vec<u8>)>,
        mut store: impl FnMut(DiskRegistrationIdentity, Vec<u8>, Vec<u8>) -> Result<()>,
    ) -> Result<()> {
        let Self { mut prefix, first, added } = self;
        first.require_original_state(source)?;
        let predecessor = first.candidate().previous_snapshot.as_deref();
        let through = prefix.disks.first()
            .ok_or_else(|| invalid("Backup disk scopes have no original registration"))?
            .header
            .clone();
        if predecessor.is_none()
            || through.through_sequence != source.state().sequence
            || through.through_snapshot.as_deref() != predecessor
            || through.publications == 0
        {
            return Err(invalid("Backup DISK history lost its complete authenticated original prefix"));
        }
        // No source row was seen on any step for these exact registrations.
        // Carry only coverage, not some old disk's empty/retired inventory.
        for registration in added.0 {
            prefix.disks.push(DiskScope {
                header: DiskHistoryHeader { identity: registration.clone(), ..through.clone() },
                previous: DiskCatalogSnapshot::empty(registration),
                changes: Changes { rows: Vec::new(), bytes: 0 },
                record_bytes: 0,
            });
        }
        let pairs = prospective::freeze_from_original_rows(
            &prefix.config, prefix.disks, source.state(), published.state(), predecessor,
            first.result(), stop, |current, history| {
                let (a, b) = encode(current, history)?;
                preparation_checkpoint(stop)?;
                require_decoded_pair(current, history, &a, &b)?;
                let bytes = a.len().checked_add(b.len()).ok_or_else(budget)?;
                Ok(((current.identity.clone(), a, b), bytes))
            },
        )?;
        for (id, a, b) in pairs {
            preparation_checkpoint(stop)?;
            store(id, a, b)?;
        }
        Ok(())
    }
}
