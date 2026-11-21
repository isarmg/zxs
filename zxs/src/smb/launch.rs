//! Private boot and custody of the internal ZXS SMB worker. A public
//! envelope never substitutes for the actual parent, original FD, current
//! Core credentials, or explicit service activation.
mod envelope;
mod kernel;
mod pair_gate;
mod cohort;
#[cfg(all(target_arch="x86_64",target_pointer_width="64"))]
#[allow(dead_code)]
mod custody;
#[cfg(all(target_arch="x86_64",target_pointer_width="64"))]
mod site;
mod low;
#[cfg(all(target_arch="x86_64",target_pointer_width="64"))]
mod web;
#[cfg(all(target_arch="x86_64",target_pointer_width="64"))]
mod application;
#[cfg(all(target_arch="x86_64",target_pointer_width="64"))]
pub use application::start_original_zxs;
#[cfg(not(all(target_arch="x86_64",target_pointer_width="64")))]
pub fn start_original_zxs()->std::io::Result<()>{
    Err(std::io::Error::new(std::io::ErrorKind::Unsupported,"the installed ZXS start requires x86-64 Linux"))
}
mod transport;
pub(crate) use transport::InheritedRun;
pub(in crate::smb) use transport::OwnedAccepted;
mod worker;
mod maintenance;
mod entry;
pub(in crate::smb) use entry::stopping;
pub use entry::try_supervised_zxs_role;

#[derive(Clone,Copy,Debug,Eq,PartialEq)]
enum Fault {Shape,Resources,Handoff}
type Result<T>=std::result::Result<T,Fault>;
