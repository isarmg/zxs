// Adapted from RustCrypto (MIT OR Apache-2.0); see ORIGIN.md and the licenses.
// ZXS retains the methods used by SMB and its known-answer tests.

use crate::smb::crypto_types::{Block, BlockSizeUser, ParBlocks, ParBlocksSizeUser, type_lengths::Unsigned};
use crate::smb::session::cipher_buffers::{InOut, InOutBuf};

/// Block encryption backend.
pub trait BlockCipherEncBackend: ParBlocksSizeUser {
    /// Encrypt single inout block.
    fn encrypt_block(&self, block: InOut<'_, '_, Block<Self>>);

    /// Encrypt inout blocks in parallel.
    #[inline(always)]
    fn encrypt_par_blocks(&self, mut blocks: InOut<'_, '_, ParBlocks<Self>>) {
        for i in 0..Self::ParBlocksSize::USIZE {
            self.encrypt_block(blocks.get(i));
        }
    }

    /// Encrypt buffer of inout blocks. Length of the buffer MUST be smaller
    /// than `Self::ParBlocksSize`.
    #[inline(always)]
    fn encrypt_tail_blocks(&self, blocks: InOutBuf<'_, '_, Block<Self>>) {
        assert!(blocks.len() < Self::ParBlocksSize::USIZE);
        for block in blocks {
            self.encrypt_block(block);
        }
    }
}

/// Trait for [`BlockCipherEncBackend`] users.
///
/// This trait is used to define rank-2 closures.
pub trait BlockCipherEncClosure: BlockSizeUser {
    /// Execute closure with the provided block cipher backend.
    fn call<B: BlockCipherEncBackend<BlockSize = Self::BlockSize>>(self, backend: &B);
}

/// Trait implemented by block cipher decryption backends.
pub trait BlockCipherDecBackend: ParBlocksSizeUser {
    /// Decrypt single inout block.
    fn decrypt_block(&self, block: InOut<'_, '_, Block<Self>>);

    /// Decrypt inout blocks in parallel.
    #[inline(always)]
    fn decrypt_par_blocks(&self, mut blocks: InOut<'_, '_, ParBlocks<Self>>) {
        for i in 0..Self::ParBlocksSize::USIZE {
            self.decrypt_block(blocks.get(i));
        }
    }

    /// Decrypt buffer of inout blocks. Length of the buffer MUST be smaller
    /// than `Self::ParBlocksSize`.
    #[inline(always)]
    fn decrypt_tail_blocks(&self, blocks: InOutBuf<'_, '_, Block<Self>>) {
        assert!(blocks.len() < Self::ParBlocksSize::USIZE);
        for block in blocks {
            self.decrypt_block(block);
        }
    }
}

/// Trait for [`BlockCipherDecBackend`] users.
///
/// This trait is used to define rank-2 closures.
pub trait BlockCipherDecClosure: BlockSizeUser {
    /// Execute closure with the provided block cipher backend.
    fn call<B: BlockCipherDecBackend<BlockSize = Self::BlockSize>>(self, backend: &B);
}
