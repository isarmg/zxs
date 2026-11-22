// Adapted from `RustCrypto` (MIT OR Apache-2.0); see ORIGIN.md.
//! Observe the written memory so the compiler keeps the preceding erase.

pub fn optimization_barrier<T: ?Sized>(val: &T) {
    #[cfg(all(
        not(miri),
        any(
            target_arch = "aarch64",
            target_arch = "arm",
            target_arch = "arm64ec",
            target_arch = "loongarch64",
            target_arch = "riscv32",
            target_arch = "riscv64",
            target_arch = "s390x",
            target_arch = "x86",
            target_arch = "x86_64",
        )
    ))]
    unsafe {
        core::arch::asm!(
            "# {}",
            in(reg) core::ptr::from_ref::<T>(val).cast::<()>(),
            options(readonly, preserves_flags, nostack),
        );
    }
    #[cfg(not(all(
        not(miri),
        any(
            target_arch = "aarch64",
            target_arch = "arm",
            target_arch = "arm64ec",
            target_arch = "loongarch64",
            target_arch = "riscv32",
            target_arch = "riscv64",
            target_arch = "s390x",
            target_arch = "x86",
            target_arch = "x86_64",
        )
    )))]
    core::hint::black_box(val);
}
