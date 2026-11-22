// Adapted from RustCrypto (MIT OR Apache-2.0); see ORIGIN.md and the licenses.
// ZXS retains the methods used by SMB and its known-answer tests.

use crate::smb::crypto_types::{Block, BlockSizeUser, array::ArraySize, type_lengths::Unsigned};
use crate::smb::session::cipher_buffers::{InOut, InOutBuf};

use super::{
    BlockCipherDecBackend, BlockCipherDecClosure, BlockCipherEncBackend, BlockCipherEncClosure,
};

/// Closure used in methods which operate over separate blocks.
pub(super) struct BlockCtx<'inp, 'out, BS: ArraySize> {
    pub block: InOut<'inp, 'out, Block<Self>>,
}

impl<BS: ArraySize> BlockSizeUser for BlockCtx<'_, '_, BS> {
    type BlockSize = BS;
}

impl<BS: ArraySize> BlockCipherEncClosure for BlockCtx<'_, '_, BS> {
    #[inline(always)]
    fn call<B: BlockCipherEncBackend<BlockSize = BS>>(self, backend: &B) {
        backend.encrypt_block(self.block);
    }
}

impl<BS: ArraySize> BlockCipherDecClosure for BlockCtx<'_, '_, BS> {
    #[inline(always)]
    fn call<B: BlockCipherDecBackend<BlockSize = BS>>(self, backend: &B) {
        backend.decrypt_block(self.block);
    }
}
/// Closure used in methods which operate over slice of blocks.
pub(super) struct BlocksCtx<'inp, 'out, BS: ArraySize> {
    pub blocks: InOutBuf<'inp, 'out, Block<Self>>,
}

impl<BS: ArraySize> BlockSizeUser for BlocksCtx<'_, '_, BS> {
    type BlockSize = BS;
}

impl<BS: ArraySize> BlockCipherEncClosure for BlocksCtx<'_, '_, BS> {
    #[inline(always)]
    fn call<B: BlockCipherEncBackend<BlockSize = BS>>(self, backend: &B) {
        if B::ParBlocksSize::USIZE > 1 {
            let (chunks, tail) = self.blocks.into_chunks();
            for chunk in chunks {
                backend.encrypt_par_blocks(chunk);
            }
            backend.encrypt_tail_blocks(tail);
        } else {
            for block in self.blocks {
                backend.encrypt_block(block);
            }
        }
    }
}

impl<BS: ArraySize> BlockCipherDecClosure for BlocksCtx<'_, '_, BS> {
    #[inline(always)]
    fn call<B: BlockCipherDecBackend<BlockSize = BS>>(self, backend: &B) {
        if B::ParBlocksSize::USIZE > 1 {
            let (chunks, tail) = self.blocks.into_chunks();
            for chunk in chunks {
                backend.decrypt_par_blocks(chunk);
            }
            backend.decrypt_tail_blocks(tail);
        } else {
            for block in self.blocks {
                backend.decrypt_block(block);
            }
        }
    }
}
