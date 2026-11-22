//! Pending blocks for digest and MAC updates.
//!
//! Adapted by ZXS from `RustCrypto`; see ORIGIN.md and the retained licenses.

use core::{fmt, mem::MaybeUninit, ptr, slice};
use hybrid_array::{Array, ArraySize, typenum::Sum};
use zeroize::{Zeroize, ZeroizeOnDrop};

mod sealed;

/// Trait implemented for supported block sizes, i.e. for types from `U1` to `U255`.
pub trait BlockSizes: ArraySize + sealed::BlockSizes {}

impl<T: ArraySize + sealed::BlockSizes> BlockSizes for T {}

/// Trait for buffer kinds.
pub trait BufferKind: sealed::Sealed {}

/// Eager block buffer kind, which guarantees that buffer position
/// always lies in the range of `0..BlockSize`.
#[derive(Copy, Clone, Debug, Default)]
pub struct Eager {}

/// Lazy block buffer kind, which guarantees that buffer position
/// always lies in the range of `0..=BlockSize`.
#[derive(Copy, Clone, Debug, Default)]
pub struct Lazy {}

impl BufferKind for Eager {}

impl BufferKind for Lazy {}

/// Block buffer error.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub struct Error;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        f.write_str("Block buffer error")
    }
}

/// Buffer for block processing of data.
pub struct BlockBuffer<BS: BlockSizes, K: BufferKind> {
    buffer: MaybeUninit<Array<u8, BS>>,
    pos: K::Pos,
}

impl<BS: BlockSizes, K: BufferKind> Default for BlockBuffer<BS, K> {
    #[inline]
    fn default() -> Self {
        let mut buffer = MaybeUninit::uninit();
        let mut pos = Default::default();
        K::set_pos(&mut buffer, &mut pos, 0);
        Self { buffer, pos }
    }
}

impl<BS: BlockSizes, K: BufferKind> Clone for BlockBuffer<BS, K> {
    #[inline]
    fn clone(&self) -> Self {
        // SAFETY: the bytes are in MaybeUninit and the sealed cursor is () or u8.
        // The copy owns no resources and has its own zeroizing Drop.
        unsafe { ptr::read(self) }
    }
}

impl<BS: BlockSizes, K: BufferKind> BlockBuffer<BS, K> {
    /// Digest data in `input` in blocks of size `BlockSize` using
    /// the `compress` function, which accepts slice of blocks.
    #[inline]
    pub fn digest_blocks(&mut self, mut input: &[u8], mut compress: impl FnMut(&[Array<u8, BS>])) {
        let pos = self.get_pos();
        // using `self.remaining()` for some reason
        // prevents panic elimination
        let rem = self.size() - pos;
        let n = input.len();
        // Note that checking condition `pos + n < BlockSize` is
        // equivalent to checking `n < rem`, where `rem` is equal
        // to `BlockSize - pos`. Using the latter allows us to work
        // around compiler accounting for possible overflow of
        // `pos + n` which results in it inserting unreachable
        // panic branches. Using `unreachable_unchecked` in `get_pos`
        // we convince compiler that `BlockSize - pos` never underflows.
        if K::invariant(n, rem) {
            // SAFETY: input fits the free bytes and the resulting cursor
            // satisfies this kind's bound.
            unsafe {
                let buf_ptr = self.buffer.as_mut_ptr().cast::<u8>().add(pos);
                ptr::copy_nonoverlapping(input.as_ptr(), buf_ptr, input.len());
                self.set_pos_unchecked(pos + input.len());
            }
            return;
        }
        if pos != 0 {
            let (left, right) = input.split_at(rem);
            input = right;

            let g = ResetGuard(self);
            let buf = &mut g.0.buffer;
            // SAFETY: length of `left` is equal to number of remaining bytes in `buffer`,
            // so we can copy data into it and process `buffer` as fully initialized block.
            // Note that this code can temporarily break the eager buffer invariant,
            // but we reset the buffer immediately after `compress` using `Drop` impl of
            // `ResetGuard`, so this code is safe even if `compress` panics.
            let block = unsafe {
                let buf_ptr = buf.as_mut_ptr().cast::<u8>().add(pos);
                ptr::copy_nonoverlapping(left.as_ptr(), buf_ptr, left.len());
                buf.assume_init_ref()
            };
            compress(slice::from_ref(block));
        }

        let (blocks, leftover) = K::split_blocks(input);
        if !blocks.is_empty() {
            compress(blocks);
        }

        // SAFETY: the eager tail is shorter than a block; a lazy tail can
        // also be one full block, satisfying its inclusive bound.
        unsafe {
            self.set_data_unchecked(leftover);
        }
    }

    /// Reset buffer by setting cursor position to zero.
    #[inline(always)]
    pub fn reset(&mut self) {
        // SAFETY: 0 is always valid position
        unsafe {
            self.set_pos_unchecked(0);
        }
    }

    /// Pad remaining data with zeros and return resulting block.
    #[inline(always)]
    pub fn pad_with_zeros(&mut self) -> Array<u8, BS> {
        let mut res = Array::<u8, BS>::default();
        let data = self.get_data();
        res[..data.len()].copy_from_slice(data);
        self.reset();
        res
    }

    /// Return current cursor position.
    #[inline(always)]
    pub fn get_pos(&self) -> usize {
        let pos = K::get_pos(&self.buffer, &self.pos);
        if !K::invariant(pos, BS::USIZE) {
            debug_assert!(false);
            // SAFETY: `pos` never breaks the invariant
            unsafe {
                core::hint::unreachable_unchecked();
            }
        }
        pos
    }

    /// Return slice of data stored inside the buffer.
    #[inline(always)]
    pub fn get_data(&self) -> &[u8] {
        // SAFETY: the `buffer` field is properly initialized up to `self.get_pos()`.
        // `get_pos` never returns position bigger than buffer size.
        unsafe { slice::from_raw_parts(self.buffer.as_ptr().cast(), self.get_pos()) }
    }

    /// Return size of the internal buffer in bytes.
    #[inline(always)]
    pub fn size(&self) -> usize {
        BS::USIZE
    }

    /// Set buffer position.
    ///
    /// # Safety
    /// The cursor must satisfy its kind's bound. The prefix `0..pos`
    /// must be initialized before reading or exposing the updated buffer.
    #[inline(always)]
    unsafe fn set_pos_unchecked(&mut self, pos: usize) {
        debug_assert!(K::invariant(pos, BS::USIZE));
        K::set_pos(&mut self.buffer, &mut self.pos, pos);
    }

    /// Set buffer data.
    ///
    /// # Safety
    /// Length of `buf` must satisfy invariant of buffer kind, i.e. for eager hashes it must be
    /// strictly smaller than block size and for lazy hashes it must be smaller or equal
    /// to block size.
    #[inline(always)]
    unsafe fn set_data_unchecked(&mut self, buf: &[u8]) {
        // SAFETY: the caller's valid length fits the destination and the
        // prefix is initialized under the exclusive borrow before it is read.
        unsafe {
            self.set_pos_unchecked(buf.len());
            let dst_ptr: *mut u8 = self.buffer.as_mut_ptr().cast();
            ptr::copy_nonoverlapping(buf.as_ptr(), dst_ptr, buf.len());
        }
    }
}

/// Size of serialized `BlockBuffer` in bytes.
pub type SerializedBufferSize<BS, K> = Sum<BS, <K as sealed::Sealed>::Overhead>;
/// `BlockBuffer` serialized as a byte array.
pub type SerializedBuffer<BS, K> = Array<u8, SerializedBufferSize<BS, K>>;

impl<BS: BlockSizes, K: BufferKind> BlockBuffer<BS, K>
where
    BS: core::ops::Add<K::Overhead>,
    Sum<BS, K::Overhead>: ArraySize,
{
    /// Serialize the cursor, pending bytes and zero tail.
    ///
    /// # Panics
    /// A pending length beyond one byte would panic; a valid buffer cannot
    /// reach it because the block size is bounded by 255.
    pub fn serialize(&self) -> SerializedBuffer<BS, K> {
        let mut buf = SerializedBuffer::<BS, K>::default();
        let data = self.get_data();
        let (pos, block) = buf.split_at_mut(1);
        pos[0] = u8::try_from(data.len()).expect("buffer size is smaller than 256");
        block[..data.len()].copy_from_slice(data);
        buf
    }

    /// Deserialize buffer from a byte array.
    ///
    /// # Errors
    /// If `buf` does not represent a valid serialization of `BlockBuffer`.
    pub fn deserialize(buf: &SerializedBuffer<BS, K>) -> Result<Self, Error> {
        let (pos, block) = buf.split_at(1);
        let pos = usize::from(pos[0]);

        if !<K as sealed::Sealed>::invariant(pos, BS::USIZE) {
            return Err(Error);
        }

        let (data, tail) = block.split_at(pos);

        if tail.iter().any(|&b| b != 0) {
            return Err(Error);
        }

        let mut res = Self::default();
        // SAFETY: the cursor was validated and data has exactly that length.
        unsafe { res.set_data_unchecked(data) };
        Ok(res)
    }
}

impl<BS: BlockSizes> BlockBuffer<BS, Eager> {
    /// Compress remaining data after padding it with `delim`, zeros and
    /// the `suffix` bytes. If there is not enough unused space, `compress`
    /// will be called twice.
    ///
    /// # Panics
    /// If suffix length is bigger than block size.
    #[inline(always)]
    pub fn digest_pad(
        &mut self,
        delim: u8,
        suffix: &[u8],
        mut compress: impl FnMut(&Array<u8, BS>),
    ) {
        let pos = self.get_pos();
        let size = self.size();
        // Number of bytes remaining in the buffer after `delim` is written to it
        // This never underflows since for eager buffers `size` is always greater than `pos`.
        let pad_len = size - pos - 1;

        let suffix_dst_pos = size
            .checked_sub(suffix.len())
            .expect("suffix must be smaller than buffer block size");

        let g = ResetGuard(self);
        // SAFETY: we fully initialize the buffer. Note that we may temporarily break
        // the buffer invariant, but we restore it using `ResetGuard`,
        // which works even if `compress` panics.
        let buf = unsafe {
            let p: *mut u8 = g.0.buffer.as_mut_ptr().cast::<u8>().add(pos);
            ptr::write(p, delim);
            ptr::write_bytes(p.add(1), 0, pad_len);
            g.0.buffer.assume_init_mut()
        };

        if pad_len < suffix.len() {
            compress(buf);
            buf.fill(0);
        }

        buf[suffix_dst_pos..].copy_from_slice(suffix);
        compress(buf);
    }

    /// Pad message with 0x80, zeros and 64-bit message length using
    /// big-endian byte order.
    #[inline]
    pub fn len64_padding_be(&mut self, data_len: u64, compress: impl FnMut(&Array<u8, BS>)) {
        self.digest_pad(0x80, &data_len.to_be_bytes(), compress);
    }

    /// Pad message with 0x80, zeros and 64-bit message length using
    /// little-endian byte order.
    #[inline]
    pub fn len64_padding_le(&mut self, data_len: u64, compress: impl FnMut(&Array<u8, BS>)) {
        self.digest_pad(0x80, &data_len.to_le_bytes(), compress);
    }

    /// Pad message with 0x80, zeros and 128-bit message length using
    /// big-endian byte order.
    #[inline]
    pub fn len128_padding_be(&mut self, data_len: u128, compress: impl FnMut(&Array<u8, BS>)) {
        self.digest_pad(0x80, &data_len.to_be_bytes(), compress);
    }
}

impl<BS: BlockSizes, K: BufferKind> Zeroize for BlockBuffer<BS, K> {
    #[inline]
    fn zeroize(&mut self) {
        self.buffer.zeroize();
        self.pos.zeroize();
    }
}

impl<BS: BlockSizes, K: BufferKind> Drop for BlockBuffer<BS, K> {
    #[inline]
    fn drop(&mut self) {
        self.zeroize();
    }
}

impl<BS: BlockSizes, K: BufferKind> ZeroizeOnDrop for BlockBuffer<BS, K> {}

/// Resets the referenced buffer on drop.
struct ResetGuard<'a, BS: BlockSizes, K: BufferKind>(&'a mut BlockBuffer<BS, K>);

impl<BS: BlockSizes, K: BufferKind> Drop for ResetGuard<'_, BS, K> {
    fn drop(&mut self) {
        self.0.reset();
    }
}

#[cfg(test)]
#[path = "../../../tests/smb/block_accumulator.rs"]
mod tests;
