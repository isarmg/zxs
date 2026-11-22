//! Private AES-128 behind SMB packet CMAC.
//!
//! Source adapted from RustCrypto; see ORIGIN.md and the original licenses.
//! Key expansion, instruction/portable backends and whole-object erasure
//! follow the previously selected AES-128 implementation, with no public API.

mod backends;

use super::cipher_interface::{
    BlockCipherDecClosure, BlockCipherDecrypt, BlockCipherEncClosure, BlockCipherEncrypt,
};
use crate::smb::crypto_types::{AlgorithmName, BlockSizeUser, Key, KeyInit, KeySizeUser, array::Array, typenum::U16};
use core::fmt;
use crate::smb::backend_select::cfg_if;

/// 128-bit AES block
type Block = Array<u8, U16>;

// Define token used for target feature detection
cfg_if! {
    if #[cfg(all(not(aes_backend = "soft"), any(target_arch = "x86_64", target_arch = "x86")))] {
        crate::smb::hardware_capabilities::probe!(token, features_aes, "aes");
        crate::smb::hardware_capabilities::probe!(token, features_vaes256, "vaes");
        crate::smb::hardware_capabilities::probe!(token, features_vaes512, "avx512f", "vaes");

        #[derive(Clone, Copy)]
        struct Token {
            aes: features_aes::InitToken,
            vaes256: features_vaes256::InitToken,
            vaes512: features_vaes512::InitToken,
        }

        impl Default for Token {
            fn default() -> Self {
                Token {
                    aes: features_aes::InitToken::init(),
                    vaes256: features_vaes256::InitToken::init(),
                    vaes512: features_vaes512::InitToken::init(),
                }
            }
        }

    } else if #[cfg(all(not(aes_backend = "soft"), target_arch = "aarch64", not(miri)))] {
        crate::smb::hardware_capabilities::probe!(token, features_aes, "aes");

        #[derive(Clone, Copy)]
        struct Token {
            aes: features_aes::InitToken,
        }

        impl Default for Token {
            fn default() -> Self {
                Token {
                    aes: features_aes::InitToken::init(),
                }
            }
        }
    }
}

#[derive(Clone, Copy)]
union Inner {
    #[cfg(all(not(aes_backend = "soft"), any(target_arch = "x86_64", target_arch = "x86")))]
    aes: backends::x86_aes::Aes128,
    #[cfg(all(not(aes_backend = "soft"), target_arch = "aarch64", not(miri)))]
    aes: backends::aarch64_aes::Aes128,
    soft: backends::soft::Aes128,
}

/// The exact AES-128 block cipher used by the SMB signing implementation.
#[derive(Clone)]
pub(in crate::smb) struct Aes128 {
    inner: Inner,
    #[cfg(all(not(aes_backend = "soft"), any(
        target_arch = "x86_64", target_arch = "x86", all(target_arch = "aarch64", not(miri)),
    )))]
    token: Token,
}

impl KeySizeUser for Aes128 {
    type KeySize = U16;
}

impl KeyInit for Aes128 {
    #[inline]
    fn new(key: &Key<Self>) -> Self {
        #[cfg(all(not(aes_backend = "soft"), any(
            target_arch = "x86_64", target_arch = "x86", all(target_arch = "aarch64", not(miri)),
        )))]
        let token = Token::default();
        let key = &key.0;

        #[cfg(not(aes_backend = "soft"))]
        cfg_if! {
            if #[cfg(any(target_arch = "x86_64", target_arch = "x86"))] {
                if token.aes.get() {
                    // SAFETY: we confirmed that the required target features are available
                    let aes = unsafe { backends::x86_aes::Aes128::new(key) };
                    let inner = Inner { aes };
                    return Self { inner, token };
                }
            } else if #[cfg(all(target_arch = "aarch64", not(miri)))] {
                if token.aes.get() {
                    // SAFETY: we confirmed that the required target features are available
                    let aes = unsafe { backends::aarch64_aes::Aes128::new(key) };
                    let inner = Inner { aes };
                    return Self { inner, token };
                }
            }
        }

        let soft = backends::soft::Aes128::new(key);
        let inner = Inner { soft };
        Self {
            inner,
            #[cfg(all(not(aes_backend = "soft"), any(
                target_arch = "x86_64", target_arch = "x86", all(target_arch = "aarch64", not(miri)),
            )))]
            token,
        }
    }
}

impl BlockCipherEncrypt for Aes128 {
    #[inline]
    fn encrypt_with_backend(&self, f: impl BlockCipherEncClosure<BlockSize = U16>) {
        #[cfg(not(aes_backend = "soft"))]
        cfg_if! {
            if #[cfg(any(target_arch = "x86_64", target_arch = "x86"))] {
                if self.token.vaes512.get() {
                    // SAFETY: we access correct union variant
                    let enc_rk = unsafe { &self.inner.aes.enc_rk };
                    // SAFETY: we confirmed that the required target features are available
                    unsafe { backends::x86_vaes512::Aes128::encrypt(enc_rk, f) };
                    return;
                }

                if self.token.vaes256.get() {
                    // SAFETY: we access correct union variant
                    let enc_rk = unsafe { &self.inner.aes.enc_rk };
                    // SAFETY: we confirmed that the required target features are available
                    unsafe { backends::x86_vaes256::Aes128::encrypt(enc_rk, f) };
                    return;
                }

                if self.token.aes.get() {
                    // SAFETY: we access correct union variant
                    let aes = unsafe { &self.inner.aes };
                    // SAFETY: we confirmed that the required target features are available
                    unsafe { aes.encrypt(f) };
                    return;
                }
            } else if #[cfg(all(target_arch = "aarch64", not(miri)))] {
                if self.token.aes.get() {
                    // SAFETY: we access correct union variant
                    let aes = unsafe { &self.inner.aes };
                    // SAFETY: we confirmed that the required target features are available
                    unsafe { aes.encrypt(f) };
                    return;
                }
            }
        }

        // SAFETY: we access correct union variant
        let aes = unsafe { &self.inner.soft };
        aes.encrypt(f);
    }
}

impl BlockCipherDecrypt for Aes128 {
    #[inline]
    fn decrypt_with_backend(&self, f: impl BlockCipherDecClosure<BlockSize = U16>) {
        #[cfg(not(aes_backend = "soft"))]
        cfg_if! {
            if #[cfg(any(target_arch = "x86_64", target_arch = "x86"))] {
                if self.token.vaes512.get() {
                    // SAFETY: we access correct union variant
                    let dec_rk = unsafe { &self.inner.aes.dec_rk };
                    // SAFETY: we confirmed that the required target features are available
                    unsafe { backends::x86_vaes512::Aes128::decrypt(dec_rk, f) };
                    return;
                }

                if self.token.vaes256.get() {
                    // SAFETY: we access correct union variant
                    let dec_rk = unsafe { &self.inner.aes.dec_rk };
                    // SAFETY: we confirmed that the required target features are available
                    unsafe { backends::x86_vaes256::Aes128::decrypt(dec_rk, f) };
                    return;
                }

                if self.token.aes.get() {
                    // SAFETY: we access correct union variant
                    let backend = unsafe { &self.inner.aes };
                    // SAFETY: we confirmed that the required target features are available
                    unsafe { backend.decrypt(f) };
                    return;
                }
            } else if #[cfg(all(target_arch = "aarch64", not(miri)))] {
                if self.token.aes.get() {
                    // SAFETY: we access correct union variant
                    let backend = unsafe { &self.inner.aes };
                    // SAFETY: we confirmed that the required target features are available
                    unsafe { backend.decrypt(f) };
                    return;
                }
            }
        }

        // SAFETY: we access correct union variant
        let aes = unsafe { &self.inner.soft };
        aes.decrypt(f);
    }
}

impl BlockSizeUser for Aes128 {
    type BlockSize = U16;
}

impl fmt::Debug for Aes128 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Aes128 { .. }")
    }
}

impl AlgorithmName for Aes128 {
    fn write_alg_name(f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Aes128")
    }
}

impl Drop for Aes128 {
    #[inline]
    fn drop(&mut self) {
        // SAFETY: only plain fixed-size round-key arrays and dispatch tokens;
        // all-zero bytes are valid for those fields, and none runs Drop.
        unsafe { crate::smb::memory_erasure::zeroize_flat_type(self) };
    }
}
impl crate::smb::memory_erasure::ZeroizeOnDrop for Aes128 {}

#[cfg(test)]
#[path = "../../../../tests/smb/aes_block.rs"]
mod tests;
