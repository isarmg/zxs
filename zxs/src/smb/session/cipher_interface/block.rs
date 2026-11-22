// Adapted from RustCrypto (MIT OR Apache-2.0); see ORIGIN.md and the licenses.
// ZXS retains the methods used by SMB and its known-answer tests.

use crate::smb::crypto_types::{Block, BlockSizeUser};

mod backends;
mod ctx;

pub use backends::{
    BlockCipherDecBackend, BlockCipherDecClosure, BlockCipherEncBackend, BlockCipherEncClosure,
};
use ctx::{BlockCtx, BlocksCtx};

/// Encrypt-only functionality for block ciphers.
pub trait BlockCipherEncrypt: BlockSizeUser + Sized {
    /// Encrypt data using backend provided to the rank-2 closure.
    fn encrypt_with_backend(&self, f: impl BlockCipherEncClosure<BlockSize = Self::BlockSize>);

    /// Encrypt single block in-place.
    #[inline]
    fn encrypt_block(&self, block: &mut Block<Self>) {
        let block = block.into();
        self.encrypt_with_backend(BlockCtx { block });
    }

    /// Encrypt blocks in-place.
    #[inline]
    fn encrypt_blocks(&self, blocks: &mut [Block<Self>]) {
        let blocks = blocks.into();
        self.encrypt_with_backend(BlocksCtx { blocks });
    }
}

/// Decrypt-only functionality for block ciphers.
pub trait BlockCipherDecrypt: BlockSizeUser {
    /// Decrypt data using backend provided to the rank-2 closure.
    fn decrypt_with_backend(&self, f: impl BlockCipherDecClosure<BlockSize = Self::BlockSize>);

    /// Decrypt single block in-place.
    #[inline]
    fn decrypt_block(&self, block: &mut Block<Self>) {
        let block = block.into();
        self.decrypt_with_backend(BlockCtx { block });
    }

    /// Decrypt blocks in-place.
    #[inline]
    fn decrypt_blocks(&self, blocks: &mut [Block<Self>]) {
        let blocks = blocks.into();
        self.decrypt_with_backend(BlocksCtx { blocks });
    }
}
