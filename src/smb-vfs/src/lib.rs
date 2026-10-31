#![deny(unsafe_op_in_unsafe_fn, clippy::undocumented_unsafe_blocks)]
//! Rust SMB VFS extension. Building the loadable module requires a configured
//! Debian 13 target 4.22.11 SDK, selected explicitly with the smb-sdk feature.
// Aborting would bypass the panic isolation around callbacks and worker jobs.
#[cfg(panic = "abort")]
compile_error!("ZXS SMB VFS requires panic=unwind to isolate panics at the host boundary");
#[cfg(not(feature = "smb-sdk"))]
compile_error!(
    "the loadable ZXS VFS module requires smb-sdk; build smb-protocol for protocol-only tools"
);
#[cfg(feature = "smb-sdk")]
mod module;
