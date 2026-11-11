//! Explicit cancellation keeps the original owner until real immutable voter
//! decisions have excluded every possible new quorum. HTTP state is not a vote.
use super::*;
use storage_engine::TerminalMetadataResumeCancelOutcome;

impl Inner {
    pub(super) fn run_terminal_resume_cancel(
        self: &Arc<Self>,
        identity: &Identity,
        intent: &mut StoredAddition,
        directory: &PrivateDirectory,
    ) -> Result<OperationResult, ApiError> {
        let mut pending = match self.acquire_terminal_resume(identity, intent, None, directory)? {
            ResumeOwner::Pending(pending) => *pending,
            // A permanent publication cannot be relabelled as cancelled.
            ResumeOwner::Published(result) => return Ok(*result),
        };
        let cancelled = (|| {
            self.current_management_identity(identity)?;
            bind_held_resume(&pending, intent)?;
            intent.status.phase = AdditionPhase::CertifyingAbort;
            save(directory, intent)?;
            let transition = intent
                .archive
                .as_ref()
                .expect("invariant: held resume bound before cancellation")
                .transition_hash
                .clone();
            self.with_terminal_authorization(identity, || {
                pending.cancel(&identity.principal, intent.status.operation_id, &transition)
            })
        })();
        match cancelled {
            Err(error) => {
                // This includes partial cancellation votes and HTTP persistence
                // failures. Keep every actual directory FD and writer lock.
                self.restore_resume(pending);
                Err(error)
            }
            Ok(TerminalMetadataResumeCancelOutcome::ContinueOriginal(_status)) => {
                intent.status.phase = AdditionPhase::ResumeQuorumAccepted;
                self.restore_resume(pending);
                Err(ApiError::conflict(
                    ErrorCode::RecoveryRequired,
                    "the new fixed quorum already chose this metadata resume; cancellation cannot reverse it, explicitly continue the original recovery",
                ))
            }
            Ok(TerminalMetadataResumeCancelOutcome::Cancelled(result)) => {
                let result = match exact_result(result, intent) {
                    Ok(result) => result,
                    Err(error) => {
                        self.restore_resume(pending);
                        return Err(error);
                    }
                };
                let management = match pending
                    .into_cancelled_management()
                    .map_err(crate::engine_backend::error)
                {
                    Ok(management) => management,
                    Err(error) => {
                        crate::lock(&self.state).backend = BackendState::ResumeRecovery;
                        return Err(error);
                    }
                };
                self.install_terminal(management);
                Ok(result)
            }
        }
    }
}
