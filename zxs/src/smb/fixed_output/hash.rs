// ZXS adaptation of RustCrypto fixed-output support; see ORIGIN.md.
use super::{FixedOutput, FixedOutputReset, Output, OutputSizeUser, Update};

pub(in crate::smb) trait HashMarker {}

pub(in crate::smb) trait Digest: OutputSizeUser {
    fn new() -> Self;
    fn update(&mut self, data: impl AsRef<[u8]>);
    #[must_use]
    fn chain_update(self, data: impl AsRef<[u8]>) -> Self;
    fn finalize(self) -> Output<Self>;
    fn finalize_into(self, out: &mut Output<Self>);
    fn finalize_reset(&mut self) -> Output<Self>
    where
        Self: FixedOutputReset;
    fn digest(data: impl AsRef<[u8]>) -> Output<Self>;
}

impl<D: FixedOutput + Default + Update + HashMarker> Digest for D {
    #[inline]
    fn new() -> Self {
        Self::default()
    }

    #[inline]
    fn update(&mut self, data: impl AsRef<[u8]>) {
        Update::update(self, data.as_ref());
    }

    #[inline]
    fn chain_update(mut self, data: impl AsRef<[u8]>) -> Self {
        Update::update(&mut self, data.as_ref());
        self
    }

    #[inline]
    fn finalize(self) -> Output<Self> {
        FixedOutput::finalize_fixed(self)
    }

    #[inline]
    fn finalize_into(self, out: &mut Output<Self>) {
        FixedOutput::finalize_into(self, out);
    }

    #[inline]
    fn finalize_reset(&mut self) -> Output<Self>
    where
        Self: FixedOutputReset,
    {
        FixedOutputReset::finalize_fixed_reset(self)
    }

    #[inline]
    fn digest(data: impl AsRef<[u8]>) -> Output<Self> {
        let mut hasher = Self::default();
        hasher.update(data.as_ref());
        hasher.finalize()
    }
}
