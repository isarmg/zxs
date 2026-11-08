//! A no-voter registry is management metadata, never an active registry.
use super::*;

/// Authenticated registrations after the final voter has left. This type has
/// deliberately no candidate, publication or ordinary-write admission method.
pub struct TerminalRegistry {
    verified: VerifiedDiskRegistry,
    retired_in_management: bool,
}

impl TerminalRegistry {
    // A no-voter successor binds exact occurrence removal, but is not serving authority.
    pub(in crate::engine) fn prepare_management_retirement(
        &self,
        request: RegistryExitRequest,
    ) -> Result<PreparedDiskRegistry> {
        encode_registry(self.retirement_state(request)?)
    }
    pub(in crate::engine) fn verify_management_retirement(
        &self,
        request: RegistryExitRequest,
        expected: &DiskRegistryCommitment,
        read: impl FnMut(&'static str, &str) -> Result<Vec<u8>>,
    ) -> Result<Self> {
        let loaded = load_registry(expected, read)?;
        if loaded != self.retirement_state(request)? {
            return Err(invalid(
                "terminal retirement changes more than its exact registration",
            ));
        }
        Ok(Self {
            verified: VerifiedDiskRegistry {
                commitment: expected.clone(),
                state: loaded,
            },
            retired_in_management: true,
        })
    }
    fn retirement_state(&self, request: RegistryExitRequest) -> Result<RegistryState> {
        request.validate()?;
        self.verified.require_identity(&request.target)?;
        if !matches!(request.effect, RegistryExitEffect::Detach { .. }) {
            return Err(invalid(
                "terminal retirement requires an exact occurrence mask",
            ));
        }
        if !self.verified.state.voters.is_empty() || self.verified.state.pending_exit.is_none() {
            return Err(invalid(
                "retirement requires the exact no-voter terminal registry",
            ));
        }
        let mut next = self.verified.state.clone();
        if next.registrations.remove(&request.target.disk_id).is_none() {
            return Err(invalid("terminal retirement target is not registered"));
        }
        next.predecessor = Some(self.commitment().clone());
        next.view_generation = next
            .view_generation
            .checked_add(1)
            .ok_or_else(|| Error::new(ErrorCode::ResourceLimit, "registry generation exhausted"))?;
        next.detached.insert(
            request.target.disk_id.clone(),
            DetachedBinding {
                predecessor: self.commitment().clone(),
                request,
                view_generation: next.view_generation,
            },
        );
        // The original terminal intent still owns the no-voter boundary. Each
        // later detached binding independently names its exact operation/mask.
        Ok(next)
    }
    pub(in crate::engine) fn require_resumable(&self) -> Result<()> {
        if self.retired_in_management {
            return Err(invalid(
                "management retirement successor has no admitted readmission generation",
            ));
        }
        Ok(())
    }

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
            retired_in_management: false,
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
