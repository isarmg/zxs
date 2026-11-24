//! Kernel mount/process evidence used by current Helper commands.
mod ancestry;
mod thread_evidence;

pub(crate) use ancestry::{DiscoveryView, Mounts as HostMounts};
pub(crate) use thread_evidence::diagnostic_worker;
#[cfg(any(test, feature = "embedded-zxs"))]
pub(crate) use thread_evidence::thread_file;

#[cfg(any(test, feature = "embedded-zxs"))]
mod host_admin;
#[cfg(any(test, feature = "embedded-zxs"))]
pub(crate) use host_admin::host_admin;
