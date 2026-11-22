//! x86 feature detection adapted from `RustCrypto`, MIT OR Apache-2.0.

/// Evaluate the given `$body` expression any of the supplied target features
/// are not enabled. Otherwise returns true.
///
/// The `$body` expression is not evaluated on SGX targets, and returns false
/// on these targets unless *all* supplied target features are enabled.
macro_rules! when_not_fixed {
    ($($tf:tt),+ => $body:expr ) => {{
        #[cfg(not(all($(target_feature=$tf,)*)))]
        {
            #[cfg(not(any(target_env = "sgx", target_os = "none", target_os = "uefi")))]
            $body

            // CPUID is not available on SGX. Freestanding and UEFI targets
            // do not support SIMD features with default compilation flags.
            #[cfg(any(target_env = "sgx", target_os = "none", target_os = "uefi"))]
            false
        }

        #[cfg(all($(target_feature=$tf,)*))]
        true
    }};
}

/// Use CPUID to detect the presence of all supplied target features.
macro_rules! detect {
    ($($tf:tt),+) => {{
        #[cfg(target_arch = "x86")]
        use core::arch::x86::{__cpuid, __cpuid_count, CpuidResult};
        #[cfg(target_arch = "x86_64")]
        use core::arch::x86_64::{__cpuid, __cpuid_count, CpuidResult};

        unsafe fn cpuid(leaf: u32) -> CpuidResult {
            __cpuid(leaf)
        }

        unsafe fn cpuid_count(leaf: u32, sub_leaf: u32) -> CpuidResult {
            __cpuid_count(leaf, sub_leaf)
        }

        let cr = unsafe {
            [cpuid(1), cpuid_count(7, 0), cpuid_count(7, 1)]
        };

        $($crate::smb::hardware_capabilities::check_capability!(cr, $tf) & )+ true
    }};
}

/// Check that OS supports required SIMD registers
macro_rules! check_registers {
    ($cr:expr, $mask:expr) => {{
        #[cfg(target_arch = "x86")]
        use core::arch::x86 as arch;
        #[cfg(target_arch = "x86_64")]
        use core::arch::x86_64 as arch;

        // Check bits 26 and 27
        let xmask = 0b11 << 26;
        let xsave = $cr[0].ecx & xmask == xmask;
        if xsave {
            let xcr0 = unsafe { arch::_xgetbv(arch::_XCR_XFEATURE_ENABLED_MASK) };
            (xcr0 & $mask) == $mask
        } else {
            false
        }
    }};
}

macro_rules! __expand_check_macro {
    ($(($name:tt, $reg_cap:tt $(, $i:expr, $reg:ident, $offset:expr)*)),* $(,)?) => {
        macro_rules! check {
            $(
                ($cr:expr, $name) => {{
                    // Register bits are listed here:
                    // https://wiki.osdev.org/CPU_Registers_x86#Extended_Control_Registers
                    let reg_cap = match $reg_cap {
                        // Bit 1
                        "xmm" => $crate::smb::hardware_capabilities::check_registers!($cr, 0b10),
                        // Bits 1 and 2
                        "ymm" => $crate::smb::hardware_capabilities::check_registers!($cr, 0b110),
                        // Bits 1, 2, 5, 6, and 7
                        "zmm" => $crate::smb::hardware_capabilities::check_registers!($cr, 0b1110_0110),
                        _ => true,
                    };
                    reg_cap
                    $(
                        & ($cr[$i].$reg & (1 << $offset) != 0)
                    )*
                }};
            )*
        }
    };
}

__expand_check_macro! {
    ("sse4.1", "", 0, ecx, 19),
    ("aes", "", 0, ecx, 25),
    ("avx2", "ymm", 1, ebx, 5, 0, ecx, 28),
    ("avx512f", "zmm", 1, ebx, 16),
    ("sha", "", 1, ebx, 29),
    ("vaes", "zmm", 1, ecx, 9),
}

pub(in crate::smb) use {check as check_capability, check_registers, detect, when_not_fixed};
