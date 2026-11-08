//! A retained registration is a separate admission, never a fresh directory.
//! Portable evidence selects one exact closed lifetime; live decision rereads
//! issue the narrower probe permit before a new epoch can enter the locator.
use super::*;
use crate::engine::disk_lifecycle::terminal::TerminalManagement;
use crate::engine::disk_registry::published::terminal::TerminalRegistry;
use crate::engine::disk_registry::{DiskRegistrationIdentity, PreparedDiskRegistry};
use crate::engine::membership_transition::readmission::{ReadmissionRequest, ReadmissionRole};
use platform_io::fs_safety::linux::FileIdentity;
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalMetadataResumeAdmission {
    FreshDirectories,
    RetainedMetadata { target: DiskRegistrationIdentity },
}

/// A SYSTEM-only projection. This has no directory handle or admission method.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalReadmissionPreview {
    pub target: DiskRegistrationIdentity,
    pub metadata_member: bool,
    pub root: PathBuf,
    pub terminal_authority_hash: String,
    pub registry: DiskRegistryCommitment,
    pub source_sequence: u64,
    pub topology_generation: u64,
    pub snapshot_hash: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serialization(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub(in crate::engine::membership_transition) enum ResumeAdmission {
    #[default]
    FreshDirectories,
    RetainedMetadata {
        request: Box<ReadmissionRequest>,
    },
}

impl TerminalManagement {
    pub fn readmission_metadata_member(&self, disk_id: &str) -> Result<Option<bool>> {
        if self.terminal_retirement_status()?.is_some() {
            return Ok(None);
        }
        let registry = &self.verified_authority().registry;
        let Some(definition) = registry
            .historical_topology()
            .find(|disk| disk.identity.disk_id == disk_id)
        else {
            return Ok(None);
        };
        Ok((definition.original_metadata_member
            && registry.registration(&definition.identity)?
                == Some(crate::RegistrationMode::Disabled))
        .then_some(true))
    }
    pub fn preview_disk_readmission(&self, disk_id: &str) -> Result<TerminalReadmissionPreview> {
        self.require_no_terminal_retirement()?;
        TerminalReadmissionPreview::from_verified(self.verified_authority(), disk_id)
    }
}

impl TerminalReadmissionPreview {
    pub(in crate::engine) fn from_verified(
        verified: &VerifiedTerminalAuthority,
        disk_id: &str,
    ) -> Result<Self> {
        let result = verified.authority.management_result(&verified.state)?;
        let target = retained_identity(&verified.registry, disk_id)?;
        let disk = verified
            .authority
            .original()
            .disks
            .iter()
            .find(|disk| disk.disk_id == disk_id)
            .expect("invariant: terminal registry binds its historical configuration");
        let boundary = verified
            .authority
            .intent()
            .boundary
            .as_ref()
            .expect("invariant: verified terminal boundary");
        Ok(TerminalReadmissionPreview {
            target,
            metadata_member: true,
            root: disk.root.clone(),
            terminal_authority_hash: verified.authority.hash()?,
            registry: verified.registry.commitment().clone(),
            source_sequence: result.sequence,
            // management_result checked this same frozen generation for overflow.
            topology_generation: verified.state.topology_generation + 1,
            snapshot_hash: boundary.snapshot_hash.clone(),
        })
    }
}

fn retained_identity(
    registry: &TerminalRegistry,
    disk_id: &str,
) -> Result<DiskRegistrationIdentity> {
    let definition = registry
        .historical_topology()
        .find(|disk| disk.identity.disk_id == disk_id)
        .ok_or_else(|| {
            Error::new(
                ErrorCode::ConfigConflict,
                "terminal readmission has no historical registration",
            )
        })?;
    if !definition.original_metadata_member
        || registry.registration(&definition.identity)? != Some(crate::RegistrationMode::Disabled)
    {
        return Err(Error::new(
            ErrorCode::ConfigConflict,
            "terminal readmission requires a retained Disabled metadata registration; Detached cannot return",
        ));
    }
    Ok(definition.identity.clone())
}

pub(in crate::engine::membership_transition) fn stopped_source(
    terminal: &TerminalAuthority,
    identity: &DiskRegistrationIdentity,
) -> Result<(OperationId, String, DiskSealFormat)> {
    if terminal.target() == *identity {
        if terminal.action() != MembershipExitAction::PlannedOffline {
            return Err(Error::new(
                ErrorCode::ConfigConflict,
                "Detached registration cannot be readmitted",
            ));
        }
        return Ok((
            terminal.operation_id(),
            terminal.transition_hash()?,
            terminal.intent().origin.predecessor().map_or(
                DiskSealFormat::ManagedDirectory,
                |hash| DiskSealFormat::MembershipGeneration {
                    authority_hash: hash.into(),
                },
            ),
        ));
    }
    terminal
        .predecessor()
        .ok_or_else(|| {
            Error::new(
                ErrorCode::PublicationInvalid,
                "retained registration has no authenticated stop lineage",
            )
        })?
        .stopped_readmission_source(identity)
}

impl ResumeAdmission {
    pub(super) fn is_fresh(&self) -> bool {
        matches!(self, Self::FreshDirectories)
    }
    pub(in crate::engine::membership_transition) fn request(&self) -> Option<&ReadmissionRequest> {
        match self {
            Self::FreshDirectories => None,
            Self::RetainedMetadata { request } => Some(request),
        }
    }

    pub(super) fn verify_target(
        &self,
        terminal: &TerminalAuthority,
        intent: &TransitionIntent,
    ) -> Result<()> {
        let Some(request) = self.request() else {
            return validate_target(terminal.original(), &intent.target);
        };
        let original = terminal.original();
        if !intent.added_roots.is_empty()
            || !admission::same_settings(original, &intent.target)
            || original.disks.len() != intent.target.disks.len()
            || intent.target.metadata_members().len() != 1
            || request.role != ReadmissionRole::Metadata
            || request.predecessor_registry != *terminal.registry_commitment()
            || terminal.io_admission_epoch(&request.target.disk_id)? != request.previous_admission
        {
            return Err(Error::new(
                ErrorCode::ConfigConflict,
                "terminal readmission changes another binding or admission lifetime",
            ));
        }
        let (operation, stopped, format) = stopped_source(terminal, &request.target)?;
        if operation != request.stopped_operation
            || stopped != request.stopped_transition
            || format != request.original_seal.format
        {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "terminal readmission does not consume its exact original stop",
            ));
        }
        let mut found = false;
        for old in &original.disks {
            let mut expected = old.clone();
            expected.metadata_member = old.disk_id == request.target.disk_id;
            if expected.metadata_member {
                found = true;
                let physical = FileIdentity {
                    device: request.original_seal.device,
                    inode: request.original_seal.inode,
                };
                let identity = DiskRegistrationIdentity {
                    pool_id: original.pool_id.clone(),
                    disk_id: old.disk_id.clone(),
                    slot_id: old.slot_id.clone(),
                    binding_generation: old.binding_generation,
                };
                let mut seal = request.original_seal.clone();
                seal.pool_id.clone_from(&identity.pool_id);
                seal.disk_id.clone_from(&identity.disk_id);
                seal.slot_id.clone_from(&identity.slot_id);
                seal.binding_generation = identity.binding_generation;
                seal.simulation = original.mode == Mode::Development;
                validate_stored_seal(&request.original_seal, &seal)?;
                if identity != request.target
                    || request.directory
                        != admission::member_identity(&intent.target, &expected, physical)
                {
                    return Err(Error::new(
                        ErrorCode::DiskIdentityMismatch,
                        "terminal readmission changes the original directory identity",
                    ));
                }
            }
            if !intent
                .target
                .disks
                .iter()
                .any(|disk| admission::same_disk(&expected, disk))
            {
                return Err(Error::new(
                    ErrorCode::ConfigConflict,
                    "terminal readmission changes historical topology or another directory",
                ));
            }
        }
        if !found {
            return Err(Error::new(
                ErrorCode::ConfigConflict,
                "terminal readmission target is absent",
            ));
        }
        Ok(())
    }

    pub(super) fn prepare_registry(
        &self,
        terminal: &TerminalRegistry,
        boundary: RegistryBoundary<'_>,
        target: &ValidatedConfig,
    ) -> Result<PreparedDiskRegistry> {
        match self.request() {
            None => terminal.prepare_resume(boundary, target),
            Some(request) => terminal.prepare_retained_resume(boundary, target, &request.target),
        }
    }

    pub(super) fn verify_registry(
        &self,
        terminal: &TerminalRegistry,
        boundary: RegistryBoundary<'_>,
        target: &ValidatedConfig,
        expected: &DiskRegistryCommitment,
        read: impl FnMut(&'static str, &str) -> Result<Vec<u8>>,
    ) -> Result<VerifiedDiskRegistry> {
        match self.request() {
            None => terminal.verify_resume_at(boundary, target, expected, read),
            Some(request) => terminal.verify_retained_resume_at(
                boundary,
                target,
                &request.target,
                expected,
                read,
            ),
        }
    }
}

impl ResumeProposal {
    pub(in crate::engine::membership_transition) fn retained_request(
        &self,
    ) -> Option<&ReadmissionRequest> {
        self.admission.request()
    }

    pub(in crate::engine::membership_transition) fn admission(
        &self,
    ) -> TerminalMetadataResumeAdmission {
        match self.retained_request() {
            None => TerminalMetadataResumeAdmission::FreshDirectories,
            Some(request) => TerminalMetadataResumeAdmission::RetainedMetadata {
                target: request.target.clone(),
            },
        }
    }

    pub(in crate::engine::membership_transition) fn member_identity(
        &self,
        disk_id: &str,
    ) -> Result<&MemberIdentity> {
        self.retained_request()
            .filter(|request| request.target.disk_id == disk_id)
            .map(|request| &request.directory)
            .or_else(|| self.intent.added_roots.get(disk_id))
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::DiskIdentityMismatch,
                    "resume member is outside its exact admitted root set",
                )
            })
    }
}

impl ResumeCore {
    pub(in crate::engine) fn retained_event(
        &self,
    ) -> Result<Option<(&ReadmissionRequest, OperationId, String)>> {
        let intent = self.intent()?;
        self.retained_request()
            .map(|request| {
                Ok((
                    request,
                    intent.operation_id,
                    object_hash(INTENT_KIND, &intent)?,
                ))
            })
            .transpose()
    }
    pub(in crate::engine) fn retained_request(&self) -> Option<&ReadmissionRequest> {
        self.proposal.retained_request()
    }
    pub(in crate::engine::membership_transition) fn member_identity(
        &self,
        disk_id: &str,
    ) -> Result<&MemberIdentity> {
        self.proposal.member_identity(disk_id)
    }

    pub(in crate::engine::membership_transition) fn verify_retained_seal(
        &self,
        root: &DiskRoot,
        disk: &DiskConfig,
    ) -> Result<()> {
        let Some(request) = self
            .retained_request()
            .filter(|request| request.target.disk_id == disk.disk_id)
        else {
            return Ok(());
        };
        let stored = decode_seal(&root.read_bytes(&root.path().join("disk.json"), MAX_METADATA)?)?;
        let mut expected = request.original_seal.clone();
        if stored.format != expected.format {
            expected.format = DiskSealFormat::MembershipGeneration {
                authority_hash: object_hash(pending::activation::AUTHORITY_KIND, self)?,
            };
        }
        validate_stored_seal(&stored, &expected)
    }
}

/// No Deserialize and no public constructor. The portable votes alone cannot
/// mint this proof: every configured new voter is reread at its physical root.
pub(in crate::engine) struct AdmittedTerminalReadmission {
    request: ReadmissionRequest,
    operation: OperationId,
    transition: String,
    generation: String,
}
impl AdmittedTerminalReadmission {
    pub(in crate::engine::membership_transition) fn verify<'a>(
        core: &ResumeCore,
        mut root_for: impl FnMut(&DiskConfig) -> Result<&'a DiskRoot>,
    ) -> Result<Option<Self>> {
        let intent = core.intent()?;
        let Some(request) = core.retained_request() else {
            return Ok(None);
        };
        for disk in intent.target.metadata_members() {
            core.verify_member(root_for(disk)?, disk)?;
        }
        Ok(Some(Self {
            request: request.clone(),
            operation: intent.operation_id,
            transition: object_hash(INTENT_KIND, &intent)?,
            generation: object_hash(pending::activation::AUTHORITY_KIND, core)?,
        }))
    }
    pub(in crate::engine) fn request(&self) -> &ReadmissionRequest {
        &self.request
    }
    pub(in crate::engine) fn operation(&self) -> OperationId {
        self.operation
    }
    pub(in crate::engine) fn transition(&self) -> &str {
        &self.transition
    }
    pub(in crate::engine) fn generation(&self) -> &str {
        &self.generation
    }
}
