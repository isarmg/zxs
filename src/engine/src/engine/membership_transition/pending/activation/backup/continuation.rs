//! Admission of later ordinary slots in the SAME Backup generation. The first
//! immutable Candidate deliberately has no registry field; later Candidates
//! must carry the exact registry derived from that actual first Publication.
//! This is an in-memory native proof, never a selector or disk-I/O capability.
use super::*;
use crate::engine::membership_transition::{
    PreparedBackupFirstCandidate, PreparedBackupFirstPublication,
};
use crate::history_package::AdmittedState;
use crate::{DiskRegistryCommitment, VerifiedDiskRegistry};

#[derive(Clone, Debug)]
pub(in crate::engine) struct BackupContinuation {
    authority: String,
    first_sequence: u64,
    first_operation: OperationId,
    first_snapshot: String,
    registry: VerifiedDiskRegistry,
    targets: BTreeMap<String, Vec<String>>,
}

impl BackupContinuation {
    pub(super) fn capture(
        generation: &Generation,
        first: &PreparedBackupFirstCandidate,
        publication: &PreparedBackupFirstPublication,
        registry: VerifiedDiskRegistry,
        published: &AdmittedState,
    ) -> Result<Self> {
        publication.require_namespace_candidate(first)?;
        first.require_state(published)?;
        registry.require_backup_namespace(first, published)?;
        if !generation.portable.is_backup()
            || generation.hash != first.authority_hash()
            || generation.first_sequence()? != first.sequence()
            || generation.snapshot().as_deref() != first.candidate().previous_snapshot.as_deref()
        {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "Backup continuation does not extend its exact original first Publication",
            ));
        }
        generation.verify_activation_candidate(first.candidate())?;
        registry.require_config(&generation.portable.effective_config()?)?;
        Ok(Self {
            authority: generation.hash.clone(),
            first_sequence: first.sequence(),
            first_operation: first.candidate().operation_id,
            first_snapshot: first.snapshot_hash().into(),
            registry,
            targets: first
                .targets()
                .iter()
                .map(|target| {
                    (
                        target.identity.slot_id.clone(),
                        target.source_prefix.clone(),
                    )
                })
                .collect(),
        })
    }

    pub(in crate::engine::membership_transition::pending::activation) fn verify_later(
        &self,
        candidate: &Candidate,
    ) -> Result<()> {
        if candidate.sequence <= self.first_sequence
            || candidate.operation_id == self.first_operation
            || candidate.registry().is_none()
            || candidate.previous_snapshot.is_none()
            || (candidate.sequence
                == self.first_sequence.checked_add(1).ok_or_else(|| {
                    Error::new(
                        ErrorCode::ResourceLimit,
                        "Backup continuation sequence exhausted",
                    )
                })?
                && candidate.previous_snapshot.as_deref() != Some(self.first_snapshot.as_str()))
        {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "ordinary Backup continuation changes its first boundary or omits the published registry",
            ));
        }
        // This proves authority, exact registry and unchanged fixed voters.
        // Normal native replay must still prove every preceding snapshot,
        // operation and full payload vector; this is not a State shortcut.
        self.registry
            .verify_lifecycle_candidate(candidate, &self.authority)
    }
}

impl Generation {
    /// Physical namespace goals may use only the targets selected by this
    /// generation's actual first Publication, never a raw Config projection.
    pub(crate) fn backup_layout_targets(&self) -> Result<Option<&BTreeMap<String, Vec<String>>>> {
        if !self.portable.is_backup() {
            return Ok(None);
        }
        self.backup_continuation
            .as_ref()
            .map(|proof| Some(&proof.targets))
            .ok_or_else(BackupCore::activation_required)
    }

    /// Backup never borrows the Exit/Add registry in PortableAuthority. Its
    /// owned graph exists only after full first-Publication admission.
    pub(in crate::engine) fn registry(&self) -> Option<&DiskRegistryCommitment> {
        if self.portable.is_backup() {
            self.backup_continuation
                .as_ref()
                .map(|proof| proof.registry.commitment())
        } else {
            self.portable.exit_registry()
        }
    }

    pub(in crate::engine) fn verified_registry(&self) -> Result<Option<VerifiedDiskRegistry>> {
        if self.portable.is_backup() {
            return self
                .backup_continuation
                .as_ref()
                .map(|proof| Some(proof.registry.clone()))
                .ok_or_else(BackupCore::activation_required);
        }
        self.portable.verified_registry()
    }
}

impl BackupWriterIoConfig {
    /// Keep the target-only I/O restriction. Installing this native continuation
    /// does not add original roots, reopen SYSTEM or make an Engine writable.
    pub(in crate::engine) fn bind_published_continuation(
        mut self,
        first: &PreparedBackupFirstCandidate,
        publication: &PreparedBackupFirstPublication,
        registry: VerifiedDiskRegistry,
        published: &AdmittedState,
    ) -> Result<Self> {
        let mut generation = (**self
            .config
            .generation()
            .expect("invariant: native Backup generation"))
        .clone();
        generation.backup_continuation = Some(BackupContinuation::capture(
            &generation,
            first,
            publication,
            registry,
            published,
        )?);
        self.config = self
            .config
            .with_backup_continuation_generation(Arc::new(generation))?;
        Ok(self)
    }
}
