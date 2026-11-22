//! Private boot and custody of the internal ZXS SMB worker. A public
//! envelope never substitutes for the actual parent, original FD, current
//! Core credentials, or explicit service activation.
mod envelope;
mod kernel;
mod descriptor;
mod held_open;
mod socket_facts;
mod pair_gate;
mod cohort;
#[cfg(all(target_arch="x86_64",target_pointer_width="64"))]
#[allow(dead_code)]
mod custody;
#[cfg(all(target_arch="x86_64",target_pointer_width="64"))]
mod site;
mod low;
mod signals;
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

pub(in crate::smb) const DIRECT_PORT:u16=445;
const DIRECT_BIND:std::net::SocketAddr=std::net::SocketAddr::V4(
    std::net::SocketAddrV4::new(std::net::Ipv4Addr::UNSPECIFIED,DIRECT_PORT));
mod worker;
mod maintenance;
mod entry;
pub(in crate::smb) use entry::stopping;
pub use entry::try_supervised_zxs_role;

// Internal exec vocabulary shared by the actual birth and the binary entry.
// A matching marker selects a guarded path, never a role's authority.
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
enum Role {Server,Preflight,Web}
impl Role{
    const ALL:[Self;3]=[Self::Server,Self::Preflight,Self::Web];
    const fn marker(self)->&'static std::ffi::CStr{
        match self{
            Self::Server=>c"zxs:internal-smb-311",
            Self::Preflight=>c"zxs:internal-smb-preflight-311",
            Self::Web=>c"zxs:internal-web-1",
        }
    }
}

#[derive(Clone,Copy,Debug,Eq,PartialEq)]
enum Fault {Shape,Resources,Handoff}
type Result<T>=std::result::Result<T,Fault>;
