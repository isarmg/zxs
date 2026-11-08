//! The complete current terminal chain selects one exact original directory.
//! Actual immutable target votes are not an active generation or a writer token.
use super::*;
use super::{
    archive::persistent::PendingControlArchive,
    decision::{DecisionReceipt, DecisionStore},
    pending::{MemberIdentity, PendingMemberDirectory, admission, reservation},
    terminal::{TerminalAuthority, VerifiedTerminalAuthority},
};
use crate::engine::disk_lifecycle::terminal::{
    TerminalManagement, retirement::ChainReadmissionProposal,
};
use platform_io::fs_safety::linux::FileIdentity;
use std::path::{Path, PathBuf};
mod target;

const REQUEST: &str = "chain-readmission.json";
const RECEIPT: &str = "chain-readmission-target.json";
const SELECTION_KIND: &str = "terminal-chain-readmission-selection";
const CERTIFICATE_KIND: &str = "terminal-chain-readmission-target-certificate";
#[derive(Clone, Debug, Serialize, Deserialize)]
enum Format {
    #[serialization(rename = "terminal-chain-readmission-target")]
    Target,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Selection {
    format: Format,
    terminal: TerminalAuthority,
    proposal: ChainReadmissionProposal,
    directory: MemberIdentity,
    original_seal: DiskSeal,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Certificate {
    selection_hash: String,
    decision: DecisionReceipt,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(rename_all = "kebab-case")]
pub enum TerminalChainReadmissionPhase {
    SourceSelected,
    TargetDecisionPreserved,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct TerminalChainReadmissionStatus {
    pub preparation: crate::TerminalChainReadmissionPreparation,
    pub selection_hash: String,
    pub phase: TerminalChainReadmissionPhase,
    pub source_request_reserved: bool,
    pub target_decision_hash: Option<String>,
    pub target_reverified_this_call: bool,
    pub active_writer_admitted: bool,
    pub metadata_resumption_supported: bool,
}

fn operation(management: &TerminalManagement) -> PathBuf {
    terminal_resume::cancellation::attempts::operation_directory(
        management.system_root(),
        &management.verified_authority().authority,
    )
}
fn load<T: data_serialization::de::DeserializeOwned>(
    root: &DiskRoot,
    path: &Path,
) -> Result<Option<T>> {
    root.open_optional_read_regular(path)?
        .map(|file| decode_strict(&read_file_bytes(file, MAX_METADATA)?).map_err(Into::into))
        .transpose()
}
pub(in crate::engine) fn owns_initial_entry(name: &str) -> bool {
    matches!(name, REQUEST | RECEIPT)
}

impl Selection {
    fn hash(&self) -> Result<String> {
        Ok(object_hash(SELECTION_KIND, self)?)
    }
    fn verify(&self, source: &VerifiedTerminalAuthority) -> Result<()> {
        if self.terminal.hash()? != source.authority.hash()? {
            return Err(Error::new(
                ErrorCode::LogFork,
                "chain selection changed its original terminal authority",
            ));
        }
        self.proposal.verify_source(source)?;
        let config = self.proposal.configuration();
        let disk = self.proposal.target_disk();
        let physical = FileIdentity {
            device: self.original_seal.device,
            inode: self.original_seal.inode,
        };
        let mut expected = self.original_seal.clone();
        expected.format = self.proposal.seal_format().clone();
        expected.pool_id.clone_from(&config.pool_id);
        expected.disk_id.clone_from(&disk.disk_id);
        expected.slot_id.clone_from(&disk.slot_id);
        expected.binding_generation = disk.binding_generation;
        expected.simulation = config.mode == Mode::Development;
        validate_stored_seal(&self.original_seal, &expected)?;
        if self.directory != admission::member_identity(config, disk, physical) {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "chain selection changed its original physical directory",
            ));
        }
        Ok(())
    }
    fn acceptance(&self) -> Result<String> {
        #[derive(Serialize)]
        struct Acceptance<'a> {
            selection: &'a str,
            checkpoint: &'a str,
            directory: &'a MemberIdentity,
        }
        Ok(object_hash(
            "terminal-chain-readmission-acceptance",
            &Acceptance {
                selection: &self.hash()?,
                checkpoint: self.proposal.checkpoint_digest(),
                directory: &self.directory,
            },
        )?)
    }
    fn preflight(&self) -> Result<()> {
        let hash = self.hash()?;
        PreparedMetadataObject::new(SELECTION_KIND, self, MAX_METADATA)?;
        let certificate = Certificate {
            selection_hash: hash.clone(),
            decision: DecisionReceipt::plan_terminal_chain(
                &hash,
                &self.directory,
                &hash,
                &self.acceptance()?,
            )?,
        };
        PreparedMetadataObject::new(CERTIFICATE_KIND, &certificate, MAX_METADATA)?;
        Ok(())
    }
    fn verify_certificate(&self, certificate: &Certificate) -> Result<()> {
        let hash = self.hash()?;
        if certificate.selection_hash != hash {
            return Err(Error::new(
                ErrorCode::LogFork,
                "chain target certificate changed the selected source",
            ));
        }
        certificate.decision.verify_terminal_chain(
            &hash,
            &self.directory,
            &hash,
            &self.acceptance()?,
        )
    }
    fn status(
        &self,
        certificate: Option<&Certificate>,
        fresh: bool,
    ) -> Result<TerminalChainReadmissionStatus> {
        if let Some(certificate) = certificate {
            self.verify_certificate(certificate)?;
        }
        Ok(TerminalChainReadmissionStatus {
            preparation: self.proposal.preparation()?,
            selection_hash: self.hash()?,
            phase: if certificate.is_some() {
                TerminalChainReadmissionPhase::TargetDecisionPreserved
            } else {
                TerminalChainReadmissionPhase::SourceSelected
            },
            source_request_reserved: true,
            target_decision_hash: certificate.map(|c| c.decision.hash().into()),
            target_reverified_this_call: fresh,
            active_writer_admitted: false,
            metadata_resumption_supported: false,
        })
    }
}

fn selected(management: &TerminalManagement) -> Result<Option<Selection>> {
    management.verify_held_terminal_writer()?;
    let root = management.system_root();
    let selection: Option<Selection> = load(root, &operation(management).join(REQUEST))?;
    let certificate: Option<Certificate> = load(root, &operation(management).join(RECEIPT))?;
    let Some(selection) = selection else {
        if certificate.is_some() {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "chain target certificate lost its source selection",
            ));
        }
        return Ok(None);
    };
    selection.verify(management.verified_authority())?;
    let (_, current) = management.current_retirement_checkpoint()?;
    if current.digest() != selection.proposal.checkpoint_digest() {
        return Err(Error::new(
            ErrorCode::GenerationMismatch,
            "chain selection does not name the complete current terminal source",
        ));
    }
    if let Some(certificate) = certificate {
        selection.verify_certificate(&certificate)?;
    }
    Ok(Some(selection))
}
pub(in crate::engine) fn validate_selection(management: &TerminalManagement) -> Result<()> {
    selected(management).map(|_| ())
}

impl TerminalManagement {
    pub(in crate::engine) fn chain_readmission_selected(&self) -> Result<bool> {
        let root = self.system_root();
        for name in [REQUEST, RECEIPT] {
            // Presence, even corrupt/partial, is restrictive until the original
            // native consumer validates it. It can never mean a vacant source.
            if root
                .optional_metadata(&operation(self).join(name))?
                .is_some()
            {
                return Ok(true);
            }
        }
        Ok(false)
    }
    pub(in crate::engine) fn require_no_chain_readmission(&self) -> Result<()> {
        if self.chain_readmission_selected()? {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "continue the exact selected terminal-chain readmission; source is reserved",
            ));
        }
        Ok(())
    }
    /// Source-only inspection of the original pending request/certificate.
    /// It does not open the target or refresh its live decision durability.
    pub fn chain_readmission_status(&self) -> Result<Option<TerminalChainReadmissionStatus>> {
        selected(self)?
            .map(|selection| {
                let certificate: Option<Certificate> =
                    load(self.system_root(), &operation(self).join(RECEIPT))?;
                selection.status(certificate.as_ref(), false)
            })
            .transpose()
    }

    /// Explicitly selects or continues exactly the original retained directory.
    /// Returns target preparation only. No active seal, locator or Engine exists.
    pub fn synchronize_chain_readmission_target(
        &mut self,
        principal: &str,
        operation_id: OperationId,
        proposal_digest: &str,
        directory: PendingMemberDirectory,
    ) -> Result<TerminalChainReadmissionStatus> {
        Engine::require_actor(principal)?;
        let prior = selected(self)?;
        let proposal = match &prior {
            Some(selected) => selected.proposal.clone(),
            None => ChainReadmissionProposal::decode(
                self.prepare_chain_readmission_proposal(
                    &directory.disk_id,
                    principal,
                    operation_id,
                )?
                .bytes(),
            )?,
        };
        if proposal.principal() != principal
            || proposal.operation() != operation_id
            || proposal.hash()? != proposal_digest
            || proposal.target().disk_id != directory.disk_id
        {
            return Err(Error::new(
                ErrorCode::IdempotencyKeyReused,
                "chain readmission differs from its exact original actor/operation/proposal",
            ));
        }
        let held = target::Held::open(self, &proposal, directory)?;
        let selection = Selection {
            format: Format::Target,
            terminal: self.verified_authority().authority.clone(),
            proposal,
            directory: held.identity.clone(),
            original_seal: held.seal.clone(),
        };
        selection.verify(self.verified_authority())?;
        selection.preflight()?;
        if let Some(prior) = prior
            && prior.hash()? != selection.hash()?
        {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "original chain readmission target changed; replacement is forbidden",
            ));
        }
        // The original source request precedes *every* target write. An uncertain
        // return is recovered only by this exact immutable operation and root.
        self.system_root()
            .write_json(&operation(self).join(REQUEST), &selection, false)?;
        require_selected(self, &selection)?;
        held.synchronize(self, &selection)
    }
}

fn require_selected(management: &TerminalManagement, expected: &Selection) -> Result<()> {
    let actual = selected(management)?.ok_or_else(|| {
        Error::new(
            ErrorCode::RecoveryRequired,
            "chain source selection disappeared",
        )
    })?;
    if actual.hash()? != expected.hash()? {
        return Err(Error::new(
            ErrorCode::LogFork,
            "chain target operation lost its exact source selection",
        ));
    }
    Ok(())
}
