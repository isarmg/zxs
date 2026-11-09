//! Strict native Backup generation identity. This selected core closes the
//! original decision/archive/routing binding, not the missing activation I/O.
//! It cannot fall through to Add or inherit the old Config as a new writer.
use super::*;
use crate::engine::backup_preparation::{
    BackupGenerationControlReport, BackupGenerationPreparationReport,
};
use crate::engine::membership_transition::archive::persistent::ArchiveRoot;
use crate::engine::membership_transition::pending::activation::{
    AUTHORITY_KIND, PortableAuthority,
};
use registry::{AdmittedBackupDecision, BackupDecisionCertificate};
mod first_candidate;
mod replay;
pub(crate) use first_candidate::PreparedBackupFirstCandidate;

#[derive(Clone, Debug, Serialize, Deserialize)]
enum Format {
    #[serialization(rename = "backup-registry-generation")]
    BackupRegistryGeneration,
}

#[derive(Clone, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(in crate::engine::membership_transition) struct BackupCore {
    format: Format,
    decision: BackupDecisionCertificate,
    archive: ArchiveRoot,
    generation_inputs_hash: String,
    routing_snapshot_hash: String,
    control_inputs_hash: String,
}
impl std::fmt::Debug for BackupCore {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("BackupCore")
            .field("format", &self.format)
            .field("routing_snapshot_hash", &self.routing_snapshot_hash)
            .finish_non_exhaustive()
    }
}
fn require_hash(hash: &str) -> Result<()> {
    if hash.len() != 64
        || !hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(Error::new(
            ErrorCode::ObjectCorrupt,
            "Backup authority reference must be a canonical native digest",
        ));
    }
    Ok(())
}
impl BackupCore {
    /// A newly selected payload disk has an authority-specific I/O lifetime.
    /// Original disks retain their original epoch; none is retired by Backup.
    pub(in crate::engine::membership_transition) fn writer_epoch(
        &self,
        authority: &str,
        disk_id: &str,
    ) -> Result<Option<String>> {
        #[derive(Serialize)]
        struct Epoch<'a> {
            authority: &'a str,
            identity: &'a crate::DiskRegistrationIdentity,
        }
        let decision = AdmittedBackupDecision::from_certificate(self.decision.clone())?;
        Ok(decision
            .projection()
            .selected_nodes()
            .iter()
            .find(|node| node.identity.disk_id == disk_id)
            .map(|node| {
                object_hash(
                    "backup-target-io-epoch",
                    &Epoch {
                        authority,
                        identity: &node.identity,
                    },
                )
            })
            .transpose()?)
    }

    pub(in crate::engine::membership_transition) fn writer_targets(
        &self,
    ) -> Result<std::collections::BTreeSet<String>> {
        let decision = AdmittedBackupDecision::from_certificate(self.decision.clone())?;
        Ok(decision
            .projection()
            .selected_nodes()
            .into_iter()
            .map(|node| node.identity.disk_id)
            .collect())
    }

    pub(in crate::engine::membership_transition) fn intent(&self) -> Result<TransitionIntent> {
        let decision = AdmittedBackupDecision::from_certificate(self.decision.clone())?;
        let intent = decision.transition();
        if intent.origin.predecessor().is_some() {
            return Err(Error::new(
                ErrorCode::UnsupportedOperation,
                "Backup authority currently requires its complete fixed Genesis source",
            ));
        }
        self.archive.verify_scope(intent)?;
        for hash in [
            &self.generation_inputs_hash,
            &self.routing_snapshot_hash,
            &self.control_inputs_hash,
        ] {
            require_hash(hash)?;
        }
        Ok(intent.clone())
    }
    /// Exact selected fanout, preserving every original disk and voter. This
    /// supplies configuration semantics only; ordinary Backup registry/writer
    /// activation remains separately fenced and cannot borrow Genesis I/O.
    pub(in crate::engine::membership_transition) fn effective_config(
        &self,
    ) -> Result<ValidatedConfig> {
        let intent = self.intent()?;
        let decision = AdmittedBackupDecision::from_certificate(self.decision.clone())?;
        ValidatedConfig::for_backup_projection(intent.origin.config(), decision.projection())
    }

    pub(in crate::engine::membership_transition) fn archive(&self) -> &ArchiveRoot {
        &self.archive
    }
    pub(in crate::engine::membership_transition) fn verify_source(
        &self,
        state: &State,
    ) -> Result<()> {
        let intent = self.intent()?;
        let Origin::BackupCutover { evidence, .. } = &intent.origin else {
            unreachable!("invariant: admitted Backup certificate has a Backup origin");
        };
        let boundary = intent
            .boundary
            .as_ref()
            .expect("invariant: Backup source has a published boundary");
        if state.sequence != boundary.sequence
            || object_hash("backup-prepared-state", state)? != evidence.state_hash()
            || prepare_snapshot(state, MAX_METADATA)?.hash != boundary.snapshot_hash
        {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "Backup authority does not bind the complete original published State",
            ));
        }
        Ok(())
    }
    pub(in crate::engine::membership_transition) fn activation_required() -> Error {
        Error::new(
            ErrorCode::UnsupportedOperation,
            "Backup generation requires active Engine Config/CurrentRegistry and serving selection; task-local DiskWriter or a reserved first Publication cannot release the source fence",
        )
    }
}

/// Actual `membership-generation` canonical object, retained only under the
/// isolated target preparation namespace. No method installs generation.json,
/// a serving seal, mutable State, journal frame or DiskResources.
pub(in crate::engine) struct PreparedBackupAuthority {
    core: BackupCore,
    object: PreparedMetadataObject,
}
impl PreparedBackupAuthority {
    pub(in crate::engine) fn capture(
        decision: &AdmittedBackupDecision,
        control: &PreparedBackupControl,
        inputs: &BackupGenerationPreparationReport,
        observed: &BackupGenerationControlReport,
        source: &State,
    ) -> Result<Self> {
        let hash = decision.hash();
        if inputs.decision_hash != hash
            || observed.decision_hash != hash
            || inputs.operation_id != decision.operation()
            || observed.operation_id != decision.operation()
            || inputs.intended_first_sequence != decision.intended_first_sequence()?
            || inputs.targets != observed.targets
            || control.root_hash() != observed.native_control_root
        {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "Backup generation inputs do not bind one original selected decision",
            ));
        }
        let core = BackupCore {
            format: Format::BackupRegistryGeneration,
            decision: decision.certificate(),
            archive: control.archive().clone(),
            generation_inputs_hash: inputs.inputs_hash.clone(),
            routing_snapshot_hash: inputs.routing_snapshot_hash.clone(),
            control_inputs_hash: observed.inputs_hash.clone(),
        };
        let authority = PortableAuthority::new_backup(core.clone())?;
        authority.verify_registry_source(source)?;
        Ok(Self {
            core,
            object: PreparedMetadataObject::new(AUTHORITY_KIND, &authority, MAX_METADATA)?,
        })
    }
    pub(in crate::engine) fn hash(&self) -> &str {
        &self.object.hash
    }
    pub(in crate::engine) fn bytes(&self) -> &[u8] {
        self.object.bytes()
    }
    pub(in crate::engine) fn verify_saved(&self, bytes: &[u8]) -> Result<()> {
        if bytes.len() > MAX_METADATA {
            return Err(Error::new(
                ErrorCode::ResourceLimit,
                "Backup generation exceeds the native metadata reader limit",
            ));
        }
        let authority: PortableAuthority = crate::journal::decode_proof(bytes)?;
        if !authority.is_backup() || authority.hash()? != self.hash() || self.bytes() != bytes {
            return Err(Error::new(
                ErrorCode::ObjectCollision,
                "saved native Backup authority differs from this exact original task",
            ));
        }
        Ok(())
    }
}
