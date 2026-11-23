//! Standalone ZXS storage. Inspection of a fresh selection is read-only.
// The optional RISC-V SHA-2 backend uses experimental intrinsics.
#![cfg_attr(
    any(sha2_backend = "riscv-zknh", sha2_256_backend = "riscv-zknh"),
    feature(riscv_ext_intrinsics)
)]

#[cfg(target_os = "linux")]
pub(crate) mod container_store;
#[cfg(target_os = "linux")]
mod fs;
#[cfg(target_os = "linux")]
mod hash;
#[cfg(target_os = "linux")]
mod headless_auth;
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod headless_boot;
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod headless_gateway;
#[cfg(target_os = "linux")]
mod headless_http;
#[cfg(target_os = "linux")]
mod headless_lifecycle;
#[cfg(target_os = "linux")]
mod headless_start;
#[cfg(target_os = "linux")]
mod workspace_protocol;

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[doc(hidden)]
pub fn start_headless() -> std::io::Result<()> {
    let gateway = headless_gateway::LaunchFds::inherit()?;
    headless_boot::run(Some(gateway), workspace_protocol::http::dispatch_request)
}
#[cfg(all(
    target_os = "linux",
    target_arch = "x86_64",
    feature = "host-activation"
))]
#[doc(hidden)]
pub fn activate_headless() -> std::io::Result<()> {
    headless_gateway::activate()
}

#[cfg(target_os = "linux")]
mod display;
#[cfg(target_os = "linux")]
mod password;

#[cfg(all(target_os = "linux", test))]
use fs::Entity;
#[cfg(all(target_os = "linux", test))]
#[path = "../tests/unit/support.rs"]
mod test_support;
