use core::marker::PhantomData;
use std::sync::MutexGuard;
use std::sync::atomic::{AtomicUsize, Ordering};
use synchronization::RawMutex as NativeRawMutex;
use synchronization::api::{GuardNoSend, RawMutex};

static RAW_DROPS: AtomicUsize = AtomicUsize::new(0);

// MutexGuard<()> is Sync but not Send. Its phantom preserves that distinction
// without constructing a real standard mutex guard or claiming destructor UB.
pub struct SyncOnlyRaw {
    inner: NativeRawMutex,
    _not_send: PhantomData<MutexGuard<'static, ()>>,
}

pub fn raw_drop_count() -> usize {
    RAW_DROPS.load(Ordering::Relaxed)
}

impl Drop for SyncOnlyRaw {
    fn drop(&mut self) {
        RAW_DROPS.fetch_add(1, Ordering::Relaxed);
    }
}

// SAFETY: Every ownership operation forwards to the same native mutex. The
// phantom and drop counter change no exclusion or unlock ownership contract.
// Sync is derived from the fields; there is no manual Send or Sync bypass.
unsafe impl RawMutex for SyncOnlyRaw {
    const INIT: Self = Self {
        inner: <NativeRawMutex as RawMutex>::INIT,
        _not_send: PhantomData,
    };

    type GuardMarker = GuardNoSend;

    fn lock(&self) {
        RawMutex::lock(&self.inner);
    }

    fn try_lock(&self) -> bool {
        RawMutex::try_lock(&self.inner)
    }

    unsafe fn unlock(&self) {
        // SAFETY: The caller owns the lock acquired on this native mutex.
        unsafe { RawMutex::unlock(&self.inner) };
    }

    fn is_locked(&self) -> bool {
        RawMutex::is_locked(&self.inner)
    }
}
