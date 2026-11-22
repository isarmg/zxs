//! Target-native layouts for the existing role-local signal setup.

use std::ffi::c_int;

type Handler = extern "C" fn(c_int);

unsafe extern "C" {
    fn install_lowered_term(handler: Handler) -> c_int;
    #[cfg(all(target_arch = "x86_64", target_pointer_width = "64"))]
    fn install_foreground_stop(handler: Handler) -> c_int;
    #[cfg(all(target_arch = "x86_64", target_pointer_width = "64"))]
    fn reset_post_clone_signals() -> c_int;
}

/// The caller must be the single-threaded lowered entry, with an
/// async-signal-safe handler valid for the installed disposition.
pub(super) unsafe fn lowered_term(handler: Handler) -> bool {
    // SAFETY: the caller supplies the role and handler constraints.
    unsafe { install_lowered_term(handler) == 1 }
}

/// The caller must be the single-threaded high foreground, with an
/// async-signal-safe handler valid for the installed dispositions.
#[cfg(all(target_arch = "x86_64", target_pointer_width = "64"))]
pub(super) unsafe fn foreground_stop(handler: Handler) -> bool {
    // SAFETY: the caller supplies the role and handler constraints.
    unsafe { install_foreground_stop(handler) == 1 }
}

/// Only the just-cloned task with its independent signal dispositions
/// may reset them here, before it can enter the lowered role.
#[cfg(all(target_arch = "x86_64", target_pointer_width = "64"))]
pub(super) unsafe fn post_clone_defaults() -> bool {
    // SAFETY: the caller is the freshly cloned task, not the parent.
    unsafe { reset_post_clone_signals() == 1 }
}
