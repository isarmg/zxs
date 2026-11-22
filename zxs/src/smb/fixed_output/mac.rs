// ZXS adaptation of RustCrypto fixed-output support; see ORIGIN.md.
use super::{FixedOutput, Update};
use cmov::CmovEq;
use core::{fmt, hint::black_box};
use crypto_common::{Output, OutputSizeUser, typenum::Unsigned};

#[inline]
fn tags_equal(expected: &[u8], supplied: &[u8]) -> bool {
    let mut choice = 0;
    expected.cmoveq(supplied, 1, &mut choice);
    // Preserve the optimizer barrier at the original choice-to-bool boundary.
    black_box(choice) != 0
}

pub(in crate::smb) trait MacMarker {}

pub(in crate::smb) trait Mac: OutputSizeUser + Sized {
    fn update(&mut self, data: &[u8]);
    fn finalize(self) -> CtOutput<Self>;
    fn verify_slice(self, tag: &[u8]) -> Result<(), MacError>;
    fn verify_truncated_left(self, tag: &[u8]) -> Result<(), MacError>;
}

impl<T: Update + FixedOutput + MacMarker> Mac for T {
    #[inline]
    fn update(&mut self, data: &[u8]) {
        Update::update(self, data);
    }

    #[inline]
    fn finalize(self) -> CtOutput<Self> {
        CtOutput::new(self.finalize_fixed())
    }

    #[inline]
    fn verify_slice(self, tag: &[u8]) -> Result<(), MacError> {
        let n = tag.len();
        if n != Self::OutputSize::USIZE {
            return Err(MacError);
        }
        let equal = tags_equal(self.finalize_fixed().as_slice(), tag);
        if equal { Ok(()) } else { Err(MacError) }
    }

    fn verify_truncated_left(self, tag: &[u8]) -> Result<(), MacError> {
        let n = tag.len();
        if n == 0 || n > Self::OutputSize::USIZE {
            return Err(MacError);
        }
        let equal = tags_equal(&self.finalize_fixed()[..n], tag);

        if equal { Ok(()) } else { Err(MacError) }
    }
}

/// Fixed size output value which provides a safe [`Eq`] implementation that
/// runs in constant time.
///
/// It is useful for implementing Message Authentication Codes (MACs).
#[derive(Clone)]
pub struct CtOutput<T: OutputSizeUser> {
    bytes: Output<T>,
}

impl<T: OutputSizeUser> CtOutput<T> {
    /// Create a new [`CtOutput`] value.
    #[inline(always)]
    fn new(bytes: Output<T>) -> Self {
        Self { bytes }
    }

    /// Get reference to the inner [`Output`] array this type wraps.
    #[inline(always)]
    pub fn as_bytes(&self) -> &Output<T> {
        &self.bytes
    }
}

impl<T: OutputSizeUser> PartialEq for CtOutput<T> {
    #[inline(always)]
    fn eq(&self, other: &CtOutput<T>) -> bool {
        tags_equal(self.bytes.as_slice(), other.bytes.as_slice())
    }
}

impl<T: OutputSizeUser> Eq for CtOutput<T> {}

impl<T: OutputSizeUser> fmt::Debug for CtOutput<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CtOutput { ... }")
    }
}

impl<T: OutputSizeUser> Drop for CtOutput<T> {
    #[inline]
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.bytes.zeroize();
    }
}

impl<T: OutputSizeUser> zeroize::ZeroizeOnDrop for CtOutput<T> {}

/// Error type for when the [`Output`] of a [`Mac`]
/// is not equal to the expected value.
#[derive(Default, Debug, Copy, Clone, Eq, PartialEq)]
pub struct MacError;

impl fmt::Display for MacError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("MAC tag mismatch")
    }
}

impl core::error::Error for MacError {}
