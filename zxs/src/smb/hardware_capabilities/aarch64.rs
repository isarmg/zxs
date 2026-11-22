//! Linux AArch64 auxiliary-vector detection adapted from `RustCrypto`.

// Evaluate the given `$body` expression any of the supplied target features
// are not enabled. Otherwise returns true.
macro_rules! when_not_fixed {
    ($($tf:tt),+ => $body:expr ) => {
        {
            #[cfg(not(all($(target_feature=$tf,)*)))]
            $body

            #[cfg(all($(target_feature=$tf,)*))]
            true
        }
    };
}

// Linux runtime detection of target CPU features using `getauxval`.
macro_rules! detect {
    ($($tf:tt),+) => {{
        let hwcaps = unsafe { libc::getauxval(libc::AT_HWCAP) };
        $($crate::smb::hardware_capabilities::check_capability!(hwcaps, $tf) & )+ true
    }};
}

macro_rules! check {
    ($caps:expr, "aes") => { ($caps & (libc::HWCAP_AES | libc::HWCAP_PMULL)) != 0 };
    ($caps:expr, "sha2") => { ($caps & libc::HWCAP_SHA2) != 0 };
    ($caps:expr, "sha3") => { ($caps & (libc::HWCAP_SHA3 | libc::HWCAP_SHA512)) != 0 };
}

pub(in crate::smb) use {check as check_capability, detect, when_not_fixed};
