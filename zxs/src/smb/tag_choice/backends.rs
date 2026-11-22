// ZXS adaptation of RustCrypto comparisons; see tag_choice/ORIGIN.md.
#[cfg(not(miri))]
#[cfg(target_arch = "aarch64")]
mod aarch64;
#[cfg(not(miri))]
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
mod x86;

// Fallback portable implementation for targets which don't have native predication instructions
// (or if they do, aren't currently supported)
#[cfg(any(
    not(any(target_arch = "aarch64", target_arch = "x86", target_arch = "x86_64")),
    miri
))]
mod soft;
