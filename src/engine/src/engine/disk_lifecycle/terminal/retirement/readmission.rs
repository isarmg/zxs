//! A complete retirement checkpoint can prepare one exact retained metadata
//! successor. This is not ResumeCore: no decision, active Current or I/O token.
use super::*;
use crate::{
    disk::{DiskSealFormat, MAX_METADATA},
    engine::membership_transition::terminal_resume::stopped_source,
    journal::PreparedMetadataObject,
};
use std::collections::BTreeMap;

const KIND: &str = "terminal-chain-retained-readmission-proposal";
#[derive(Clone, Debug, Serialize, Deserialize)]
enum ProposalFormat {
    #[serialization(rename = "terminal-chain-retained-readmission-proposal")]
    Prepared,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct StoppedLifetime {
    operation_id: OperationId,
    transition_hash: String,
    seal_format: DiskSealFormat,
    previous_admission: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct GraphObject {
    kind: String,
    value: json_codec::Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(in crate::engine) struct Proposal {
    format: ProposalFormat,
    operation_id: OperationId,
    principal: String,
    source: RetirementCheckpointProof,
    checkpoint_digest: String,
    target: DiskRegistrationIdentity,
    stopped: StoppedLifetime,
    configuration: ValidatedConfig,
    registry: DiskRegistryCommitment,
    objects: BTreeMap<String, GraphObject>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct TerminalChainReadmissionPreparation {
    pub proposal_digest: String,
    pub operation_id: OperationId,
    pub principal: String,
    pub source: TerminalRetirementCheckpoint,
    pub target: DiskRegistrationIdentity,
    pub target_root: std::path::PathBuf,
    pub proposed_registry: DiskRegistryCommitment,
    pub proposed_configuration_hash: String,
    pub previous_admission: Option<String>,
    pub complete_namespace_projection_prepared: bool,
    pub target_directory_verified: bool,
    pub target_decision_recorded: bool,
    pub active_namespace_admitted: bool,
    pub metadata_resumption_supported: bool,
}

/// Only an authenticated source capture or strict native replay can construct
/// these bytes. No commit/activate conversion exists in this preparation slice.
pub struct TerminalChainReadmissionProposal {
    bytes: Vec<u8>,
    report: TerminalChainReadmissionPreparation,
}
impl TerminalChainReadmissionProposal {
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn preparation(&self) -> &TerminalChainReadmissionPreparation {
        &self.report
    }
}

impl TerminalManagement {
    /// Captures the whole current settled chain and prepares only one original
    /// Disabled metadata registration. Neither target nor ancestor paths open.
    pub fn prepare_chain_readmission_proposal(
        &self,
        disk_id: &str,
        principal: &str,
        operation_id: OperationId,
    ) -> Result<TerminalChainReadmissionProposal> {
        Engine::require_actor(principal)?;
        self.require_no_chain_readmission()?;
        let (source, checkpoint) = self.current_retirement_checkpoint()?;
        let (target, configuration, stopped) =
            select_target(&self.verified, &checkpoint, disk_id, operation_id)?;
        let prepared =
            TerminalRegistry::prepare_checkpoint_readmission(&checkpoint, &configuration, &target)?;
        let objects = prepared
            .objects()
            .map(|(kind, hash, bytes)| {
                Ok((
                    hash.to_owned(),
                    GraphObject {
                        kind: kind.to_owned(),
                        value: crate::decode_strict(bytes)?,
                    },
                ))
            })
            .collect::<Result<_>>()?;
        let proposal = Proposal {
            format: ProposalFormat::Prepared,
            operation_id,
            principal: principal.into(),
            source,
            checkpoint_digest: checkpoint.digest().into(),
            target,
            stopped,
            configuration,
            registry: prepared.commitment().clone(),
            objects,
        };
        // Reuse the typed complete replay; do not clone/replay frozen State for
        // every candidate graph check. Persisted ingress repeats full replay.
        proposal.verify_projection(&self.verified, &checkpoint)?;
        let object = PreparedMetadataObject::new(KIND, &proposal, MAX_METADATA)?;
        self.store.verify_writer(&self._locks)?;
        Ok(TerminalChainReadmissionProposal {
            report: proposal.report(object.hash.clone())?,
            bytes: object.bytes().to_vec(),
        })
    }

    /// Checks a historical proposal relative to the original trusted control
    /// archive and an independently retained digest. Does not assert latest-ness.
    pub fn verify_chain_readmission_proposal(
        &self,
        bytes: &[u8],
        expected_digest: &str,
    ) -> Result<TerminalChainReadmissionPreparation> {
        self.store.verify_writer(&self._locks)?;
        expected_digest
            .parse::<storage_contract::ContentHash>()
            .map_err(|source| {
                Error::from(std::io::Error::other(source)).context(
                    ErrorCode::ObjectCorrupt,
                    "invalid trusted chain readmission proposal digest",
                )
            })?;
        if bytes.len() > MAX_METADATA {
            return Err(failure(
                ErrorCode::ResourceLimit,
                "chain readmission proposal exceeds metadata budget",
            ));
        }
        let proposal: Proposal = crate::decode_strict(bytes)?;
        let object = PreparedMetadataObject::new(KIND, &proposal, MAX_METADATA)?;
        if object.hash != expected_digest {
            return Err(failure(
                ErrorCode::ObjectCorrupt,
                "chain readmission proposal differs from its trusted digest",
            ));
        }
        let checkpoint = proposal.source.verify(&self.verified)?;
        proposal.verify_projection(&self.verified, &checkpoint)?;
        proposal.report(object.hash)
    }
}

fn select_target(
    source: &VerifiedTerminalAuthority,
    checkpoint: &VerifiedRetirementCheckpoint,
    disk_id: &str,
    operation: OperationId,
) -> Result<(DiskRegistrationIdentity, ValidatedConfig, StoppedLifetime)> {
    if checkpoint.state().operations.contains_key(&operation)
        || checkpoint
            .state()
            .operation_principals
            .contains_key(&operation)
    {
        return Err(failure(
            ErrorCode::IdempotencyKeyReused,
            "chain readmission operation already belongs to permanent history",
        ));
    }
    let target = checkpoint
        .registry()
        .historical_topology()
        .find(|disk| disk.identity.disk_id == disk_id)
        .ok_or_else(|| {
            failure(
                ErrorCode::NotFound,
                "chain readmission has no original registration",
            )
        })?;
    if !target.original_metadata_member
        || checkpoint.registry().registration(&target.identity)?
            != Some(crate::RegistrationMode::Disabled)
    {
        return Err(failure(
            ErrorCode::ConfigConflict,
            "chain readmission requires retained Disabled metadata; Detached cannot return",
        ));
    }
    let authority = &source.authority;
    let (operation_id, transition_hash, seal_format) = stopped_source(authority, &target.identity)?;
    let stopped = StoppedLifetime {
        operation_id,
        transition_hash,
        seal_format,
        previous_admission: authority.io_admission_epoch(disk_id)?,
    };
    let mut config = (**authority.original()).clone();
    for disk in &mut config.disks {
        disk.metadata_member = disk.disk_id == disk_id;
    }
    Ok((
        target.identity.clone(),
        ValidatedConfig::try_from(config)?,
        stopped,
    ))
}

impl Proposal {
    fn verify_projection(
        &self,
        original: &VerifiedTerminalAuthority,
        checkpoint: &VerifiedRetirementCheckpoint,
    ) -> Result<()> {
        Engine::require_actor(&self.principal)?;
        if self.checkpoint_digest != checkpoint.digest() {
            return Err(failure(
                ErrorCode::LogFork,
                "readmission proposal checkpoint selection changed",
            ));
        }
        let (target, configuration, stopped) = select_target(
            original,
            checkpoint,
            &self.target.disk_id,
            self.operation_id,
        )?;
        if target != self.target
            || stopped != self.stopped
            || object_hash("membership-target", &configuration)?
                != object_hash("membership-target", &self.configuration)?
        {
            return Err(failure(
                ErrorCode::PublicationInvalid,
                "chain readmission changed original binding, stopped lifetime or configuration",
            ));
        }
        self.namespaces(checkpoint)?;
        Ok(())
    }
    fn read(&self, kind: &'static str, hash: &str) -> Result<Vec<u8>> {
        let value = self.objects.get(hash).ok_or_else(|| {
            failure(
                ErrorCode::ObjectCorrupt,
                "chain readmission registry object missing",
            )
        })?;
        if value.kind != kind {
            return Err(failure(
                ErrorCode::ObjectCorrupt,
                "chain readmission registry object kind changed",
            ));
        }
        let object = PreparedMetadataObject::new(kind, &value.value, MAX_METADATA)?;
        if object.hash != hash {
            return Err(failure(
                ErrorCode::ObjectCorrupt,
                "chain readmission registry object digest changed",
            ));
        }
        Ok(object.bytes().to_vec())
    }
    fn report(&self, digest: String) -> Result<TerminalChainReadmissionPreparation> {
        Ok(TerminalChainReadmissionPreparation {
            proposal_digest: digest,
            operation_id: self.operation_id,
            principal: self.principal.clone(),
            source: self.source.report(self.checkpoint_digest.clone()),
            target: self.target.clone(),
            target_root: self.target_disk().root.clone(),
            proposed_registry: self.registry.clone(),
            proposed_configuration_hash: object_hash("membership-target", &self.configuration)?,
            previous_admission: self.stopped.previous_admission.clone(),
            complete_namespace_projection_prepared: true,
            target_directory_verified: false,
            target_decision_recorded: false,
            active_namespace_admitted: false,
            metadata_resumption_supported: false,
        })
    }
}

impl Proposal {
    pub(in crate::engine) fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_METADATA {
            return Err(failure(
                ErrorCode::ResourceLimit,
                "chain proposal exceeds budget",
            ));
        }
        crate::decode_strict(bytes).map_err(Into::into)
    }
    pub(in crate::engine) fn hash(&self) -> Result<String> {
        Ok(object_hash(KIND, self)?)
    }
    pub(in crate::engine) fn verify_source(
        &self,
        source: &VerifiedTerminalAuthority,
    ) -> Result<()> {
        let checkpoint = self.source.verify(source)?;
        self.verify_projection(source, &checkpoint)
    }
    pub(in crate::engine) fn activation_source(
        &self,
        source: &VerifiedTerminalAuthority,
    ) -> Result<VerifiedRetirementCheckpoint> {
        let checkpoint = self.source.verify(source)?;
        self.verify_projection(source, &checkpoint)?;
        Ok(checkpoint)
    }
    pub(in crate::engine) fn namespaces(
        &self,
        checkpoint: &VerifiedRetirementCheckpoint,
    ) -> Result<crate::engine::disk_registry::current::PreparedCurrentNamespaces> {
        let registry = TerminalRegistry::verify_checkpoint_readmission(
            checkpoint,
            &self.configuration,
            &self.target,
            &self.registry,
            |kind, hash| self.read(kind, hash),
        )?;
        checkpoint
            .namespaces()
            .clone()
            .continue_terminal_registry(checkpoint.registry(), &registry)
    }
    pub(in crate::engine) fn visit_registry_objects(
        &self,
        visit: &mut impl FnMut(&str, &str, &[u8]) -> Result<()>,
    ) -> Result<()> {
        for (hash, object) in &self.objects {
            let bytes = crate::canonical_json(&object.value)?;
            visit(&object.kind, hash, &bytes)?;
        }
        Ok(())
    }
    pub(in crate::engine) fn checkpoint_digest(&self) -> &str {
        &self.checkpoint_digest
    }
    pub(in crate::engine) fn target(&self) -> &DiskRegistrationIdentity {
        &self.target
    }
    pub(in crate::engine) fn configuration(&self) -> &ValidatedConfig {
        &self.configuration
    }
    pub(in crate::engine) fn operation(&self) -> OperationId {
        self.operation_id
    }
    pub(in crate::engine) fn principal(&self) -> &str {
        &self.principal
    }
    pub(in crate::engine) fn seal_format(&self) -> &DiskSealFormat {
        &self.stopped.seal_format
    }
    pub(in crate::engine) fn preparation(&self) -> Result<TerminalChainReadmissionPreparation> {
        self.report(self.hash()?)
    }
    pub(in crate::engine) fn target_disk(&self) -> &crate::DiskConfig {
        self.configuration
            .disks
            .iter()
            .find(|disk| disk.disk_id == self.target.disk_id)
            .expect(
                "invariant: verified chain proposal selects a configured original metadata disk",
            )
    }
}
