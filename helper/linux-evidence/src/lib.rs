//! Fixed Linux process and signal boundaries, and local filesystem type admission.
#![deny(unsafe_code, unsafe_op_in_unsafe_fn)]
#[cfg(not(all(
    target_os = "linux",
    target_arch = "x86_64",
    target_pointer_width = "64"
)))]
compile_error!("helper-linux-evidence supports x86_64 Linux LP64 only");

pub mod diagnostic_process;
pub mod prompt_signals;

use linux_raw_sys::general as uapi;

/// Local data filesystem types only; this check does not prove mount identity.
#[must_use]
pub fn supported_local_magic(magic: u32) -> bool {
    matches!(
        magic,
        uapi::EXT4_SUPER_MAGIC
            | uapi::XFS_SUPER_MAGIC
            | uapi::BTRFS_SUPER_MAGIC
            | uapi::F2FS_SUPER_MAGIC
    )
}
