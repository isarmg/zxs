use core::cell::Cell;
use synchronization::RawRwLock as NativeRawRwLock;
use synchronization::api::{GuardNoSend, RawRwLock, RawRwLockUpgrade};

// Cell deliberately makes this owner non-Sync. Queries record observations;
// every lock mode and ownership transition is delegated to the native lock.
pub struct LocalRaw {
    inner: NativeRawRwLock,
    queries: Cell<u32>,
}

// SAFETY: Shared/exclusive ownership operations use the same native lock.
// The local observation counter changes neither exclusion nor return values.
// No Sync implementation is provided for this owner.
unsafe impl RawRwLock for LocalRaw {
    const INIT: Self = Self {
        inner: <NativeRawRwLock as RawRwLock>::INIT,
        queries: Cell::new(0),
    };

    type GuardMarker = GuardNoSend;

    fn lock_shared(&self) {
        RawRwLock::lock_shared(&self.inner);
    }

    fn try_lock_shared(&self) -> bool {
        RawRwLock::try_lock_shared(&self.inner)
    }

    unsafe fn unlock_shared(&self) {
        // SAFETY: The caller owns the shared mode on the forwarded native lock.
        unsafe { RawRwLock::unlock_shared(&self.inner) };
    }

    fn lock_exclusive(&self) {
        RawRwLock::lock_exclusive(&self.inner);
    }

    fn try_lock_exclusive(&self) -> bool {
        RawRwLock::try_lock_exclusive(&self.inner)
    }

    unsafe fn unlock_exclusive(&self) {
        // SAFETY: The caller owns the exclusive mode on the same native lock.
        unsafe { RawRwLock::unlock_exclusive(&self.inner) };
    }

    fn is_locked(&self) -> bool {
        self.queries.set(self.queries.get().wrapping_add(1));
        RawRwLock::is_locked(&self.inner)
    }

    fn is_locked_exclusive(&self) -> bool {
        RawRwLock::is_locked_exclusive(&self.inner)
    }
}

// SAFETY: Native operations maintain the single-upgrader protocol and transfer
// ownership between upgradable and exclusive modes on the same lock.
unsafe impl RawRwLockUpgrade for LocalRaw {
    fn lock_upgradable(&self) {
        RawRwLockUpgrade::lock_upgradable(&self.inner);
    }

    fn try_lock_upgradable(&self) -> bool {
        RawRwLockUpgrade::try_lock_upgradable(&self.inner)
    }

    unsafe fn unlock_upgradable(&self) {
        // SAFETY: The caller owns the forwarded upgradable mode.
        unsafe { RawRwLockUpgrade::unlock_upgradable(&self.inner) };
    }

    unsafe fn upgrade(&self) {
        // SAFETY: The caller owns the upgradable mode being transferred.
        unsafe { RawRwLockUpgrade::upgrade(&self.inner) };
    }

    unsafe fn try_upgrade(&self) -> bool {
        // SAFETY: The caller owns upgradable mode; the native result preserves
        // that mode on failure and transfers it to exclusive mode on success.
        unsafe { RawRwLockUpgrade::try_upgrade(&self.inner) }
    }
}
