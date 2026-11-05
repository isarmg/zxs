//! Discovery is read-only; published registration and explicit wizard actions
//! remain separate authority boundaries.
use super::*;

impl ManagementBackend {
    pub(super) fn discovered_disks(
        &self,
        identity: &Identity,
    ) -> Result<storage_contract::storage_directories::DiscoveredDisks, ApiError> {
        let (excluded, stopped) = self.discovery_scope(identity)?;
        crate::storage_directories::discover_disks(
            self.inner.discovery_root.as_str(),
            &excluded,
            &stopped,
        )
        .map_err(ApiError::from)
    }

    pub(super) fn discovery_scope(
        &self,
        identity: &Identity,
    ) -> Result<(Vec<PathBuf>, Vec<PathBuf>), ApiError> {
        let mut excluded = vec![
            self.inner.data_root.clone(),
            self.inner.management_root.clone(),
        ];
        let stopped = if let Some(terminal) = self.inner.terminal(identity)? {
            terminal
                .original_configuration()
                .disks
                .iter()
                .map(|disk| disk.root.clone())
                .collect()
        } else if let Some(backend) = crate::lock(&self.inner.state)
            .backend
            .active_engine()
            .cloned()
        {
            let (config, stopped) = backend.addition_directory_scope(identity)?;
            excluded.extend(config.disks.into_iter().map(|disk| disk.root));
            stopped
        } else {
            Vec::new()
        };
        Ok((excluded, stopped))
    }
}
