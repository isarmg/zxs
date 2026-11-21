//! RustCrypto SHA-2 (MIT OR Apache-2.0); original notices in handshake_hash.
//! Modified by ZXS to invoke only the private, otherwise unchanged selector.
crate::smb::handshake_hash::backend_select::cfg_if! {
    if #[cfg(sha2_backend_soft = "compact")] {
        mod compact;
        pub(super) use compact::compress;
    } else {
        mod unroll;
        pub(super) use unroll::compress;
    }
}

#[inline(always)]
fn to_u64s(block: &[u8; 128]) -> [u64; 16] {
    core::array::from_fn(|i| {
        let chunk = block[8 * i..][..8].try_into().unwrap();
        u64::from_be_bytes(chunk)
    })
}
