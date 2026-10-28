//! Restart-only process policy, deliberately absent from persisted configuration.
use super::Engine;
use crate::{Error, Result, ValidatedConfig};

/// Explicit finite retained-evidence limits. These may raise, but never lower,
/// the persisted pool baseline; all metadata and non-cold records stay charged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetainedWorkspaceLimits {
    pub max_retained_workspaces: usize,
    pub max_retained_workspaces_per_principal: usize,
    pub max_retained_workspace_metadata_bytes: u64,
    pub max_retained_workspace_metadata_bytes_per_principal: u64,
}

impl RetainedWorkspaceLimits {
    /// Validate process options without reading configuration or touching disk.
    pub fn validate(&self) -> Result<()> {
        if self.max_retained_workspaces == 0
            || self.max_retained_workspaces_per_principal == 0
            || self.max_retained_workspace_metadata_bytes == 0
            || self.max_retained_workspace_metadata_bytes_per_principal == 0
        {
            return Err(Error::new(
                storage_contract::ErrorCode::ConfigInvalid,
                "retained workspace runtime limits must be positive",
            ));
        }
        Ok(())
    }

    /// An override cannot reduce any actual persisted configuration dimension.
    pub fn validate_for(&self, config: &ValidatedConfig) -> Result<()> {
        if self.max_retained_workspaces < config.max_retained_workspaces
            || self.max_retained_workspaces_per_principal
                < config.max_retained_workspaces_per_principal
            || self.max_retained_workspace_metadata_bytes
                < config.max_retained_workspace_metadata_bytes
            || self.max_retained_workspace_metadata_bytes_per_principal
                < config.max_retained_workspace_metadata_bytes_per_principal
        {
            return Err(Error::new(
                storage_contract::ErrorCode::ConfigInvalid,
                "retained workspace runtime limits cannot be below the persisted configuration",
            ));
        }
        Ok(())
    }

    pub(super) fn from_config(config: &ValidatedConfig) -> Self {
        Self {
            max_retained_workspaces: config.max_retained_workspaces,
            max_retained_workspaces_per_principal: config.max_retained_workspaces_per_principal,
            max_retained_workspace_metadata_bytes: config.max_retained_workspace_metadata_bytes,
            max_retained_workspace_metadata_bytes_per_principal: config
                .max_retained_workspace_metadata_bytes_per_principal,
        }
    }
}

impl Engine {
    pub(super) fn effective_workspace_limits(&self) -> RetainedWorkspaceLimits {
        self.config.effective_workspace_limits()
    }
}
