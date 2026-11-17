//! One scope graph under the existing genesis or native publication decision.
//! Neither a locally valid SQLite image nor a registration is such a decision.
use crate::{Config, ErrorCode, Result, ValidatedConfig};
use storage_contract::{Generation, OperationId};

mod genesis;
mod images;
mod objects;
mod readback;
mod schema;
pub(in crate::engine) use images::{PreparedImage, Representation, Schema};
pub(super) use genesis::{GenesisPreparation, initial_state};

#[derive(Debug, error_definition::Error)]
pub enum ScopeClosureError {
    #[error("fixed genesis does not bind every original root to one initialization")]
    GenesisIdentity,
    #[error("the database image does not match its exact bytes or logical scope")]
    ImageMismatch,
    #[error("the captured scope exceeds its resource budget")]
    Budget,
    #[error("the original genesis scope differs from the admitted empty pool")]
    GenesisMismatch,
}
impl ScopeClosureError {
    pub(crate) fn code(&self) -> ErrorCode {
        match self {
            Self::GenesisIdentity => ErrorCode::DiskIdentityMismatch,
            Self::ImageMismatch | Self::GenesisMismatch => ErrorCode::ObjectCorrupt,
            Self::Budget => ErrorCode::ResourceLimit,
        }
    }
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
