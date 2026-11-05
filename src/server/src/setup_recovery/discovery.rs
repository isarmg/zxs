//! The discovery boundary belongs to immutable service-manager configuration.
//! It never moves admitted pool roots or reads obsolete Web-managed settings.
use super::*;
use storage_contract::storage_directories::DiscoveryRootSettings;

impl SetupRecovery {
    /// No filesystem I/O: an absent discovery directory cannot block management.
    pub fn discovery_root(&self) -> DiscoveryRootSettings {
        DiscoveryRootSettings {
            discovery_root: self.discovery_root.as_str().into(),
        }
    }

    pub fn with_discovery_root(mut self, root: crate::storage_directories::DiscoveryRoot) -> Self {
        self.discovery_root = root;
        self
    }
}
