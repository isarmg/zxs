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

    pub(crate) fn validate_seal(
        &self,
        config: &super::ValidatedConfig,
        disk: &crate::DiskConfig,
    ) -> Result<crate::disk::DiskSeal> {
        let identity = crate::disk::validate_seal(config, disk);
        if disk.metadata_member {
            self.observe(identity)
        } else {
            identity
        }
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
    integrity: &'a MetadataIntegrity,
}

impl MetadataIntegrity {
    pub(crate) fn read_context<'a>(
        &'a self,
        config: &'a super::ValidatedConfig,
        identity: &'a crate::disk::DiskSeal,
    ) -> MetadataReadContext<'a> {
        MetadataReadContext {
            config,
            identity,
            integrity: self,
        }
    }
}

impl MetadataReadContext<'_> {
    pub(crate) fn membership(
        &self,
        loaded: Result<super::ValidatedConfig>,
    ) -> Result<super::ValidatedConfig> {
        let saved = self.integrity.observe(loaded)?;
        self.integrity
            .observe(self.config.verify_saved_metadata_membership(&saved))?;
        Ok(saved)
    }
    pub(crate) fn seal(
        &self,
        loaded: Result<crate::disk::DiskSeal>,
    ) -> Result<crate::disk::DiskSeal> {
        let seal = self.integrity.observe(loaded)?;
        self.integrity
            .observe(crate::disk::validate_stored_seal(&seal, self.identity))?;
        Ok(seal)
    }
}
