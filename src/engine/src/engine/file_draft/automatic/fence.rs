//! Process-local admission is only a resource/cancellation gate. It cannot
//! create an eligible task, select its revision, or replace the durable CAS.
use crate::{Error, Result, State};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, Weak, atomic::{AtomicBool, Ordering}};
use storage_contract::{FileId, PoolId};

type Active = (FileId, Arc<AtomicBool>);
static ACTIVE: Mutex<BTreeMap<PoolId, Active>> = Mutex::new(BTreeMap::new());

/// Only a currently held worker can issue a checkpoint. It cannot keep the
/// lease alive, create another task or invalidate an already accepted decision.
pub(in crate::engine::file_draft) struct PreparationCheck(Weak<AtomicBool>);

impl PreparationCheck {
    pub(in crate::engine::file_draft) fn check(&self) -> Result<()> {
        if self.0.upgrade().is_some_and(|flag| flag.load(Ordering::Acquire)) {
            Ok(())
        } else {
            Err(super::stale())
        }
    }
}

pub(super) struct Lease {
    pool: PoolId,
    active: Arc<AtomicBool>,
}
impl Lease {
    pub(super) fn claim(pool: PoolId, file: FileId) -> Result<Self> {
        let mut slots = ACTIVE.lock().expect("invariant: automatic FILE gate poisoned");
        if slots.contains_key(&pool) {
            return Err(Error::new(storage_contract::ErrorCode::CommitPreparationBusy,
                "an automatic FILE preparation already owns this pool").retryable());
        }
        let active = Arc::new(AtomicBool::new(true));
        slots.insert(pool, (file, Arc::clone(&active)));
        Ok(Self { pool, active })
    }
    pub(super) fn preparation_check(&self) -> PreparationCheck {
        PreparationCheck(Arc::downgrade(&self.active))
    }

    pub(super) fn check(&self) -> Result<()> {
        if self.active.load(Ordering::Acquire) { Ok(()) } else { Err(super::stale()) }
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        let mut slots = ACTIVE.lock().unwrap_or_else(|poison| poison.into_inner());
        if slots.get(&self.pool).is_some_and(|(_, flag)| Arc::ptr_eq(flag, &self.active)) {
            slots.remove(&self.pool);
        }
    }
}

pub(in crate::engine::file_draft) fn invalidate_changed(pool: PoolId, previous: &State, next: &State) {
    let slots = ACTIVE.lock().expect("invariant: automatic FILE gate poisoned");
    if let Some((file, active)) = slots.get(&pool)
        && previous.file_containers.get(file) != next.file_containers.get(file)
    { active.store(false, Ordering::Release); }
}
