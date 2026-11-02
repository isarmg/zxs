use std::marker::PhantomData;
use std::ptr::NonNull;

/// An erased mutable reference to a value `&'a mut T`
///
/// Example:
/// ```rust
/// use erased::ErasedMut;
///
/// let value = &mut 5usize;
/// let mut erased = ErasedMut::new(value);
///
/// // Safety: Matches the type of `value` exactly, which was used to create the `erased` value
/// let r2 = unsafe { erased.get::<usize>() };
/// *r2 = 42;
///
/// // Safety: Matches the type of `value` exactly, which was used to create the `erased` value
/// assert_eq!(*unsafe { erased.get_ref::<usize>() }, 42);
/// ```
#[derive(Debug)]
pub struct ErasedMut<'a> {
    ptr: NonNull<()>,
    phantom: PhantomData<&'a ()>,
}

impl<'a> ErasedMut<'a> {
    /// Create a new erased mutable reference from a mutable reference to `T`
    pub fn new<T>(t: &'a mut T) -> ErasedMut<'a> {
        Self {
            ptr: NonNull::from(t).cast(),
            phantom: PhantomData,
        }
    }

    /// Get a mutable reference to `T` back from the erased mutable reference.
    ///
    /// The returned reference borrows this wrapper exclusively, so another
    /// reference cannot be obtained from it while that borrow remains in use.
    ///
    /// ```compile_fail
    /// use erased::ErasedMut;
    ///
    /// let mut value = 5usize;
    /// let mut erased = ErasedMut::new(&mut value);
    /// // SAFETY: Both retrievals use the exact type passed to `new`.
    /// let first = unsafe { erased.get::<usize>() };
    /// let second = unsafe { erased.get::<usize>() };
    /// *first += *second;
    /// ```
    ///
    /// # Safety
    /// The generic argument `T` of this function must match the `T` that was used to create this erased reference in `ErasedMut::new` exactly.
    /// Pay specific attention that any lifetime parameters of `T` match.
    ///
    /// It is **strongly recommended** to provide `T` explicitly, even if it can be inferred. This is to make sure that the value of `T` is not accidentally changed.
    pub unsafe fn get<T>(&mut self) -> &mut T {
        // Safety: From the safety comment the `T` matches the `T` this erased mutable reference was created with.
        // The mutable reference is still valid since its lifetime `'a` is still alive, and the `&mut self` provides exclusive access.
        self.ptr.cast::<T>().as_mut()
    }

    /// Get a reference to `T` back from the erased mutable reference.
    ///
    /// The returned reference borrows this wrapper, preventing a mutable
    /// retrieval while the shared reference remains in use.
    ///
    /// ```compile_fail
    /// use erased::ErasedMut;
    ///
    /// let mut value = 5usize;
    /// let mut erased = ErasedMut::new(&mut value);
    /// // SAFETY: Both retrievals use the exact type passed to `new`.
    /// let shared = unsafe { erased.get_ref::<usize>() };
    /// *unsafe { erased.get::<usize>() } = 42;
    /// assert_eq!(*shared, 42);
    /// ```
    ///
    /// # Safety
    /// The generic argument `T` of this function must match the `T` that was used to create this erased reference in `ErasedMut::new` exactly.
    /// Pay specific attention that any lifetime parameters of `T` match.
    ///
    /// It is **strongly recommended** to provide `T` explicitly, even if it can be inferred. This is to make sure that the value of `T` is not accidentally changed.
    pub unsafe fn get_ref<T>(&self) -> &T {
        // Safety: From the safety comment the `T` matches the `T` this reference was created with. The reference is still valid since its lifetime `'a` is still alive.
        self.ptr.cast::<T>().as_ref()
    }
}

impl<'a, T> From<&'a mut T> for ErasedMut<'a> {
    fn from(value: &'a mut T) -> Self {
        Self::new(value)
    }
}

#[cfg(test)]
include!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../../../test/foundation/erased/unit/src/erased_mut_ref/tests_67.rs"));
