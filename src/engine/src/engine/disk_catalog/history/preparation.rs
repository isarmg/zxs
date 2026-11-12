//! Pure original-scope projections fed by an authenticated native prefix.
//! These scopes are not relabelled with a prepared replacement's identity.
use super::*;
use crate::history_package::{AdmittedState, preparation_checkpoint};
use crate::journal::Candidate;
use std::sync::atomic::AtomicBool;

mod backup;

const MAX_DISKS: usize = 32;
const MAX_SET_BYTES: usize = 64 * 1024 * 1024;

struct DiskScope {
    header: DiskHistoryHeader,
    previous: DiskCatalogSnapshot,
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
        Ok(Self::from_disks(&config.pool_id, config.disks.iter()))
    }

    pub(in crate::engine) fn new_for_move(engine: &Engine) -> Result<Self> {
        engine.require_native_move_authority()?;
        let enabled = engine
            .registered_disks()
            .filter(|disk| disk.registration_mode == crate::RegistrationMode::Enabled)
            .map(|disk| disk.identity())
            .collect::<Vec<_>>();
        if enabled.len() > MAX_DISKS || enabled.is_empty() {
            return Err(Error::new(
                ErrorCode::ResourceLimit,
                "move has no bounded enabled disk scopes",
            ));
        }
        Ok(Self::from_disks(
            &engine.config.pool_id,
            engine.config.disks.iter().filter(|disk| {
                enabled.iter().any(|identity| {
                    identity.slot_id == disk.slot_id
                        && identity.disk_id == disk.disk_id
                        && identity.binding_generation == disk.binding_generation
                })
            }),
        ))
    }

    fn from_disks<'a>(pool: &str, disks: impl Iterator<Item = &'a crate::DiskConfig>) -> Self {
        Self {
            disks: disks
                .map(|disk| DiskScope {
                    header: DiskHistoryHeader {
                        identity: DiskRegistrationIdentity {
                            pool_id: pool.into(),
                            disk_id: disk.disk_id.clone(),
                            slot_id: disk.slot_id.clone(),
                            binding_generation: disk.binding_generation,
                        },
                        through_sequence: 0,
                        through_snapshot: None,
                        publications: 0,
                    },
                    previous: DiskCatalogSnapshot::empty(DiskRegistrationIdentity {
                        pool_id: pool.into(),
                        disk_id: disk.disk_id.clone(),
                        slot_id: disk.slot_id.clone(),
                        binding_generation: disk.binding_generation,
                    }),
                    changes: Changes {
                        rows: Vec::new(),
                        bytes: 0,
                    },
                    record_bytes: 0,
                })
                .collect(),
        }
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
                || candidate.sequence <= disk.header.through_sequence
                || candidate.previous_snapshot != disk.header.through_snapshot
                || admitted.state().sequence != candidate.sequence
                || disk.header.publications as usize == MAX_PUBLICATIONS
            {
                return Err(invalid(
                    "prepared disk scopes do not extend their exact authenticated prefix",
                ));
            }
            let next = snapshot(admitted.state(), disk.header.identity.clone())?;
            disk.changes
                .append(candidate, hash, &disk.previous, &next)?;
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
            disk.previous = next;
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
        store: impl FnMut(DiskRegistrationIdentity, Vec<u8>, Vec<u8>) -> Result<()>,
    ) -> Result<()> {
        self.finish_with_encoder(admitted, stop, Self::encode_pair, store)
    }

    /// Supply derived bytes only. Prefix/row checks and actual SQLite readback
    /// remain outside the encoder so a memo cannot certify its own result.
    pub(in crate::engine) fn finish_with_encoder(
        self,
        admitted: &AdmittedState,
        stop: &AtomicBool,
        mut encode: impl FnMut(
            &DiskCatalogSnapshot,
            &DiskHistorySnapshot,
        ) -> Result<(Vec<u8>, Vec<u8>)>,
        mut store: impl FnMut(DiskRegistrationIdentity, Vec<u8>, Vec<u8>) -> Result<()>,
    ) -> Result<()> {
        for disk in self.disks {
            preparation_checkpoint(stop)?;
            let current = snapshot(admitted.state(), disk.header.identity.clone())?;
            if disk.header.through_sequence != admitted.state().sequence
                || !current.same_rows(&disk.previous)
            {
                return Err(invalid(
                    "prepared disk prefix differs from the admitted frozen State",
                ));
            }
            let history = DiskHistorySnapshot {
                header: disk.header,
                changes: disk.changes.rows,
            };
            let (current_bytes, history_bytes) = encode(&current, &history)?;
            preparation_checkpoint(stop)?;
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

    pub(in crate::engine) fn encode_pair(
        current: &DiskCatalogSnapshot,
        history: &DiskHistorySnapshot,
    ) -> Result<(Vec<u8>, Vec<u8>)> {
        let current_hash = storage_contract::object_hash("disk-current-catalog", current)?;
        let current_bytes = super::super::database::encode(current, &current_hash)?;
        let history_bytes = database::encode(history, &digest(history)?)?;
        Ok((current_bytes, history_bytes))
    }
}

#[cfg(test)]
#[path = "../../../../../../test/engine/unit/engine/disk_scope_encoding_tests.rs"]
mod encoding_tests;
