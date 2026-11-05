//! Cancellation retains the complete pending owner through every real write.
//! It never reopens an old target, erases a pending directory, or owns a task
//! whose I/O can survive the synchronous call's return.
use super::super::super::decision::{DecisionStore, TerminalResumeChoice};
use super::super::super::terminal_resume::cancellation::{
    ResumeCancellation, ResumeCancellationIntent, attempts,
};
use super::*;

const CANCEL_INTENT_FILE: &str = "resume-cancel-intent.json";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalMetadataResumeCancellationPhase {
    Cancellable,
    Cancelling,
    ResumeQuorumAccepted,
    GenerationSelected,
    Cancelled,
}

impl TerminalMetadataResumeCancellationPhase {
    pub fn cancellation_available(self) -> bool {
        matches!(self, Self::Cancellable | Self::Cancelling)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum TerminalMetadataResumeCancelOutcome {
    Cancelled(OperationResult),
    ContinueOriginal(TerminalMetadataResumeStatus),
}

impl TerminalMetadataResume {
    /// Read actual held roots and exact SYSTEM evidence. This chooses nothing,
    /// synchronizes nothing, and does not infer absence from a cached vote map.
    pub fn cancellation_phase(&self) -> Result<TerminalMetadataResumeCancellationPhase> {
        let attempts = attempts::load(
            self.management.system_root(),
            self.management.verified_authority(),
        )?;
        if let Some(cancelled) = attempts.cancelled.iter().find(|cancelled| {
            cancelled.proposal.intent.operation_id == self.proposal.intent.operation_id
        }) {
            self.verify_cancelled_request(cancelled)?;
            return Ok(TerminalMetadataResumeCancellationPhase::Cancelled);
        }
        self.require_selected(&attempts)?;
        if self
            .management
            .system_root()
            .open_optional_read_regular(&attempts.path.join("resume-generation.json"))?
            .is_some()
        {
            activation::terminal_resume::selected_portable(
                self.management.system_root(),
                self.management.verified_authority(),
            )?;
            return Ok(TerminalMetadataResumeCancellationPhase::GenerationSelected);
        }
        let decisions = self.cancellation_observations()?;
        if decisions
            .values()
            .filter(|choice| matches!(choice, TerminalResumeChoice::Resume))
            .count()
            >= self.proposal.intent.target.metadata_quorum()
        {
            return Ok(TerminalMetadataResumeCancellationPhase::ResumeQuorumAccepted);
        }
        if self.cancel_started(&attempts.path)?
            || decisions
                .values()
                .any(|choice| matches!(choice, TerminalResumeChoice::Cancel))
        {
            return Ok(TerminalMetadataResumeCancellationPhase::Cancelling);
        }
        Ok(TerminalMetadataResumeCancellationPhase::Cancellable)
    }

    /// The exact original actor/op/digest must be supplied. Failure keeps this
    /// owner intact. A fixed new quorum or selected generation is never revoked.
    pub fn cancel(
        &mut self,
        principal: &str,
        operation_id: OperationId,
        transition_hash: &str,
    ) -> Result<TerminalMetadataResumeCancelOutcome> {
        require_request(&self.proposal, principal, operation_id, transition_hash)?;
        let attempts = attempts::load(
            self.management.system_root(),
            self.management.verified_authority(),
        )?;
        if let Some(cancelled) = attempts
            .cancelled
            .iter()
            .find(|cancelled| cancelled.proposal.intent.operation_id == operation_id)
        {
            self.verify_cancelled_request(cancelled)?;
            let result = cancelled.result()?;
            attempts::synchronize_history(
                self.management.system_root(),
                self.management.verified_authority(),
            )?;
            self.management.refresh_cancelled_resume_results()?;
            return Ok(TerminalMetadataResumeCancelOutcome::Cancelled(result));
        }
        self.require_selected(&attempts)?;
        if self
            .management
            .system_root()
            .open_optional_read_regular(&attempts.path.join("resume-generation.json"))?
            .is_some()
        {
            activation::terminal_resume::selected_portable(
                self.management.system_root(),
                self.management.verified_authority(),
            )?;
            return Ok(TerminalMetadataResumeCancelOutcome::ContinueOriginal(
                self.status()?,
            ));
        }
        let choices = self.cancellation_observations()?;
        let available = choices
            .values()
            .filter(|choice| !matches!(choice, TerminalResumeChoice::Resume))
            .count();
        if available < self.proposal.cancellation_threshold() {
            return Ok(TerminalMetadataResumeCancelOutcome::ContinueOriginal(
                self.status()?,
            ));
        }
        self.proposal.preflight_outcomes(&attempts.cancelled)?;
        // This intent fences later Resume retries before the first local choice.
        // It is not a completed cancellation and cannot release any ownership.
        self.management.system_root().write_json(
            &attempts.path.join(CANCEL_INTENT_FILE),
            &ResumeCancellationIntent::new(&self.proposal)?,
            false,
        )?;
        let decisions = self.choose_cancellation(choices)?;
        let cancelled = ResumeCancellation::new(self.proposal.clone(), decisions)?;
        attempts::synchronize_history(
            self.management.system_root(),
            self.management.verified_authority(),
        )?;
        self.management.system_root().write_json(
            &attempts.path.join(attempts::CANCEL_FILE),
            &cancelled,
            false,
        )?;
        let saved = attempts::load(
            self.management.system_root(),
            self.management.verified_authority(),
        )?;
        let completed = saved
            .cancelled
            .iter()
            .find(|item| item.proposal.intent.operation_id == operation_id)
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::RecoveryRequired,
                    "resume cancellation completion was not selected",
                )
            })?;
        if completed.hash()? != cancelled.hash()? {
            return Err(Error::new(
                ErrorCode::LogFork,
                "resume cancellation changed during completion",
            ));
        }
        let result = completed.result()?;
        self.management.refresh_cancelled_resume_results()?;
        Ok(TerminalMetadataResumeCancelOutcome::Cancelled(result))
    }

    /// Only after a durable Cancelled result. Actual pending descriptors and
    /// fences are destroyed before returning the still-authenticated management
    /// owner; no target file or old stop record is changed by this handoff.
    pub fn into_cancelled_management(self) -> Result<TerminalManagement> {
        if self.cancellation_phase()? != TerminalMetadataResumeCancellationPhase::Cancelled {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "resume has not completed cancellation",
            ));
        }
        // A readable certificate can follow a failed file/directory barrier.
        // Reestablish durability before returning a usable management owner.
        attempts::synchronize_history(
            self.management.system_root(),
            self.management.verified_authority(),
        )?;
        let Self {
            mut management,
            source,
            proposal: _,
            members,
            votes: _,
            installed: _,
            ready_data_disks: _,
            core: _,
        } = self;
        drop(members);
        drop(source);
        management.refresh_cancelled_resume_results()?;
        Ok(management)
    }

    pub(super) fn require_resume_allowed(&self) -> Result<()> {
        match self.cancellation_phase()? {
            TerminalMetadataResumeCancellationPhase::Cancelling
            | TerminalMetadataResumeCancellationPhase::Cancelled => Err(Error::new(
                ErrorCode::RecoveryRequired,
                "terminal resume cancellation is irreversible; continue the original cancellation",
            )),
            TerminalMetadataResumeCancellationPhase::Cancellable
            | TerminalMetadataResumeCancellationPhase::ResumeQuorumAccepted
            | TerminalMetadataResumeCancellationPhase::GenerationSelected => Ok(()),
        }
    }

    fn require_selected(&self, attempts: &attempts::ResumeAttempts) -> Result<()> {
        if attempts
            .current
            .as_ref()
            .map(ResumeProposal::hash)
            .transpose()?
            .as_ref()
            != Some(&self.proposal.hash()?)
        {
            return Err(Error::new(
                ErrorCode::LogFork,
                "pending owner does not hold the selected resume attempt",
            ));
        }
        Ok(())
    }

    fn verify_cancelled_request(&self, cancelled: &ResumeCancellation) -> Result<()> {
        if cancelled.proposal.hash()? != self.proposal.hash()? {
            return Err(Error::new(
                ErrorCode::LogFork,
                "cancelled receipt differs from this held resume request",
            ));
        }
        Ok(())
    }

    fn cancel_started(&self, path: &Path) -> Result<bool> {
        let Some(file) = self
            .management
            .system_root()
            .open_optional_read_regular(&path.join(CANCEL_INTENT_FILE))?
        else {
            return Ok(false);
        };
        let intent: ResumeCancellationIntent =
            decode_strict(&read_file_bytes(file, MAX_METADATA)?)?;
        intent.verify(&self.proposal)?;
        Ok(true)
    }

    fn cancellation_observations(&self) -> Result<BTreeMap<String, TerminalResumeChoice>> {
        for member in self.members.values() {
            reservation::verify_writer(&member.root, &member._lock)?;
            admission::inspect_new_root(&member.root, &member.seal, &member.reservation_name)?;
        }
        let transition = object_hash(INTENT_KIND, &self.proposal.intent)?;
        let mut decisions = BTreeMap::new();
        for disk in self.proposal.intent.target.metadata_members() {
            let member = &self.members[&disk.disk_id];
            let choice = DecisionStore::observe_terminal_resume(
                &member.root,
                &member.seal.identity,
                &transition,
                &self.proposal.hash()?,
                &self.proposal.acceptance_hash(&disk.disk_id)?,
                &self.proposal.cancellation_hash()?,
                &self.proposal.cancel_acceptance(&disk.disk_id)?,
            )?;
            decisions.insert(disk.disk_id.clone(), choice);
        }
        Ok(decisions)
    }

    fn choose_cancellation(
        &self,
        choices: BTreeMap<String, TerminalResumeChoice>,
    ) -> Result<BTreeMap<String, super::super::super::decision::DecisionReceipt>> {
        let mut decisions = BTreeMap::new();
        let transition = object_hash(INTENT_KIND, &self.proposal.intent)?;
        for (disk_id, choice) in choices {
            if matches!(choice, TerminalResumeChoice::Resume) {
                continue;
            }
            let member = &self.members[&disk_id];
            reservation::verify_writer(&member.root, &member._lock)?;
            member
                .root
                .write_json(&member.root.path().join("disk.json"), &member.seal, false)?;
            let store =
                DecisionStore::open(&member.root, &member.seal.identity, &transition, true)?;
            let receipt = store.terminal_resume_cancel_receipt(
                &self.proposal.cancellation_hash()?,
                &self.proposal.cancel_acceptance(&disk_id)?,
            )?;
            decisions.insert(disk_id, receipt);
        }
        Ok(decisions)
    }
}

fn require_request(
    proposal: &ResumeProposal,
    principal: &str,
    operation_id: OperationId,
    transition_hash: &str,
) -> Result<()> {
    Engine::require_actor(principal)?;
    if proposal.intent.operation_id != operation_id
        || proposal.intent.principal != principal
        || object_hash(INTENT_KIND, &proposal.intent)? != transition_hash
    {
        return Err(Error::new(
            ErrorCode::IdempotencyKeyReused,
            "terminal cancellation differs from its original operation, actor or request digest",
        ));
    }
    Ok(())
}

impl TerminalManagement {
    /// Pure protected-SYSTEM lookup for response loss and cold HTTP recovery.
    /// A missing result does not certify that a pending request is cancellable.
    pub fn metadata_resume_cancelled(
        root: &Path,
        principal: &str,
        operation_id: OperationId,
        transition_hash: &str,
    ) -> Result<Option<OperationResult>> {
        Engine::require_actor(principal)?;
        let (system, verified) = Self::verified_completed_source(root)?;
        if let Some(result) = verified.state.operations.get(&operation_id) {
            if result.kind != "terminal-metadata-resume-cancelled" {
                return Ok(None);
            }
            if result.request_digest != transition_hash
                || verified
                    .state
                    .operation_principals
                    .get(&operation_id)
                    .map(String::as_str)
                    != Some(principal)
            {
                return Err(Error::new(
                    ErrorCode::IdempotencyKeyReused,
                    "terminal cancellation differs from historical actor or request digest",
                ));
            }
            return Ok(Some(result.clone()));
        }
        let attempts = attempts::load(&system, &verified)?;
        let Some(cancelled) = attempts
            .cancelled
            .iter()
            .find(|item| item.proposal.intent.operation_id == operation_id)
        else {
            return Ok(None);
        };
        require_request(
            &cancelled.proposal,
            principal,
            operation_id,
            transition_hash,
        )?;
        cancelled.result().map(Some)
    }
}
