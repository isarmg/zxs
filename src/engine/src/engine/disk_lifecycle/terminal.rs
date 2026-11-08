//! Explicit no-voter management. Protected system authority and complete old
//! fixed-quorum evidence replace neither an active pool nor ordinary writes.
pub(in crate::engine) mod retirement;
mod store;
use super::bootstrap::{
    IntentDigest,
    runtime::{management_error, open_store},
};
use crate::engine::disk_registry::current::PreparedCurrentNamespaces;
use crate::{
    DiskRegistrationIdentity, Engine, Error, ErrorCode, MembershipControlArchive,
    MembershipExitAction, OperationId, OperationResult, RegisteredDisk, Result, State,
    TerminalRegistry, ValidatedConfig,
    disk::{DiskIoBinding, DiskRoot, RecoveredDiskIo, WriterLocks},
    engine::membership_transition::terminal::{TerminalAuthority, VerifiedTerminalAuthority},
};
pub use retirement::{
    TerminalChainReadmissionPreparation, TerminalChainReadmissionProposal,
    TerminalRetirementArchive, TerminalRetirementCheckpoint, TerminalRetirementPhase,
    TerminalRetirementPlan, TerminalRetirementStatus,
};
use std::{path::Path, sync::Arc};
use store::TerminalStore;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalRegistrationState {
    RetainedRegistrations,
    EmptyRegistry,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalManagementStatus {
    pub authority_hash: String,
    pub operation_id: OperationId,
    pub target: DiskRegistrationIdentity,
    pub action: MembershipExitAction,
    pub registrations: TerminalRegistrationState,
    pub registered_disks: usize,
    pub active_metadata_members: usize,
    pub writable: bool,
    pub original_metadata_members: usize,
    pub original_metadata_quorum: usize,
}

/// Durable preparation, not a claim that descriptors or writer tails closed.
/// Retain this handle and retry `complete` when existing I/O drains.
pub struct TerminalDiskExit {
    store: TerminalStore,
    binding: DiskIoBinding,
    verified: VerifiedTerminalAuthority,
}

/// Management has no disk handles, mutable Engine, candidate or normal-write
/// method. Its system lock and process-wide target exclusion stay owned here.
pub struct TerminalManagement {
    store: TerminalStore,
    verified: VerifiedTerminalAuthority,
    state: Arc<State>,
    result: OperationResult,
    retirement_base_state: Arc<State>,
    retirement_completion: Option<retirement::Completed>,
    retirement_history: Vec<retirement::HistoricalCompleted>,
    retirement_base_registry: Option<TerminalRegistry>,
    retirement_base_namespaces: PreparedCurrentNamespaces,
    retirement_directory: Option<DiskRoot>,
    _locks: WriterLocks,
    _target_stop: RecoveredDiskIo,
}

/// Constructed only after complete archive authentication and an exact durable
/// bootstrap stop. It cannot certify runtime drain; the disk layer does that.
pub(crate) struct VerifiedTerminalStop {
    identity: DiskRegistrationIdentity,
    admission_epoch: Option<String>,
}
impl VerifiedTerminalStop {
    pub(crate) fn identity(&self) -> &DiskRegistrationIdentity {
        &self.identity
    }
    pub(crate) fn admission_epoch(&self) -> Option<String> {
        self.admission_epoch.clone()
    }
}

impl Engine {
    /// Freeze and preserve only necessary control history in the protected
    /// system directory. This writes no payload backup on any disk.
    pub fn prepare_terminal_disk_exit(&mut self) -> Result<TerminalDiskExit> {
        let authority = self.prepare_terminal_authority()?;
        let target = authority.target();
        let disk = self
            .config
            .disks
            .iter()
            .find(|disk| disk.disk_id == target.disk_id)
            .expect("invariant: terminal target is the final configured voter");
        let binding = self.disk_resources.get(disk)?.root.io_binding()?.clone();
        let store = TerminalStore::prepare(self, &authority)?;
        let verified = store.load_authority()?;
        Ok(TerminalDiskExit {
            store,
            binding,
            verified,
        })
    }
}

impl TerminalDiskExit {
    pub fn control_archive(&self) -> MembershipControlArchive {
        self.verified.authority.archive(self.store.system().path())
    }
    pub fn authority_hash(&self) -> &str {
        self.store.authority_hash()
    }
    /// The system preparation, exact stop, real descriptor closure, then final
    /// management publication are distinct durable/runtime barriers.
    pub fn complete(&mut self, engine: &mut Engine) -> Result<TerminalManagement> {
        engine.require_no_commit_preparation()?;
        self.store.verify_writer(&engine._locks)?;
        self.verified = self.store.load_authority()?;
        let authority = &self.verified.authority;
        let status = engine.membership_fence_status().ok_or_else(|| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "terminal completion lost its exact old fence",
            )
        })?;
        if status.transition_hash != authority.transition_hash()?
            || status.operation_id != authority.operation_id()
        {
            return Err(Error::new(
                ErrorCode::ConfigConflict,
                "terminal handle differs from the owning Engine fence",
            ));
        }
        self.store.synchronize_preparation(authority)?;
        let stop = synchronize_stop(&self.store, &self.verified, Some(&self.binding))?;
        #[cfg(feature = "fault-injection")]
        engine.failpoint("after_terminal_stop")?;
        drop(self.binding.begin_quiesce()?);
        engine.journals.remove(&stop.identity.disk_id);
        let closed = self.binding.closed_proof()?;
        #[cfg(feature = "fault-injection")]
        engine.failpoint("after_terminal_io_closed")?;
        let target_stop = RecoveredDiskIo::admit_terminal(&stop)?;
        // Removal consumes only actual final closure evidence. No filesystem
        // operation after this point resolves the stopped target's pathname.
        let disk = engine
            .config
            .disks
            .iter()
            .find(|disk| disk.disk_id == stop.identity.disk_id)
            .expect("invariant: terminal binding belongs to frozen configuration");
        if engine.disk_resources.available(disk).is_some() {
            engine.disk_resources = Arc::new(engine.disk_resources.excluding_closed(&closed)?);
            engine._locks = engine._locks.excluding_closed(&closed)?;
        }
        engine.blocked = true;
        let verified = self.store.load_authority()?;
        self.store.publish(&verified.authority)?;
        #[cfg(feature = "fault-injection")]
        engine.failpoint("after_terminal_publication")?;
        TerminalManagement::from_verified(
            self.store.reopen_held()?,
            verified,
            engine._locks.system_only(),
            target_stop,
        )
    }
}

impl TerminalManagement {
    /// A protected head selects the management path before any storage probe.
    /// Presence alone grants no authority; `open` authenticates the full closure.
    pub fn requires_management(root: &Path) -> Result<bool> {
        store::requires_management(root)
    }
    /// Open only protected SYSTEM state. Missing, corrupt, foreign or stale
    /// evidence is an error, never permission to initialize an empty pool.
    /// A crash after preparation resumes the same exact stop and publication.
    pub fn open(root: &Path) -> Result<Self> {
        let (store, locks) = TerminalStore::open(root)?;
        store.require_no_resume()?;
        let verified = store.load_authority()?;
        crate::engine::membership_transition::terminal_resume::cancellation::attempts::synchronize_history(store.system(), &verified)?;
        store.synchronize_preparation(&verified.authority)?;
        let stop = synchronize_stop(&store, &verified, None)?;
        let target_stop = RecoveredDiskIo::admit_terminal(&stop)?;
        store.publish(&verified.authority)?;
        Self::from_verified(store, verified, locks, target_stop)
    }

    /// Resume consumers can authenticate the completed source while a durable
    /// resume request exists. This never completes a pending terminal exit and
    /// never grants ordinary management or storage writes.
    pub(in crate::engine) fn open_for_resume(root: &Path) -> Result<Self> {
        Self::open_for_resume_with(root, |_, _| Ok(()))
    }

    /// Acquire the existing SYSTEM writer and recheck the complete source that
    /// the caller authorized before any locator repair or Stop synchronization.
    /// Keep this owner through directory resupply or selected-generation completion.
    pub fn open_metadata_resume_checked(
        root: &Path,
        expected: &crate::TerminalMetadataResumeRecovery,
    ) -> Result<Self> {
        Self::open_for_resume_with(root, |system, verified| {
            expected.verify_locked(system, verified)
        })
    }

    fn open_for_resume_with(
        root: &Path,
        verify: impl FnOnce(&DiskRoot, &VerifiedTerminalAuthority) -> Result<()>,
    ) -> Result<Self> {
        let (store, locks) = TerminalStore::open(root)?;
        retirement::require_no_evidence(&store)?;
        let verified = store.load_authority()?;
        store.verify_publication(&verified.authority)?;
        verify(store.system(), &verified)?;
        crate::engine::membership_transition::pending::activation::terminal_resume::resume_bootstrap_extension(store.system(), &verified)?;
        let stop = synchronize_stop(&store, &verified, None)?;
        let target_stop = RecoveredDiskIo::admit_terminal(&stop)?;
        Self::from_verified(store, verified, locks, target_stop)
    }

    fn from_verified(
        store: TerminalStore,
        verified: VerifiedTerminalAuthority,
        locks: WriterLocks,
        target_stop: RecoveredDiskIo,
    ) -> Result<Self> {
        store.verify_publication(&verified.authority)?;
        let (mut state, result) = verified.authority.management_state(&verified.state)?;
        let attempts =
            crate::engine::membership_transition::terminal_resume::cancellation::attempts::load(
                store.system(),
                &verified,
            )?;
        crate::engine::membership_transition::terminal_resume::cancellation::apply_history(
            Arc::get_mut(&mut state).expect("invariant: a new management projection has one owner"),
            &attempts.cancelled,
        )?;
        let mut management = Self {
            store,
            retirement_base_state: Arc::clone(&state),
            retirement_base_namespaces: verified.namespaces.clone(),
            verified,
            retirement_completion: None,
            retirement_history: Vec::new(),
            retirement_base_registry: None,
            retirement_directory: None,
            state,
            result,
            _locks: locks,
            _target_stop: target_stop,
        };
        management.validate_terminal_retirement()?;
        management.restore_terminal_retirement()?;
        crate::engine::membership_transition::terminal_chain::validate_selection(&management)?;
        Ok(management)
    }

    pub fn status(&self) -> TerminalManagementStatus {
        let registered_disks = self.registry().registered_disks().len();
        let authority = &self.verified.authority;
        TerminalManagementStatus {
            authority_hash: self.store.authority_hash().to_owned(),
            operation_id: authority.operation_id(),
            target: authority.target(),
            action: authority.action(),
            registrations: if registered_disks == 0 {
                TerminalRegistrationState::EmptyRegistry
            } else {
                TerminalRegistrationState::RetainedRegistrations
            },
            registered_disks,
            active_metadata_members: 0,
            writable: false,
            original_metadata_members: authority.original().metadata_members().len(),
            original_metadata_quorum: authority.original().metadata_quorum(),
        }
    }

    pub fn registered_disks(&self) -> impl ExactSizeIterator<Item = RegisteredDisk<'_>> {
        self.registry().registered_disks()
    }
    pub fn registry(&self) -> &TerminalRegistry {
        self.retirement_completion
            .as_ref()
            .map(|completed| &completed.registry)
            .unwrap_or_else(|| self.retirement_base_registry())
    }
    pub fn result(&self) -> &OperationResult {
        &self.result
    }
    pub fn state(&self) -> &State {
        &self.state
    }
    pub fn auth_state(&self) -> Option<json_codec::Value> {
        self.state.control_auth.clone()
    }
    pub fn auth_state_ref(&self) -> Option<&json_codec::Value> {
        self.state.control_auth.as_ref()
    }
    /// Historical logical usage only. There are no disk probes, capacity
    /// measurements or claims that a registered directory is currently online.
    pub fn disk_status(&self) -> Vec<crate::DiskStatus> {
        let used = crate::engine::observations::logical_used_bytes(&self.state);
        self.registered_disks()
            .map(|disk| crate::DiskStatus {
                slot_id: disk.slot_id.to_owned(),
                disk_id: disk.disk_id.to_owned(),
                level: disk.level,
                upstream_slot_id: disk.upstream_slot_id.map(str::to_owned),
                online: false,
                metadata_member: false,
                physical_device: None,
                available_bytes: None,
                observation_failure: None,
                used_bytes: used.get(disk.slot_id).copied().unwrap_or(0),
                simulation: self.original_configuration().mode == crate::Mode::Development,
            })
            .collect()
    }
    pub fn operation(&self, operation: OperationId) -> Option<&OperationResult> {
        self.state.operations.get(&operation)
    }
    pub fn original_configuration(&self) -> &ValidatedConfig {
        self.verified.authority.original()
    }
    pub fn control_archive(&self) -> MembershipControlArchive {
        self.verified.authority.archive(self.store.system().path())
    }

    pub(in crate::engine) fn verified_authority(&self) -> &VerifiedTerminalAuthority {
        &self.verified
    }
    pub(in crate::engine) fn verify_held_terminal_writer(&self) -> Result<()> {
        self.store.verify_writer(&self._locks)
    }
    pub(in crate::engine) fn system_root(&self) -> &DiskRoot {
        self.store.system()
    }
    /// Includes the permanent terminal operation receipt. The archive's frozen
    /// State remains available separately in verified_authority for proof replay.
    pub(in crate::engine) fn management_state(&self) -> &Arc<State> {
        &self.state
    }
    pub(in crate::engine) fn writer_locks(&self) -> Result<WriterLocks> {
        self.require_no_terminal_retirement()?;
        self._locks.retain()
    }

    pub(in crate::engine) fn refresh_cancelled_resume_results(&mut self) -> Result<()> {
        self.require_no_terminal_retirement()?;
        let attempts =
            crate::engine::membership_transition::terminal_resume::cancellation::attempts::load(
                self.system_root(),
                &self.verified,
            )?;
        let mut state = self
            .verified
            .authority
            .management_state(&self.verified.state)?
            .0;
        crate::engine::membership_transition::terminal_resume::cancellation::apply_history(
            Arc::get_mut(&mut state).expect("invariant: a new management projection has one owner"),
            &attempts.cancelled,
        )?;
        self.retirement_base_state = Arc::clone(&state);
        self.state = state;
        self.restore_terminal_retirement()?;
        Ok(())
    }
}

fn synchronize_stop(
    store: &TerminalStore,
    verified: &VerifiedTerminalAuthority,
    live_binding: Option<&DiskIoBinding>,
) -> Result<VerifiedTerminalStop> {
    let identity = verified.authority.target();
    let admission_epoch = verified.authority.io_admission_epoch(&identity.disk_id)?;
    let mut bootstrap = open_store(store.system().path())?;
    store.synchronize_stop_plan(&mut bootstrap, verified, live_binding)?;
    Ok(VerifiedTerminalStop {
        identity,
        admission_epoch,
    })
}

/// Called before any configured storage directory is opened by normal startup.
/// Only the separate management consumer may interpret the terminal ledger.
pub(in crate::engine) fn require_active_startup(root: &Path) -> Result<()> {
    store::require_active_startup(root)
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/engine/terminal_management_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../../../test/engine/unit/engine/terminal_management_adversarial_tests.rs"]
mod adversarial_tests;

#[cfg(test)]
#[path = "../../../../../test/engine/unit/engine/terminal_stop_publication_tests.rs"]
mod stop_publication_tests;
