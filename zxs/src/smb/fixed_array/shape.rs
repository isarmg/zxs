// Adapted from `RustCrypto` (MIT OR Apache-2.0); see ORIGIN.md.
//! Size and array representation bounds.

use super::Array;
use core::{
    borrow::{Borrow, BorrowMut},
    fmt::Debug,
    mem::MaybeUninit,
    ops::{Index, IndexMut, Range},
};
use typenum::Unsigned;

/// Trait which associates a [`usize`] size and `ArrayType` with a
/// `typenum`-provided [`Unsigned`] integer.
///
/// # Safety
///
/// `ArrayType` MUST be an array with a number of elements exactly equal to
/// [`Unsigned::USIZE`]. Breaking this requirement will cause undefined behavior.
pub unsafe trait ArraySize: Unsigned + Debug {
    /// Array type which corresponds to this size.
    ///
    /// This is always defined to be `[T; N]` where `N` is the same as
    /// [`ArraySize::USIZE`][`typenum::Unsigned::USIZE`].
    type ArrayType<T>: AssocArraySize<Size = Self>
        + AsRef<[T]>
        + AsMut<[T]>
        + Borrow<[T]>
        + BorrowMut<[T]>
        + From<Array<T, Self>>
        + Index<usize>
        + Index<Range<usize>>
        + IndexMut<usize>
        + IndexMut<Range<usize>>
        + Into<Array<T, Self>>
        + IntoIterator<Item = T>;

    /// Construct the exact array of uninitialized element slots.
    fn uninit_array<T>() -> Self::ArrayType<MaybeUninit<T>>;
}

/// Associates an array with its exact type-level length.
pub trait AssocArraySize: Sized {
    /// Size of an array type, expressed as a [`typenum`]-based [`ArraySize`].
    type Size: ArraySize;
}

impl<T, U> AssocArraySize for Array<T, U>
where
    U: ArraySize,
{
    type Size = U;
}
