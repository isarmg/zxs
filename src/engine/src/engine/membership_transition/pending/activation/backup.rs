//! Ordinary generation/archive consumers share the original Backup recipe.
//! The separate sealed writer projection replays a held archive and grants
//! only selected-target task I/O. It never selects Engine Current or a vote.
use super::*;
use crate::engine::membership_transition::backup::BackupCore;
use std::sync::atomic::AtomicBool;

mod continuation;
pub(in crate::engine) use continuation::BackupContinuation;
mod locator;
pub(in crate::engine) mod startup;
pub(in crate::engine) use locator::PreparedBackupTargetLocator;

/// Private task capability, not an Engine config or a persisted selector.
/// Construction consumes a native first Publication at its caller and replays
/// the actual frozen archive before granting any target-only I/O context.
pub(in crate::engine) struct BackupWriterIoConfig {
    config: ValidatedConfig,
}

impl BackupWriterIoConfig {
    pub(in crate::engine::membership_transition) fn capture(
        portable: PortableAuthority,
        archive: DiskRoot,
        first: &Candidate,
        stop: &AtomicBool,
    ) -> Result<Self> {
        if !portable.is_backup() {
            return Err(BackupCore::activation_required());
        }
        crate::history_package::preparation_checkpoint(stop)?;
        let (base, history) = portable.read_archive_base(archive)?;
        crate::history_package::preparation_checkpoint(stop)?;
        let config = portable.effective_config()?;
        let generation = Arc::new(Generation {
            backup_continuation: None,
            hash: portable.hash()?,
            workspace_roots: portable.workspace_root_aliases()?,
            intent: portable.intent()?,
            original: portable.genesis_config()?,
            epoch: portable.epoch()?,
            portable,
            history,
            base: Arc::new(base),
        });
        generation.verify_activation_candidate(first)?;
        Ok(Self {
            config: config.with_backup_writer_generation(generation)?,
        })
    }

    pub(in crate::engine) fn config(&self) -> &ValidatedConfig {
        &self.config
    }
}

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

/// The first complete Backup recipe is a distinct logical transition. Its
/// physical feedback comes from the original common decision, not the generic
/// automatic Add receipt. This loan has no filesystem or publication methods.
pub(crate) struct VerifiedBackupFirstLayout<'a> {
    previous: &'a State,
    next: &'a State,
    candidate: &'a Candidate,
}

impl VerifiedBackupFirstLayout<'_> {
    pub(crate) fn require_pair(
        &self, previous: &State, next: &State, candidate: &Candidate,
    ) -> Result<()> {
        if !std::ptr::eq(self.previous, previous) || !std::ptr::eq(self.next, next)
            || !std::ptr::eq(self.candidate, candidate)
        {
            return Err(crate::engine::fresh_layout::FreshLayoutError::BackupLayoutEvidence.into());
        }
        Ok(())
    }
}

impl ValidatedConfig {
    pub(crate) fn verify_backup_first_layout<'a>(
        &self, previous: &'a State, next: &'a State, candidate: &'a Candidate,
    ) -> Result<Option<VerifiedBackupFirstLayout<'a>>> {
        let Some(generation) = self.generation() else {
            return Ok(None);
        };
        let Some(core) = generation.portable.backup_core() else {
            return Ok(None);
        };
        if candidate.sequence != generation.first_sequence()? {
            return Ok(None);
        }
        // Preserve envelope rejection before the full original recipe. The
        // source must be the actual frozen prefix, not a self-consistent next.
        generation.verify_candidate(candidate)?;
        core.verify_source(previous)?;
        let authority = PreparedMetadataObject::new(
            AUTHORITY_KIND, &generation.portable, MAX_METADATA,
        )?;
        let expected = core.first_candidate(&authority, previous, &AtomicBool::new(false))?;
        if object_hash("candidate", candidate)? != expected.hash() {
            return Err(crate::engine::fresh_layout::FreshLayoutError::BackupLayoutEvidence.into());
        }
        expected.require_state_projection(next)?;
        Ok(Some(VerifiedBackupFirstLayout { previous, next, candidate }))
    }
}
