//! RustCrypto HMAC (MIT OR Apache-2.0); see ORIGIN.md and the retained notices.
//! Modified by ZXS for integration as an internal SMB session module.
#![forbid(unsafe_code)]

/// Block-level implementation.
pub mod block_api;
mod simple;
mod simple_reset;
mod utils;

use core::fmt;
use digest::block_api::{AlgorithmName, CoreProxy, EagerHash};

digest::buffer_fixed!(
    /// Generic HMAC instance.
    #[derive(Clone)]
    pub struct Hmac<D: EagerHash>(block_api::HmacCore<D>);
    impl: MacTraits KeyInit;
);

impl<D: EagerHash + AlgorithmName> AlgorithmName for Hmac<D> {
    fn write_alg_name(f: &mut fmt::Formatter<'_>) -> fmt::Result {
        <Self as CoreProxy>::Core::write_alg_name(f)
    }
}

digest::buffer_fixed!(
    /// Generic HMAC instance with reset support.
    #[derive(Clone)]
    pub struct HmacReset<D: EagerHash>(block_api::HmacResetCore<D>);
    impl: ResetMacTraits KeyInit;
);

impl<D: EagerHash + AlgorithmName> AlgorithmName for HmacReset<D> {
    fn write_alg_name(f: &mut fmt::Formatter<'_>) -> fmt::Result {
        <Self as CoreProxy>::Core::write_alg_name(f)
    }
}
