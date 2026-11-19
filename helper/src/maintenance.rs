//! Maintenance internals; no CLI or Web command can allocate, publish or adopt a register.
mod ancestry;
mod genesis;
mod identity;
mod pending;
mod root_set;
mod register_flock;
mod thread_evidence;

// These two host observations have no dependence on the experimental MAINT-1 medium.
pub(crate) use ancestry::Mounts as HostMounts;
pub(crate) use thread_evidence::{WorkerPolicy,diagnostic_worker};

/// Fixed entries of the same registration, never names supplied by a caller.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RegisterName { Pending, Ready }
impl RegisterName {
    fn entry(self) -> &'static std::ffi::CStr {
        match self { Self::Pending => c"INITIALIZING", Self::Ready => c"REGISTRATION" }
    }
}
