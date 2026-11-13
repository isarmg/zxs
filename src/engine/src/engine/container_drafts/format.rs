//! Container-edit consumers use the single completed storage admission.
//! Config bytes and a DISK selector alone never authorize SYSTEM mutations.
use crate::authority_selection::FreshSelection;
use crate::authority_selection::manual::ManualFormatError;
use crate::engine::container_drafts::format_error;
use crate::{Config, Engine, OperationId, Result, ValidatedConfig};
use std::path::Path;

pub(crate) fn require_manual_storage(config: &ValidatedConfig) -> Result<()> {
    if config.fresh_bootstrap().is_none() || config.fresh_completion().is_none() {
        return Err(format_error(ManualFormatError::UnsupportedReader));
    }
    Ok(())
}

pub(crate) fn admit_saved_genesis(
    saved: ValidatedConfig,
    projected: &ValidatedConfig,
    seal: &crate::disk::DiskSeal,
    selector_bytes: &[u8],
) -> Result<ValidatedConfig> {
    require_manual_storage(projected)?;
    let admitted = FreshSelection::admit_disk(saved, seal, selector_bytes)?;
    if admitted.fresh_bootstrap() != projected.fresh_bootstrap() {
        return Err(format_error(ManualFormatError::Changed));
    }
    let proof = projected.fresh_completion()
        .ok_or_else(|| format_error(ManualFormatError::Incomplete))?;
    Ok(admitted.with_fresh_completion(proof.clone()))
}

impl Engine {
    /// Explicit fresh initialization uses the same format and transaction as
    /// directory storage. Neither former WIP format is accepted or migrated.
    pub fn initialize_manual_container_storage(
        root: &Path,
        config: Config,
        operation: OperationId,
    ) -> Result<Self> {
        Self::initialize_storage(root, config, operation)
    }
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/manual_container_initialization_tests.rs"]
mod tests;
