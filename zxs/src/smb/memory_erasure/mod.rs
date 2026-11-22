// Adapted from `RustCrypto` (MIT OR Apache-2.0); see ORIGIN.md.
//! In-place erasure for SMB secrets, buffered state and schedules.

mod barrier;
mod bulk;

use barrier::optimization_barrier;
use bulk::zeroize_byte_ptr;
use core::{mem::{MaybeUninit, size_of}, ops, ptr, slice::IterMut};

/// Trait for securely erasing values from memory.
pub trait Zeroize {
    /// Zero out this object from memory using Rust intrinsics which ensure the
    /// zeroization operation is not "optimized away" by the compiler.
    fn zeroize(&mut self);
}

/// Marker trait signifying that this type will [`Zeroize::zeroize`] itself on [`Drop`].
pub trait ZeroizeOnDrop {}

/// Marker trait for types whose [`Default`] is the desired zeroization result
pub trait DefaultIsZeroes: Copy + Default + Sized {}

impl<Z> Zeroize for Z
where
    Z: DefaultIsZeroes,
{
    fn zeroize(&mut self) {
        volatile_write(self, Z::default());
    }
}

macro_rules! impl_zeroize_with_default {
    ($($type:ty),+) => {
        $(impl DefaultIsZeroes for $type {})+
    };
}

impl_zeroize_with_default!((), u8, u32, u64, u128);

/// `()` is zero sized so provide a `ZeroizeOnDrop` implementation.
impl ZeroizeOnDrop for () {}

/// Impl [`Zeroize`] on arrays of types that impl [`Zeroize`].
impl<Z, const N: usize> Zeroize for [Z; N]
where
    Z: Zeroize,
{
    fn zeroize(&mut self) {
        self.iter_mut().zeroize();
    }
}

/// Impl [`ZeroizeOnDrop`] on arrays of types that impl [`ZeroizeOnDrop`].
impl<Z, const N: usize> ZeroizeOnDrop for [Z; N] where Z: ZeroizeOnDrop {}

impl<Z> Zeroize for IterMut<'_, Z>
where
    Z: Zeroize,
{
    fn zeroize(&mut self) {
        for elem in self {
            elem.zeroize();
        }
    }
}

/// Impl [`Zeroize`] on [`MaybeUninit`] types.
///
/// The underlying type's validity requirements do not apply to a
/// [`MaybeUninit`] slot.
impl<Z> Zeroize for MaybeUninit<Z> {
    fn zeroize(&mut self) {
        // SAFETY: `MaybeUninit` is valid for any byte pattern, including zeros.
        unsafe { ptr::write_volatile(self, MaybeUninit::zeroed()) }
    }
}

/// Erase each element to its type's zero value.
impl<Z> Zeroize for [Z]
where
    Z: DefaultIsZeroes,
{
    fn zeroize(&mut self) {
        bulk::fill_slice(self, Z::default());
    }
}

/// `Zeroizing` is a wrapper for any `Z: Zeroize` type which implements a
/// `Drop` handler which zeroizes dropped values.
///
/// `Zeroizing<T>` is defined with `repr(transparent)`, which means it is
/// guaranteed to have the same physical representation as the underlying type.
#[derive(Default, Eq, PartialEq)]
#[repr(transparent)]
pub struct Zeroizing<Z: Zeroize + ?Sized>(Z);

impl<Z: Zeroize + ?Sized> core::fmt::Debug for Zeroizing<Z> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Zeroizing").finish_non_exhaustive()
    }
}

impl<Z> Zeroizing<Z>
where
    Z: Zeroize,
{
    /// Move value inside a `Zeroizing` wrapper which ensures it will be
    /// zeroized when it's dropped.
    #[inline(always)]
    pub fn new(value: Z) -> Self {
        Self(value)
    }
}

impl<Z: Zeroize + Clone> Clone for Zeroizing<Z> {
    #[inline(always)]
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }

    #[inline(always)]
    fn clone_from(&mut self, source: &Self) {
        self.0.zeroize();
        self.0.clone_from(&source.0);
    }
}

impl<Z> ops::Deref for Zeroizing<Z>
where
    Z: Zeroize + ?Sized,
{
    type Target = Z;

    #[inline(always)]
    fn deref(&self) -> &Z {
        &self.0
    }
}

impl<Z> ops::DerefMut for Zeroizing<Z>
where
    Z: Zeroize + ?Sized,
{
    #[inline(always)]
    fn deref_mut(&mut self) -> &mut Z {
        &mut self.0
    }
}

impl<Z> Zeroize for Zeroizing<Z>
where
    Z: Zeroize + ?Sized,
{
    fn zeroize(&mut self) {
        self.0.zeroize();
    }
}

impl<Z> ZeroizeOnDrop for Zeroizing<Z> where Z: Zeroize + ?Sized {}

impl<Z> Drop for Zeroizing<Z>
where
    Z: Zeroize + ?Sized,
{
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// Erase the entire flat object, including padding, through the byte backend.
///
/// # Safety
///
/// For a nonzero size, `data` must provide exclusive access to the whole object
/// within one allocation. The object must contain no references, dynamically
/// allocated storage or fields that implement `Drop`; zero bytes must be valid
/// for it. Only use this for values that are no longer needed.
#[inline(always)]
pub unsafe fn zeroize_flat_type<F: Sized>(data: *mut F) {
    // `zeroize_byte_ptr` requires a non-NULL pointer, but we allow passing NULL pointers for ZSTs
    if size_of::<F>() == 0 {
        return;
    }

    // SAFETY: the caller provides exclusive access to the object's bytes and
    // guarantees that zeroes are valid. The backend permits uninitialized padding.
    unsafe { zeroize_byte_ptr(data.cast::<u8>(), size_of::<F>()) }
}

/// Perform a volatile write to the destination
#[inline(always)]
fn volatile_write<T: Copy + Sized>(dst: &mut T, src: T) {
    // SAFETY: the mutable reference supplies an aligned, writable destination.
    unsafe { ptr::write_volatile(dst, src) }
}

#[cfg(test)]
#[path = "../../../tests/smb/memory_erasure.rs"]
mod tests;
