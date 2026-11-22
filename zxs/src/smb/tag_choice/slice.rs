// ZXS adaptation of RustCrypto comparisons; see tag_choice/ORIGIN.md.
use super::{CmovEq, Condition};
use core::slice;

#[cfg(not(target_pointer_width = "64"))]
type Word = u32;
#[cfg(target_pointer_width = "64")]
type Word = u64;
const WORD_SIZE: usize = size_of::<Word>();

impl CmovEq for [u8] {
    #[inline]
    fn cmovne(&self, rhs: &Self, input: Condition, output: &mut Condition) {
        // Short-circuit the comparison if the slices are of different lengths, and set the output
        // condition to the input condition.
        if self.len() != rhs.len() {
            *output = input;
            return;
        }

        let (self_chunks, self_remainder) = slice_as_chunks::<u8, WORD_SIZE>(self);
        let (rhs_chunks, rhs_remainder) = slice_as_chunks::<u8, WORD_SIZE>(rhs);

        for (self_chunk, rhs_chunk) in self_chunks.iter().zip(rhs_chunks.iter()) {
            let a = Word::from_ne_bytes(*self_chunk);
            let b = Word::from_ne_bytes(*rhs_chunk);
            a.cmovne(&b, input, output);
        }

        cmovne_remainder(self_remainder, rhs_remainder, input, output);
    }
}

#[inline]
fn cmovne_remainder<T>(
    a_remainder: &[T],
    b_remainder: &[T],
    input: Condition,
    output: &mut Condition,
) where
    T: Copy,
    Word: From<T>,
{
    let a = slice_to_word(a_remainder);
    let b = slice_to_word(b_remainder);
    a.cmovne(&b, input, output);
}

#[inline]
fn slice_to_word<T>(slice: &[T]) -> Word
where
    T: Copy,
    Word: From<T>,
{
    debug_assert!(size_of_val(slice) <= WORD_SIZE, "slice too large");
    slice.iter().rev().copied().fold(0, |acc, n| {
        (acc << (const { size_of::<T>() * 8 })) | Word::from(n)
    })
}

// Retain the original copies of the Rust core chunk helpers.
#[inline]
#[track_caller]
#[must_use]
fn slice_as_chunks<T, const N: usize>(slice: &[T]) -> (&[[T; N]], &[T]) {
    assert!(N != 0, "chunk size must be non-zero");
    let len_rounded_down = slice.len() / N * N;
    // SAFETY: The rounded-down value is always the same or smaller than the
    // original length, and thus must be in-bounds of the slice.
    let (multiple_of_n, remainder) = unsafe { slice.split_at_unchecked(len_rounded_down) };
    // SAFETY: We already panicked for zero, and ensured by construction
    // that the length of the subslice is a multiple of N.
    let array_slice = unsafe { slice_as_chunks_unchecked(multiple_of_n) };
    (array_slice, remainder)
}

#[inline]
#[must_use]
#[track_caller]
unsafe fn slice_as_chunks_unchecked<T, const N: usize>(slice: &[T]) -> &[[T; N]] {
    // Caller must guarantee that `N` is nonzero and exactly divides the slice length
    const { debug_assert!(N != 0) };
    debug_assert_eq!(slice.len() % N, 0);
    let new_len = slice.len() / N;

    // SAFETY: We cast a slice of `new_len * N` elements into
    // a slice of `new_len` many `N` elements chunks.
    unsafe { slice::from_raw_parts(slice.as_ptr().cast(), new_len) }
}
