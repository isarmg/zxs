// ZXS adaptation of RustCrypto fixed-output support; see ORIGIN.md.
//! Fixed hash and MAC support used only by SMB.
#![forbid(unsafe_code)]

pub(super) mod block_api;
mod buffer_fixed;
mod hash;
mod mac;

pub(super) use super::block_accumulator as block_buffer;
pub(super) use buffer_fixed::buffer_fixed;
pub(super) use super::crypto_types as common;
pub(super) use crate::smb::crypto_types::{InvalidLength, Key, KeyInit, Output, OutputSizeUser, Reset, array, type_lengths};
pub(super) use crate::smb::crypto_types::type_lengths::consts;
pub(super) use hash::{Digest, HashMarker};
pub(super) use mac::{Mac, MacMarker};
pub(super) use super::memory_erasure as zeroize;
use core::fmt;

/// Types which consume data with byte granularity.
pub trait Update {
    /// Update state using the provided data.
    fn update(&mut self, data: &[u8]);
}

/// Trait for hash functions with fixed-size output.
pub trait FixedOutput: Update + OutputSizeUser + Sized {
    /// Consume value and write result into provided array.
    fn finalize_into(self, out: &mut Output<Self>);

    /// Retrieve result and consume the hasher instance.
    #[inline]
    fn finalize_fixed(self) -> Output<Self> {
        let mut out = Default::default();
        self.finalize_into(&mut out);
        out
    }
}

/// Trait for hash functions with fixed-size output able to reset themselves.
pub trait FixedOutputReset: FixedOutput + Reset {
    /// Write result into provided array and reset the hasher state.
    fn finalize_into_reset(&mut self, out: &mut Output<Self>);

    /// Retrieve result and reset the hasher state.
    #[inline]
    fn finalize_fixed_reset(&mut self) -> Output<Self> {
        let mut out = Default::default();
        self.finalize_into_reset(&mut out);
        out
    }
}

/// An invalid output size for a hash core.
#[derive(Clone, Copy, Debug, Default)]
pub struct InvalidOutputSize;

impl fmt::Display for InvalidOutputSize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("invalid output size")
    }
}

impl core::error::Error for InvalidOutputSize {}
