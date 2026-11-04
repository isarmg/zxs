//! Exact continuation of the no-voter management registry. Historical bindings,
//! detach masks and disabled registrations survive; only fresh metadata members
//! are introduced. This graph alone never admits an active Engine.
use super::terminal::TerminalRegistry;
use super::*;

impl TerminalRegistry {
    pub(in crate::engine) fn prepare_resume(
        &self,
        terminal_authority: &str,
        source: &State,
        snapshot: &str,
        target: &ValidatedConfig,
    ) -> Result<PreparedDiskRegistry> {
        encode_registry(self.resume_state(terminal_authority, source, snapshot, target)?)
    }

    fn resume_state(
        &self,
        authority: &str,
        source: &State,
        snapshot: &str,
        target: &ValidatedConfig,
    ) -> Result<RegistryState> {
        self.resume_state_at(
            authority,
            source.sequence,
            source.topology_generation,
            snapshot,
            target,
        )
    }

    pub(in crate::engine) fn verify_resume_at(
        &self,
        authority: &str,
        source_sequence: u64,
        source_topology_generation: u64,
        snapshot: &str,
        target: &ValidatedConfig,
        expected: &DiskRegistryCommitment,
        read: impl FnMut(&'static str, &str) -> Result<Vec<u8>>,
    ) -> Result<VerifiedDiskRegistry> {
        let loaded = load_registry(expected, read)?;
        if loaded
            != self.resume_state_at(
                authority,
                source_sequence,
                source_topology_generation,
                snapshot,
                target,
            )?
        {
            return Err(invalid(
                "terminal resume differs from its exact frozen source",
            ));
        }
        Ok(VerifiedDiskRegistry {
            commitment: expected.clone(),
            state: loaded,
        })
    }

    fn resume_state_at(
        &self,
        authority: &str,
        sequence: u64,
        topology_generation: u64,
        snapshot: &str,
        target: &ValidatedConfig,
    ) -> Result<RegistryState> {
        digest(authority)?;
        digest(snapshot)?;
        let previous = self.verified_registry();
        let old = &previous.state;
        if !old.voters.is_empty()
            || old.pending_exit.is_none()
            || old.source.pool_id != target.pool_id
            || old.source.sequence != sequence
            || old.source.topology_generation != topology_generation
            || old.source.snapshot_hash.as_deref() != Some(snapshot)
        {
            return Err(invalid(
                "resume requires the exact certified no-voter boundary",
            ));
        }
        let mut next = old.clone();
        next.predecessor = Some(previous.commitment.clone());
        next.view_generation = next
            .view_generation
            .checked_add(1)
            .ok_or_else(|| Error::new(ErrorCode::ResourceLimit, "registry generation exhausted"))?;
        next.source.membership_authority = Some(authority.to_owned());
        next.pending_exit = None;
        for disk in &target.disks {
            if next.topology.contains_key(&disk.disk_id) {
                if disk.metadata_member {
                    return Err(invalid(
                        "terminal resume cannot revive an old metadata binding",
                    ));
                }
                continue;
            }
            if !disk.metadata_member
                || disk.binding_generation != 1
                || next
                    .topology
                    .values()
                    .any(|old| old.identity.slot_id == disk.slot_id)
            {
                return Err(invalid(
                    "resume requires fresh metadata identities and historical-slot separation",
                ));
            }
            let identity = DiskRegistrationIdentity {
                pool_id: target.pool_id.clone(),
                disk_id: disk.disk_id.clone(),
                slot_id: disk.slot_id.clone(),
                binding_generation: disk.binding_generation,
            };
            next.topology.insert(
                disk.disk_id.clone(),
                HistoricalDiskDefinition {
                    identity: identity.clone(),
                    level: disk.level,
                    upstream_slot_id: disk.upstream_slot_id.clone(),
                    original_metadata_member: true,
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
            next.voters.insert(disk.disk_id.clone(), identity);
        }
        if next.voters.is_empty() {
            return Err(invalid(
                "terminal resume requires at least one new metadata member",
            ));
        }
        let result = VerifiedDiskRegistry {
            commitment: previous.commitment.clone(),
            state: next,
        };
        result.require_config(target)?;
        Ok(result.state)
    }
}

impl super::super::current::PreparedCurrentNamespaces {
    /// Both views remain preparation. This only changes the registry binding of
    /// an already verified terminal mask; the real first publication admits it.
    pub(in crate::engine) fn continue_terminal_registry(
        mut self,
        terminal: &TerminalRegistry,
        resumed: &VerifiedDiskRegistry,
    ) -> Result<Self> {
        if self.view.registry != *terminal.commitment()
            || resumed.state.predecessor.as_ref() != Some(terminal.commitment())
            || resumed.state.pending_exit.is_some()
            || resumed.state.voters.is_empty()
            || resumed.state.source.sequence != self.view.source_sequence
            || resumed.state.source.snapshot_hash.as_deref()
                != Some(self.view.source_snapshot.as_str())
            || Some(resumed.commitment.view_generation())
                != terminal.commitment().view_generation().checked_add(1)
        {
            return Err(invalid(
                "terminal namespace projection does not continue its exact resumed registry",
            ));
        }
        self.view.registry = resumed.commitment.clone();
        self.view.registry_state = std::sync::Arc::new(resumed.clone());
        Ok(self)
    }
}
