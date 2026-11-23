//! Standalone ZXS storage. Inspection of a fresh selection is read-only.

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
#[cfg(target_os = "linux")]
mod headless_http;
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod headless_import_worker;
#[cfg(target_os = "linux")]
mod headless_lifecycle;
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod headless_offline;
#[cfg(target_os = "linux")]
mod headless_start;
#[cfg(target_os = "linux")]
mod workspace_protocol;

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[doc(hidden)]
pub fn start_headless(listen: std::net::SocketAddr) -> std::io::Result<()> {
    headless_boot::run(listen, workspace_protocol::http::dispatch_request)
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[doc(hidden)]
pub fn offline_live_names_v1() -> std::io::Result<()> {
    headless_offline::run()
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[doc(hidden)]
pub fn offline_import_one_v1() -> std::io::Result<()> {
    headless_import_worker::run()
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
