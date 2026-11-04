//! A no-voter registry is management metadata, never an active registry.
use super::*;

/// Authenticated registrations after the final voter has left. This type has
/// deliberately no candidate, publication or ordinary-write admission method.
pub struct TerminalRegistry {
    verified: VerifiedDiskRegistry,
}

impl TerminalRegistry {
    pub fn commitment(&self) -> &DiskRegistryCommitment {
        self.verified.commitment()
    }

    pub fn registered_disks(
        &self,
    ) -> impl ExactSizeIterator<Item = super::super::RegisteredDisk<'_>> {
        self.verified
            .registered_definitions()
            .map(|(disk, mode, voter)| {
                debug_assert!(!voter);
                super::super::RegisteredDisk {
                    pool_id: &disk.identity.pool_id,
                    disk_id: &disk.identity.disk_id,
                    slot_id: &disk.identity.slot_id,
                    binding_generation: disk.identity.binding_generation,
                    level: disk.level,
                    upstream_slot_id: disk.upstream_slot_id.as_deref(),
                    metadata_member: false,
                    registration_mode: mode,
                }
            })
    }

    pub fn historical_topology(&self) -> impl ExactSizeIterator<Item = &HistoricalDiskDefinition> {
        self.verified.historical_topology()
    }

    pub fn registration(
        &self,
        identity: &DiskRegistrationIdentity,
    ) -> Result<Option<RegistrationMode>> {
        self.verified.registration(identity)
    }

    pub(in crate::engine) fn exit(&self) -> &RegistryExitRequest {
        self.verified
            .pending_exit()
            .expect("invariant: terminal registry has its exact exit")
    }
    pub(in crate::engine) fn verified_registry(&self) -> &VerifiedDiskRegistry {
        &self.verified
    }
}

impl VerifiedDiskRegistry {
    pub(in crate::engine) fn prepare_terminal_exit(
        &self,
        request: RegistryExitRequest,
    ) -> Result<PreparedDiskRegistry> {
        self.require_terminal_target(&request.target)?;
        encode_registry(self.exiting_state(request)?)
    }

    pub(in crate::engine) fn verify_terminal_successor(
        &self,
        expected: &DiskRegistryCommitment,
        read: impl FnMut(&'static str, &str) -> Result<Vec<u8>>,
    ) -> Result<TerminalRegistry> {
        let loaded = load_registry(expected, read)?;
        let request = loaded
            .pending_exit
            .clone()
            .ok_or_else(|| invalid("terminal registry lacks its exit intent"))?;
        self.require_terminal_target(&request.target)?;
        if !loaded.voters.is_empty() || loaded != self.exiting_state(request)? {
            return Err(invalid(
                "terminal registry differs from the exact final-voter exit",
            ));
        }
        Ok(TerminalRegistry {
            verified: Self {
                commitment: expected.clone(),
                state: loaded,
            },
        })
    }

    fn require_terminal_target(&self, identity: &DiskRegistrationIdentity) -> Result<()> {
        self.require_identity(identity)?;
        if self.state.voters.len() != 1
            || self.state.voters.get(&identity.disk_id) != Some(identity)
        {
            return Err(invalid(
                "management authority must retire the exact final voter",
            ));
        }
        Ok(())
    }
}
