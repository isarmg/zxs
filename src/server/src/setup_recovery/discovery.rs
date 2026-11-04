//! The server-owned discovery boundary is separate from admitted pool roots.
//! Changing it affects future selection only; it never relocates existing data.
use super::*;
use storage_contract::storage_directories::{
    DEFAULT_DISCOVERY_ROOT, DiscoveryRootSettings, SetDiscoveryRootRequest,
};

const SETTINGS: &str = "storage-discovery.json";

#[derive(Serialize, Deserialize)]
enum DiscoveryFormat {
    #[serialization(rename = "storage-directory-discovery")]
    Directories,
}

#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct StoredDiscovery {
    format: DiscoveryFormat,
    discovery_root: String,
}

impl SetupRecovery {
    /// Reads only system configuration. An unavailable discovery directory must
    /// not block login, status, or recovery of a previously selected pool.
    pub fn discovery_root(&self) -> Result<DiscoveryRootSettings, ApiError> {
        if !activation::present(&self.data_root.join(SETTINGS))? {
            return Ok(DiscoveryRootSettings {
                discovery_root: DEFAULT_DISCOVERY_ROOT.into(),
            });
        }
        let saved: StoredDiscovery = self.read_durable_control(SETTINGS)?;
        Ok(DiscoveryRootSettings {
            discovery_root: saved.discovery_root,
        })
    }

    /// A current administrator may change future discovery only. The same lock
    /// serializes activation so no request can race a root change with admission.
    pub fn set_discovery_root(
        &self,
        identity: &Identity,
        request: SetDiscoveryRootRequest,
        revalidate: impl FnMut(&Identity) -> Result<(), ApiError>,
    ) -> Result<DiscoveryRootSettings, ApiError> {
        self.set_discovery_root_with_authority(identity, request, None, &[], revalidate)
    }

    pub(crate) fn set_terminal_discovery_root(
        &self,
        identity: &Identity,
        request: SetDiscoveryRootRequest,
        terminal: &storage_engine::TerminalManagement,
        revalidate: impl FnMut(&Identity) -> Result<(), ApiError>,
    ) -> Result<DiscoveryRootSettings, ApiError> {
        let stopped: Vec<_> = terminal
            .original_configuration()
            .disks
            .iter()
            .map(|disk| disk.root.clone())
            .collect();
        self.set_discovery_root_with_authority(
            identity,
            request,
            Some(terminal),
            &stopped,
            revalidate,
        )
    }

    pub(crate) fn set_active_discovery_root(
        &self,
        identity: &Identity,
        request: SetDiscoveryRootRequest,
        stopped: &[PathBuf],
        revalidate: impl FnMut(&Identity) -> Result<(), ApiError>,
    ) -> Result<DiscoveryRootSettings, ApiError> {
        self.set_discovery_root_with_authority(identity, request, None, stopped, revalidate)
    }

    fn set_discovery_root_with_authority(
        &self,
        identity: &Identity,
        request: SetDiscoveryRootRequest,
        terminal: Option<&storage_engine::TerminalManagement>,
        stopped: &[PathBuf],
        mut revalidate: impl FnMut(&Identity) -> Result<(), ApiError>,
    ) -> Result<DiscoveryRootSettings, ApiError> {
        revalidate(identity)?;
        let _lock = self.lock()?;
        if !self.management_authority_present()? {
            return Err(ApiError::unauthorized());
        }
        if terminal.is_none()
            && activation::present(&self.data_root.join(activation::ACTIVATION_INTENT))?
        {
            self.activation_admitted()?;
        }
        let current = self.discovery_root()?;
        if current.discovery_root != request.expected_discovery_root {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::ConfigConflict,
                "storage discovery settings changed; reload before saving",
            ));
        }
        crate::storage_directories::validate_fresh_discovery_root(
            &request.discovery_root,
            &[self.data_root.clone(), self.management_root.clone()],
            stopped,
        )?;
        revalidate(identity)?;
        if current.discovery_root == request.discovery_root {
            return Ok(current);
        }
        let saved = StoredDiscovery {
            format: DiscoveryFormat::Directories,
            discovery_root: request.discovery_root.clone(),
        };
        if activation::present(&self.data_root.join(SETTINGS))? {
            let bytes = json_codec::to_vec(&saved)
                .expect("invariant: internal discovery settings serialization succeeds");
            let directory =
                platform_io::fs_safety::PrivateDirectory::open_existing(&self.data_root)
                    .map_err(control_publication_error)?;
            let path = platform_io::fs_safety::RelativePath::new(SETTINGS)
                .expect("invariant: static discovery settings filename");
            platform_io::fs_safety::AtomicFile::replace(&directory, &path, &bytes)
                .map_err(control_publication_error)?;
        } else {
            self.write_control(SETTINGS, &saved)?;
        }
        Ok(DiscoveryRootSettings {
            discovery_root: request.discovery_root,
        })
    }
}
