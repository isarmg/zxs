//! RustCrypto MD5 (MIT OR Apache-2.0); see ORIGIN.md and the retained notices.
//! Modified by ZXS for its private NTLM verifier, not for general hashing.

mod block_api;
mod compress;
mod consts;

digest::buffer_fixed!(
    /// MD5 hasher state.
    pub(super) struct Md5(block_api::Md5Core);
    impl: BaseFixedTraits AlgorithmName Default Clone HashMarker Reset FixedOutputReset SerializableState;
);

// The old FixedHashTraits invocation selected this marker in the MD5 crate.
// An expanded Cargo cfg would now belong to ZXS, where erasure is mandatory.
use digest::{block_api::Buffer, zeroize::ZeroizeOnDrop};
const _: () = {
    fn requires_erasure<T: ZeroizeOnDrop>() {}
    let _ = requires_erasure::<block_api::Md5Core>;
    let _ = requires_erasure::<Buffer<block_api::Md5Core>>;
};
impl ZeroizeOnDrop for Md5 {}

#[cfg(test)]
mod tests {
    use super::Md5;
    use digest::Digest;

    #[test]
    fn ntlm_digest_matches_rfc1321_at_every_buffer_split() {
        // RFC 1321 Appendix A.5: empty, short, long-padding and full-block inputs.
        let examples: [(&[u8], &str); 4] = [
            (b"", "d41d8cd98f00b204e9800998ecf8427e"),
            (b"abc", "900150983cd24fb0d6963f7d28e17f72"),
            (b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789", "d174ab98d277d9f5a5611c2c9f419d9f"),
            (b"12345678901234567890123456789012345678901234567890123456789012345678901234567890",
                "57edf4a22be3c955ac49da2e2107b67a"),
        ];
        for (input, hex) in examples {
            let tag: [u8; 16] = std::array::from_fn(|i| u8::from_str_radix(&hex[2*i..2*i+2], 16).unwrap());
            for split in 0..=input.len() {
                let mut state = Md5::new();
                state.update(&input[..split]);
                state.update(&input[split..]);
                assert_eq!(state.finalize().as_slice(), &tag, "{}/{split}",input.len());
            }
        }
    }
}
