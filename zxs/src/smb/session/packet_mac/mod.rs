//! RustCrypto CMAC (MIT OR Apache-2.0); see ORIGIN.md and the retained notices.
//! Modified by ZXS for use inside the SMB session boundary.
#![forbid(unsafe_code)]

/// Block-level implementation.
mod block_api;
mod double;

use block_api::CmacCipher;
use core::fmt;
use digest::block_api::{AlgorithmName, CoreProxy};

digest::buffer_fixed!(
    /// Generic CMAC instance.
    #[derive(Clone)]
    pub struct Cmac<C: CmacCipher>(block_api::CmacCore<C>);
    impl: ResetMacTraits InnerInit;
);

impl<C: CmacCipher + AlgorithmName> AlgorithmName for Cmac<C> {
    fn write_alg_name(f: &mut fmt::Formatter<'_>) -> fmt::Result {
        <Self as CoreProxy>::Core::write_alg_name(f)
    }
}
