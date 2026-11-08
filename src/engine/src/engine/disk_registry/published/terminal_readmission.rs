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
        self.retained_resume_graph(boundary, target, identity)
    }

    fn retained_resume_graph(
        &self,
        boundary: RegistryBoundary<'_>,
        target: &ValidatedConfig,
        identity: &DiskRegistrationIdentity,
    ) -> Result<RegistryState> {
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

// No boolean bypass for require_resumable: this separate preparation lane needs
// the unforgeable result of complete checkpoint replay. It still cannot publish.
use crate::engine::disk_lifecycle::terminal::retirement::VerifiedRetirementCheckpoint;
impl TerminalRegistry {
    pub(in crate::engine) fn prepare_checkpoint_readmission(
        source: &VerifiedRetirementCheckpoint,
        target: &ValidatedConfig,
        identity: &DiskRegistrationIdentity,
    ) -> Result<PreparedDiskRegistry> {
        encode_registry(Self::checkpoint_readmission_graph(
            source, target, identity,
        )?)
    }
    pub(in crate::engine) fn verify_checkpoint_readmission(
        source: &VerifiedRetirementCheckpoint,
        target: &ValidatedConfig,
        identity: &DiskRegistrationIdentity,
        expected: &DiskRegistryCommitment,
        read: impl FnMut(&'static str, &str) -> Result<Vec<u8>>,
    ) -> Result<VerifiedDiskRegistry> {
        let loaded = load_registry(expected, read)?;
        if loaded != Self::checkpoint_readmission_graph(source, target, identity)? {
            return Err(invalid(
                "checkpoint readmission registry changed another registration",
            ));
        }
        Ok(VerifiedDiskRegistry {
            commitment: expected.clone(),
            state: loaded,
        })
    }
    fn checkpoint_readmission_graph(
        source: &VerifiedRetirementCheckpoint,
        target: &ValidatedConfig,
        identity: &DiskRegistrationIdentity,
    ) -> Result<RegistryState> {
        let registry = source.registry();
        registry.require_checkpoint_lower_chain(identity)?;
        let frozen = &registry.verified_registry().state.source;
        let snapshot = frozen
            .snapshot_hash
            .as_deref()
            .ok_or_else(|| invalid("checkpoint readmission lacks its frozen snapshot"))?;
        registry.retained_resume_graph(
            RegistryBoundary {
                authority: source.digest(),
                snapshot,
                sequence: frozen.sequence,
                topology_generation: frozen.topology_generation,
            },
            target,
            identity,
        )
    }
    fn require_checkpoint_lower_chain(&self, identity: &DiskRegistrationIdentity) -> Result<()> {
        let registry = self.verified_registry();
        registry.require_identity(identity)?;
        let mut disk = &registry.state.topology[&identity.disk_id];
        // The topology is already validated. We still require the exact lower
        // registrations to be Enabled now, not just present in historical Config.
        let by_slot: BTreeMap<_, _> = registry
            .state
            .topology
            .values()
            .map(|definition| (definition.identity.slot_id.as_str(), definition))
            .collect();
        while disk.level > 1 {
            let lower = disk
                .upstream_slot_id
                .as_deref()
                .and_then(|slot| by_slot.get(slot))
                .ok_or_else(|| invalid("checkpoint readmission lacks its complete lower chain"))?;
            if lower.level.checked_add(1) != Some(disk.level)
                || registry.registration(&lower.identity)? != Some(RegistrationMode::Enabled)
            {
                return Err(invalid(
                    "checkpoint readmission lower ancestor is not an Enabled registration",
                ));
            }
            disk = lower;
        }
        Ok(())
    }
}
