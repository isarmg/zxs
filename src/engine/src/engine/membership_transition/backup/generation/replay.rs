//! Backup's first slot keeps its original generation envelope. Admitting that
//! envelope is not admission of an active registry or a later writer slot.
use super::*;

impl BackupCore {
    pub(in crate::engine::membership_transition) fn verify_first_slot(
        &self,
        candidate: &Candidate,
        authority_hash: &str,
    ) -> Result<()> {
        let intent = self.intent()?;
        let first = intent.fence_sequence.checked_add(1).ok_or_else(|| {
            Error::new(ErrorCode::ResourceLimit, "Backup first sequence exhausted")
        })?;
        if candidate.sequence != first {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "Backup authority admits only its original first slot until an active writer continuation is installed",
            ));
        }
        if candidate.generation() != Some(authority_hash)
            || candidate.registry().is_some()
            || candidate.operation_id != intent.operation_id
            || candidate.request_digest != object_hash(INTENT_KIND, &intent)?
            || candidate.transaction_id != format!("tx_{}", &authority_hash[..32])
            || candidate.previous_snapshot.as_deref()
                != intent
                    .boundary
                    .as_ref()
                    .map(|boundary| boundary.snapshot_hash.as_str())
        {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "Backup first slot changes its original generation, operation, representation or source boundary",
            ));
        }
        // New payload disks never become voters. This includes the exact pool
        // and original fixed-member set, not an online or selected-target set.
        intent
            .origin
            .config()
            .verify_original_candidate_membership(candidate)
    }
}
