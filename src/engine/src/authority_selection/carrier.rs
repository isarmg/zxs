//! A recovered SYSTEM is a successor carrier of the same immutable origin.
//! Both variants occupy the one initialized.json slot. Recovery never changes
//! the origin commitment used by DISK seals or the original bootstrap receipts.
use crate::authority_selection::{FreshBootstrapError, FreshSelection, SelectionGuard};
use data_serialization::{Deserialize, Serialize};
use platform_io::fs_safety::{EntryName, PayloadOpenMode, PrivateDirectory};
use platform_io::fs_safety::linux::{MountPolicy, OpenAt2Root};
use std::io::Write;
use storage_contract::{ContentHash, OperationId, Principal};
use storage_contract::working_tree::{LocalEntityIdentity, MaterialCommitment};

const MAX_BYTES: usize = 1024 * 1024;
type Result<T> = std::result::Result<T, FreshBootstrapError>;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
enum Format {
    #[serialization(rename = "unified-storage-system-v2")]
    UnifiedStorage,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
enum Carrier {
    #[serialization(rename = "recovery")]
    Recovery,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct RecoveryRequest {
    pub(crate) operation: OperationId,
    pub(crate) principal: Principal,
    pub(crate) control_intent: MaterialCommitment,
    pub(crate) source_evidence: MaterialCommitment,
    pub(crate) source_candidate: ContentHash,
    pub(crate) source_snapshot: ContentHash,
    pub(crate) system: LocalEntityIdentity,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct RecoverySelection {
    format: Format,
    carrier: Carrier,
    pub(crate) origin: FreshSelection,
    pub(crate) request: RecoveryRequest,
}

impl RecoverySelection {
    pub(crate) fn new(origin: FreshSelection, request: RecoveryRequest) -> Self {
        Self { format: Format::UnifiedStorage, carrier: Carrier::Recovery, origin, request }
    }

    pub(crate) fn bytes(&self) -> Result<Vec<u8>> {
        let bytes = storage_contract::canonical_json(self).map_err(FreshBootstrapError::Codec)?;
        if bytes.len() > MAX_BYTES {
            return Err(FreshBootstrapError::Format);
        }
        Ok(bytes)
    }

    /// Inspect the single selector. Absence of a Recovery carrier is accepted
    /// only for an actual valid Original selector, never for arbitrary JSON.
    pub(crate) fn read(directory: &PrivateDirectory) -> Result<Option<Self>> {
        let name = EntryName::new(crate::authority_selection::FRESH_SELECTOR)
            .map_err(FreshBootstrapError::Filesystem)?;
        let bytes = match directory.read_bounded(&name, MAX_BYTES) {
            Ok(bytes) => bytes,
            Err(platform_io::fs_safety::Error::Io(error))
                if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(FreshBootstrapError::Filesystem(error)),
        };
        let value = storage_contract::strict_json(&bytes).map_err(FreshBootstrapError::Codec)?;
        if value.get("carrier").is_none() {
            FreshSelection::read(directory)?;
            return Ok(None);
        }
        let selected: Self = storage_contract::decode_strict(&bytes)
            .map_err(FreshBootstrapError::Codec)?;
        if selected.bytes()? != bytes {
            return Err(FreshBootstrapError::Format);
        }
        Ok(Some(selected))
    }

    pub(crate) fn require_directory(&self, directory: &PrivateDirectory) -> Result<()> {
        directory.verify_identity().map_err(FreshBootstrapError::Filesystem)?;
        let held = OpenAt2Root::from_private_directory(
            directory.try_clone().map_err(FreshBootstrapError::Filesystem)?, MountPolicy::SameMount,
        ).map_err(FreshBootstrapError::Filesystem)?;
        let observed = held.identity();
        if self.request.system != LocalEntityIdentity::new(observed.device, observed.inode)
            || self.request.system == self.origin.system
            || self.origin.disks.iter().any(|disk| {
                disk.entity == self.request.system
                    || directory.path().starts_with(&disk.path)
                    || disk.path.starts_with(directory.path())
            })
        {
            return Err(FreshBootstrapError::Changed);
        }
        Ok(())
    }
}

impl SelectionGuard {
    /// Caller must retain the independently admitted original source owner.
    /// This persists intent only; it is not normal completion/write authority.
    pub(crate) fn select_recovery(mut self, expected: RecoverySelection) -> crate::Result<Self> {
        let select = || -> Result<()> {
            expected.require_directory(&self.directory)?;
            let root = OpenAt2Root::from_private_directory(
                self.directory.try_clone().map_err(FreshBootstrapError::Filesystem)?,
                MountPolicy::SameMount,
            ).map_err(FreshBootstrapError::Filesystem)?;
            if root.entries().map_err(FreshBootstrapError::Filesystem)?
                .next().transpose().map_err(FreshBootstrapError::Filesystem)?.is_some()
            {
                return Err(FreshBootstrapError::Occupied);
            }
            let name = EntryName::new(crate::authority_selection::FRESH_SELECTOR)
                .map_err(FreshBootstrapError::Filesystem)?;
            let mut file = self.directory.open_payload(&name, PayloadOpenMode::CreateNew)
                .map_err(FreshBootstrapError::Filesystem)?;
            // The existing deny name precedes every writer sidecar and payload.
            self.directory.sync().map_err(FreshBootstrapError::Filesystem)?;
            file.write_all(&expected.bytes()?)
                .map_err(|error| FreshBootstrapError::Filesystem(error.into()))?;
            file.sync_all().map_err(|error| FreshBootstrapError::Filesystem(error.into()))?;
            self.directory.verify_payload(&name, &file).map_err(FreshBootstrapError::Filesystem)?;
            self.directory.sync().map_err(FreshBootstrapError::Filesystem)?;
            if RecoverySelection::read(&self.directory)?.as_ref() != Some(&expected) {
                return Err(FreshBootstrapError::Changed);
            }
            Ok(())
        };
        select().map_err(|error| crate::Error::FreshBootstrap(Box::new(error)))?;
        self.fresh = Some(expected.origin.clone());
        self.recovery = Some(expected);
        self.first_write = true;
        Ok(self)
    }

    pub(crate) fn admit_recovery(mut self, expected: RecoverySelection) -> crate::Result<Self> {
        expected.require_directory(&self.directory)
            .map_err(|error| crate::Error::FreshBootstrap(Box::new(error)))?;
        if RecoverySelection::read(&self.directory)
            .map_err(|error| crate::Error::FreshBootstrap(Box::new(error)))?.as_ref()
            != Some(&expected)
        {
            return Err(crate::Error::FreshBootstrap(Box::new(FreshBootstrapError::Changed)));
        }
        self.fresh = Some(expected.origin.clone());
        self.recovery = Some(expected);
        self.first_write = false;
        Ok(self)
    }

}
