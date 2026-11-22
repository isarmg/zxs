//! Block and stream interfaces used by SMB packet signing and NTLM key exchange.
#![forbid(unsafe_code)]

mod block;
mod stream;

pub(super) use block::{
    BlockCipherDecBackend, BlockCipherDecClosure, BlockCipherDecrypt, BlockCipherEncBackend,
    BlockCipherEncClosure, BlockCipherEncrypt,
};
pub(super) use stream::{StreamCipher, StreamCipherError};
