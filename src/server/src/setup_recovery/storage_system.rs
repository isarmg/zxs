//! New installations bind their storage SYSTEM separately from management controls.
//! A created child without its durable control intent is not recoverable authority.
use crate::ApiError;
use crate::setup_recovery::control_publication_error;
use data_serialization::{Deserialize, Serialize};
use platform_io::fs_safety::{PrivateDirectory, linux::{MountPolicy, OpenAt2Root}};
use std::path::Path;
use storage_contract::{OperationId, working_tree::LocalEntityIdentity};

mod layout;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct StorageSystemBinding {
    pub(super) operation: OperationId,
    control: LocalEntityIdentity,
    system: LocalEntityIdentity,
}

fn entity(root: &OpenAt2Root) -> LocalEntityIdentity {
    let identity = root.identity();
    LocalEntityIdentity::new(identity.device, identity.inode)
}

fn control(path: &Path) -> Result<OpenAt2Root, ApiError> {
    let directory = PrivateDirectory::open_existing(path).map_err(control_publication_error)?;
    OpenAt2Root::from_private_directory(directory, MountPolicy::SameMount)
        .map_err(control_publication_error)
}

impl StorageSystemBinding {
    pub(super) fn evidence(&self) -> storage_engine::StorageControlBinding {
        storage_engine::StorageControlBinding {
            operation: self.operation, control: self.control, system: self.system,
        }
    }

    pub(super) fn from_evidence(value: &storage_engine::StorageControlBinding) -> Self {
        Self { operation: value.operation, control: value.control, system: value.system }
    }

    pub(super) fn require_missing_original(&self, path: &Path) -> Result<(), ApiError> {
        let parent = self.verify_parent(path)?;
        match parent.open_directory(&layout::SYSTEM) {
            Err(platform_io::fs_safety::Error::Io(error))
                if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(control_publication_error(error)),
            Ok(_) => Err(ApiError::conflict(storage_contract::ErrorCode::SetupProofInvalid,
                "existing or replaced SYSTEM is preserved; only a missing original carrier can be recovered")),
        }
    }

    pub(super) fn create(
        control_directory: &PrivateDirectory,
        operation: OperationId,
    ) -> Result<(Self, PrivateDirectory), ApiError> {
        let path = control_directory.path();
        let parent = OpenAt2Root::from_private_directory(
            control_directory.try_clone().map_err(control_publication_error)?,
            MountPolicy::SameMount,
        ).map_err(control_publication_error)?;
        // EEXIST is a refusal, including an empty child. No prior unbound root is adopted.
        rustix::fs::mkdirat(parent.directory(), layout::SYSTEM.as_path(),
            rustix::fs::Mode::from_raw_mode(0o700))
            .map_err(std::io::Error::from).map_err(ApiError::io)?;
        let file = parent.open_directory(&layout::SYSTEM).map_err(control_publication_error)?;
        let directory = PrivateDirectory::from_directory(file, path.join(layout::SYSTEM.as_path()))
            .map_err(control_publication_error)?;
        directory.sync().map_err(control_publication_error)?;
        parent.directory().sync_all().map_err(ApiError::io)?;
        let held = OpenAt2Root::from_private_directory(
            directory.try_clone().map_err(control_publication_error)?, MountPolicy::SameMount,
        ).map_err(control_publication_error)?;
        let binding = Self { operation, control: entity(&parent), system: entity(&held) };
        binding.verify_parent(path)?;
        directory.verify_identity().map_err(control_publication_error)?;
        Ok((binding, directory))
    }

    fn verify_parent(&self, path: &Path) -> Result<OpenAt2Root, ApiError> {
        let parent = control(path)?;
        if entity(&parent) != self.control {
            return Err(ApiError::conflict(storage_contract::ErrorCode::SystemRootChanged,
                "storage control root differs from its original initialization binding"));
        }
        Ok(parent)
    }

    pub(super) fn open(&self, path: &Path) -> Result<PrivateDirectory, ApiError> {
        let parent = self.verify_parent(path)?;
        let file = parent.open_directory(&layout::SYSTEM).map_err(control_publication_error)?;
        let directory = PrivateDirectory::from_directory(file, path.join(layout::SYSTEM.as_path()))
            .map_err(control_publication_error)?;
        let held = OpenAt2Root::from_private_directory(
            directory.try_clone().map_err(control_publication_error)?, MountPolicy::SameMount,
        ).map_err(control_publication_error)?;
        if entity(&held) != self.system {
            return Err(ApiError::conflict(storage_contract::ErrorCode::SystemRootChanged,
                "storage SYSTEM differs from its original initialization binding"));
        }
        Ok(directory)
    }
}

#[cfg(test)]
#[path = "../../../../test/server/unit/setup_recovery/storage_system_tests.rs"]
mod tests;

impl crate::setup_recovery::SetupRecovery {
    /// Admit only the original child named by a durable new-installation intent.
    /// The returned descriptor is the storage SYSTEM, not the management/control root.
    pub fn storage_system(&self) -> Result<PrivateDirectory, ApiError> {
        self.storage_system_if_bound()?.ok_or_else(|| ApiError::conflict(
            storage_contract::ErrorCode::SetupProofInvalid,
            "storage SYSTEM has no durable initialization binding",
        ))
    }

    /// Absence is meaningful only before storage initialization. Existing unbound
    /// material is an error and is never interpreted as an alternate SYSTEM layout.
    pub fn storage_system_if_bound(&self) -> Result<Option<PrivateDirectory>, ApiError> {
        self.control_directory.verify_identity().map_err(control_publication_error)?;
        use crate::setup_recovery::{SetupIntent, activation::{
            ACTIVATION_INTENT, StorageActivationIntent, present,
        }};
        let activation = present(&self.data_root.join(ACTIVATION_INTENT))?;
        let setup = present(&self.data_root.join("setup-intent.json"))?;
        let recovery = present(&self.data_root.join(super::recovery::RECOVERY_INTENT))?;
        if recovery {
            let intent: super::recovery::StorageRecoveryIntent =
                self.read_durable_control(super::recovery::RECOVERY_INTENT)?;
            self.require_recovery_predecessor(&intent.control.predecessor)?;
            return StorageSystemBinding::from_evidence(&intent.control.binding)
                .open(&self.data_root).map(Some);
        }
        let binding = match (activation, setup) {
            (true, false) => {
                let intent: StorageActivationIntent = self.read_durable_control(ACTIVATION_INTENT)?;
                Some(intent.storage_system)
            }
            (false, true) => {
                let intent: SetupIntent = self.read_durable_control("setup-intent.json")?;
                Some(intent.storage_system)
            }
            // Direct setup creates storage from Empty; activation creates it only
            // from ManagementReady. A retained setup intent fences that state, so
            // these are distinct owners, never normal stages of the same operation.
            (true, true) => return Err(ApiError::conflict(
                storage_contract::ErrorCode::SetupProofInvalid,
                "multiple initialization owners cannot authorize a storage SYSTEM",
            )),
            (false, false) => None,
        };
        if present(&self.data_root.join("config.json"))? {
            return Err(ApiError::conflict(storage_contract::ErrorCode::SetupProofInvalid,
                "unbound storage configuration in control root is preserved but unsupported"));
        }
        match binding {
            Some(binding) => binding.open(&self.data_root).map(Some),
            None if present(&self.data_root.join(layout::SYSTEM.as_path()))? => {
                Err(ApiError::conflict(storage_contract::ErrorCode::SetupProofInvalid,
                    "existing storage SYSTEM has no original initialization binding"))
            }
            None => Ok(None),
        }
    }
}


pub(super) fn read_configuration(
    system: &PrivateDirectory,
) -> Result<storage_engine::Config, ApiError> {
    let bytes = system.read_bounded(&layout::CONFIG, crate::setup_recovery::MAX_LOCAL_CONFIG_BYTES)
        .map_err(control_publication_error)?;
    storage_contract::decode_strict(&bytes).map_err(ApiError::invalid_source)
}

pub(super) fn configuration_present(system: &PrivateDirectory) -> Result<bool, ApiError> {
    match system.read_bounded(&layout::CONFIG, crate::setup_recovery::MAX_LOCAL_CONFIG_BYTES) {
        Ok(bytes) => {
            let _: storage_engine::Config = storage_contract::decode_strict(&bytes)
                .map_err(ApiError::invalid_source)?;
            Ok(true)
        }
        Err(platform_io::fs_safety::Error::Io(error))
            if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(control_publication_error(error)),
    }
}
