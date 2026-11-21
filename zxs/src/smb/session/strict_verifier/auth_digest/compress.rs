//! RustCrypto MD5 (MIT OR Apache-2.0); see the retained notices.
//! Modified by ZXS to spell the same kernel selection as complementary cfgs.

// Upstream selected soft when forced, otherwise assembly only on LoongArch.
#[cfg_attr(all(target_arch = "loongarch64", not(md5_backend = "soft")), path = "compress/loongarch64_asm.rs")]
#[cfg_attr(any(not(target_arch = "loongarch64"), md5_backend = "soft"), path = "compress/soft.rs")]
mod backend;

/// MD5 compression function
pub(super) fn compress(state: &mut [u32; 4], blocks: &[[u8; 64]]) {
    backend::compress(state, blocks)
}
