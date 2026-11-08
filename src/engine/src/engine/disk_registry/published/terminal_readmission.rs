//! Restore one retained metadata registration without adding or rebinding it.
use super::continuation::RegistryBoundary;
use super::terminal::TerminalRegistry;
use super::*;

impl TerminalRegistry {
    pub(in crate::engine) fn prepare_retained_resume(
        &self,
        boundary: RegistryBoundary<'_>,
        target: &ValidatedConfig,
        identity: &DiskRegistrationIdentity,
    ) -> Result<PreparedDiskRegistry> {
        encode_registry(self.retained_resume_state(boundary, target, identity)?)
    }

    pub(in crate::engine) fn verify_retained_resume_at(
        &self,
        boundary: RegistryBoundary<'_>,
        target: &ValidatedConfig,
        identity: &DiskRegistrationIdentity,
        expected: &DiskRegistryCommitment,
        read: impl FnMut(&'static str, &str) -> Result<Vec<u8>>,
    ) -> Result<VerifiedDiskRegistry> {
        let loaded = load_registry(expected, read)?;
        if loaded != self.retained_resume_state(boundary, target, identity)? {
            return Err(invalid(
                "terminal readmission differs from its exact retained registration",
            ));
        }
        Ok(VerifiedDiskRegistry {
            commitment: expected.clone(),
            state: loaded,
        })
    }

    fn retained_resume_state(
        &self,
        boundary: RegistryBoundary<'_>,
        target: &ValidatedConfig,
        identity: &DiskRegistrationIdentity,
    ) -> Result<RegistryState> {
        self.require_resumable()?;
        digest(boundary.authority)?;
        digest(boundary.snapshot)?;
        let previous = self.verified_registry();
        previous.require_identity(identity)?;
        let old = &previous.state;
        if !old.voters.is_empty()
            || old.pending_exit.is_none()
            || old.source.pool_id != target.pool_id
            || old.source.sequence != boundary.sequence
            || old.source.topology_generation != boundary.topology_generation
            || old.source.snapshot_hash.as_deref() != Some(boundary.snapshot)
            || previous.registration(identity)? != Some(RegistrationMode::Disabled)
            || old.detached.contains_key(&identity.disk_id)
            || !old.topology[&identity.disk_id].original_metadata_member
        {
            return Err(invalid(
                "terminal readmission requires the exact Disabled metadata boundary",
            ));
        }
        let mut next = old.clone();
        next.predecessor = Some(previous.commitment.clone());
        next.view_generation = next
            .view_generation
            .checked_add(1)
            .ok_or_else(|| Error::new(ErrorCode::ResourceLimit, "registry generation exhausted"))?;
        next.source.membership_authority = Some(boundary.authority.into());
        next.pending_exit = None;
        next.registrations
            .get_mut(&identity.disk_id)
            .expect("invariant: authenticated Disabled registration")
            .mode = RegistrationMode::Enabled;
        next.voters
            .insert(identity.disk_id.clone(), identity.clone());
        let verified = VerifiedDiskRegistry {
            commitment: previous.commitment.clone(),
            state: next,
        };
        verified.require_config(target)?;
        Ok(verified.state)
    }
}
