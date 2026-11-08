//! Pure original-scope projections fed by an authenticated native prefix.
//! These scopes are not relabelled with a prepared replacement's identity.
use super::*;
use crate::history_package::{AdmittedState, preparation_checkpoint};
use crate::journal::Candidate;
use std::sync::atomic::AtomicBool;

const MAX_DISKS: usize = 32;
const MAX_SET_BYTES: usize = 64 * 1024 * 1024;

struct DiskScope {
    header: DiskHistoryHeader,
    previous: Vec<DiskCatalogVersion>,
    changes: Changes,
    record_bytes: usize,
}

pub(in crate::engine) struct PreparedDiskScopes {
    disks: Vec<DiskScope>,
}

impl PreparedDiskScopes {
    pub(in crate::engine) fn new(config: &crate::ValidatedConfig) -> Result<Self> {
        if config.disks.len() > MAX_DISKS || config.generation().is_some() {
            return Err(Error::new(
                ErrorCode::UnsupportedOperation,
                "prepared disk scopes require at most 32 fixed genesis bindings",
            ));
        }
        Ok(Self {
            disks: config
                .disks
                .iter()
                .map(|disk| DiskScope {
                    header: DiskHistoryHeader {
                        identity: DiskRegistrationIdentity {
                            pool_id: config.pool_id.clone(),
                            disk_id: disk.disk_id.clone(),
                            slot_id: disk.slot_id.clone(),
                            binding_generation: disk.binding_generation,
                        },
                        through_sequence: 0,
                        through_snapshot: None,
                        publications: 0,
                    },
                    previous: Vec::new(),
                    changes: Changes {
                        rows: Vec::new(),
                        bytes: 0,
                    },
                    record_bytes: 0,
                })
                .collect(),
        })
    }

    pub(in crate::engine) fn observe(
        &mut self,
        candidate: &Candidate,
        hash: &str,
        admitted: &AdmittedState,
        stop: &AtomicBool,
    ) -> Result<()> {
        let mut total = self
            .disks
            .iter()
            .map(|disk| disk.record_bytes)
            .sum::<usize>();
        for disk in &mut self.disks {
            preparation_checkpoint(stop)?;
            if candidate.pool_id != disk.header.identity.pool_id
                || candidate.sequence != disk.header.through_sequence + 1
                || candidate.previous_snapshot != disk.header.through_snapshot
                || admitted.state().sequence != candidate.sequence
                || disk.header.publications as usize == MAX_PUBLICATIONS
            {
                return Err(invalid(
                    "prepared disk scopes do not extend their exact genesis prefix",
                ));
            }
            let next = snapshot(admitted.state(), disk.header.identity.clone())?;
            disk.changes
                .append(candidate, hash, &disk.previous, &next.versions)?;
            let current_bytes = crate::disk::encode_metadata(&next, MAX_RECORD_BYTES)?.len();
            let replacement = disk
                .changes
                .bytes
                .checked_add(current_bytes)
                .ok_or_else(budget)?;
            total = (total - disk.record_bytes)
                .checked_add(replacement)
                .ok_or_else(budget)?;
            if total > MAX_SET_BYTES {
                return Err(budget());
            }
            disk.record_bytes = replacement;
            disk.previous = next.versions;
            disk.header.through_sequence = candidate.sequence;
            disk.header.through_snapshot = Some(candidate.snapshot_hash.clone());
            disk.header.publications += 1;
        }
        Ok(())
    }

    /// Consume one scope at a time so serializing the bounded final byte set
    /// does not retain an additional copy of all historical row collections.
    pub(in crate::engine) fn finish(
        self,
        admitted: &AdmittedState,
        stop: &AtomicBool,
        mut store: impl FnMut(DiskRegistrationIdentity, Vec<u8>, Vec<u8>) -> Result<()>,
    ) -> Result<()> {
        for disk in self.disks {
            preparation_checkpoint(stop)?;
            let current = snapshot(admitted.state(), disk.header.identity.clone())?;
            if disk.header.through_sequence != admitted.state().sequence
                || current.versions != disk.previous
            {
                return Err(invalid(
                    "prepared disk prefix differs from the admitted frozen State",
                ));
            }
            let history = DiskHistorySnapshot {
                header: disk.header,
                changes: disk.changes.rows,
            };
            let current_hash = storage_contract::object_hash("disk-current-catalog", &current)?;
            let current_bytes = super::super::database::encode(&current, &current_hash)?;
            let history_bytes = database::encode(&history, &digest(&history)?)?;
            if super::super::database::read(&current_bytes, &current.identity)? != current
                || database::read(&history_bytes, &current.identity)? != history
            {
                return Err(invalid(
                    "prepared disk databases changed their exact source scopes",
                ));
            }
            store(current.identity, current_bytes, history_bytes)?;
        }
        Ok(())
    }
}
