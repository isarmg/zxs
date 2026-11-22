//! Private SMB 3.1.1 implementation of the one ZXS application. The
//! authenticated startup and Core storage authority are not public interfaces.
mod backend_select;
mod handshake_hash;
#[allow(dead_code, reason = "input layer is staged before the network runtime")]
mod wire;
mod local;
#[allow(dead_code, reason = "Rust supervisor handoff is staged before activation")]
mod launch;
pub(crate) use launch::InheritedRun;
pub use launch::{start_original_zxs,try_supervised_zxs_role};
#[allow(dead_code, reason = "per-transport accounting is staged before the network runtime")]
mod connection;
pub(crate) use connection::sessions::ordinary::RustSessionWitness;
pub(crate) use connection::accepted::incoming::OriginalMoveFence;
#[allow(dead_code, reason = "signing is staged before the authenticated network runtime")]
mod session;
#[allow(dead_code, reason = "share votes are staged before real storage-open integration")]
mod backend;
pub(crate) use backend::{DirectoryBirthVote,FileBirthVote,FileRenameTarget,OrdinaryDirectoryRenameAdmission,OrdinaryMoveAdmission,OrdinaryRenameAdmission};

#[cfg(test)]
#[path = "../test/smb/mod.rs"]
mod tests;
