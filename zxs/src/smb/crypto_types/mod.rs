//! Block, output and key sizes shared by the SMB ciphers and digests.
//!
//! Adapted by ZXS from `RustCrypto`; see ORIGIN.md and the retained licenses.
#![forbid(unsafe_code)]

pub mod state;

pub use hybrid_array as array;
pub use hybrid_array::typenum;
use core::fmt;
use hybrid_array::{Array, ArraySize};

pub type Block<B> = Array<u8, <B as BlockSizeUser>::BlockSize>;
pub type ParBlocks<T> = Array<Block<T>, <T as ParBlocksSizeUser>::ParBlocksSize>;
pub type Output<T> = Array<u8, OutputSize<T>>;
pub type OutputSize<T> = <T as OutputSizeUser>::OutputSize;
pub type Key<B> = Array<u8, <B as KeySizeUser>::KeySize>;

pub trait BlockSizeUser {
    type BlockSize: ArraySize;
}

pub trait ParBlocksSizeUser: BlockSizeUser {
    type ParBlocksSize: ArraySize;
}

pub trait OutputSizeUser {
    type OutputSize: ArraySize;
}

pub trait KeySizeUser {
    type KeySize: ArraySize;
}

pub trait InnerUser {
    type Inner;
}

pub trait Reset {
    fn reset(&mut self);
}

pub trait AlgorithmName {
    fn write_alg_name(f: &mut fmt::Formatter<'_>) -> fmt::Result;
}

pub trait KeyInit: KeySizeUser + Sized {
    fn new(key: &Key<Self>) -> Self;

    /// Borrow the key as the required fixed size rather than copying it.
    ///
    /// # Errors
    /// Returns `InvalidLength` if the slice does not have the required size.
    #[inline]
    fn new_from_slice(key: &[u8]) -> Result<Self, InvalidLength> {
        <&Key<Self>>::try_from(key)
            .map(Self::new)
            .map_err(|_| InvalidLength)
    }
}

pub trait InnerInit: InnerUser + Sized {
    fn inner_init(inner: Self::Inner) -> Self;
}

impl<T> KeySizeUser for T
where
    T: InnerUser,
    T::Inner: KeySizeUser,
{
    type KeySize = <T::Inner as KeySizeUser>::KeySize;
}

impl<T> KeyInit for T
where
    T: InnerInit,
    T::Inner: KeyInit,
{
    #[inline]
    fn new(key: &Key<Self>) -> Self {
        Self::inner_init(T::Inner::new(key))
    }

    #[inline]
    fn new_from_slice(key: &[u8]) -> Result<Self, InvalidLength> {
        T::Inner::new_from_slice(key)
            .map_err(|_| InvalidLength)
            .map(Self::inner_init)
    }
}

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub struct InvalidLength;

impl fmt::Display for InvalidLength {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        f.write_str("Invalid Length")
    }
}

impl core::error::Error for InvalidLength {}
