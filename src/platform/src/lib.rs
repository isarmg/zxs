//! ZXS's independent, narrowly selected platform foundation.
//!
//! SQLite is a rebuildable query index. Neither its transactions nor any
//! authentication/session helper constitute a cross-disk commit decision.
//! The storage engine owns ACCEPT, FINALIZE, PUBLICATION, and recovery.
#![deny(unsafe_code, unsafe_op_in_unsafe_fn)]

#[cfg(not(all(
    target_os = "linux",
    target_arch = "x86_64",
    target_pointer_width = "64"
)))]
compile_error!(
    "platform-io requires Linux amd64 (64-bit x86_64); browser clients must use storage-contract"
);

pub mod fs_safety;
pub mod payload;
pub mod schema_identity;
pub mod secret;
pub mod sqlite;
