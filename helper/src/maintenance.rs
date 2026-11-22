//! Kernel mount/process evidence used by live Helper commands. Old
//! MAINT-1 registration workflows exist only in the development tests.
mod ancestry;
mod thread_evidence;

// Keep historical proof fixtures buildable without admitting these operations
// to the product. Active INIT, receipt and account work use sqlite_install.
#[cfg(test)]
mod genesis;
#[cfg(test)]
mod identity;
#[cfg(test)]
mod pending;
#[cfg(test)]
mod root_set;
#[cfg(test)]
mod register_flock;

pub(crate) use ancestry::{DiscoveryMount,DiscoveryView,Mounts as HostMounts};
pub(crate) use thread_evidence::{diagnostic_worker,thread_file};
#[cfg(test)]
pub(crate) use thread_evidence::WorkerPolicy;

/// Fixed entries of the legacy registration fixtures, not product paths.
#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq)]
enum RegisterName { Pending, Ready }
#[cfg(test)]
impl RegisterName {
    fn entry(self) -> &'static std::ffi::CStr {
        match self { Self::Pending => c"INITIALIZING", Self::Ready => c"REGISTRATION" }
    }
}
