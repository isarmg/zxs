//! Read and write views for in-place SMB block and byte transforms.
//!
//! Adapted by ZXS from `RustCrypto` (MIT OR Apache-2.0); see ORIGIN.md and the licenses.
//! Constructors borrow one initialized destination; only disjoint subviews are split out.

use core::{marker::PhantomData, ptr};
use crate::smb::fixed_array::{Array, ArraySize};

pub(in crate::smb) struct InOut<'inp, 'out, T> {
    in_ptr: *const T,
    out_ptr: *mut T,
    _pd: PhantomData<(&'inp T, &'out mut T)>,
}

impl<T> InOut<'_, '_, T> {
    #[inline(always)]
    #[must_use]
    pub(super) fn get_in(&self) -> &T {
        // SAFETY: the input stays initialized and borrowing this handle excludes a write.
        unsafe { &*self.in_ptr }
    }

    #[inline(always)]
    pub(super) fn get_out(&mut self) -> &mut T {
        // SAFETY: only this handle controls this output slot, and it is borrowed exclusively.
        unsafe { &mut *self.out_ptr }
    }
}

impl<T: Clone> InOut<'_, '_, T> {
    #[inline(always)]
    #[must_use]
    pub(super) fn clone_in(&self) -> T {
        // SAFETY: the input stays initialized and cannot be written during this borrow.
        unsafe { (*self.in_ptr).clone() }
    }
}

impl<'a, T> From<&'a mut T> for InOut<'a, 'a, T> {
    #[inline(always)]
    fn from(val: &'a mut T) -> Self {
        let p = ptr::from_mut(val);
        Self {
            in_ptr: p,
            out_ptr: p,
            _pd: PhantomData,
        }
    }
}

impl<T, N: ArraySize> InOut<'_, '_, Array<T, N>> {
    #[inline(always)]
    pub(super) fn get(&mut self, pos: usize) -> InOut<'_, '_, T> {
        assert!(pos < N::USIZE);
        // SAFETY: Array<T,N> is an aligned [T;N]; this in-bounds slot
        // and its two views stay borrowed through the exclusive parent handle.
        unsafe {
            InOut {
                in_ptr: self.in_ptr.cast::<T>().add(pos),
                out_ptr: self.out_ptr.cast::<T>().add(pos),
                _pd: PhantomData,
            }
        }
    }
}

pub(in crate::smb) struct InOutBuf<'inp, 'out, T> {
    in_ptr: *const T,
    out_ptr: *mut T,
    len: usize,
    _pd: PhantomData<(&'inp T, &'out mut T)>,
}

impl<'a, T> From<&'a mut [T]> for InOutBuf<'a, 'a, T> {
    #[inline(always)]
    fn from(buf: &'a mut [T]) -> Self {
        let p = buf.as_mut_ptr();
        Self {
            in_ptr: p,
            out_ptr: p,
            len: buf.len(),
            _pd: PhantomData,
        }
    }
}

impl<'inp, 'out, T> InOutBuf<'inp, 'out, T> {
    #[inline(always)]
    #[must_use]
    pub(super) fn len(&self) -> usize {
        self.len
    }

    #[inline(always)]
    #[must_use]
    pub(super) fn into_chunks<N: ArraySize>(
        self,
    ) -> (InOutBuf<'inp, 'out, Array<T, N>>, InOutBuf<'inp, 'out, T>) {
        let chunks = self.len() / N::USIZE;
        let tail_pos = N::USIZE * chunks;
        let tail_len = self.len() - tail_pos;
        // SAFETY: Array<T,N> has the layout of [T;N]. The prefix covers
        // complete arrays and the equally borrowed suffix is disjoint.
        // With no tail the pointer is one-past-end and never dereferenced.
        unsafe {
            let chunks = InOutBuf {
                in_ptr: self.in_ptr.cast::<Array<T, N>>(),
                out_ptr: self.out_ptr.cast::<Array<T, N>>(),
                len: chunks,
                _pd: PhantomData,
            };
            let tail = InOutBuf {
                in_ptr: self.in_ptr.add(tail_pos),
                out_ptr: self.out_ptr.add(tail_pos),
                len: tail_len,
                _pd: PhantomData,
            };
            (chunks, tail)
        }
    }
}

impl<'inp, 'out, T> IntoIterator for InOutBuf<'inp, 'out, T> {
    type Item = InOut<'inp, 'out, T>;
    type IntoIter = InOutBufIter<'inp, 'out, T>;

    #[inline(always)]
    fn into_iter(self) -> Self::IntoIter {
        InOutBufIter { buf: self, pos: 0 }
    }
}

pub(in crate::smb) struct InOutBufIter<'inp, 'out, T> {
    buf: InOutBuf<'inp, 'out, T>,
    pos: usize,
}

impl<'inp, 'out, T> Iterator for InOutBufIter<'inp, 'out, T> {
    type Item = InOut<'inp, 'out, T>;

    #[inline(always)]
    fn next(&mut self) -> Option<Self::Item> {
        if self.buf.len() == self.pos {
            return None;
        }
        // SAFETY: pos starts at zero and advances once after each result;
        // the current slot is in bounds and not borrowed by an earlier result.
        let res = unsafe {
            InOut {
                in_ptr: self.buf.in_ptr.add(self.pos),
                out_ptr: self.buf.out_ptr.add(self.pos),
                _pd: PhantomData,
            }
        };
        self.pos += 1;
        Some(res)
    }
}

#[cfg(test)]
#[path = "../../../../tests/smb/cipher_buffers.rs"]
mod tests;
