//! RustCrypto SHA-2 (MIT OR Apache-2.0); see ORIGIN.md and the retained notices.
//! Modified by ZXS to keep the two fixed hashes required by the SMB handshake.
#![allow(clippy::needless_range_loop)]

mod backend_select;
mod block_api;
#[rustfmt::skip]
mod consts;
mod sha256;
mod sha512;

use digest::{
    block_api::{Buffer, CtOutWrapper},
    consts::{U32, U64},
    zeroize::ZeroizeOnDrop,
};

type Core256 = CtOutWrapper<block_api::Sha256VarCore, U32>;
type Core512 = CtOutWrapper<block_api::Sha512VarCore, U64>;

digest::buffer_fixed!(
    /// SHA-256 for this SMB session's key derivation.
    pub(super) struct Sha256(Core256);
    impl: BaseFixedTraits AlgorithmName Default Clone HashMarker Reset FixedOutputReset SerializableState;
);
digest::buffer_fixed!(
    /// SHA-512 for the SMB 3.1.1 negotiation transcript.
    pub(super) struct Sha512(Core512);
    impl: BaseFixedTraits AlgorithmName Default Clone HashMarker Reset FixedOutputReset SerializableState;
);

// The old FixedHashTraits macro selected these markers via the SHA-2 crate
// feature. The used zeroize mode remains mandatory inside this module.
const _: () = {
    fn requires_erasure<T: ZeroizeOnDrop>() {}
    let _ = requires_erasure::<Core256>;
    let _ = requires_erasure::<Core512>;
    let _ = requires_erasure::<Buffer<Core256>>;
    let _ = requires_erasure::<Buffer<Core512>>;
};
impl ZeroizeOnDrop for Sha256 {}
impl ZeroizeOnDrop for Sha512 {}

#[cfg(test)]
mod tests {
    use super::{Sha256,Sha512};
    use digest::Digest;

    fn split_answer<D: Digest>(input: &[u8], hex: &str) {
        let bytes: Vec<u8> = (0..hex.len()).step_by(2).map(|i|
            u8::from_str_radix(&hex[i..i+2], 16).unwrap()
        ).collect();
        for split in 0..=input.len() {
            let mut state = D::new();
            state.update(&input[..split]);
            state.update(&input[split..]);
            assert_eq!(state.finalize().as_slice(), bytes.as_slice(), "{split}");
        }
    }

    #[test]
    fn fixed_handshake_hashes_match_rfc6234_at_padding_edges() {
        // RFC 6234 section 8.5: independent one- and two-block padded examples.
        split_answer::<Sha256>(b"abc",
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        split_answer::<Sha256>(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq",
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1");
        split_answer::<Sha512>(b"abc", concat!(
            "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a",
            "2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"));
        split_answer::<Sha512>(concat!(
            "abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmn",
            "hijklmnoijklmnopjklmnopqklmnopqrlmnopqrsmnopqrstnopqrstu").as_bytes(), concat!(
            "8e959b75dae313da8cf4f72814fc143f8f7779c6eb9f7fa17299aeadb68890185",
            "01d289e4900f7e4331b99dec4b5433ac7d329eeb6dd26545e96e55b874be909"));
    }
}
