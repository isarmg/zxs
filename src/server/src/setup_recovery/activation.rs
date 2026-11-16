//! Management credentials and storage genesis have separate durable lifecycles.
//! An empty management installation is not a zero-member storage pool. Once
//! activation starts, its retained intent fences ordinary backend admission until
//! the existing administrator has completed the credential replica and receipt.
use super::*;

const MANAGEMENT_INTENT: &str = "management-initialization.json";
pub(super) const ACTIVATION_INTENT: &str = "storage-activation-intent.json";
const ACTIVATION_READY: &str = "storage-activation-ready.json";
const ACTIVATION_COMPLETE: &str = "storage-activation-complete.json";

#[derive(Serialize, Deserialize)]
enum ManagementFormat {
    #[serialization(rename = "management-without-storage")]
    WithoutStorage,
}

#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct ManagementInitialization {
    format: ManagementFormat,
}

/// Encode the current management-without-storage intent. This is not a storage
/// readiness marker and never authorizes a backend to admit a half-built pool.
pub fn management_initialization_record() -> Vec<u8> {
    json_codec::to_vec(&ManagementInitialization {
        format: ManagementFormat::WithoutStorage,
    })
    .expect("invariant: current management initialization record serializes")
}

#[derive(Clone, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct StorageActivationRequest {
    pub selections: Vec<NamedStorageSelection>,
    pub mode: Mode,
}

pub use storage_contract::storage_directories::NamedStorageSelection;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serialization(rename_all = "snake_case")]
pub enum StorageActivationState {
    Pending,
    Active,
    RecoveryRequired,
    Unavailable,
    TerminalManagement,
}

#[derive(Serialize)]
pub struct StorageActivationStatus {
    pub state: StorageActivationState,
    pub disk_ids: Vec<String>,
    pub completion_available: bool,
    pub allow_development: bool,
    pub terminal: Option<storage_contract::storage_lifecycle::TerminalStorageStatus>,
}

#[derive(Serialize, Deserialize)]
enum ActivationFormat {
    #[serialization(rename = "initial-storage-activation")]
    InitialStorage,
}

#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct StorageActivationIntent {
    format: ActivationFormat,
    config: ValidatedConfig,
    selections: Vec<NamedStorageSelection>,
}

#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct StorageActivationReady {
    intent_digest: String,
    disk_seals: BTreeMap<String, String>,
}

#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct StorageActivationComplete {
    intent_digest: String,
}

struct InitialActivationProof {
    intent: StorageActivationIntent,
    probe: storage_engine::InitialActivationProbeGuard,
}

pub(super) fn present(path: &Path) -> Result<bool, ApiError> {
    match path.symlink_metadata() {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(ApiError::io(error)),
    }
}

impl SetupRecovery {
    /// Public first-account status reads only system authority evidence. It
    /// never probes configured recovery disks or attempts old completion proof.
    pub fn management_status(&self) -> InstallationState {
        self.installation_state()
            .unwrap_or(InstallationState::Unavailable)
    }

    pub(super) fn installation_state(&self) -> Result<InstallationState, ApiError> {
        if present(&self.data_root.join("config.json"))? {
            return Ok(InstallationState::Existing);
        }
        if present(&self.data_root.join(ACTIVATION_INTENT))?
            || present(&self.data_root.join("setup-intent.json"))?
            || present(&self.data_root.join("auth.json"))?
        {
            return Ok(InstallationState::RecoveryRequired);
        }
        if present(&self.data_root.join(MANAGEMENT_INTENT))? {
            self.management_intent()?;
            return Ok(if self.management_authority_present()? {
                InstallationState::ManagementReady
            } else if !present(&self.management_root.join("management-auth.json"))?
                && self.empty_system_directory(&["setup.lock", MANAGEMENT_INTENT])?
            {
                // An intent written before credentials is retryable without a
                // secret. Any authority or other system evidence closes entry.
                InstallationState::Empty
            } else {
                InstallationState::RecoveryRequired
            });
        }
        if present(&self.data_root.join("management-authority.json"))?
            || present(&self.management_root.join("management-auth.json"))?
        {
            return Ok(InstallationState::RecoveryRequired);
        }
        Ok(if self.empty_installation()? {
            InstallationState::Empty
        } else {
            InstallationState::RecoveryRequired
        })
    }

    fn management_intent(&self) -> Result<ManagementInitialization, ApiError> {
        let intent: ManagementInitialization = self.read_durable_control(MANAGEMENT_INTENT)?;
        Ok(intent)
    }

    pub(super) fn management_authority_present(&self) -> Result<bool, ApiError> {
        if !present(&self.data_root.join("management-authority.json"))?
            || !present(&self.management_root.join("management-auth.json"))?
        {
            return Ok(false);
        }
        // Authentication itself remains the caller's current authority check.
        private_file(&self.data_root.join("management-authority.json"), false)?;
        private_file(&self.management_root.join("management-auth.json"), false)?;
        Ok(true)
    }

    /// Create management credentials without selecting, opening or initializing
    /// any storage directory. This first-account entry is intentionally public;
    /// the exclusive lock and retained authority evidence prevent later resets.
    pub fn initialize_management(
        &self,
        request: CompleteSetupRequest,
        install: impl FnOnce(&str, &str) -> Result<(), ApiError>,
    ) -> Result<(), ApiError> {
        authentication::normalize_administrator_username(&request.username)
            .map_err(ApiError::invalid_source)?;
        authentication::validate_password(&request.password).map_err(ApiError::invalid_source)?;
        let _lock = self.lock()?;
        if present(&self.management_root.join("management-auth.json"))?
            || !self.empty_system_directory(&["setup.lock", MANAGEMENT_INTENT])?
        {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::RecoveryRequired,
                "existing system authority or storage evidence prohibits account initialization",
            ));
        }
        if present(&self.data_root.join(MANAGEMENT_INTENT))? {
            self.management_intent()?;
        } else {
            self.write_control(
                MANAGEMENT_INTENT,
                &ManagementInitialization {
                    format: ManagementFormat::WithoutStorage,
                },
            )?;
        }
        install(&request.username, &request.password)?;
        if !self.management_authority_present()? {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::AuthorityRecoveryRequired,
                "management credentials and authority must both be durable before completion",
            ));
        }
        Ok(())
    }

    pub fn storage_status(&self) -> StorageActivationStatus {
        match storage_engine::TerminalManagement::requires_management(&self.data_root) {
            Ok(false) => {}
            terminal => {
                // Until ManagementBackend admits the full authority, report a
                // closed state without evaluating old activation disk proofs.
                return StorageActivationStatus {
                    state: if terminal.is_ok() {
                        StorageActivationState::RecoveryRequired
                    } else {
                        StorageActivationState::Unavailable
                    },
                    disk_ids: Vec::new(),
                    completion_available: false,
                    allow_development: self.allow_development,
                    terminal: None,
                };
            }
        }
        let state = match self.installation_state() {
            Ok(InstallationState::ManagementReady) => StorageActivationState::Pending,
            Ok(InstallationState::Existing) if self.activation_admitted().is_ok() => {
                StorageActivationState::Active
            }
            Ok(InstallationState::Unavailable) | Err(_) => StorageActivationState::Unavailable,
            Ok(_) => StorageActivationState::RecoveryRequired,
        };
        StorageActivationStatus {
            state,
            disk_ids: self.disks.keys().cloned().collect(),
            completion_available: state != StorageActivationState::Active
                && present(&self.data_root.join(ACTIVATION_INTENT))
                    .and_then(|exists| {
                        if exists {
                            self.activation_proof().map(|_| true)
                        } else {
                            Ok(false)
                        }
                    })
                    .unwrap_or(false),
            allow_development: self.allow_development,
            terminal: None,
        }
    }

    /// Call before ordinary background Engine::open. A config file created by
    /// an interrupted genesis is not permission to admit user storage writes.
    pub fn activation_admitted(&self) -> Result<(), ApiError> {
        if !present(&self.data_root.join(ACTIVATION_INTENT))? {
            if present(&self.data_root.join(ACTIVATION_COMPLETE))?
                || (present(&self.data_root.join(MANAGEMENT_INTENT))?
                    && present(&self.data_root.join("config.json"))?)
            {
                return Err(ApiError::conflict(
                    storage_contract::ErrorCode::SetupProofInvalid,
                    "storage activation receipt is missing its intent",
                ));
            }
            return Ok(());
        }
        let intent: StorageActivationIntent = self.read_durable_control(ACTIVATION_INTENT)?;
        if !present(&self.data_root.join(ACTIVATION_COMPLETE))? {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::RecoveryRequired,
                "storage activation must be explicitly completed before data admission",
            ));
        }
        let complete: StorageActivationComplete = self.read_durable_control(ACTIVATION_COMPLETE)?;
        self.verify_activation_configuration(&intent)?;
        if complete.intent_digest != value_digest(&intent)? {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::SetupProofInvalid,
                "storage activation receipt does not match its intent",
            ));
        }
        Ok(())
    }

    /// First storage activation only. Existing pools retain their fixed members;
    /// this operation cannot add, drain, remove or reset an already active pool.
    pub fn activate(
        &self,
        identity: &Identity,
        request: StorageActivationRequest,
        mut revalidate: impl FnMut(&Identity) -> Result<(), ApiError>,
        install: impl FnOnce(Engine) -> Result<(), ApiError>,
    ) -> Result<(), ApiError> {
        revalidate(identity)?;
        let _lock = self.lock()?;
        if self.installation_state()? != InstallationState::ManagementReady {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::RecoveryRequired,
                "first storage activation requires initialized management without a storage pool",
            ));
        }
        if request.selections.is_empty() || request.selections.len() > 32 {
            return Err(ApiError::invalid(
                "select between one and 32 storage directories",
            ));
        }
        let discovery = self.discovery_root();
        let mut paths = Vec::with_capacity(request.selections.len());
        let mut directories = Vec::with_capacity(request.selections.len());
        for named in &request.selections {
            let selection = &named.selection;
            let mut excluded = vec![self.data_root.clone(), self.management_root.clone()];
            excluded.extend(paths.iter().cloned());
            let held = crate::storage_directories::revalidate_within(
                &discovery.discovery_root,
                selection,
                &excluded,
            )?;
            paths.push(PathBuf::from(&selection.root_path));
            directories.push(held.into_storage_directory());
        }
        if request.mode == Mode::Development && !self.allow_development {
            return Err(ApiError::forbidden());
        }
        let named = request.selections.iter().map(|disk| disk.name.clone()).zip(paths);
        let config = Config::independent_roots(request.mode, named)
            .map_err(ApiError::contract_input)?;
        crate::backend_storage_metadata::disk_directory_names(&config.disks)?;
        let config = ValidatedConfig::try_from(config)
            .and_then(|config| config.with_workspace_limits(self.retained_workspace_limits))
            .map_err(engine_error)?;
        revalidate(identity)?;
        let intent = StorageActivationIntent {
            format: ActivationFormat::InitialStorage,
            config: config.clone(),
            selections: request.selections,
        };
        self.write_control(ACTIVATION_INTENT, &intent)?;
        let engine =
            Engine::initialize_held(&self.data_root, config, directories).map_err(engine_error)?;
        let observation = engine.initial_storage_observation(&intent.config).map_err(engine_error)?;
        let seals = self.observed_activation_seals(&intent, &observation)?;
        self.write_activation_ready(&intent, seals)?;
        revalidate(identity)?;
        install(engine)?;
        self.write_control(
            ACTIVATION_COMPLETE,
            &StorageActivationComplete {
                intent_digest: value_digest(&intent)?,
            },
        )
    }

    pub fn complete_activation(
        &self,
        identity: &Identity,
        revalidate: impl FnMut(&Identity) -> Result<(), ApiError>,
        install: impl FnOnce(Engine) -> Result<(), ApiError>,
    ) -> Result<(), ApiError> {
        self.complete_activation_inner(identity, None, revalidate, install)
    }

    /// A retained offline request may only complete its exact original disk
    /// selection and mode. The comparison is repeated under the setup lock;
    /// completed evidence still runs the caller's required durability checks.
    pub fn complete_activation_exact(
        &self,
        identity: &Identity,
        expected: &StorageActivationRequest,
        revalidate: impl FnMut(&Identity) -> Result<(), ApiError>,
        install: impl FnOnce(Engine) -> Result<(), ApiError>,
    ) -> Result<(), ApiError> {
        self.complete_activation_inner(identity, Some(expected), revalidate, install)
    }

    /// Read-only correlation, not authorization or permission to reset a pool.
    /// False means no activation intent, not that the system is proven empty.
    pub fn initial_activation_matches(
        &self,
        expected: &StorageActivationRequest,
    ) -> Result<bool, ApiError> {
        if !present(&self.data_root.join(ACTIVATION_INTENT))? {
            if present(&self.data_root.join(ACTIVATION_COMPLETE))? {
                return Err(ApiError::conflict(
                    storage_contract::ErrorCode::SetupProofInvalid,
                    "activation completion is missing its original intent",
                ));
            }
            return Ok(false);
        }
        let intent: StorageActivationIntent = self.read_durable_control(ACTIVATION_INTENT)?;
        if intent.selections != expected.selections || intent.config.mode != expected.mode {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::SetupProofInvalid,
                "retained activation belongs to a different exact selection or mode",
            ));
        }
        Ok(true)
    }

    fn complete_activation_inner(
        &self,
        identity: &Identity,
        expected: Option<&StorageActivationRequest>,
        mut revalidate: impl FnMut(&Identity) -> Result<(), ApiError>,
        install: impl FnOnce(Engine) -> Result<(), ApiError>,
    ) -> Result<(), ApiError> {
        revalidate(identity)?;
        let _lock = self.lock()?;
        if let Some(expected) = expected
            && !self.initial_activation_matches(expected)?
        {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::SetupProofInvalid,
                "exact activation recovery requires its original intent",
            ));
        }
        let completed = present(&self.data_root.join(ACTIVATION_COMPLETE))?;
        if completed {
            self.activation_admitted()?;
            if expected.is_none() {
                return Ok(());
            }
        }
        let InitialActivationProof { intent, probe } = self.activation_proof()?;
        let directories = self.activation_directories(&intent)?;
        let (mut engine, observation) = Engine::open_initial_storage_completion(
            probe,
            self.retained_workspace_limits,
            directories,
        )
        .map_err(engine_error)?;
        crate::engine_backend::report_recovery_warnings(&mut engine);
        if engine.is_read_only() {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::RecoveryRequired,
                "pending storage decisions require recovery before activation completion",
            ));
        }
        // Use the result admitted before recovery through the same held roots.
        // A second probe here would compete with this Engine's writer/selection
        // lock and would discard the actual initialization-result evidence.
        let seals = self.observed_activation_seals(&intent, &observation)?;
        self.verify_activation_ready(&intent, &seals)?;
        if !present(&self.data_root.join(ACTIVATION_READY))? {
            self.write_activation_ready(&intent, seals)?;
        }
        let intent_digest = value_digest(&intent)?;
        revalidate(identity)?;
        install(engine)?;
        if completed {
            Ok(())
        } else {
            self.write_control(
                ACTIVATION_COMPLETE,
                &StorageActivationComplete { intent_digest },
            )
        }
    }

    fn verify_activation_configuration(
        &self,
        intent: &StorageActivationIntent,
    ) -> Result<(), ApiError> {
        let config: Config = read_control(&self.data_root.join("config.json"))?;
        if !config.is_initialization_result_of(&intent.config)
            && !crate::management::storage_configuration::admits_configuration(
                &self.management_root,
                &config,
                &intent.config,
            )?
        {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::SetupProofInvalid,
                "storage configuration differs from its activation intent",
            ));
        }
        Ok(())
    }

    fn initial_activation_probe(
        &self,
        intent: &StorageActivationIntent,
    ) -> Result<storage_engine::InitialActivationProbeGuard, ApiError> {
        let config: Config = read_control(&self.data_root.join("config.json"))?;
        // This is structural correlation only. The original request did not
        // contain physical roots. Created-root feedback must be proven against
        // all fixed Genesis anchors and held entities before completion.
        // Successor intents never authorize replay of the original selections.
        if !config.is_initialization_result_of(&intent.config) {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::SetupProofInvalid,
                "initial activation configuration differs from its retained intent",
            ));
        }
        Engine::begin_initial_storage_completion(&self.data_root, &intent.config)
            .map_err(engine_error)
    }

    fn activation_directories(
        &self,
        intent: &StorageActivationIntent,
    ) -> Result<Vec<File>, ApiError> {
        use platform_io::fs_safety::linux::{MountPolicy, OpenAt2Root};
        intent
            .selections
            .iter()
            .map(|named| {
                let selection = &named.selection;
                let root =
                    OpenAt2Root::open(Path::new(&selection.root_path), MountPolicy::SameMount)
                        .map_err(ApiError::invalid_source)?;
                let identity = root.identity();
                if identity.device != selection.root_identity.device
                    || identity.inode != selection.root_identity.inode
                {
                    return Err(ApiError::conflict(
                        storage_contract::ErrorCode::DiskChanged,
                        "storage activation directory identity changed before recovery",
                    ));
                }
                Ok(root.into_directory())
            })
            .collect()
    }

    fn activation_seals(
        &self,
        intent: &StorageActivationIntent,
    ) -> Result<BTreeMap<String, String>, ApiError> {
        if intent.selections.len() != intent.config.disks.len() {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::SetupProofInvalid,
                "storage activation selection count differs from its intent",
            ));
        }
        intent
            .selections
            .iter()
            .zip(&intent.config.disks)
            .map(|(named, disk)| {
                let selection = &named.selection;
                let path = PathBuf::from(&selection.root_path);
                // The mount ID is an observation in the creating namespace, not a
                // durable identity. Recovery binds the stored directory identity
                // and core seal; it never substitutes a newly inspected empty path.
                if named.name != disk.name
                    || path != disk.root
                    || checked_directory(&path)? != path
                    || directory_identity(&path)?
                        != (
                            selection.root_identity.device,
                            selection.root_identity.inode,
                        )
                    || !path.starts_with(&selection.mount_path)
                {
                    return Err(ApiError::conflict(
                        storage_contract::ErrorCode::DiskChanged,
                        "storage activation directory identity changed",
                    ));
                }
                let seal: json_codec::Value = read_control(&path.join("disk.json"))?;
                Ok((disk.disk_id.clone(), value_digest(&seal)?))
            })
            .collect()
    }

    fn observed_activation_seals(
        &self,
        intent: &StorageActivationIntent,
        observation: &storage_engine::InitialStorageObservation,
    ) -> Result<BTreeMap<String, String>, ApiError> {
        if intent.selections.len() != intent.config.disks.len()
            || observation.disks().len() != intent.config.disks.len()
        {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::SetupProofInvalid,
                "initial storage observation does not cover the original selections",
            ));
        }
        intent.selections.iter().zip(&intent.config.disks).map(|(named, disk)| {
            let id = disk.disk_id.parse::<storage_contract::DiskId>()
                .map_err(ApiError::invalid_source)?;
            let observed = observation.disks().get(&id).ok_or_else(|| {
                ApiError::conflict(
                    storage_contract::ErrorCode::SetupProofInvalid,
                    "initial storage observation names a different original disk",
                )
            })?;
            let selection = &named.selection;
            let path = Path::new(&selection.root_path);
            if named.name != disk.name
                || path != disk.root
                || !path.starts_with(&selection.mount_path)
                || observed.root().device() != selection.root_identity.device
                || observed.root().inode() != selection.root_identity.inode
            {
                return Err(ApiError::conflict(
                    storage_contract::ErrorCode::DiskChanged,
                    "initial storage observation differs from its selected directory",
                ));
            }
            // Preserve the existing control receipt wire, including its digest
            // over the original request, not the created Config or an epoch tip.
            Ok((id.to_string(), observed.seal().to_string()))
        }).collect()
    }

    fn write_activation_ready(
        &self,
        intent: &StorageActivationIntent,
        disk_seals: BTreeMap<String, String>,
    ) -> Result<(), ApiError> {
        self.write_control(
            ACTIVATION_READY,
            &StorageActivationReady {
                intent_digest: value_digest(intent)?,
                disk_seals,
            },
        )
    }

    fn activation_proof(&self) -> Result<InitialActivationProof, ApiError> {
        self.management_intent()?;
        if !self.management_authority_present()? {
            return Err(ApiError::unauthorized());
        }
        let intent: StorageActivationIntent = self.read_durable_control(ACTIVATION_INTENT)?;
        let probe = self.initial_activation_probe(&intent)?;
        let seals = self.activation_seals(&intent)?;
        self.verify_activation_ready(&intent, &seals)?;
        Ok(InitialActivationProof { intent, probe })
    }

    fn verify_activation_ready(
        &self,
        intent: &StorageActivationIntent,
        seals: &BTreeMap<String, String>,
    ) -> Result<(), ApiError> {
        if present(&self.data_root.join(ACTIVATION_READY))? {
            let ready: StorageActivationReady = self.read_durable_control(ACTIVATION_READY)?;
            if ready.intent_digest != value_digest(intent)? || &ready.disk_seals != seals {
                return Err(ApiError::conflict(
                    storage_contract::ErrorCode::SetupProofInvalid,
                    "storage activation evidence or selected directory identity changed",
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../../test/server/unit/setup_recovery/activation_tests.rs"]
mod tests;
