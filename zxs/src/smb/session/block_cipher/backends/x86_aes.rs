// ZXS adaptation of RustCrypto AES; see block_cipher/ORIGIN.md.
use cipher::{
    Block, BlockCipherDecBackend, BlockCipherDecClosure, BlockCipherEncBackend,
    BlockCipherEncClosure, BlockSizeUser, ParBlocks, ParBlocksSizeUser,
    consts::{U8, U16},
    inout::InOut,
};

mod encdec;
mod expand;
mod utils;

pub(crate) use encdec::{decrypt, encrypt};
pub(crate) use expand::RoundKeys;

pub(crate) type Aes128 = Aes<11>;
pub(crate) type Aes192 = Aes<13>;
pub(crate) type Aes256 = Aes<15>;



type ParBlocksSize = U8;

#[derive(Clone, Copy)]
pub(crate) struct Aes<const RK: usize> {
    pub(crate) enc_rk: RoundKeys<RK>,
    pub(crate) dec_rk: RoundKeys<RK>,
}

impl<const RK: usize> Aes<RK> {
    #[inline]
    #[target_feature(enable = "aes")]
    pub(crate) fn encrypt(&self, f: impl BlockCipherEncClosure<BlockSize = U16>) {
        f.call(self);
    }

    #[inline]
    #[target_feature(enable = "aes")]
    pub(crate) fn decrypt(&self, f: impl BlockCipherDecClosure<BlockSize = U16>) {
        f.call(self);
    }
}

impl<const RK: usize> BlockSizeUser for Aes<RK> {
    type BlockSize = U16;
}

impl<const RK: usize> ParBlocksSizeUser for Aes<RK> {
    type ParBlocksSize = ParBlocksSize;
}

impl<const RK: usize> BlockCipherEncBackend for Aes<RK> {
    #[inline(always)]
    fn encrypt_block(&self, block: InOut<'_, '_, Block<Self>>) {
        // SAFETY: this trait impl is used only by the `Self::encrypt` method marked with
        // `#[target_feature(enable = "aes")]`
        unsafe { encdec::encrypt(&self.enc_rk, block) };
    }

    #[inline(always)]
    fn encrypt_par_blocks(&self, blocks: InOut<'_, '_, ParBlocks<Self>>) {
        // SAFETY: this trait impl is used only by the `Self::encrypt` method marked with
        // `#[target_feature(enable = "aes")]`
        unsafe { encdec::batch_encrypt(&self.enc_rk, blocks) };
    }
}

impl<const RK: usize> BlockCipherDecBackend for Aes<RK> {
    #[inline(always)]
    fn decrypt_block(&self, block: InOut<'_, '_, Block<Self>>) {
        // SAFETY: this trait impl is used only by the `Self::decrypt` method marked with
        // `#[target_feature(enable = "aes")]`
        unsafe { encdec::decrypt(&self.dec_rk, block) };
    }

    #[inline(always)]
    fn decrypt_par_blocks(&self, blocks: InOut<'_, '_, ParBlocks<Self>>) {
        // SAFETY: this trait impl is used only by the `Self::decrypt` method marked with
        // `#[target_feature(enable = "aes")]`
        unsafe { encdec::decrypt_par(&self.dec_rk, blocks) };
    }
}

impl Aes128 {
    #[inline]
    #[target_feature(enable = "aes")]
    pub(crate) fn new(key: &[u8; 16]) -> Self {
        let enc_rk = expand::aes128_expand_key(key);
        let dec_rk = expand::inv_expanded_keys(&enc_rk);
        Self { enc_rk, dec_rk }
    }
}
