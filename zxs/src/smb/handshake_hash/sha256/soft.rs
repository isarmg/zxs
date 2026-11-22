//! RustCrypto SHA-2 (MIT OR Apache-2.0); original notices in handshake_hash.
//! Modified by ZXS to invoke only the private, otherwise unchanged selector.
crate::smb::backend_select::cfg_if! {
    if #[cfg(sha2_backend_soft = "compact")] {
        mod compact;
        pub(super) use compact::compress;
    } else {
        mod unroll;
        pub(super) use unroll::compress;
    }
}

fn to_u32s(block: &[u8; 64]) -> [u32; 16] {
    core::array::from_fn(|i| {
        let chunk = block[4 * i..][..4].try_into().unwrap();
        u32::from_be_bytes(chunk)
    })
}
