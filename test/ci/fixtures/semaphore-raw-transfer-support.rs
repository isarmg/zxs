use core::marker::PhantomData;
use std::sync::MutexGuard;
use std::sync::atomic::{AtomicUsize, Ordering};
use synchronization::RawMutex as NativeRawMutex;
use synchronization::api::{GuardNoSend, RawMutex};

static RAW_DROPS: AtomicUsize = AtomicUsize::new(0);

// The phantom is Sync and !Send without constructing a real standard guard.
// It witnesses a type contract; it does not demonstrate destructor UB.
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

// SAFETY: All lock ownership operations forward to the same native raw mutex.
// The phantom and observation counter do not change exclusion or unlocking.
// Send and Sync are derived from fields; neither has a manual implementation.
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
        // SAFETY: The caller owns the corresponding native raw lock.
        unsafe { RawMutex::unlock(&self.inner) };
    }

    fn is_locked(&self) -> bool {
        RawMutex::is_locked(&self.inner)
    }
}
