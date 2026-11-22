// Adapted from `RustCrypto` (MIT OR Apache-2.0); see ORIGIN.md.
//! Owning and borrowed array iterators.

use super::{Array, ArraySize};
use core::slice::{Iter, IterMut};

impl<T, U> IntoIterator for Array<T, U>
where
    U: ArraySize,
{
    type Item = T;
    type IntoIter = <U::ArrayType<T> as IntoIterator>::IntoIter;

    /// Creates a consuming iterator, that is, one that moves each value out of the array (from
    /// start to end).
    ///
    /// The array cannot be used after calling this unless `T` implements `Copy`, so the whole
    /// array is copied.
    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a, T, U> IntoIterator for &'a Array<T, U>
where
    U: ArraySize,
{
    type Item = &'a T;
    type IntoIter = Iter<'a, T>;

    #[inline]
    fn into_iter(self) -> Iter<'a, T> {
        self.iter()
    }
}

impl<'a, T, U> IntoIterator for &'a mut Array<T, U>
where
    U: ArraySize,
{
    type Item = &'a mut T;
    type IntoIter = IterMut<'a, T>;

    #[inline]
    fn into_iter(self) -> IterMut<'a, T> {
        self.iter_mut()
    }
}
