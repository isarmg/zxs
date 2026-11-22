// ZXS adaptation of RustCrypto comparisons; see tag_choice/ORIGIN.md.
//! Portable best-effort comparison. Keep the original optimizer barriers.

use super::super::{CmovEq, Condition};
use core::ops::{BitAnd, BitOr, Not};

macro_rules! masknz {
    ($value:tt : $uint:ident) => {{
        let mut value: $uint = $value;
        value |= value.wrapping_neg(); // has MSB `1` if non-zero, `0` if zero

        // use `black_box` to obscure we're computing a 1-bit value
        core::hint::black_box(
            value >> ($uint::BITS - 1), // Extract MSB
        )
        .wrapping_neg() // Generate $uint::MAX mask if `black_box` outputs `1`
    }};
}

impl CmovEq for u16 {
    #[inline]
    fn cmovne(&self, rhs: &Self, input: Condition, output: &mut Condition) {
        *output = masksel(
            *output,
            input,
            (masknz32((*self ^ *rhs).into()) & 0xFF) as Condition,
        );
    }

    #[inline]
    fn cmoveq(&self, rhs: &Self, input: Condition, output: &mut Condition) {
        *output = masksel(
            *output,
            input,
            (!masknz32((*self ^ *rhs).into()) & 0xFF) as Condition,
        );
    }
}

impl CmovEq for u32 {
    #[inline]
    fn cmovne(&self, rhs: &Self, input: Condition, output: &mut Condition) {
        *output = masksel(*output, input, (maskne32(*self, *rhs) & 0xFF) as Condition);
    }

    #[inline]
    fn cmoveq(&self, rhs: &Self, input: Condition, output: &mut Condition) {
        *output = masksel(*output, input, (maskeq32(*self, *rhs) & 0xFF) as Condition);
    }
}

impl CmovEq for u64 {
    #[inline]
    fn cmovne(&self, rhs: &Self, input: Condition, output: &mut Condition) {
        *output = masksel(*output, input, (maskne64(*self, *rhs) & 0xFF) as Condition);
    }

    #[inline]
    fn cmoveq(&self, rhs: &Self, input: Condition, output: &mut Condition) {
        *output = masksel(*output, input, (maskeq64(*self, *rhs) & 0xFF) as Condition);
    }
}

/// Returns `u32::MAX` if `x` is equal to `y`, otherwise returns `0` (32-bit version)
#[inline]
fn maskeq32(x: u32, y: u32) -> u32 {
    !maskne32(x, y)
}

/// Returns `u64::MAX` if `x` is equal to `y`, otherwise returns `0` (64-bit version)
#[inline]
fn maskeq64(x: u64, y: u64) -> u64 {
    !maskne64(x, y)
}

/// Returns `0` if `x` is equal to `y`, otherwise returns `u32::MAX` (32-bit version)
#[inline]
fn maskne32(x: u32, y: u32) -> u32 {
    masknz32(x ^ y)
}

/// Returns `0` if `x` is equal to `y`, otherwise returns `u64::MAX` (64-bit version)
#[inline]
fn maskne64(x: u64, y: u64) -> u64 {
    masknz64(x ^ y)
}

/// Return a `u32::MAX` mask if `condition` is non-zero, otherwise return zero for a zero input.
#[cfg(any(
    miri,
    not(any(target_arch = "arm", target_arch = "riscv32", target_arch = "riscv64"))
))]
fn masknz32(condition: u32) -> u32 {
    masknz!(condition: u32)
}

/// Return a `u64::MAX` mask if `condition` is non-zero, otherwise return zero for a zero input.
#[cfg(any(
    miri,
    not(any(target_arch = "arm", target_arch = "riscv32", target_arch = "riscv64"))
))]
fn masknz64(condition: u64) -> u64 {
    masknz!(condition: u64)
}

/// Optimized mask generation for ARM32 targets.
///
/// This is written in assembly both for performance and because we've had problematic code
/// generation in this routine in the past which lead to the insertion of a branch, which using
/// assembly should guarantee won't happen again in the future (CVE-2026-23519).
#[cfg(all(target_arch = "arm", not(miri)))]
fn masknz32(condition: u32) -> u32 {
    let mut mask: u32;
    unsafe {
        core::arch::asm!(
            "rsbs {0}, {1}, #0",  // Reverse subtract
            "sbcs {0}, {0}, {0}", // Subtract with carry, setting flags
            lateout(reg) mask,
            in(reg) condition,
            options(nostack, nomem),
        );
    }
    mask
}

/// Optimized mask generation for riscv32 targets.
#[cfg(all(target_arch = "riscv32", not(miri)))]
fn masknz32(condition: u32) -> u32 {
    let mut mask: u32;
    unsafe {
        core::arch::asm!(
            "seqz {0}, {1}",  // Set-if-not-zero pseudo-instruction
            "addi {0}, {0}, -1", // Subtract 1, to have either full ones or full zeroes mask
            lateout(reg) mask,
            in(reg) condition,
            options(nostack, nomem),
        );
    }
    mask
}

/// Optimized mask generation for riscv32 targets.
#[cfg(all(target_arch = "riscv64", not(miri)))]
fn masknz32(condition: u32) -> u32 {
    (masknz64(condition.into()) & 0xFFFF_FFFF) as u32
}

/// Optimized mask generation for riscv64 targets.
#[cfg(all(target_arch = "riscv64", not(miri)))]
fn masknz64(condition: u64) -> u64 {
    let mut mask: u64;
    unsafe {
        core::arch::asm!(
            "seqz {0}, {1}",  // Set-if-not-zero pseudo-instruction
            "addi {0}, {0}, -1", // Subtract 1, to have either full ones or full zeroes mask
            lateout(reg) mask,
            in(reg) condition,
            options(nostack, nomem),
        );
    }
    mask
}

/// 64-bit wrapper for targets that implement 32-bit mask generation in assembly.
#[cfg(all(any(target_arch = "arm", target_arch = "riscv32"), not(miri)))]
fn masknz64(condition: u64) -> u64 {
    let lo = masknz32((condition & 0xFFFF_FFFF) as u32);
    let hi = masknz32((condition >> 32) as u32);
    let mask = u64::from(lo | hi);
    mask | mask << 32
}

/// Given a supplied mask of `0` or all 1-bits (i.e. `u*::MAX`), select `a` if the mask is all-zeros
/// and `b` if the mask is all-ones.
///
/// This function shouldn't be used with a mask that isn't `0` or `u*::MAX`.
#[inline]
fn masksel<T>(a: T, b: T, mask: T) -> T
where
    T: BitAnd<Output = T> + BitOr<Output = T> + Copy + Not<Output = T>,
{
    (a & !mask) | (b & mask)
}
