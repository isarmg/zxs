//! Explicit readmission of a retained Disabled directory into a new I/O lifetime.
//! The old identity, stop, decisions, archives and closed gates remain immutable.
use super::archive::persistent::{
    ArchiveRoot, PendingControlArchive, abort_view::PortableAbortArchive, valid_hash,
};
use super::decision::{DecisionReceipt, DecisionStore};
use super::pending::activation::PortableAuthority;
use super::pending::{MemberIdentity, PendingMemberDirectory};
use super::*;
use crate::engine::disk_registry::published::continuation::RegistryBoundary;
use crate::engine::disk_registry::{
    DiskRegistrationIdentity, DiskRegistryCommitment, VerifiedDiskRegistry,
};
use platform_io::fs_safety::linux::FileIdentity;
use std::{collections::BTreeSet, path::Path, sync::Arc};

mod core;
mod probe;
mod runtime;
pub(in crate::engine::membership_transition) use core::ReadmissionCore;
pub(in crate::engine::membership_transition) use probe::persist_probe_admission;
pub(in crate::engine::membership_transition) use probe::persist_terminal_probe_admission;
pub(in crate::engine) use probe::{
    ReadmissionProbeAdmission, RecoveredReadmission, persist_recovered_admissions,
};
pub(in crate::engine::membership_transition) use runtime::status;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiskReadmissionPhase {
    Fenced,
    Activating,
    Published,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiskReadmissionStatus {
    pub operation_id: OperationId,
    /// Original actor from the authenticated core intent, not a management cache.
    pub principal: String,
    pub transition_hash: String,
    pub target: DiskRegistrationIdentity,
    pub phase: DiskReadmissionPhase,
    pub metadata_member: bool,
    /// BLAKE3 of the serialized original Config from the admitted transition.
    pub source_configuration_digest: String,
    pub source_root_path: std::path::PathBuf,
    pub source_membership_authority_hash: String,
    pub source_registry_root_hash: String,
    pub source_registry_generation: u64,
    pub source_sequence: u64,
    pub source_topology_generation: u64,
    pub source_snapshot_hash: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(in crate::engine) struct ReadmissionRequest {
    pub(in crate::engine) target: DiskRegistrationIdentity,
    pub(in crate::engine) directory: MemberIdentity,
    pub(super) original_seal: DiskSeal,
    pub(super) predecessor_registry: DiskRegistryCommitment,
    pub(in crate::engine) stopped_operation: OperationId,
    pub(in crate::engine) stopped_transition: String,
    pub(in crate::engine) previous_admission: Option<String>,
    pub(super) role: ReadmissionRole,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(rename_all = "kebab-case")]
pub(super) enum ReadmissionRole {
    DataOnly,
    Metadata,
}

impl ReadmissionRequest {
    pub(super) fn verify_target(&self, intent: &TransitionIntent) -> Result<()> {
        let source = intent.origin.config();
        let target = &intent.target;
        Engine::require_actor(&intent.principal)?;
        if intent.effect.is_some()
            || intent.fence_scope.is_some()
            || !intent.added_roots.is_empty()
            || intent.origin.predecessor().is_none()
            || intent.boundary.is_none()
            || !pending::admission::same_settings(source, target)
            || source.disks.len() != target.disks.len()
            || self.target.pool_id != source.pool_id
            || !valid_hash(&self.stopped_transition)
            || self
                .previous_admission
                .as_ref()
                .is_some_and(|hash| !valid_hash(hash))
        {
            return Err(Error::new(
                ErrorCode::ConfigConflict,
                "invalid readmission source or target scope",
            ));
        }
        self.predecessor_registry.validate()?;
        let mut found = false;
        for disk in &source.disks {
            let mut expected = disk.clone();
            if disk.disk_id == self.target.disk_id {
                found = true;
                if disk.metadata_member
                    || disk.slot_id != self.target.slot_id
                    || disk.binding_generation != self.target.binding_generation
                {
                    return Err(Error::new(
                        ErrorCode::ConfigConflict,
                        "readmission does not name a stopped registration",
                    ));
                }
                expected.metadata_member = self.role == ReadmissionRole::Metadata;
                let physical = FileIdentity {
                    device: self.original_seal.device,
                    inode: self.original_seal.inode,
                };
                if self.directory
                    != pending::admission::member_identity(target, &expected, physical)
                {
                    return Err(Error::new(
                        ErrorCode::DiskIdentityMismatch,
                        "readmission directory identity differs",
                    ));
                }
                let mut seal = self.original_seal.clone();
                seal.pool_id = target.pool_id.clone();
                seal.disk_id = expected.disk_id.clone();
                seal.slot_id = expected.slot_id.clone();
                seal.binding_generation = expected.binding_generation;
                seal.simulation = target.mode == Mode::Development;
                validate_stored_seal(&self.original_seal, &seal)?;
            }
            if !target
                .disks
                .iter()
                .any(|new| pending::admission::same_disk(&expected, new))
            {
                return Err(Error::new(
                    ErrorCode::ConfigConflict,
                    "readmission changes historical topology or another registration",
                ));
            }
        }
        if !found
            || intent.fence_sequence
                <= intent
                    .boundary
                    .as_ref()
                    .expect("invariant: boundary checked")
                    .sequence
        {
            return Err(Error::new(
                ErrorCode::ConfigConflict,
                "readmission target or frozen sequence is absent",
            ));
        }
        Ok(())
    }
}

pub(super) fn request(intent: &TransitionIntent) -> Result<&ReadmissionRequest> {
    match &intent.origin {
        Origin::Readmission { request, .. } => Ok(request),
        _ => Err(Error::new(
            ErrorCode::ConfigConflict,
            "authority is not a directory readmission",
        )),
    }
}

/// Issued only after real independent old-member decisions and installed full
/// archives have been checked, not by deserializing a portable certificate.
pub(in crate::engine) struct AdmittedReadmission {
    request: ReadmissionRequest,
    operation: OperationId,
    transition: String,
}
impl AdmittedReadmission {
    pub(in crate::engine) fn request(&self) -> &ReadmissionRequest {
        &self.request
    }
    pub(in crate::engine) fn operation(&self) -> OperationId {
        self.operation
    }
    pub(in crate::engine) fn transition(&self) -> &str {
        &self.transition
    }
}

impl AdmittedReadmission {
    pub(in crate::engine::membership_transition) fn verify<'a>(
        portable: &PortableAuthority,
        mut root_for: impl FnMut(&DiskConfig) -> Result<&'a DiskRoot>,
    ) -> Result<Self> {
        let core = portable.readmission_core().ok_or_else(|| {
            Error::new(ErrorCode::ConfigConflict, "expected readmission authority")
        })?;
        let intent = core.intent()?;
        let request = request(&intent)?;
        let generation = portable.hash()?;
        let transition = object_hash(INTENT_KIND, &intent)?;
        let target = intent
            .target
            .disks
            .iter()
            .find(|disk| disk.disk_id == request.target.disk_id)
            .expect("invariant: verified readmission target");
        for disk in intent
            .origin
            .config()
            .metadata_members()
            .chain(std::iter::once(target))
        {
            let root = root_for(disk)?;
            core.verify_member(root, disk, &generation, true)?;
            let path = pending::member_directory(disk, &transition)
                .join(pending::CONTROL_ARCHIVE_DIRECTORY);
            let (state, _) = super::archive::persistent::read_generation_base(
                root.private_child(&path)?,
                &portable.archive_hash()?,
                &transition,
                &intent,
            )?;
            core.verify_source(&state)?;
        }
        Ok(Self {
            request: request.clone(),
            operation: intent.operation_id,
            transition,
        })
    }
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/engine/disk_readmission_tests.rs"]
mod tests;
