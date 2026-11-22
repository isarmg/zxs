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

use crate::smb::backend_select::cfg_if;
use crate::smb::crypto_types::array::Array;

pub(super) mod aes128;

mod mix_columns;
mod sbox;
mod utils;
mod word;

pub(super) use word::Word;

type State<W> = [W; 8];

// The original width priority keeps ARMv7 and wasm32 on 64-bit batches.
// See NATIVE-WORD-ORIGIN.md for the retained attribution.
cfg_if! {
    if #[cfg(cpubits = "16")] {
        pub(super) type NativeWord = u16;
    } else if #[cfg(cpubits = "32")] {
        pub(super) type NativeWord = u32;
    } else if #[cfg(cpubits = "64")] {
        pub(super) type NativeWord = u64;
    } else if #[cfg(target_pointer_width = "16")] {
        pub(super) type NativeWord = u16;
    } else if #[cfg(all(
        target_pointer_width = "32",
        not(any(all(target_arch = "arm", target_feature = "v7"), target_arch = "wasm32")),
    ))] {
        pub(super) type NativeWord = u32;
    } else if #[cfg(any(
        target_pointer_width = "64", all(target_arch = "arm", target_feature = "v7"),
        target_arch = "wasm32",
    ))] {
        pub(super) type NativeWord = u64;
    } else {
        compile_error!("unsupported target pointer width");
    }
}

pub(super) type NativeBatchSize = <NativeWord as Word>::Blocks;
pub(super) type BatchBlocks<W> = Array<crate::smb::session::block_cipher::Block, <W as Word>::Blocks>;
pub(super) type MinWord = u16;
