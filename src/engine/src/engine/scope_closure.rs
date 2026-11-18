//! One scope graph under the existing genesis or native publication decision.
//! Neither a locally valid SQLite image nor a registration is such a decision.
use crate::{Config, ErrorCode, Result, ValidatedConfig};
use storage_contract::{Generation, OperationId};

pub(in crate::engine) mod directory;
mod anchor;
pub(in crate::engine) use anchor::{Anchor, FixedGenesis};
mod genesis;
pub(in crate::engine) mod file;
mod images;
mod objects;
mod ordinal;
pub(in crate::engine) use ordinal::{OrdinalBuilder, OrdinalPages};
mod publication;
pub(in crate::engine) use publication::{HistoryReference, VerifiedScopeSnapshot};
mod readback;
mod schema;
pub(in crate::engine) mod snapshot_bodies;
pub(in crate::engine) mod system;
pub(in crate::engine) use images::{PreparedImage, Representation, Schema};
pub(super) use genesis::{GenesisPreparation, initial_state};

#[derive(Debug, error_definition::Error)]
pub enum ScopeClosureError {
    #[error("fixed genesis does not bind every original root to one initialization")]
    GenesisIdentity,
    #[error("the database image does not match its exact bytes or logical scope")]
    ImageMismatch,
    #[error("the original native publication does not bind this exact local scope")]
    PublicationMismatch,
    #[error("the local scope page differs from the complete original occurrence table")]
    PageMismatch,
    #[error("the captured scope exceeds its resource budget")]
    Budget,
    #[error("the original genesis scope differs from the admitted empty pool")]
    GenesisMismatch,
}
impl ScopeClosureError {
    pub(crate) fn code(&self) -> ErrorCode {
        match self {
            Self::GenesisIdentity => ErrorCode::DiskIdentityMismatch,
            Self::ImageMismatch | Self::PageMismatch | Self::GenesisMismatch
            | Self::PublicationMismatch => ErrorCode::ObjectCorrupt,
            Self::Budget => ErrorCode::ResourceLimit,
        }
    }
}

/// Discovery is not an independent publication vote. Refuse to identify a
/// damaged original scope as healthy; never borrow a missing peer's material.
pub(crate) fn verify_recognized_holder(
    config: &ValidatedConfig, disk: &crate::DiskConfig, root: &crate::disk::DiskRoot,
) -> Result<()> {
    Anchor::verify_managed_recognition(config, disk, root)
}

pub(super) fn require_disk_budget(config: &ValidatedConfig) -> Result<()> {
    if config.disks.is_empty() || config.disks.len() > genesis::MAX_DISKS {
        return Err(ScopeClosureError::Budget.into());
    }
    Ok(())
}

/// This proof applies only to the original fixed configuration, never to a
/// live successor configuration containing a mix of Add/Backup occurrences.
pub(super) fn genesis_operation(config: &Config) -> Result<OperationId> {
    let mut roots = config.storage_roots.values();
    let first = roots.next().ok_or(ScopeClosureError::GenesisIdentity)?;
    let operation = first.operation_id;
    if config.storage_roots.len() != config.disks.len()
        || !std::iter::once(first).chain(roots).all(|binding| {
            binding.operation_id == operation && binding.generation == Generation::new(1)
        })
    {
        return Err(ScopeClosureError::GenesisIdentity.into());
    }
    Ok(operation)
}

pub(super) fn capture_initial_roots(
    config: &ValidatedConfig,
    disks: &crate::disk::DiskResources,
) -> Result<OperationId> {
    let operation = genesis_operation(config)?;
    for disk in &config.disks {
        let held = &disks.get(disk)?.root;
        let binding = config.storage_roots.get(&disk.disk_id.parse::<storage_contract::DiskId>()?)
            .ok_or(ScopeClosureError::GenesisIdentity)?;
        crate::engine::fresh_layout::verify_root(config, disk, held)?;
        let areas = crate::engine::fresh_layout::root_areas::HeldRootAreas::observe(
            held, &binding.roots,
        )?;
        areas.require_empty()?;
        areas.verify(held)?;
    }
    Ok(operation)
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/engine/scope_genesis_tests.rs"]
mod tests;

/// Internal local diagnostic: the entity is the admitted ORDINARY host and
/// the commitment describes a reservation. It is never a usable repository
/// or authority to adopt even correctly shaped material currently at its name.
pub(crate) fn measure_initial_directory_reservation(
    engine: &crate::Engine, repository: storage_contract::working_tree::SnapshotRepositoryId,
) -> Result<(OperationId, storage_contract::working_tree::MaterialCommitment,
            platform_io::fs_safety::linux::FileIdentity)> {
    let pending = directory::PendingRepository::original(&engine.planning_context(), repository)?;
    Ok((pending.reservation(), pending.root(), pending.host_entity()))
}

/// Read-only diagnostics of the original complete node-only repository. A
/// pending or genuinely sparse repository cannot use this subproducer.
pub(crate) fn measure_published_directory_nodes(
    engine: &crate::Engine, repository: storage_contract::working_tree::SnapshotRepositoryId,
) -> Result<(storage_contract::working_tree::MaterialCommitment, usize, usize,
            storage_contract::working_tree::LocalEntityIdentity)> {
    let held = directory::PublishedRepository::original(&engine.planning_context(), repository)?;
    let (nodes, bodies) = held.counts();
    Ok((held.root(), nodes, bodies, held.entity()))
}
