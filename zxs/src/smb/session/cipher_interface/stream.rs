// Adapted from RustCrypto (MIT OR Apache-2.0); see ORIGIN.md and the licenses.
// ZXS retains the methods used by SMB and its known-answer tests.

use core::fmt;
use inout::InOutBuf;

/// Apply a checked stream to the existing buffer.
pub trait StreamCipher {
    /// Check that the cipher can generate a keystream with a length of `data_len` bytes.
    ///
    /// # Errors
    /// Returns [`StreamCipherError`] in the event it cannot.
    fn check_remaining(&self, data_len: usize) -> Result<(), StreamCipherError>;

    /// Apply after the caller has checked the requested keystream length.
    fn unchecked_apply_keystream_inout(&mut self, buf: InOutBuf<'_, '_, u8>);

    /// Apply keystream to `inout` data.
    ///
    /// # Errors
    /// If the end of the keystream is reached with the given buffer length,
    /// the method will return [`StreamCipherError`] without modifying `buf`.
    fn try_apply_keystream_inout(
        &mut self,
        buf: InOutBuf<'_, '_, u8>,
    ) -> Result<(), StreamCipherError> {
        self.check_remaining(buf.len())?;
        self.unchecked_apply_keystream_inout(buf);
        Ok(())
    }

    /// Apply keystream to data behind `buf`.
    ///
    /// # Errors
    /// If the end of the keystream is reached with the given buffer length,
    /// the method will return [`StreamCipherError`] without modifying `buf`.
    #[inline]
    fn try_apply_keystream(&mut self, buf: &mut [u8]) -> Result<(), StreamCipherError> {
        self.try_apply_keystream_inout(buf.into())
    }
}

/// The requested keystream length is unavailable.
#[derive(Copy, Clone, Debug)]
pub struct StreamCipherError;

impl fmt::Display for StreamCipherError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        f.write_str("Loop Error")
    }
}

impl core::error::Error for StreamCipherError {}
