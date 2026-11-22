//! `RustCrypto` MD5 (MIT OR Apache-2.0); see ORIGIN.md and the retained notices.
//! Modified by ZXS for its private NTLM verifier, not for general hashing.

mod block_api;
mod compress;
mod consts;

crate::smb::fixed_output::buffer_fixed!(
    /// MD5 hasher state.
    pub(super) struct Md5(block_api::Md5Core);
    impl: BaseFixedTraits AlgorithmName Default Clone HashMarker Reset FixedOutputReset SerializableState;
);

// The old FixedHashTraits invocation selected this marker in the MD5 crate.
// An expanded Cargo cfg would now belong to ZXS, where erasure is mandatory.
use crate::smb::fixed_output::{block_api::Buffer, zeroize::ZeroizeOnDrop};
const _: () = {
    fn requires_erasure<T: ZeroizeOnDrop>() {}
    let _ = requires_erasure::<block_api::Md5Core>;
    let _ = requires_erasure::<Buffer<block_api::Md5Core>>;
};
impl ZeroizeOnDrop for Md5 {}

#[cfg(test)]
#[path = "../../../../../tests/unit/attached/smb/session/strict_verifier/auth_digest/tests.rs"]
mod tests;
