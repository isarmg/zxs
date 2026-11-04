//! Runtime ownership for one physical disk binding, independent of membership epochs.
//!
//! This is admission and drain accounting, not a persisted offline decision. The
//! coordinator must cover every consumer before it can advertise a disk as disabled.
//! Control transitions are currently compiled only for tests/fault injection;
//! normal builds retain real file ownership and fail-closed accounting.
mod upload;
pub(crate) use upload::{UploadFinish, UploadIo};

use super::DiskSeal;
use crate::{Error, ErrorCode, Result, ValidatedConfig};
use platform_io::fs_safety::{EntryName, linux::DirectoryEntries};
use std::{
    collections::BTreeMap,
    fs::{File, Metadata},
    io::{self, Read, Seek, SeekFrom, Write},
    os::unix::fs::FileExt,
    sync::{
        Arc, Mutex, MutexGuard, OnceLock, TryLockError, Weak,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct DiskIoIdentity {
    pub(crate) pool_id: String,
    pub(crate) disk_id: String,
    pub(crate) slot_id: String,
    pub(crate) binding_generation: u64,
    pub(crate) device: u64,
    pub(crate) inode: u64,
}

impl From<&DiskSeal> for DiskIoIdentity {
    fn from(seal: &DiskSeal) -> Self {
        Self {
            pool_id: seal.pool_id.clone(),
            disk_id: seal.disk_id.clone(),
            slot_id: seal.slot_id.clone(),
            binding_generation: seal.binding_generation,
            device: seal.device,
            inode: seal.inode,
        }
    }
}

/// Cloning a root copies this binding, never a new admission domain.
#[derive(Clone, Debug)]
pub(crate) struct DiskIoBinding {
    gate: Arc<DiskIoGate>,
    authority: ValidatedConfig,
}

#[derive(Debug)]
struct DiskIoGate {
    identity: DiskIoIdentity,
    state: Mutex<GateState>,
}

fn binding_gates() -> &'static Mutex<BTreeMap<DiskIoIdentity, Weak<DiskIoGate>>> {
    static GATES: OnceLock<Mutex<BTreeMap<DiskIoIdentity, Weak<DiskIoGate>>>> = OnceLock::new();
    GATES.get_or_init(|| Mutex::new(BTreeMap::new()))
}

impl Drop for DiskIoGate {
    fn drop(&mut self) {
        match binding_gates().lock() {
            Ok(mut gates) => {
                // Another opener may have replaced an expired weak entry while
                // this last owner's destructor was waiting for the registry.
                if gates
                    .get(&self.identity)
                    .is_some_and(|gate| std::ptr::eq(gate.as_ptr(), std::ptr::from_ref(self)))
                {
                    gates.remove(&self.identity);
                }
            }
            Err(_) => tracing::error!("disk I/O binding registry poisoned during cleanup"),
        }
    }
}

#[derive(Debug)]
struct GateState {
    authority: ValidatedConfig,
    phase: Phase,
    ordinary: usize,
    #[cfg(any(test, feature = "fault-injection"))]
    control: usize,
    failed: bool,
    resources: BTreeMap<usize, Weak<dyn RevocableResource>>,
}

#[derive(Debug)]
enum Phase {
    Active,
    #[cfg(any(test, feature = "fault-injection"))]
    Quiescing(Arc<()>),
}

impl Phase {
    fn allows(&self, responsibility: Responsibility) -> bool {
        match responsibility {
            Responsibility::Ordinary => matches!(self, Self::Active),
            #[cfg(any(test, feature = "fault-injection"))]
            Responsibility::Control => true,
        }
    }
}

#[cfg(any(test, feature = "fault-injection"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DiskIoStatus {
    pub(crate) quiescing: bool,
    pub(crate) ordinary: usize,
    pub(crate) control: usize,
}

#[cfg(any(test, feature = "fault-injection"))]
impl DiskIoStatus {
    /// Zero is meaningful only after ordinary admission has been closed.
    /// This does not certify persistence, membership removal or complete coverage.
    pub(crate) fn drained(self) -> bool {
        self.quiescing && self.ordinary == 0 && self.control == 0
    }
}

impl DiskIoGate {
    fn state(&self) -> Result<MutexGuard<'_, GateState>> {
        let state = self.state.lock().map_err(|_| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "disk I/O admission lock poisoned",
            )
        })?;
        if state.failed {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "disk I/O responsibility accounting failed",
            ));
        }
        Ok(state)
    }

    fn unavailable(&self) -> Error {
        Error::new(
            ErrorCode::DiskUnavailable,
            format!(
                "disk {} binding {} is quiescing",
                self.identity.disk_id, self.identity.binding_generation,
            ),
        )
    }
}

impl DiskIoBinding {
    pub(crate) fn new(seal: &DiskSeal, authority: &ValidatedConfig) -> Result<Self> {
        let identity = DiskIoIdentity::from(seal);
        let mut gates = binding_gates().lock().map_err(|_| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "disk I/O binding registry poisoned",
            )
        })?;
        let gate = match gates.get(&identity).and_then(Weak::upgrade) {
            Some(gate) => gate,
            None => {
                let gate = Arc::new(DiskIoGate {
                    identity: identity.clone(),
                    state: Mutex::new(GateState {
                        authority: authority.clone(),
                        phase: Phase::Active,
                        ordinary: 0,
                        #[cfg(any(test, feature = "fault-injection"))]
                        control: 0,
                        failed: false,
                        resources: BTreeMap::new(),
                    }),
                });
                gates.insert(identity, Arc::downgrade(&gate));
                gate
            }
        };
        // Schema admission and matching physical identity never change an
        // existing gate's authority. Only verified generation activation does.
        Ok(Self {
            gate,
            authority: authority.clone(),
        })
    }

    pub(crate) fn for_authority(
        &self,
        seal: &DiskSeal,
        authority: &ValidatedConfig,
    ) -> Result<Self> {
        if self.gate.identity != DiskIoIdentity::from(seal) {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "retained I/O gate belongs to a different disk binding",
            ));
        }
        Ok(Self {
            gate: Arc::clone(&self.gate),
            authority: authority.clone(),
        })
    }

    /// Publish only after all held roots have been prepared. Gate locks have a
    /// stable identity order, and no authority changes if any member rejects.
    /// Historical reads keep running; only control authority advances.
    pub(crate) fn activate_generation(
        bindings: &[Self],
        authority: &ValidatedConfig,
    ) -> Result<()> {
        if authority.generation().is_none() {
            return Err(Error::new(
                ErrorCode::ConfigConflict,
                "I/O authority update requires an admitted membership generation",
            ));
        }
        let mut ordered = bindings.iter().collect::<Vec<_>>();
        ordered.sort_unstable_by(|left, right| left.gate.identity.cmp(&right.gate.identity));
        if ordered
            .windows(2)
            .any(|pair| pair[0].gate.identity == pair[1].gate.identity)
        {
            return Err(Error::new(
                ErrorCode::ConfigInvalid,
                "duplicate retained disk I/O binding",
            ));
        }
        let mut states = ordered
            .iter()
            .map(|binding| binding.gate.state())
            .collect::<Result<Vec<_>>>()?;
        for (binding, state) in ordered.iter().zip(&states) {
            binding.validate_authority_update(state, authority)?;
        }
        for state in &mut states {
            state.authority = authority.clone();
        }
        Ok(())
    }

    fn validate_authority_update(
        &self,
        state: &GateState,
        authority: &ValidatedConfig,
    ) -> Result<()> {
        if !matches!(state.phase, Phase::Active) {
            return Err(self.gate.unavailable());
        }
        if !same_authority(&state.authority, &self.authority)
            && !same_authority(&state.authority, authority)
        {
            return Err(Error::new(
                ErrorCode::ConfigConflict,
                "retained I/O authority has already changed",
            ));
        }
        if authority.membership_epoch() <= state.authority.membership_epoch()
            && !same_authority(&state.authority, authority)
        {
            return Err(Error::new(
                ErrorCode::ConfigConflict,
                "I/O authority cannot regress or replace the current membership epoch",
            ));
        }
        Ok(())
    }

    pub(crate) fn admit(&self) -> Result<DiskIoLease> {
        let mut state = self.gate.state()?;
        if !matches!(state.phase, Phase::Active) {
            return Err(self.gate.unavailable());
        }
        // Each count owns a distinct Arc allocation, so allocation limits bound
        // this counter below usize::MAX. No filesystem work runs under the lock.
        state.ordinary += 1;
        Ok(self.lease(Responsibility::Ordinary))
    }

    fn lease(&self, responsibility: Responsibility) -> DiskIoLease {
        DiskIoLease {
            owner: Arc::new(LeaseOwner {
                binding: self.clone(),
                responsibility,
            }),
        }
    }

    #[cfg(any(test, feature = "fault-injection"))]
    pub(crate) fn status(&self) -> Result<DiskIoStatus> {
        let state = self.gate.state()?;
        Ok(DiskIoStatus {
            quiescing: matches!(state.phase, Phase::Quiescing(_)),
            ordinary: state.ordinary,
            control: state.control,
        })
    }

    /// Close normal admission and reserve exactly one control responsibility in
    /// the same critical section. Repeated requests never mint another permit.
    /// No ordinary public Engine/API operation currently calls this primitive.
    #[cfg(any(test, feature = "fault-injection"))]
    pub(crate) fn begin_quiesce(&self) -> Result<Option<DiskIoTransition>> {
        let mut state = self.gate.state()?;
        if !same_authority(&state.authority, &self.authority) {
            return Err(Error::new(
                ErrorCode::ConfigConflict,
                "stale disk I/O authority cannot initiate quiescing",
            ));
        }
        if matches!(state.phase, Phase::Quiescing(_)) {
            return Ok(None);
        }
        let transition = Arc::new(());
        state.phase = Phase::Quiescing(Arc::clone(&transition));
        state.control = 1;
        let resources = std::mem::take(&mut state.resources);
        let controller = DiskIoTransition {
            gate: Arc::clone(&self.gate),
            transition,
            control: Some(self.lease(Responsibility::Control)),
        };
        // Never acquire a resource mutex while holding the gate. Resource I/O
        // holds its own mutex and then checks the gate, in that fixed order.
        drop(state);
        for resource in resources
            .into_values()
            .filter_map(|resource| resource.upgrade())
        {
            resource.revoke();
        }
        Ok(Some(controller))
    }
}

fn same_authority(left: &ValidatedConfig, right: &ValidatedConfig) -> bool {
    left.same_admission(right)
        || match (left.generation_hash(), right.generation_hash()) {
            (Some(left), Some(right)) => left == right,
            (None, None) => left.same_genesis_io_authority(right),
            _ => false,
        }
}

#[derive(Clone, Copy, Debug)]
enum Responsibility {
    Ordinary,
    #[cfg(any(test, feature = "fault-injection"))]
    Control,
}

#[derive(Debug)]
struct LeaseOwner {
    binding: DiskIoBinding,
    responsibility: Responsibility,
}

impl Drop for LeaseOwner {
    fn drop(&mut self) {
        let mut state = match self.binding.gate.state.lock() {
            Ok(state) => state,
            Err(_) => {
                // Do not recover possibly corrupted counts or panic in Drop.
                tracing::error!("disk I/O cleanup blocked by poisoned admission registry");
                return;
            }
        };
        let count = match self.responsibility {
            Responsibility::Ordinary => &mut state.ordinary,
            #[cfg(any(test, feature = "fault-injection"))]
            Responsibility::Control => &mut state.control,
        };
        if *count == 0 {
            state.failed = true;
            tracing::error!("disk I/O cleanup found an invalid responsibility count");
        } else {
            *count -= 1;
        }
    }
}

/// One admitted operation, including its blocking work and final cleanup. It is
/// deliberately not Clone. Only resource duplication can retain its ownership.
#[derive(Debug)]
pub(crate) struct DiskIoLease {
    owner: Arc<LeaseOwner>,
}

impl DiskIoLease {
    /// Consumers call this between bounded chunks. It is not an I/O error with
    /// Interrupted kind: retry-on-EINTR loops must not retry permanent revocation.
    /// Cleanup already owned by the operation is still allowed before Drop.
    pub(crate) fn check_running(&self) -> Result<()> {
        let state = self.owner.binding.gate.state()?;
        if state.phase.allows(self.owner.responsibility) {
            Ok(())
        } else {
            Err(self.owner.binding.gate.unavailable())
        }
    }

    /// Move the real operation resource or cleanup owner into its admission.
    /// No method can later extract T or the lease independently.
    pub(crate) fn hold<T: Send + 'static>(self, resource: T) -> DiskIoResource<T> {
        let binding = self.owner.binding.clone();
        let responsibility = self.owner.responsibility;
        let resource = Arc::new(ResourceSlot {
            value: Mutex::new(Some(ResourceValue {
                resource,
                lease: self,
            })),
            cancelled: AtomicBool::new(false),
            gate: Arc::clone(&binding.gate),
            responsibility,
        });
        let erase: Arc<dyn RevocableResource> = resource.clone();
        let registered = match binding.gate.state() {
            Ok(mut state) => match responsibility {
                Responsibility::Ordinary => {
                    if state.phase.allows(responsibility) {
                        state
                            .resources
                            .insert(resource.key(), Arc::downgrade(&erase));
                        true
                    } else {
                        false
                    }
                }
                #[cfg(any(test, feature = "fault-injection"))]
                Responsibility::Control => matches!(state.phase, Phase::Quiescing(_)),
            },
            Err(_) => false,
        };
        // A pre-quiesce operation may finish opening just after the snapshot.
        // Its resource is closed here instead of escaping the revoked registry.
        if !registered {
            resource.revoke();
        }
        DiskIoResource {
            resource,
            binding,
            responsibility,
        }
    }

    fn retain(&self) -> Self {
        Self {
            owner: Arc::clone(&self.owner),
        }
    }
}

/// This exact controller has one control lease, not a switch that permits new
/// work indefinitely. Dropping an unused controller retires its reserved lease.
#[cfg(any(test, feature = "fault-injection"))]
pub(crate) struct DiskIoTransition {
    gate: Arc<DiskIoGate>,
    transition: Arc<()>,
    control: Option<DiskIoLease>,
}

#[cfg(any(test, feature = "fault-injection"))]
impl DiskIoTransition {
    /// Reserve the unique responsibility. Holding it does not authorize a root
    /// transition write while earlier ordinary operations are still draining.
    pub(crate) fn take_control(&mut self, binding: &DiskIoBinding) -> Result<DiskIoLease> {
        drop(self.current_state(binding)?);
        self.consume_control()
    }

    /// Root control I/O starts only after the real tails of ordinary operations
    /// have retired. A busy result leaves the unique lease available for retry.
    /// Once quiescing has zero ordinary owners, no ordinary admission can race
    /// this check: the gate is closed and every pre-admitted owner is counted.
    #[cfg(test)]
    pub(crate) fn take_drained_control(&mut self, binding: &DiskIoBinding) -> Result<DiskIoLease> {
        let state = self.current_state(binding)?;
        if state.ordinary != 0 {
            return Err(Error::new(
                ErrorCode::DiskUnavailable,
                "disk transition I/O is waiting for ordinary operation cleanup",
            ));
        }
        drop(state);
        self.consume_control()
    }

    fn current_state(&self, binding: &DiskIoBinding) -> Result<MutexGuard<'_, GateState>> {
        if !Arc::ptr_eq(&self.gate, &binding.gate) {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "control lease requested for another disk binding",
            ));
        }
        let state = self.gate.state()?;
        if !matches!(&state.phase, Phase::Quiescing(id) if Arc::ptr_eq(id, &self.transition)) {
            return Err(Error::new(
                ErrorCode::ConfigConflict,
                "control lease does not own the current quiescing transition",
            ));
        }
        Ok(state)
    }

    fn consume_control(&mut self) -> Result<DiskIoLease> {
        self.control.take().ok_or_else(|| {
            Error::new(
                ErrorCode::ConfigConflict,
                "control lease has already been consumed",
            )
        })
    }
}

/// Declaration order keeps the real resource's cleanup ahead of lease release.
#[derive(Debug)]
struct ResourceValue<T> {
    resource: T,
    lease: DiskIoLease,
}

trait RevocableResource: Send + Sync {
    fn revoke(&self);
}

#[derive(Debug)]
struct ResourceSlot<T> {
    value: Mutex<Option<ResourceValue<T>>>,
    cancelled: AtomicBool,
    gate: Arc<DiskIoGate>,
    responsibility: Responsibility,
}

impl<T> ResourceSlot<T> {
    fn key(&self) -> usize {
        std::ptr::from_ref(self) as usize
    }
}

impl<T: Send> RevocableResource for ResourceSlot<T> {
    fn revoke(&self) {
        self.cancelled.store(true, Ordering::Release);
        let mut value = match self.value.try_lock() {
            Ok(value) => value,
            // The current syscall owns the descriptor until it returns. Its
            // ResourceAccess guard performs this same close at the real tail.
            Err(TryLockError::WouldBlock) => return,
            // Exclusive teardown only: never reuse a poisoned resource.
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
        };
        let retired = value.take();
        drop(value);
        drop(retired);
    }
}

impl<T> Drop for ResourceSlot<T> {
    fn drop(&mut self) {
        // A weak registry entry does not retain a resource. Remove it on normal
        // completion so admission costs O(log active resources), not O(history).
        match self.gate.state.lock() {
            Ok(mut state) => {
                state.resources.remove(&self.key());
            }
            Err(_) => tracing::error!("disk I/O resource cleanup found poisoned admission"),
        }
    }
}

/// A syscall's borrow and its actual descriptor are locked together. Revocation
/// cannot close/reuse that descriptor until this guard is released, even during
/// unwinding. Cleanup runs outside both resource and gate mutexes.
struct ResourceAccess<'a, T: Send> {
    value: Option<MutexGuard<'a, Option<ResourceValue<T>>>>,
    owner: &'a ResourceSlot<T>,
}

impl<T: Send> ResourceAccess<'_, T> {
    fn value(&mut self) -> &mut ResourceValue<T> {
        self.value
            .as_mut()
            .and_then(|value| value.as_mut())
            .expect("invariant: admitted resource access owns its value")
    }
}

impl<T: Send> Drop for ResourceAccess<'_, T> {
    fn drop(&mut self) {
        let state = match self.owner.gate.state.lock() {
            Ok(state) => Some(state),
            Err(_) => {
                tracing::error!("disk I/O tail found poisoned admission registry");
                None
            }
        };
        let cancel = self.owner.cancelled.load(Ordering::Acquire)
            || state
                .as_ref()
                .is_none_or(|state| state.failed || !state.phase.allows(self.owner.responsibility));
        let retired = if cancel {
            self.owner.cancelled.store(true, Ordering::Release);
            self.value.as_mut().and_then(|value| value.take())
        } else {
            None
        };
        // Unlock the resource while still holding the gate. If quiescing won
        // the gate first, we close at this tail. Otherwise its later snapshot
        // sees this resource unlocked. No try_lock/flag wakeup can be missed.
        // The lock order remains resource -> gate; no gate holder locks a resource.
        drop(self.value.take());
        drop(state);
        drop(retired);
    }
}

/// Revocable ownership of a real resource. Idle resources close at quiesce even
/// if their caller never polls or drops the wrapper again. There is deliberately
/// no Deref, raw File getter, fd getter, mutable resource getter or into_inner.
#[derive(Debug)]
pub struct DiskIoResource<T: Send + 'static> {
    resource: Arc<ResourceSlot<T>>,
    binding: DiskIoBinding,
    responsibility: Responsibility,
}

/// A held disk file and its inseparable operation responsibility.
pub type DiskFile = DiskIoResource<File>;

impl<T: Send + 'static> DiskIoResource<T> {
    /// Check for cancellation between bounded chunks. Cleanup remains counted
    /// until the actual resource is retired, even if this wrapper stays alive.
    ///
    /// # Errors
    /// Returns DiskUnavailable after quiescing, or RecoveryRequired if the
    /// admission registry cannot safely establish its state.
    pub fn check_running(&self) -> Result<()> {
        let state = self.binding.gate.state()?;
        if self.resource.cancelled.load(Ordering::Acquire) {
            return Err(self.binding.gate.unavailable());
        }
        if state.phase.allows(self.responsibility) {
            Ok(())
        } else {
            Err(self.binding.gate.unavailable())
        }
    }

    fn check_io(&self) -> io::Result<()> {
        self.check_running()
            .map_err(|source| io::Error::new(io::ErrorKind::PermissionDenied, source))
    }

    fn access_checked(&self) -> Result<ResourceAccess<'_, T>> {
        let value = self.resource.value.lock().map_err(|_| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "disk I/O resource lock poisoned",
            )
        })?;
        let access = ResourceAccess {
            value: Some(value),
            owner: &self.resource,
        };
        self.check_running()?;
        Ok(access)
    }

    fn access(&self) -> io::Result<ResourceAccess<'_, T>> {
        self.access_checked()
            .map_err(|source| io::Error::new(io::ErrorKind::PermissionDenied, source))
    }
}

impl DiskIoResource<File> {
    /// Never expose the inner File: verification holds its real resource guard
    /// through identity, reads and cleanup, with cancellation between chunks.
    pub(crate) fn verify_full(
        &self,
        expectation: platform_io::payload::Expectation,
    ) -> std::result::Result<platform_io::payload::Receipt, platform_io::payload::Error> {
        let mut access = self.access()?;
        platform_io::payload::verify_full_observed(
            &mut access.value().resource,
            expectation,
            &mut || self.check_io(),
        )
    }

    pub fn metadata(&self) -> io::Result<Metadata> {
        self.access()?.value().resource.metadata()
    }

    pub fn sync_all(&self) -> io::Result<()> {
        self.access()?.value().resource.sync_all()
    }

    pub fn sync_data(&self) -> io::Result<()> {
        self.access()?.value().resource.sync_data()
    }

    pub fn set_len(&self, size: u64) -> io::Result<()> {
        self.access()?.value().resource.set_len(size)
    }

    pub fn try_clone(&self) -> io::Result<Self> {
        let (file, lease) = {
            let mut access = self.access()?;
            let value = access.value();
            (value.resource.try_clone()?, value.lease.retain())
        };
        let file = lease.hold(file);
        file.check_io()?;
        Ok(file)
    }
}

impl DiskIoResource<DirectoryEntries> {
    /// The same resource guard covers buffered names and actual getdents calls.
    /// Keep the existing platform iterator error shape, including the original
    /// admission error as the source of a non-retryable PermissionDenied error.
    pub(super) fn next_entry(
        &mut self,
    ) -> Option<std::result::Result<EntryName, platform_io::fs_safety::Error>> {
        match self.access() {
            Ok(mut access) => access.value().resource.next(),
            Err(error) => Some(Err(error.into())),
        }
    }
}

impl FileExt for DiskIoResource<File> {
    fn read_at(&self, output: &mut [u8], offset: u64) -> io::Result<usize> {
        self.access()?.value().resource.read_at(output, offset)
    }

    fn write_at(&self, input: &[u8], offset: u64) -> io::Result<usize> {
        self.access()?.value().resource.write_at(input, offset)
    }
}

impl<T: Read + Send + 'static> Read for DiskIoResource<T> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        self.access()?.value().resource.read(output)
    }
}

impl<T: Write + Send + 'static> Write for DiskIoResource<T> {
    fn write(&mut self, input: &[u8]) -> io::Result<usize> {
        self.access()?.value().resource.write(input)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.access()?.value().resource.flush()
    }
}

impl<T: Seek + Send + 'static> Seek for DiskIoResource<T> {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.access()?.value().resource.seek(position)
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/disk/io_admission_tests.rs"]
mod tests;
