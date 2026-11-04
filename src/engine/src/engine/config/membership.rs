//! Fixed membership admitted from configuration, never from observed voters.
use crate::{Config, Error, ErrorCode, Result, journal::Candidate};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq)]
struct MemberBinding {
    slot_id: String,
    binding_generation: u64,
}

/// Cached immutable identity. Roots, array order and resource limits do not
/// change voters; disk IDs, slots, bindings and pool identity do. Construction
/// validates no persisted authority: recovery separately compares the original
/// saved anchor before admitting even an empty journal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct MetadataMembership {
    pool_id: String,
    members: BTreeMap<String, MemberBinding>,
}

impl MetadataMembership {
    /// Config admission already validated identities and non-emptiness.
    pub(super) fn from_config(config: &Config) -> Self {
        Self {
            pool_id: config.pool_id.clone(),
            members: config
                .disks
                .iter()
                .filter(|disk| disk.metadata_member)
                .map(|disk| {
                    (
                        disk.disk_id.clone(),
                        MemberBinding {
                            slot_id: disk.slot_id.clone(),
                            binding_generation: disk.binding_generation,
                        },
                    )
                })
                .collect(),
        }
    }

    pub(super) fn verify_candidate(&self, candidate: &Candidate) -> Result<()> {
        if candidate.registry().is_some() {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "lifecycle candidate requires authenticated registry serving authority",
            ));
        }
        let actual: BTreeSet<_> = candidate.metadata_disk_ids.iter().collect();
        if candidate.pool_id != self.pool_id
            || actual.len() != candidate.metadata_disk_ids.len()
            || !actual.into_iter().eq(self.members.keys())
        {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "candidate pool or membership differs from admitted configuration",
            ));
        }
        Ok(())
    }
}
