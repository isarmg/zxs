//! A complete genesis replay establishes a trusted prefix. Normal publication
//! extends that prefix from the exact two admitted States; disk bytes alone
//! never establish continuity. Only small headers/digests remain in memory.
use super::*;
use crate::journal::Candidate;
use std::{collections::BTreeSet, sync::Weak};

#[derive(Debug, error_definition::Error)]
#[error("disk history maintenance failed at publication {sequence}: {source}")]
pub struct DiskHistoryFailure {
    sequence: u64,
    #[source]
    source: Error,
}
impl DiskHistoryFailure {
    pub fn sequence(&self) -> u64 {
        self.sequence
    }
    pub fn cause(&self) -> &Error {
        &self.source
    }
}

/// Process-local maintenance facts, not read admission or physical disk health.
/// Unrelated publications can advance observed coverage without changing the
/// persisted header; both boundaries must remain visible to consumers.
#[derive(Clone, Debug)]
pub enum DiskHistoryMaintenanceState {
    Unbuilt,
    Excluded,
    Pending {
        persisted_through_sequence: Option<u64>,
    },
    Tracked {
        persisted_through_sequence: u64,
        observed_through_sequence: u64,
        observed_publications: u32,
        records_hash: String,
    },
    Unavailable(Arc<DiskHistoryFailure>),
}

#[derive(Clone, Debug)]
pub struct DiskHistoryStatus {
    pub identity: DiskRegistrationIdentity,
    pub observed_sequence: u64,
    pub state: DiskHistoryMaintenanceState,
}

/// The persisted digest authenticates the complete row prefix against our
/// earlier source replay/install. Coverage may include additional publications
/// proven not to change this disk's rows. It is never loaded from SQLite.
#[derive(Clone)]
struct Coverage {
    persisted: DiskHistoryHeader,
    observed: DiskHistoryHeader,
    records_hash: String,
}

enum Availability {
    Tracked(Coverage),
    Unavailable(Arc<DiskHistoryFailure>),
}

struct Entry {
    identity: DiskRegistrationIdentity,
    config: ValidatedConfig,
    state: Weak<State>,
    availability: Availability,
}
impl Entry {
    fn binding_matches(&self, identity: &DiskRegistrationIdentity, engine: &Engine) -> bool {
        self.identity == *identity && self.config.same_admission(&engine.config)
    }

    fn visible(&self, state: &Arc<State>) -> DiskHistoryMaintenanceState {
        match &self.availability {
            Availability::Unavailable(failure) => {
                DiskHistoryMaintenanceState::Unavailable(Arc::clone(failure))
            }
            Availability::Tracked(coverage) if self.state.ptr_eq(&Arc::downgrade(state)) => {
                DiskHistoryMaintenanceState::Tracked {
                    persisted_through_sequence: coverage.persisted.through_sequence,
                    observed_through_sequence: coverage.observed.through_sequence,
                    observed_publications: coverage.observed.publications,
                    records_hash: coverage.records_hash.clone(),
                }
            }
            Availability::Tracked(coverage) => DiskHistoryMaintenanceState::Pending {
                persisted_through_sequence: Some(coverage.persisted.through_sequence),
            },
        }
    }
}

#[derive(Default)]
pub(in crate::engine::disk_catalog) struct Maintenance {
    entries: BTreeMap<String, Entry>,
}

impl Engine {
    /// No I/O. Tracked is an in-process provenance observation, not permission
    /// to trust a disk or claim its persisted header reached the observed head.
    pub fn disk_history_statuses(&self) -> Vec<DiskHistoryStatus> {
        self.registered_disks()
            .map(|registered| {
                let identity = registered.identity();
                let state = if registered.registration_mode != RegistrationMode::Enabled {
                    DiskHistoryMaintenanceState::Excluded
                } else {
                    match self
                        .catalog_maintenance
                        .histories
                        .entries
                        .get(&identity.slot_id)
                    {
                        Some(entry) if entry.binding_matches(&identity, self) => {
                            entry.visible(&self.state)
                        }
                        _ => DiskHistoryMaintenanceState::Unbuilt,
                    }
                };
                DiskHistoryStatus {
                    identity,
                    observed_sequence: self.state.sequence,
                    state,
                }
            })
            .collect()
    }

    pub(super) fn forget_disk_history(&mut self, slot: &str) {
        self.catalog_maintenance.histories.entries.remove(slot);
    }

    pub(super) fn record_history_install(&mut self, receipt: &DiskHistoryReceipt) {
        self.record_history_coverage(Coverage {
            persisted: receipt.header.clone(),
            observed: receipt.header.clone(),
            records_hash: receipt.records_hash.clone(),
        });
    }

    fn record_history_coverage(&mut self, coverage: Coverage) {
        let identity = coverage.observed.identity.clone();
        self.catalog_maintenance.histories.entries.insert(
            identity.slot_id.clone(),
            Entry {
                identity,
                config: self.config.clone(),
                state: Arc::downgrade(&self.state),
                availability: Availability::Tracked(coverage),
            },
        );
    }

    fn record_history_failure(&mut self, identity: DiskRegistrationIdentity, source: Error) {
        let failure = Arc::new(DiskHistoryFailure {
            sequence: self.state.sequence,
            source,
        });
        tracing::warn!(slot_id = %identity.slot_id, disk_id = %identity.disk_id,
            error = ?failure, "disk history unavailable; authoritative publication remains unchanged");
        self.catalog_maintenance.histories.entries.insert(
            identity.slot_id.clone(),
            Entry {
                identity,
                config: self.config.clone(),
                state: Arc::downgrade(&self.state),
                availability: Availability::Unavailable(failure),
            },
        );
    }

    fn disk_history_targets(&self) -> Vec<DiskRegistrationIdentity> {
        self.registered_disks()
            .filter(|disk| disk.registration_mode == RegistrationMode::Enabled)
            .map(|disk| disk.identity())
            .collect()
    }

    /// Normal open only, after authoritative recovery has finished. Never run
    /// per replay prefix, during first initialization, or for PreservePending.
    pub(in crate::engine) fn restore_published_disk_histories(&mut self) {
        for identity in self.disk_history_targets() {
            if let Err(source) = self.rebuild_disk_history(&identity.slot_id) {
                self.record_history_failure(identity, source);
            }
        }
    }

    /// Only a completed ordinary publication supplies these internal inputs.
    /// Missing/stale proof requires full source reconstruction, not adopting a
    /// convenient SQLite prefix or deriving a prefix from the final placement.
    pub(in crate::engine) fn advance_published_disk_histories(
        &mut self,
        previous: &Arc<State>,
        candidate: &Candidate,
        candidate_hash: &str,
    ) {
        let dirty = super::super::maintenance::changed_bindings(previous, &self.state);
        for identity in self.disk_history_targets() {
            let unchanged = dirty.as_ref().is_some_and(|dirty| {
                !dirty.contains(&(identity.slot_id.clone(), identity.binding_generation))
            });
            let entry = self
                .catalog_maintenance
                .histories
                .entries
                .get(&identity.slot_id);
            let matching = entry.filter(|entry| entry.binding_matches(&identity, self));
            if unchanged
                && matching
                    .is_some_and(|entry| matches!(entry.availability, Availability::Unavailable(_)))
            {
                continue; // explicit rebuild/cold open/local change retries
            }
            let coverage = matching.and_then(|entry| match &entry.availability {
                Availability::Tracked(coverage)
                    if entry.state.ptr_eq(&Arc::downgrade(previous)) =>
                {
                    Some(coverage.clone())
                }
                _ => None,
            });
            let result = match coverage {
                Some(coverage) => self.extend_disk_history(
                    previous,
                    candidate,
                    candidate_hash,
                    coverage,
                    unchanged,
                ),
                None => self.rebuild_disk_history(&identity.slot_id).map(|_| ()),
            };
            if let Err(source) = result {
                self.record_history_failure(identity, source);
            }
        }
        let registered: BTreeSet<_> = self
            .registered_disks()
            .map(|disk| disk.slot_id.to_owned())
            .collect();
        self.catalog_maintenance
            .histories
            .entries
            .retain(|slot, _| registered.contains(slot));
    }

    fn extend_disk_history(
        &mut self,
        previous: &State,
        candidate: &Candidate,
        candidate_hash: &str,
        mut coverage: Coverage,
        unchanged: bool,
    ) -> Result<()> {
        let prior = &coverage.observed;
        if self.config.generation().is_some()
            || prior.through_sequence != previous.sequence
            || candidate.previous_snapshot != prior.through_snapshot
            || candidate.sequence <= prior.through_sequence
            || candidate.sequence != self.state.sequence
            || self.snapshot.as_ref() != Some(&candidate.snapshot_hash)
            || candidate.pool_id != prior.identity.pool_id
        {
            return Err(invalid(
                "disk history continuation does not match the admitted publication",
            ));
        }
        if prior.publications as usize >= MAX_PUBLICATIONS {
            return Err(budget());
        }
        coverage.observed.through_sequence = candidate.sequence;
        coverage.observed.through_snapshot = Some(candidate.snapshot_hash.clone());
        coverage.observed.publications += 1;
        if unchanged {
            self.record_history_coverage(coverage);
            return Ok(());
        }
        let identity = &coverage.observed.identity;
        let before = snapshot(previous, identity.clone())?;
        let after = snapshot(&self.state, identity.clone())?;
        if before.same_rows(&after) {
            self.record_history_coverage(coverage);
            return Ok(());
        }
        let retained = self.read_tracked_disk_history(&coverage)?;
        let mut changes = Changes::retained(retained.changes)?;
        changes.append(candidate, candidate_hash, &before, &after)?;
        let slot = identity.slot_id.clone();
        let captured = DiskHistorySnapshot {
            header: coverage.observed,
            changes: changes.rows,
        };
        let receipt = self.install_disk_history(&slot, captured)?;
        self.record_history_install(&receipt);
        Ok(())
    }

    fn read_tracked_disk_history(&self, coverage: &Coverage) -> Result<DiskHistorySnapshot> {
        let identity = &coverage.persisted.identity;
        let (disk, current, handle) = self.catalog_disk(&identity.slot_id)?;
        if current != *identity {
            return Err(invalid(
                "disk history continuation changed physical registration",
            ));
        }
        let lease = handle.root.admit_io()?;
        let named = validate_seal_observed(&self.config, &disk, &mut |_| lease.check_running())?;
        validate_stored_seal(&handle.seal, &named)?;
        let bytes = handle.root.read_bytes(
            &handle.root.path().join(DISK_HISTORY_DATABASE_NAME),
            MAX_DATABASE_BYTES,
        )?;
        let stored = database::read(&bytes, identity)?;
        if stored.header != coverage.persisted || digest(&stored)? != coverage.records_hash {
            return Err(invalid(
                "disk history bytes differ from the previously authenticated prefix",
            ));
        }
        let named = validate_seal_observed(&self.config, &disk, &mut |_| lease.check_running())?;
        validate_stored_seal(&handle.seal, &named)?;
        lease.check_running()?;
        Ok(stored)
    }
}
