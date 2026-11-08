//! Current-catalog maintenance follows authoritative publication. It cannot
//! change its receipt; failed projection writes remain separately observable.
use super::*;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Weak,
};

#[derive(Debug, error_definition::Error)]
#[error("disk catalog refresh failed at publication {sequence}: {source}")]
pub struct DiskCatalogFailure {
    sequence: u64,
    #[source]
    source: Error,
}
impl DiskCatalogFailure {
    pub fn sequence(&self) -> u64 {
        self.sequence
    }
    pub fn cause(&self) -> &Error {
        &self.source
    }
}

/// Maintenance facts, not physical-disk health or read admission. Installed
/// records when bytes were acknowledged; ordinary reads still verify those bytes.
#[derive(Clone, Debug)]
pub enum DiskCatalogMaintenanceState {
    Unbuilt,
    Excluded,
    Pending {
        last_materialized_sequence: Option<u64>,
    },
    Installed {
        materialized_sequence: u64,
        local_state_sequence: u64,
        records_hash: String,
    },
    Unavailable(Arc<DiskCatalogFailure>),
}

#[derive(Clone, Debug)]
pub struct DiskCatalogStatus {
    pub identity: DiskRegistrationIdentity,
    pub observed_sequence: u64,
    pub state: DiskCatalogMaintenanceState,
}

struct Entry {
    identity: DiskRegistrationIdentity,
    config: ValidatedConfig,
    state: Weak<State>,
    status: DiskCatalogMaintenanceState,
}

#[derive(Default)]
pub(in crate::engine) struct Maintenance {
    entries: BTreeMap<String, Entry>,
    pub(super) histories: history::Maintenance,
}

impl Entry {
    fn binding_matches(&self, identity: &DiskRegistrationIdentity, engine: &Engine) -> bool {
        self.identity == *identity && self.config.same_admission(&engine.config)
    }
    fn visible(&self, state: &Arc<State>) -> DiskCatalogMaintenanceState {
        if self.state.ptr_eq(&Arc::downgrade(state)) {
            return self.status.clone();
        }
        match &self.status {
            DiskCatalogMaintenanceState::Installed {
                materialized_sequence,
                ..
            } => DiskCatalogMaintenanceState::Pending {
                last_materialized_sequence: Some(*materialized_sequence),
            },
            // A previous error remains a previous error, with its own sequence.
            status => status.clone(),
        }
    }
}

impl Engine {
    /// No I/O or authority changes. A missed refresh cannot report Installed
    /// against a different State/configuration/physical registration.
    pub fn disk_catalog_statuses(&self) -> Vec<DiskCatalogStatus> {
        self.registered_disks()
            .map(|registered| {
                let identity = registered.identity();
                let state = if registered.registration_mode != RegistrationMode::Enabled {
                    DiskCatalogMaintenanceState::Excluded
                } else {
                    match self.catalog_maintenance.entries.get(&identity.slot_id) {
                        Some(entry) if entry.binding_matches(&identity, self) => {
                            entry.visible(&self.state)
                        }
                        _ => DiskCatalogMaintenanceState::Unbuilt,
                    }
                };
                DiskCatalogStatus {
                    identity,
                    observed_sequence: self.state.sequence,
                    state,
                }
            })
            .collect()
    }

    pub(super) fn record_catalog_install(&mut self, receipt: &DiskCatalogReceipt) {
        self.catalog_maintenance.entries.insert(
            receipt.identity.slot_id.clone(),
            Entry {
                identity: receipt.identity.clone(),
                config: self.config.clone(),
                state: Arc::downgrade(&self.state),
                status: DiskCatalogMaintenanceState::Installed {
                    materialized_sequence: receipt.source_sequence,
                    local_state_sequence: self.state.sequence,
                    records_hash: receipt.records_hash.clone(),
                },
            },
        );
    }

    /// Only normal post-publication/open paths call this. It does not run for
    /// unpublished historical replay slots or offline PreservePending admission.
    pub(in crate::engine) fn refresh_published_disk_catalogs(
        &mut self,
        previous: Option<&Arc<State>>,
    ) {
        let dirty = previous.and_then(|before| changed_bindings(before, &self.state));
        let targets: Vec<_> = self
            .registered_disks()
            .filter(|disk| disk.registration_mode == RegistrationMode::Enabled)
            .map(|disk| disk.identity())
            .collect();
        let current = Arc::downgrade(&self.state);
        for identity in targets {
            let unchanged = dirty.as_ref().is_some_and(|dirty| {
                !dirty.contains(&(identity.slot_id.clone(), identity.binding_generation))
            });
            let retained = self.catalog_maintenance.entries.get(&identity.slot_id);
            if unchanged
                && let Some(entry) = retained
                && entry.binding_matches(&identity, self)
            {
                // Failure is not retried for unrelated publications. A changed
                // local row, explicit rebuild or normal cold open retries it.
                if matches!(entry.status, DiskCatalogMaintenanceState::Unavailable(_)) {
                    continue;
                }
                if previous.is_some_and(|before| entry.state.ptr_eq(&Arc::downgrade(before)))
                    && matches!(entry.status, DiskCatalogMaintenanceState::Installed { .. })
                {
                    let entry = self
                        .catalog_maintenance
                        .entries
                        .get_mut(&identity.slot_id)
                        .expect(
                            "invariant: the same exclusive maintenance map entry remains present",
                        );
                    entry.state = current.clone();
                    if let DiskCatalogMaintenanceState::Installed {
                        local_state_sequence,
                        ..
                    } = &mut entry.status
                    {
                        *local_state_sequence = self.state.sequence;
                    }
                    continue;
                }
            }
            if let Err(source) = self.rebuild_disk_catalog(&identity.slot_id) {
                let failure = Arc::new(DiskCatalogFailure {
                    sequence: self.state.sequence,
                    source,
                });
                tracing::warn!(slot_id = %identity.slot_id, disk_id = %identity.disk_id,
                    error = ?failure, "published disk catalog unavailable; authoritative operation remains published");
                self.catalog_maintenance.entries.insert(
                    identity.slot_id.clone(),
                    Entry {
                        identity,
                        config: self.config.clone(),
                        state: current.clone(),
                        status: DiskCatalogMaintenanceState::Unavailable(failure),
                    },
                );
            }
        }
        // Drop observations of no-longer-registered bindings, never their files.
        let registered: BTreeSet<_> = self
            .registered_disks()
            .map(|disk| disk.slot_id.to_owned())
            .collect();
        self.catalog_maintenance
            .entries
            .retain(|slot, _| registered.contains(slot));
    }
}

/// Conservative write-set attribution uses both time points. Departing slots
/// are dirtied even when no current placement references them. Binding numbers
/// prevent a reused slot from inheriting an earlier physical disk's facts.
pub(super) fn changed_bindings(before: &State, after: &State) -> Option<BTreeSet<(String, u64)>> {
    macro_rules! keys {
        ($field:ident) => {
            match after.$field.changes_since(&before.$field) {
                MapChanges::Keys(keys) => keys.iter().cloned().collect::<BTreeSet<String>>(),
                MapChanges::All => return None,
            }
        };
    }
    let mut versions = keys!(versions);
    versions.extend(keys!(placements));
    versions.extend(keys!(content_states));
    let mut files = keys!(file_locations);
    files.extend(keys!(file_backup_paths));
    if !files.is_empty() {
        for state in [before, after] {
            versions.extend(
                state
                    .versions
                    .iter()
                    .filter(|(_, version)| files.contains(&version.file_id))
                    .map(|(id, _)| id.clone()),
            );
        }
    }
    let mut dirty = BTreeSet::new();
    for state in [before, after] {
        for id in &versions {
            if let Some(placement) = state.placements.get(id) {
                for slot in &placement.route_slot_ids {
                    let generation = placement
                        .binding_generations
                        .get(slot)
                        .expect("invariant: admitted placement binds every route slot");
                    dirty.insert((slot.clone(), *generation));
                }
            }
        }
    }
    Some(dirty)
}
