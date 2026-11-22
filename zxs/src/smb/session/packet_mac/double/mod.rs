//! RustCrypto's big-endian 128-bit block doubling used by AES-CMAC.
//! Modified by ZXS to retain only the forward 128-bit operation; see ../ORIGIN.md.
#![forbid(unsafe_code)]

use crate::smb::fixed_output::array::{Array, typenum::U16};

const C128: u64 = 0b1000_0111;

mod sealed {
    pub trait Sealed {}
}

/// Double an AES block over `GF(2^128)` in big-endian byte order.
pub trait Dbl: sealed::Sealed {
    /// Double block. (alternatively: multiply block by x)
    ///
    /// If most significant bit of the block equals to zero will return
    /// `block<<1`, otherwise `(block<<1)^C`, where `C` is the non-leading
    /// coefficients of the lexicographically first irreducible degree-b binary
    /// polynomial with the minimal number of ones.
    #[must_use]
    fn dbl(self) -> Self;
}

impl Dbl for Array<u8, U16> {
    #[inline]
    fn dbl(self) -> Self {
        let mut val = [
            u64::from_be_bytes(self[..8].try_into().unwrap()),
            u64::from_be_bytes(self[8..].try_into().unwrap()),
        ];

        let b = val[1] >> 63;
        let a = val[0] >> 63;

        val[0] <<= 1;
        val[0] ^= b;
        val[1] <<= 1;
        val[1] ^= a * C128;

        let mut res = Self::default();
        res[..8].copy_from_slice(&val[0].to_be_bytes());
        res[8..].copy_from_slice(&val[1].to_be_bytes());
        res
    }
}

impl sealed::Sealed for Array<u8, U16> {}
