// Adapted from `RustCrypto` (MIT OR Apache-2.0); see ORIGIN.md.
//! Fixed layout, borrowed views and initialization for SMB cipher and digest arrays.

mod initialize;
mod iteration;
mod shape;
mod sizes;

pub use shape::{ArraySize, AssocArraySize};
pub use typenum;

use core::{
    array::TryFromSliceError,
    fmt::{self, Debug},
    mem::{self, ManuallyDrop, MaybeUninit},
    ops::{Add, Deref, DerefMut, Index, IndexMut, Sub},
    ptr,
    slice::{self, Iter, IterMut},
};
use typenum::{Diff, Sum};
use zeroize::{Zeroize, ZeroizeOnDrop};

/// A transparent wrapper around the `[T; N]` selected by `U`.
#[repr(transparent)]
pub struct Array<T, U: ArraySize>(pub U::ArrayType<T>);

type SplitResult<T, U, N> = (Array<T, N>, Array<T, Diff<U, N>>);
type SplitRefResult<'a, T, U, N> = (&'a Array<T, N>, &'a Array<T, Diff<U, N>>);

impl<T, U> Array<T, U>
where
    U: ArraySize,
{
    /// Returns a slice containing the entire array. Equivalent to `&s[..]`.
    #[inline]
    pub const fn as_slice(&self) -> &[T] {
        // SAFETY: `Array<T, U>` is a transparent wrapper over `[T; U::USIZE]`.
        unsafe { slice::from_raw_parts(self.as_ptr(), U::USIZE) }
    }

    /// Returns a mutable slice containing the entire array. Equivalent to `&mut s[..]`.
    #[inline]
    pub const fn as_mut_slice(&mut self) -> &mut [T] {
        // SAFETY: `Array<T, U>` is a transparent wrapper over `[T; U::USIZE]`.
        unsafe { slice::from_raw_parts_mut(self.as_mut_ptr(), U::USIZE) }
    }

    /// Returns a pointer to the start of the array.
    pub const fn as_ptr(&self) -> *const T {
        ptr::from_ref::<Self>(self).cast::<T>()
    }

    /// Returns a mutable pointer to the start of the array.
    pub const fn as_mut_ptr(&mut self) -> *mut T {
        ptr::from_mut::<Self>(self).cast::<T>()
    }

    /// Returns an iterator over the array.
    #[inline]
    pub fn iter(&self) -> Iter<'_, T> {
        self.as_slice().iter()
    }

    /// Returns an iterator that allows modifying each value.
    #[inline]
    pub fn iter_mut(&mut self) -> IterMut<'_, T> {
        self.0.as_mut().iter_mut()
    }

    /// Concatenates `self` with `other`.
    #[inline]
    pub fn concat<N>(self, other: Array<T, N>) -> Array<T, Sum<U, N>>
    where
        N: ArraySize,
        U: Add<N>,
        Sum<U, N>: ArraySize,
    {
        let mut c = Array::uninit();
        let (left, right) = c.split_at_mut(self.len());
        for (val, dst) in self.into_iter().zip(left) {
            dst.write(val);
        }
        for (val, dst) in other.into_iter().zip(right) {
            dst.write(val);
        }
        // SAFETY: We wrote to every element of `c`.
        unsafe { c.assume_init() }
    }

    /// Splits `self` at index `N` in two arrays.
    ///
    /// New arrays hold the original memory from `self`.
    #[inline]
    pub fn split<N>(self) -> SplitResult<T, U, N>
    where
        U: Sub<N>,
        N: ArraySize,
        Diff<U, N>: ArraySize,
    {
        // SAFETY: the second size is `U - N`; each source item is moved once.
        unsafe {
            let array = ManuallyDrop::new(self);
            let head = ptr::read(array.as_ptr().cast());
            let tail = ptr::read(array.as_ptr().add(N::USIZE).cast());
            (head, tail)
        }
    }

    /// Splits `&self` at index `N` in two array references.
    #[inline]
    pub fn split_ref<N>(&self) -> SplitRefResult<'_, T, U, N>
    where
        U: Sub<N>,
        N: ArraySize,
        Diff<U, N>: ArraySize,
    {
        // SAFETY: the typed prefix and suffix fit, and both borrows come from `self`.
        unsafe {
            let array_ptr = self.as_ptr();
            let head = &*array_ptr.cast();
            let tail = &*array_ptr.add(N::USIZE).cast();
            (head, tail)
        }
    }

    /// Splits the shared slice into a slice of `U`-element arrays, starting at the beginning
    /// of the slice, and a remainder slice with length strictly less than `U`.
    ///
    /// # Panics
    /// Panics if `U` is 0.
    #[inline]
    pub const fn slice_as_chunks(buf: &[T]) -> (&[Self], &[T]) {
        assert!(U::USIZE != 0, "chunk size must be non-zero");
        // `tail_pos` is at most `buf.len()`; the prefix and suffix stay in the same slice.
        let chunks_len = buf.len() / U::USIZE;
        let tail_pos = U::USIZE * chunks_len;
        let tail_len = buf.len() - tail_pos;
        unsafe {
            let ptr = buf.as_ptr();
            let chunks = slice::from_raw_parts(ptr.cast(), chunks_len);
            let tail = slice::from_raw_parts(ptr.add(tail_pos), tail_len);
            (chunks, tail)
        }
    }
}

// Impls which depend on the inner array type being `[T; N]`.
impl<T, U, const N: usize> Array<T, U>
where
    U: ArraySize<ArrayType<T> = [T; N]>,
{
    /// Cast a reference to a core array to an [`Array`] reference.
    #[inline]
    pub const fn cast_from_core(array_ref: &[T; N]) -> &Self {
        // SAFETY: `Self` is a `repr(transparent)` newtype for `[T; N]`
        unsafe { &*array_ref.as_ptr().cast() }
    }

    /// Cast a mutable reference to a core array to an [`Array`] reference.
    #[inline]
    pub const fn cast_from_core_mut(array_ref: &mut [T; N]) -> &mut Self {
        // SAFETY: `Self` is a `repr(transparent)` newtype for `[T; N]`
        unsafe { &mut *array_ref.as_mut_ptr().cast() }
    }

    /// Borrow the underlying core arrays without copying.
    #[inline]
    pub const fn cast_slice_to_core(slice: &[Self]) -> &[[T; N]] {
        // SAFETY: `Self` is a `repr(transparent)` newtype for `[T; N]`
        unsafe { slice::from_raw_parts(slice.as_ptr().cast(), slice.len()) }
    }
}

impl<T, U> Array<MaybeUninit<T>, U>
where
    U: ArraySize,
{
    /// Create an uninitialized array of [`MaybeUninit`]s for the given type.
    #[must_use]
    pub fn uninit() -> Array<MaybeUninit<T>, U> {
        Self(U::uninit_array())
    }

    /// Extract the values from an array of `MaybeUninit` containers.
    ///
    /// # Safety
    ///
    /// It is up to the caller to guarantee that all elements of the array are in an initialized
    /// state.
    #[inline]
    pub unsafe fn assume_init(self) -> Array<T, U> {
        unsafe {
            // `Array` is a `repr(transparent)` newtype for a generic inner type which is constrained to
            // be `[T; N]` by the `ArraySize` implementations.
            //
            // Since we're working with a type-erased inner type and ultimately trying to convert
            // `[MaybeUninit<T>; N]` to `[T; N]`, we can't use simpler approaches like a pointer cast
            // or `transmute`, since the compiler can't prove to itself that the size will be the same.
            //
            // We've taken unique ownership of `self`, which is a `MaybeUninit` array, and as such we
            // don't need to worry about `Drop` impls because `MaybeUninit` does not impl `Drop`.
            // Since we have unique ownership of `self`, it's okay to make a copy because we're throwing
            // the original away (and this should all get optimized to a noop by the compiler, anyway).
            mem::transmute_copy(&self)
        }
    }
}

impl<T, U> AsRef<Array<T, U>> for Array<T, U>
where
    U: ArraySize,
{
    #[inline]
    fn as_ref(&self) -> &Self {
        self
    }
}

impl<T, U> AsRef<[T]> for Array<T, U>
where
    U: ArraySize,
{
    #[inline]
    fn as_ref(&self) -> &[T] {
        self.0.as_ref()
    }
}

impl<T, U, const N: usize> AsRef<[T; N]> for Array<T, U>
where
    U: ArraySize<ArrayType<T> = [T; N]>,
{
    #[inline]
    fn as_ref(&self) -> &[T; N] {
        &self.0
    }
}

impl<T, U> AsMut<Array<T, U>> for Array<T, U>
where
    U: ArraySize,
{
    #[inline]
    fn as_mut(&mut self) -> &mut Self {
        self
    }
}

impl<T, U> AsMut<[T]> for Array<T, U>
where
    U: ArraySize,
{
    #[inline]
    fn as_mut(&mut self) -> &mut [T] {
        self.0.as_mut()
    }
}

impl<T, U, const N: usize> AsMut<[T; N]> for Array<T, U>
where
    U: ArraySize<ArrayType<T> = [T; N]>,
{
    #[inline]
    fn as_mut(&mut self) -> &mut [T; N] {
        &mut self.0
    }
}

impl<T, U> Clone for Array<T, U>
where
    T: Clone,
    U: ArraySize,
{
    #[inline]
    fn clone(&self) -> Self {
        Self::from_fn(|n| self.0.as_ref()[n].clone())
    }
}

impl<T, U> Copy for Array<T, U>
where
    T: Copy,
    U: ArraySize,
    U::ArrayType<T>: Copy,
{
}

impl<T, U> Debug for Array<T, U>
where
    T: Debug,
    U: ArraySize,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Array").field(&self.0.as_ref()).finish()
    }
}

impl<T, U> Default for Array<T, U>
where
    T: Default,
    U: ArraySize,
{
    #[inline]
    fn default() -> Self {
        Self::from_fn(|_| Default::default())
    }
}

impl<T, U> Deref for Array<T, U>
where
    U: ArraySize,
{
    type Target = [T];

    #[inline]
    fn deref(&self) -> &[T] {
        self.0.as_ref()
    }
}

impl<T, U> DerefMut for Array<T, U>
where
    U: ArraySize,
{
    #[inline]
    fn deref_mut(&mut self) -> &mut [T] {
        self.0.as_mut()
    }
}

impl<T, U> Eq for Array<T, U>
where
    T: Eq,
    U: ArraySize,
{
}

impl<T, U, const N: usize> From<[T; N]> for Array<T, U>
where
    U: ArraySize<ArrayType<T> = [T; N]>,
{
    #[inline]
    fn from(arr: [T; N]) -> Array<T, U> {
        Array(arr)
    }
}

impl<T, U, const N: usize> From<Array<T, U>> for [T; N]
where
    U: ArraySize<ArrayType<T> = [T; N]>,
{
    #[inline]
    fn from(arr: Array<T, U>) -> [T; N] {
        arr.0
    }
}

impl<'a, T, U, const N: usize> From<&'a [T; N]> for &'a Array<T, U>
where
    U: ArraySize<ArrayType<T> = [T; N]>,
{
    #[inline]
    fn from(array_ref: &'a [T; N]) -> &'a Array<T, U> {
        Array::cast_from_core(array_ref)
    }
}

impl<'a, T, U, const N: usize> From<&'a Array<T, U>> for &'a [T; N]
where
    U: ArraySize<ArrayType<T> = [T; N]>,
{
    #[inline]
    fn from(array_ref: &'a Array<T, U>) -> &'a [T; N] {
        array_ref.as_ref()
    }
}

impl<'a, T, U, const N: usize> From<&'a mut [T; N]> for &'a mut Array<T, U>
where
    U: ArraySize<ArrayType<T> = [T; N]>,
{
    #[inline]
    fn from(array_ref: &'a mut [T; N]) -> &'a mut Array<T, U> {
        Array::cast_from_core_mut(array_ref)
    }
}

impl<'a, T, U, const N: usize> From<&'a mut Array<T, U>> for &'a mut [T; N]
where
    U: ArraySize<ArrayType<T> = [T; N]>,
{
    #[inline]
    fn from(array_ref: &'a mut Array<T, U>) -> &'a mut [T; N] {
        array_ref.as_mut()
    }
}

impl<T, I, U> Index<I> for Array<T, U>
where
    [T]: Index<I>,
    U: ArraySize,
{
    type Output = <[T] as Index<I>>::Output;

    #[inline]
    fn index(&self, index: I) -> &Self::Output {
        Index::index(self.as_slice(), index)
    }
}

impl<T, I, U> IndexMut<I> for Array<T, U>
where
    [T]: IndexMut<I>,
    U: ArraySize,
{
    #[inline]
    fn index_mut(&mut self, index: I) -> &mut Self::Output {
        IndexMut::index_mut(self.as_mut_slice(), index)
    }
}

impl<T, U> PartialEq for Array<T, U>
where
    T: PartialEq,
    U: ArraySize,
{
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.0.as_ref().eq(other.0.as_ref())
    }
}

impl<T, U, const N: usize> PartialEq<[T; N]> for Array<T, U>
where
    T: PartialEq,
    U: ArraySize<ArrayType<T> = [T; N]>,
{
    #[inline]
    fn eq(&self, other: &[T; N]) -> bool {
        self.0.eq(other)
    }
}

impl<T, U, const N: usize> PartialEq<Array<T, U>> for [T; N]
where
    T: PartialEq,
    U: ArraySize<ArrayType<T> = [T; N]>,
{
    #[inline]
    fn eq(&self, other: &Array<T, U>) -> bool {
        self.eq(&other.0)
    }
}

// SAFETY: a transparent array may be sent when its elements may be sent.
unsafe impl<T, U: ArraySize> Send for Array<T, U> where T: Send {}

// SAFETY: a transparent array may be shared when its elements may be shared.
unsafe impl<T, U: ArraySize> Sync for Array<T, U> where T: Sync {}

impl<'a, T, U> TryFrom<&'a [T]> for &'a Array<T, U>
where
    U: ArraySize,
{
    type Error = TryFromSliceError;

    #[inline]
    fn try_from(slice: &'a [T]) -> Result<Self, TryFromSliceError> {
        check_slice_length::<T, U>(slice)?;

        // SAFETY: `Array<T, U>` is a `repr(transparent)` newtype for a core
        // array with length checked above.
        Ok(unsafe { &*slice.as_ptr().cast() })
    }
}

impl<'a, T, U> TryFrom<&'a mut [T]> for &'a mut Array<T, U>
where
    U: ArraySize,
{
    type Error = TryFromSliceError;

    #[inline]
    fn try_from(slice: &'a mut [T]) -> Result<Self, TryFromSliceError> {
        check_slice_length::<T, U>(slice)?;

        // SAFETY: `Array<T, U>` is a `repr(transparent)` newtype for a core
        // array with length checked above.
        Ok(unsafe { &mut *slice.as_mut_ptr().cast() })
    }
}

impl<'a, T, U> TryFrom<&'a [T]> for Array<T, U>
where
    Self: Clone,
    U: ArraySize,
{
    type Error = TryFromSliceError;

    #[inline]
    fn try_from(slice: &'a [T]) -> Result<Array<T, U>, TryFromSliceError> {
        <&'a Self>::try_from(slice).cloned()
    }
}

impl<T, U> Zeroize for Array<T, U>
where
    T: Zeroize,
    U: ArraySize,
{
    #[inline]
    fn zeroize(&mut self) {
        self.0.as_mut().iter_mut().zeroize();
    }
}

impl<T, U> ZeroizeOnDrop for Array<T, U>
where
    T: ZeroizeOnDrop,
    U: ArraySize,
{
}

/// Generate a [`TryFromSliceError`] if the slice doesn't match the given length.
fn check_slice_length<T, U: ArraySize>(slice: &[T]) -> Result<(), TryFromSliceError> {
    debug_assert_eq!(Array::<(), U>::default().len(), U::USIZE);

    if slice.len() == U::USIZE {
        Ok(())
    } else {
        // Hack: `TryFromSliceError` lacks a public constructor, so this fakes one
        <&[T; 1]>::try_from([].as_slice()).map(|_| ())
    }
}

#[cfg(test)]
#[path = "../../../tests/smb/fixed_array.rs"]
mod tests;
