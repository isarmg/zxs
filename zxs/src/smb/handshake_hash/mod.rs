//! RustCrypto SHA-2 (MIT OR Apache-2.0); see ORIGIN.md and the retained notices.
//! Modified by ZXS to keep the two fixed hashes required by the SMB handshake.
#![allow(clippy::needless_range_loop)]

mod block_api;
#[rustfmt::skip]
mod consts;
mod sha256;
mod sha512;

use crate::smb::fixed_output::{
    block_api::{Buffer, CtOutWrapper},
    consts::{U32, U64},
    zeroize::ZeroizeOnDrop,
};

type Core256 = CtOutWrapper<block_api::Sha256VarCore, U32>;
type Core512 = CtOutWrapper<block_api::Sha512VarCore, U64>;

crate::smb::fixed_output::buffer_fixed!(
    /// SHA-256 for this SMB session's key derivation.
    pub(super) struct Sha256(Core256);
    impl: BaseFixedTraits AlgorithmName Default Clone HashMarker Reset FixedOutputReset SerializableState;
);
crate::smb::fixed_output::buffer_fixed!(
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
#[path = "../../../tests/unit/attached/smb/handshake_hash/tests.rs"]
mod tests;
