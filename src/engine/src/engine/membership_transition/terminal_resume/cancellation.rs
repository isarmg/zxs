//! An immutable blocking set of NEW membership choices cancels one exact
//! unpublished request. The old terminal quorum never votes in the new slots.
use super::*;

pub(in crate::engine) mod attempts;

const CORE_KIND: &str = "terminal-resume-cancellation";
const ACCEPTANCE_KIND: &str = "terminal-resume-cancel-acceptance";
const CERTIFICATE_KIND: &str = "terminal-resume-cancel-certificate";

#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(in crate::engine::membership_transition) struct ResumeCancellationIntent {
    proposal_hash: String,
    cancellation_hash: String,
}
impl ResumeCancellationIntent {
    pub(in crate::engine::membership_transition) fn new(proposal: &ResumeProposal) -> Result<Self> {
        Ok(Self {
            proposal_hash: proposal.hash()?,
            cancellation_hash: proposal.cancellation_hash()?,
        })
    }
    pub(in crate::engine::membership_transition) fn verify(
        &self,
        proposal: &ResumeProposal,
    ) -> Result<()> {
        if self.proposal_hash != proposal.hash()?
            || self.cancellation_hash != proposal.cancellation_hash()?
        {
            return Err(Error::new(
                ErrorCode::LogFork,
                "terminal cancellation intent names another request",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
enum CancellationFormat {
    #[serialization(rename = "cancelled-terminal-resume")]
    Cancelled,
}

#[derive(Serialize)]
struct CancellationCore<'a> {
    proposal_hash: &'a str,
    terminal_authority_hash: &'a str,
    transition_hash: &'a str,
}

#[derive(Serialize)]
struct Acceptance<'a> {
    cancellation_hash: &'a str,
    identity: &'a MemberIdentity,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(in crate::engine) struct ResumeCancellation {
    format: CancellationFormat,
    pub(in crate::engine::membership_transition) proposal: ResumeProposal,
    decisions: BTreeMap<String, DecisionReceipt>,
}

impl ResumeProposal {
    /// Reserve enough bounded encoding space for either complete outcome before
    /// consuming any immutable vote. These planned values never leave this
    /// function or replace DecisionStore's actual read-back receipts.
    pub(in crate::engine::membership_transition) fn preflight_outcomes(
        &self,
        cancelled: &[ResumeCancellation],
    ) -> Result<()> {
        let transition = object_hash(INTENT_KIND, &self.intent)?;
        let mut resume = BTreeMap::new();
        let mut cancel = BTreeMap::new();
        for disk in self.intent.target.metadata_members() {
            let identity = self.member_identity(&disk.disk_id)?;
            resume.insert(
                disk.disk_id.clone(),
                DecisionReceipt::plan_terminal_resume(
                    &transition,
                    identity,
                    &self.hash()?,
                    &self.acceptance_hash(&disk.disk_id)?,
                )?,
            );
            if self.retained_request().is_none() {
                cancel.insert(
                    disk.disk_id.clone(),
                    DecisionReceipt::plan_terminal_resume_cancel(
                        &transition,
                        identity,
                        &self.cancellation_hash()?,
                        &self.cancel_acceptance(&disk.disk_id)?,
                    )?,
                );
            }
        }
        ResumeCore::with_cancellations(self.clone(), resume, cancelled.to_vec())?;
        if self.retained_request().is_none() {
            ResumeCancellation::new(self.clone(), cancel)?;
        }
        Ok(())
    }
    pub(in crate::engine::membership_transition) fn follow_cancellation(
        &mut self,
        hash: Option<String>,
    ) {
        self.previous_cancellation_hash = hash;
    }
    pub(in crate::engine::membership_transition) fn cancellation_hash(&self) -> Result<String> {
        object_hash(
            CORE_KIND,
            &CancellationCore {
                proposal_hash: &self.hash()?,
                terminal_authority_hash: &self.terminal.hash()?,
                transition_hash: &object_hash(INTENT_KIND, &self.intent)?,
            },
        )
        .map_err(Into::into)
    }

    pub(in crate::engine::membership_transition) fn cancel_acceptance(
        &self,
        disk_id: &str,
    ) -> Result<String> {
        let identity = self.intent.added_roots.get(disk_id).ok_or_else(|| {
            Error::new(
                ErrorCode::DiskIdentityMismatch,
                "cancellation is not a fresh resume directory",
            )
        })?;
        object_hash(
            ACCEPTANCE_KIND,
            &Acceptance {
                cancellation_hash: &self.cancellation_hash()?,
                identity,
            },
        )
        .map_err(Into::into)
    }

    pub(in crate::engine::membership_transition) fn cancellation_threshold(&self) -> usize {
        self.intent.target.metadata_members().len() - self.intent.target.metadata_quorum() + 1
    }

    pub(in crate::engine::membership_transition) fn cancelled_result(
        &self,
    ) -> Result<OperationResult> {
        Ok(OperationResult {
            operation_id: self.intent.operation_id,
            kind: "terminal-metadata-resume-cancelled".into(),
            commit_id: None,
            history_tree_id: None,
            branch_id: None,
            commit_state: storage_contract::CommitState::Published,
            protection_at_publish: storage_contract::ProtectionAtPublish::NotApplicable,
            current_health: storage_contract::CurrentHealth::Unknown,
            request_digest: object_hash(INTENT_KIND, &self.intent)?,
            // This management receipt consumes no active journal sequence and
            // changes neither business state nor the terminal topology.
            sequence: self.intent.fence_sequence,
            result: json_codec::json!({
                "state": "cancelled",
                "authority_kind": "terminal-resume-cancellation",
                "terminal_authority": self.terminal.hash()?,
                "proposal": self.hash()?,
                "cancellation": self.cancellation_hash()?,
                "metadata_members": self.intent.target.metadata_members().len(),
                "metadata_quorum": self.intent.target.metadata_quorum(),
                "writable": false,
            }),
        })
    }
}

impl ResumeCore {
    pub(in crate::engine::membership_transition) fn with_cancellations(
        proposal: ResumeProposal,
        votes: BTreeMap<String, DecisionReceipt>,
        cancelled_attempts: Vec<ResumeCancellation>,
    ) -> Result<Self> {
        let core = Self {
            format: ResumeFormat::TerminalResume,
            proposal,
            votes,
            cancelled_attempts,
        };
        core.intent()?;
        PreparedMetadataObject::new(pending::activation::AUTHORITY_KIND, &core, MAX_METADATA)?;
        Ok(core)
    }
}

impl ResumeCancellation {
    pub(in crate::engine::membership_transition) fn new(
        proposal: ResumeProposal,
        decisions: BTreeMap<String, DecisionReceipt>,
    ) -> Result<Self> {
        let result = Self {
            format: CancellationFormat::Cancelled,
            proposal,
            decisions,
        };
        result.verify()?;
        PreparedMetadataObject::new(CERTIFICATE_KIND, &result, MAX_METADATA)?;
        Ok(result)
    }

    pub(in crate::engine) fn hash(&self) -> Result<String> {
        object_hash(CERTIFICATE_KIND, self).map_err(Into::into)
    }

    pub(in crate::engine) fn verify(&self) -> Result<()> {
        self.proposal.verify()?;
        if self.proposal.retained_request().is_some() {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "fresh resume cancellation cannot certify retained readmission",
            ));
        }
        if self.decisions.len() < self.proposal.cancellation_threshold() {
            return Err(Error::new(
                ErrorCode::MetadataQuorumUnavailable,
                "resume cancellation does not block every fixed new quorum",
            ));
        }
        let transition = object_hash(INTENT_KIND, &self.proposal.intent)?;
        let cancellation = self.proposal.cancellation_hash()?;
        for (disk_id, decision) in &self.decisions {
            let disk = self
                .proposal
                .intent
                .target
                .metadata_members()
                .find(|disk| &disk.disk_id == disk_id)
                .ok_or_else(|| {
                    Error::new(
                        ErrorCode::PublicationInvalid,
                        "nonvoter in terminal resume cancellation",
                    )
                })?;
            let identity = self
                .proposal
                .intent
                .added_roots
                .get(&disk.disk_id)
                .ok_or_else(|| {
                    Error::new(
                        ErrorCode::PublicationInvalid,
                        "retained roots cannot vote in fresh resume cancellation",
                    )
                })?;
            decision.verify_terminal_resume_cancel(
                &transition,
                identity,
                &cancellation,
                &self.proposal.cancel_acceptance(disk_id)?,
            )?;
        }
        Ok(())
    }

    pub(in crate::engine) fn result(&self) -> Result<OperationResult> {
        self.verify()?;
        self.proposal.cancelled_result()
    }
}

/// Checks a linear hash-linked chain. Cancelled operations are permanent and
/// cannot be hidden by a later request, actor change, or successful resumption.
pub(super) fn verify_history(
    terminal: &TerminalAuthority,
    cancelled: &[ResumeCancellation],
    selected: Option<&ResumeProposal>,
) -> Result<()> {
    let authority_hash = terminal.hash()?;
    let mut previous = None;
    let mut operations = BTreeSet::from([terminal.operation_id()]);
    for cancellation in cancelled {
        cancellation.verify()?;
        if cancellation.proposal.terminal.hash()? != authority_hash
            || cancellation.proposal.previous_cancellation_hash != previous
            || !operations.insert(cancellation.proposal.intent.operation_id)
        {
            return Err(Error::new(
                ErrorCode::LogFork,
                "terminal resume cancellation history is not the exact successor chain",
            ));
        }
        previous = Some(cancellation.hash()?);
    }
    if let Some(selected) = selected
        && (selected.terminal.hash()? != authority_hash
            || selected.previous_cancellation_hash != previous
            || !operations.insert(selected.intent.operation_id))
    {
        return Err(Error::new(
            ErrorCode::LogFork,
            "terminal resume request omits or reuses its cancelled predecessor",
        ));
    }
    Ok(())
}

/// Terminal completion has no selected resumption proposal. Keep that private
/// proposal type inside the resumption owner while validating the same chain.
pub(in crate::engine) fn verify_cancelled_history(
    terminal: &TerminalAuthority,
    cancelled: &[ResumeCancellation],
) -> Result<()> {
    verify_history(terminal, cancelled, None)
}

pub(in crate::engine) fn apply_history(
    state: &mut State,
    cancelled: &[ResumeCancellation],
) -> Result<()> {
    for cancellation in cancelled {
        let result = cancellation.result()?;
        if state.operations.contains_key(&result.operation_id)
            || state
                .operation_principals
                .contains_key(&result.operation_id)
        {
            return Err(Error::new(
                ErrorCode::IdempotencyKeyReused,
                "cancelled resume operation already belongs to permanent history",
            ));
        }
        state.operation_principals.insert(
            result.operation_id,
            cancellation.proposal.intent.principal.clone(),
        );
        state.operations.insert(result.operation_id, result);
    }
    Ok(())
}

impl ResumeCancellation {
    pub(in crate::engine) fn verify_frozen_source(&self, source: &State) -> Result<()> {
        self.proposal.verify_source(source)
    }
}
