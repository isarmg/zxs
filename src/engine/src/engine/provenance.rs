//! Same-writer revocation shared by live and detached observations.
//!
//! An epoch is not a health or durability certificate. Only successful complete
//! publication can bind one to PublishedStateBase. It never survives reopen.
use crate::{Error, ErrorCode, Result};
use std::sync::{Arc, Mutex};

#[derive(Debug, Default)]
pub(crate) struct MetadataIntegrity {
    generation: Mutex<Arc<()>>,
}

#[derive(Clone)]
pub(super) struct MetadataEpoch {
    owner: Arc<MetadataIntegrity>,
    generation: Arc<()>,
}

impl MetadataIntegrity {
    pub(super) fn capture(self: &Arc<Self>) -> MetadataEpoch {
        MetadataEpoch {
            owner: Arc::clone(self),
            generation: Arc::clone(
                &self
                    .generation
                    .lock()
                    .expect("invariant: metadata generation lock poisoned"),
            ),
        }
    }

    pub(crate) fn invalidate(&self) {
        // Old plans retain their generation allocation, so pointer equality has
        // no ABA and a later successful publication cannot revive an old token.
        *self
            .generation
            .lock()
            .expect("invariant: metadata generation lock poisoned") = Arc::new(());
    }

    pub(crate) fn fault(&self, error: &Error) {
        // Cancellation can interrupt a real metadata read without observing a
        // fault. Scan-wide budgets are handled by the scrub before this source
        // boundary; ResourceLimit here can mean stored bytes exceeded their
        // authenticated bound and must not preserve an old publication token.
        if error.code() != ErrorCode::OperationAborted {
            self.invalidate();
        }
    }

    pub(crate) fn observe<T>(&self, result: Result<T>) -> Result<T> {
        if let Err(error) = &result {
            self.fault(error);
        }
        result
    }
}

impl MetadataEpoch {
    /// The decision's short admission fence holds the epoch while its other
    /// mutable authority (schedule/deadline) is checked. No I/O runs here.
    pub(super) fn admits(
        &self,
        owner: &Arc<MetadataIntegrity>,
        condition: impl FnOnce() -> bool,
    ) -> bool {
        let generation = owner
            .generation
            .lock()
            .expect("invariant: metadata generation lock poisoned");
        Arc::ptr_eq(&self.owner, owner) && Arc::ptr_eq(&self.generation, &generation) && condition()
    }

    pub(super) fn same_generation(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.owner, &other.owner) && Arc::ptr_eq(&self.generation, &other.generation)
    }

    pub(super) fn matches(&self, owner: &Arc<MetadataIntegrity>) -> bool {
        Arc::ptr_eq(&self.owner, owner)
            && Arc::ptr_eq(
                &self.generation,
                &owner
                    .generation
                    .lock()
                    .expect("invariant: metadata generation lock poisoned"),
            )
    }
}

/// The live reader's admitted membership, not an assertion about version-local
/// proofs. Only shared membership/identity reads use this fault scope.
pub(crate) struct MetadataReadContext<'a> {
    config: &'a super::ValidatedConfig,
    identity: &'a crate::disk::DiskSeal,
    integrity: Option<&'a MetadataIntegrity>,
}

impl MetadataIntegrity {
    pub(crate) fn read_context<'a>(
        &'a self,
        config: &'a super::ValidatedConfig,
        identity: &'a crate::disk::DiskSeal,
    ) -> MetadataReadContext<'a> {
        MetadataReadContext::for_disk(config, identity, Some(self))
    }
}

impl MetadataReadContext<'_> {
    pub(crate) fn for_disk<'a>(
        config: &'a super::ValidatedConfig,
        identity: &'a crate::disk::DiskSeal,
        integrity: Option<&'a MetadataIntegrity>,
    ) -> MetadataReadContext<'a> {
        MetadataReadContext {
            config,
            identity,
            integrity,
        }
    }

    fn observe<T>(&self, result: Result<T>) -> Result<T> {
        match self.integrity {
            Some(integrity) => integrity.observe(result),
            None => result,
        }
    }
    pub(crate) fn membership(
        &self,
        loaded: Result<super::ValidatedConfig>,
    ) -> Result<super::ValidatedConfig> {
        let saved = self.observe(loaded)?;
        self.observe(self.config.verify_saved_metadata_membership(&saved))?;
        Ok(saved)
    }
    pub(crate) fn seal(
        &self,
        loaded: Result<crate::disk::DiskSeal>,
    ) -> Result<crate::disk::DiskSeal> {
        let seal = self.observe(loaded)?;
        self.observe(crate::disk::validate_stored_seal(&seal, self.identity))?;
        Ok(seal)
    }
}

/// Re-admit mutable seal bytes on the already owned resource, without looking
/// up the configured root or re-sampling device topology.
pub(crate) fn validate_owned_seal(
    resources: &crate::disk::DiskResources,
    disk: &crate::DiskConfig,
    integrity: &MetadataIntegrity,
) -> Result<()> {
    let result = (|| {
        let handle = resources.get(disk)?;
        let stored = crate::disk::decode_seal(&handle.root.read_bytes_observed(
            &disk.root.join("disk.json"),
            crate::disk::MAX_METADATA,
            &mut |_| Ok(()),
        )?)?;
        crate::disk::validate_stored_seal(&stored, &handle.seal)
    })();
    if disk.metadata_member {
        integrity.observe(result)
    } else {
        result
    }
}

pub(crate) fn validate_candidate_seals(
    config: &super::ValidatedConfig,
    resources: &crate::disk::DiskResources,
    candidate: &crate::journal::Candidate,
    integrity: &MetadataIntegrity,
) -> Result<()> {
    for disk in &config.disks {
        if disk.metadata_member
            || candidate
                .versions
                .iter()
                .any(|evidence| evidence.placement.route_slot_ids.contains(&disk.slot_id))
        {
            validate_owned_seal(resources, disk, integrity)?;
        }
    }
    Ok(())
}
