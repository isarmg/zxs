// Adapted from Paho Lurie-Gregg and contributors (MIT OR Apache-2.0); see ORIGIN.md.
//! Type-level unsigned lengths used by SMB's fixed storage.
#![forbid(unsafe_code)]

mod bits;
mod canonical;
mod division;
mod ordering;
mod unsigned;
pub mod consts;

pub use consts::*;
pub use ordering::{IsLess, IsLessOrEqual};

use core::ops::{Add, Div, Shl, Sub};
use bits::{B0, B1, Bit};
use ordering::{Cmp, Equal, Greater, Less};
use unsigned::{UInt, UTerm};

/// A supported binary unsigned integer.
pub trait Unsigned: sealed::Sealed + Copy + Default + 'static {
    const U64: u64;
    const USIZE: usize;
}

/// Marker for a nonzero unsigned integer.
pub trait NonZero: sealed::Sealed {}

pub type Sum<A, B> = <A as Add<B>>::Output;
pub type Diff<A, B> = <A as Sub<B>>::Output;
pub type Quot<A, B> = <A as Div<B>>::Output;

type Add1<A> = <A as Add<B1>>::Output;
type Sub1<A> = <A as Sub<B1>>::Output;
type Shleft<A, B> = <A as Shl<B>>::Output;
type Compare<A, B> = <A as Cmp<B>>::Output;

mod sealed {
    use super::{B0, B1, Bit, Equal, Greater, Less, UInt, UTerm, Unsigned};
    pub trait Sealed {}

    impl Sealed for B0 {}
    impl Sealed for B1 {}
    impl Sealed for UTerm {}
    impl<U: Unsigned, B: Bit> Sealed for UInt<U, B> {}
    impl Sealed for Less {}
    impl Sealed for Equal {}
    impl Sealed for Greater {}
}

#[cfg(test)]
#[path = "../../../tests/smb/type_lengths.rs"]
mod tests;
