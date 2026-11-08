//! Ordinary generation/archive consumers share the original Backup recipe.
//! Nothing here opens a root, creates a vote, installs Current or grants I/O.
use super::*;
use crate::engine::membership_transition::backup::BackupCore;
use std::sync::atomic::AtomicBool;

impl Generation {
    pub(super) fn backup_first_state(&self, core: &BackupCore) -> Result<(State, OperationResult)> {
        let state = core.first_state(&self.hash, &self.base, &AtomicBool::new(false))?;
        let result = state
            .operations
            .get(&self.intent.operation_id)
            .expect("invariant: Backup first-state recipe inserted the original operation")
            .clone();
        Ok((state, result))
    }

    pub(super) fn verify_backup_first_candidate(
        &self,
        core: &BackupCore,
        candidate: &Candidate,
    ) -> Result<()> {
        // Reject wrong slots/member envelopes before expanding the complete
        // source graph. The shared recipe then verifies the frozen source and
        // all retained routing, receipts, principals, snapshots and payloads.
        self.verify_candidate(candidate)?;
        let authority = PreparedMetadataObject::new(AUTHORITY_KIND, &self.portable, MAX_METADATA)?;
        let expected = core.first_candidate(&authority, &self.base, &AtomicBool::new(false))?;
        if object_hash("candidate", candidate)? != expected.hash() {
            return Err(Error::new(
                ErrorCode::LogFork,
                "Backup first generation candidate is not the exact original whole-unit decision",
            ));
        }
        Ok(())
    }
}
