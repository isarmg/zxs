//! Exact registry continuation after an admitted source publication. These
//! functions authenticate immutable graphs; they do not admit a serving view or
//! prove that the predecessor lifecycle publication actually happened.
use super::*;

#[derive(Clone, Copy)]
pub(in crate::engine) struct RegistryBoundary<'a> {
    pub(in crate::engine) authority: &'a str,
    pub(in crate::engine) snapshot: &'a str,
    pub(in crate::engine) sequence: u64,
    pub(in crate::engine) topology_generation: u64,
}

impl VerifiedDiskRegistry {
    /// Used only with a commitment selected by authenticated authority. The
    /// caller must additionally validate its precise lineage before admission.
    pub(in crate::engine) fn read_graph(
        expected: &DiskRegistryCommitment,
        read: impl FnMut(&'static str, &str) -> Result<Vec<u8>>,
    ) -> Result<Self> {
        Ok(Self {
            commitment: expected.clone(),
            state: load_registry(expected, read)?,
        })
    }

    pub(in crate::engine) fn prepared_copy(&self) -> Result<PreparedDiskRegistry> {
        let copy = encode_registry(self.state.clone())?;
        if copy.commitment() != self.commitment() {
            return Err(invalid("registry graph is not its canonical encoding"));
        }
        Ok(copy)
    }

    pub(in crate::engine) fn matches_current_source(
        &self,
        previous: &VerifiedDiskRegistry,
        config: &ValidatedConfig,
        state: &State,
        snapshot: &str,
        authority: &str,
    ) -> Result<bool> {
        Ok(self.state
            == previous.continuation_state(
                config,
                config,
                RegistryBoundary {
                    authority,
                    snapshot,
                    sequence: state.sequence,
                    topology_generation: state.topology_generation,
                },
            )?)
    }

    pub(in crate::engine) fn prepare_continuation(
        &self,
        source: &ValidatedConfig,
        target: &ValidatedConfig,
        boundary: RegistryBoundary<'_>,
    ) -> Result<PreparedDiskRegistry> {
        encode_registry(self.continuation_state(source, target, boundary)?)
    }

    pub(in crate::engine) fn verify_continuation(
        &self,
        source: &ValidatedConfig,
        target: &ValidatedConfig,
        boundary: RegistryBoundary<'_>,
        expected: &DiskRegistryCommitment,
        read: impl FnMut(&'static str, &str) -> Result<Vec<u8>>,
    ) -> Result<Self> {
        let loaded = load_registry(expected, read)?;
        if loaded != self.continuation_state(source, target, boundary)? {
            return Err(invalid(
                "registry continuation changes its exact predecessor",
            ));
        }
        Ok(Self {
            commitment: expected.clone(),
            state: loaded,
        })
    }

    fn continuation_state(
        &self,
        source: &ValidatedConfig,
        target: &ValidatedConfig,
        boundary: RegistryBoundary<'_>,
    ) -> Result<RegistryState> {
        digest(boundary.authority)?;
        digest(boundary.snapshot)?;
        self.require_config(source)?;
        if source.pool_id != target.pool_id
            || boundary.sequence <= self.state.source.sequence
            || boundary.topology_generation <= self.state.source.topology_generation
            || source.disks.iter().any(|old| {
                !target.disks.iter().any(|new| {
                    old.disk_id == new.disk_id
                        && old.slot_id == new.slot_id
                        && old.binding_generation == new.binding_generation
                        && old.level == new.level
                        && old.upstream_slot_id == new.upstream_slot_id
                        && old.capacity_bytes == new.capacity_bytes
                        && old.metadata_member == new.metadata_member
                })
            })
        {
            return Err(invalid(
                "registry continuation does not preserve its frozen configuration",
            ));
        }
        let mut next = self.state.clone();
        next.source = RegistrySource {
            pool_id: source.pool_id.clone(),
            membership_authority: Some(boundary.authority.to_owned()),
            snapshot_hash: Some(boundary.snapshot.to_owned()),
            sequence: boundary.sequence,
            topology_generation: boundary.topology_generation,
        };
        next.view_generation = next
            .view_generation
            .checked_add(1)
            .ok_or_else(|| Error::new(ErrorCode::ResourceLimit, "registry generation exhausted"))?;
        next.predecessor = Some(self.commitment.clone());
        // Clearing this reservation is safe only once the enclosing authority
        // verifier has checked the archived prescribed publication of the parent.
        next.pending_exit = None;
        for disk in target
            .disks
            .iter()
            .filter(|disk| !source.disks.iter().any(|old| old.disk_id == disk.disk_id))
        {
            if disk.binding_generation != 1 || next.topology.contains_key(&disk.disk_id) {
                return Err(invalid(
                    "registry continuation reuses a historical identity",
                ));
            }
            let identity = DiskRegistrationIdentity {
                pool_id: target.pool_id.clone(),
                disk_id: disk.disk_id.clone(),
                slot_id: disk.slot_id.clone(),
                binding_generation: disk.binding_generation,
            };
            if next
                .topology
                .values()
                .any(|old| old.identity.slot_id == identity.slot_id)
            {
                return Err(invalid("registry continuation reuses a historical slot"));
            }
            next.topology.insert(
                disk.disk_id.clone(),
                HistoricalDiskDefinition {
                    identity: identity.clone(),
                    level: disk.level,
                    upstream_slot_id: disk.upstream_slot_id.clone(),
                    original_metadata_member: disk.metadata_member,
                    capacity_bytes: disk.capacity_bytes,
                },
            );
            next.registrations.insert(
                disk.disk_id.clone(),
                Registration {
                    identity: identity.clone(),
                    mode: RegistrationMode::Enabled,
                },
            );
            if disk.metadata_member {
                next.voters.insert(disk.disk_id.clone(), identity);
            }
        }
        let result = Self {
            commitment: self.commitment.clone(),
            state: next,
        };
        result.require_config(target)?;
        Ok(result.state)
    }

    pub(in crate::engine) fn require_config(&self, config: &ValidatedConfig) -> Result<()> {
        if config.pool_id != self.state.source.pool_id
            || config.disks.len() != self.state.topology.len()
            || config.metadata_members().len() != self.state.voters.len()
        {
            return Err(invalid(
                "registry differs from historical configuration or fixed voters",
            ));
        }
        for disk in &config.disks {
            let historical = self.state.topology.get(&disk.disk_id).ok_or_else(|| {
                invalid("registry is missing a historical configuration identity")
            })?;
            if historical.identity.pool_id != config.pool_id
                || historical.identity.disk_id != disk.disk_id
                || historical.identity.slot_id != disk.slot_id
                || historical.identity.binding_generation != disk.binding_generation
                || historical.level != disk.level
                || historical.upstream_slot_id != disk.upstream_slot_id
                || historical.capacity_bytes != disk.capacity_bytes
                || self
                    .state
                    .voters
                    .get(&disk.disk_id)
                    .is_some_and(|id| id != &historical.identity)
                || self.state.voters.contains_key(&disk.disk_id) != disk.metadata_member
            {
                return Err(invalid(
                    "registry historical binding or fixed voter differs",
                ));
            }
            let registered = self.state.registrations.get(&disk.disk_id);
            let detached = self.state.detached.get(&disk.disk_id);
            if registered.is_some() == detached.is_some()
                || registered.is_some_and(|entry| entry.identity != historical.identity)
                || registered.is_some_and(|entry| entry.mode == RegistrationMode::Disabled)
                    && disk.metadata_member
                || detached.is_some() && disk.metadata_member
            {
                return Err(invalid(
                    "registry current registration is not an exact historical partition",
                ));
            }
        }
        Ok(())
    }
}
