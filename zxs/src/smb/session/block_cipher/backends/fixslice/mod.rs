// ZXS adaptation of RustCrypto AES; see block_cipher/ORIGIN.md.
//! Fixsliced implementation of AES-128
//! adapted from the C implementation.
//!
//! All implementations are fully bitsliced and do not rely on any
//! Look-Up Table (LUT).
//!
//! See the paper at <https://eprint.iacr.org/2020/1123.pdf> for more details.
//!
//! # Author (original C code)
//!
//! Alexandre Adomnicai, Nanyang Technological University, Singapore
//! <alexandre.adomnicai@ntu.edu.sg>
//!
//! Originally licensed MIT. Relicensed as Apache 2.0+MIT with permission.

use cipher::array::Array;

pub(super) mod aes128;

mod mix_columns;
mod sbox;
mod utils;
mod word;

pub(super) use word::Word;

type State<W> = [W; 8];

cpubits::cpubits! {
    16 => {
        pub(super) type NativeWord = u16;
    }
    32 => {
        pub(super) type NativeWord = u32;
    }
    64 => {
        pub(super) type NativeWord = u64;
    }
}

pub(super) type NativeBatchSize = <NativeWord as Word>::Blocks;
pub(super) type BatchBlocks<W> = Array<crate::smb::session::block_cipher::Block, <W as Word>::Blocks>;
pub(super) type MinWord = u16;
