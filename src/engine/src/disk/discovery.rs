//! Read-only recognition of a held current-format disk directory and its recorded
//! role. This is not registration, a complete history proof or permission to write.
use super::*;
use crate::persist::layout;

/// Strength of the local configuration evidence, never an import permit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiskRoleEvidence {
    /// The initial seal binds identity but does not hash the full topology. This
    /// is a schema-checked, matching local declaration, not authenticated history.
    InitialConfiguration,
    /// The seal-bound generation graph and any data enrollment dependency verify.
    /// Business publications, latest HEAD and complete history are not inspected.
    MembershipGeneration { authority_hash: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedDiskRole {
    level: u32,
    upstream_slot_id: Option<String>,
    metadata_member: bool,
    evidence: DiskRoleEvidence,
}
impl RecordedDiskRole {
    pub fn level(&self) -> u32 {
        self.level
    }
    pub fn upstream_slot_id(&self) -> Option<&str> {
        self.upstream_slot_id.as_deref()
    }
    pub fn metadata_member(&self) -> bool {
        self.metadata_member
    }
    pub fn evidence(&self) -> &DiskRoleEvidence {
        &self.evidence
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecognizedDisk {
    identity: crate::DiskRegistrationIdentity,
    simulation: bool,
    role: RecordedDiskRole,
}
impl RecognizedDisk {
    pub fn identity(&self) -> &crate::DiskRegistrationIdentity {
        &self.identity
    }
    pub fn simulation(&self) -> bool {
        self.simulation
    }
    pub fn role(&self) -> &RecordedDiskRole {
        &self.role
    }
}

/// Read the current seal and local configuration, anchored to the caller's handle.
/// Unknown formats, symbolic links, unsafe permissions and stale physical
/// identities are rejected without scanning history, creating or repairing. No
/// saved path for another disk is opened. A role never proves complete importability.
pub fn recognize_disk(directory: File, path: PathBuf) -> Result<RecognizedDisk> {
    let metadata = directory.metadata()?;
    let root = DiskRoot::from_directory(path.clone(), directory)?;
    let bytes = root.read_bytes(&path.join(layout::DISK_IDENTITY.as_path()), MAX_METADATA)?;
    let seal = decode_seal(&bytes)?;
    validate_id(&seal.pool_id, "pool")?;
    validate_id(&seal.disk_id, "disk")?;
    name_policy::validate_slot(&seal.slot_id)?;
    if let DiskSealFormat::MembershipGeneration { authority_hash } = &seal.format {
        authority_hash
            .parse::<storage_contract::ContentHash>()
            .map_err(|source| {
                Error::from(std::io::Error::other(source)).context(
                    ErrorCode::DiskIdentityMismatch,
                    "disk format contains an invalid membership authority digest",
                )
            })?;
    }
    if seal.binding_generation == 0 || seal.device != metadata.dev() || seal.inode != metadata.ino()
    {
        return Err(Error::new(
            ErrorCode::DiskIdentityMismatch,
            "recognized disk seal differs from its held directory identity",
        ));
    }
    let config =
        crate::engine::membership_transition::pending::activation::read_local_disk_configuration(
            &path,
            &seal,
            &mut |file| root.read_bytes(file, MAX_METADATA),
        )?;
    if matches!(seal.format, DiskSealFormat::ManagedDirectory)
        && root
            .open_optional_read_regular(&path.join("metadata/pool-membership/generation.json"))?
            .is_some()
    {
        return Err(Error::new(
            ErrorCode::RecoveryRequired,
            "an unpromoted generation cannot be classified as an initial disk",
        ));
    }
    let disk = config
        .disks
        .iter()
        .find(|disk| disk.disk_id == seal.disk_id)
        .expect("invariant: local configuration verified this seal");
    let role = RecordedDiskRole {
        level: disk.level,
        upstream_slot_id: disk.upstream_slot_id.clone(),
        metadata_member: disk.metadata_member,
        evidence: match seal.format {
            DiskSealFormat::ManualContainerDrafts { .. } => {
                return Err(crate::engine::container_drafts::format_error(
                    crate::authority_selection::manual::ManualFormatError::UnsupportedReader,
                ));
            }
            DiskSealFormat::ManagedDirectory => DiskRoleEvidence::InitialConfiguration,
            DiskSealFormat::MembershipGeneration { authority_hash } => {
                DiskRoleEvidence::MembershipGeneration { authority_hash }
            }
        },
    };
    if root.read_bytes(&path.join(layout::DISK_IDENTITY.as_path()), MAX_METADATA)? != bytes {
        return Err(Error::new(
            ErrorCode::DiskIdentityMismatch,
            "disk seal changed during role discovery",
        ));
    }
    // Recheck the named binding without transferring reads to the reopened root.
    DiskRoot::from_directory(path.clone(), root.directory(&path)?)?;
    Ok(RecognizedDisk {
        identity: crate::DiskRegistrationIdentity {
            pool_id: seal.pool_id,
            disk_id: seal.disk_id,
            slot_id: seal.slot_id,
            binding_generation: seal.binding_generation,
        },
        simulation: seal.simulation,
        role,
    })
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/disk/discovery_tests.rs"]
mod tests;
