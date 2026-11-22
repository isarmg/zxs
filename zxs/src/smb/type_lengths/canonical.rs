// Adapted from Paho Lurie-Gregg and contributors (MIT OR Apache-2.0); see ORIGIN.md.
//! Canonicalize intermediate binary results before exposing a width.

use super::{B0, B1, Bit, UInt, UTerm, Unsigned};

/// A marker for restricting a method on a public trait to internal use only.
pub(crate) enum Internal {}

pub trait InternalMarker {}

impl InternalMarker for Internal {}

/// Convenience trait. Calls `Invert` -> `TrimTrailingZeros` -> `Invert`
pub trait Trim {
    type Output;

    fn trim(self) -> Self::Output;
}
pub type TrimOut<A> = <A as Trim>::Output;

/// Gets rid of all zeros until it hits a one.
// ONLY IMPLEMENT FOR INVERTED NUMBERS!
pub trait TrimTrailingZeros {
    type Output;

    fn trim_trailing_zeros(self) -> Self::Output;
}

/// Converts between standard numbers and inverted ones that have the most significant
/// digit on the outside.
pub trait Invert {
    type Output;

    fn invert(self) -> Self::Output;
}

/// Doubly private! Called by invert to make the magic happen once its done the first step.
/// The Rhs is what we've got so far.
pub trait PrivateInvert<Rhs> {
    type Output;

    fn private_invert(self, rhs: Rhs) -> Self::Output;
}
pub type PrivateInvertOut<A, Rhs> = <A as PrivateInvert<Rhs>>::Output;

/// Terminating character for `InvertedUInt`s
pub struct InvertedUTerm;

/// Inverted `UInt` (has most significant digit on the outside)
pub struct InvertedUInt<IU: InvertedUnsigned, B: Bit> {
    msb: IU,
    lsb: B,
}

/// Does the real subtraction for `UInt`s; `Sub` just calls this and then `Trim`.
pub trait PrivateSub<Rhs = Self> {
    type Output;

    fn private_sub(self, rhs: Rhs) -> Self::Output;
}
pub type PrivateSubOut<A, Rhs> = <A as PrivateSub<Rhs>>::Output;

pub trait InvertedUnsigned {}

impl InvertedUnsigned for InvertedUTerm {}
impl<IU: InvertedUnsigned, B: Bit> InvertedUnsigned for InvertedUInt<IU, B> {}

impl Invert for UTerm {
    type Output = InvertedUTerm;

    #[inline]
    fn invert(self) -> Self::Output {
        InvertedUTerm
    }
}

impl<U: Unsigned, B: Bit> Invert for UInt<U, B>
where
    U: PrivateInvert<InvertedUInt<InvertedUTerm, B>>,
{
    type Output = PrivateInvertOut<U, InvertedUInt<InvertedUTerm, B>>;

    #[inline]
    fn invert(self) -> Self::Output {
        self.msb.private_invert(InvertedUInt {
            msb: InvertedUTerm,
            lsb: self.lsb,
        })
    }
}

impl<IU: InvertedUnsigned> PrivateInvert<IU> for UTerm {
    type Output = IU;

    #[inline]
    fn private_invert(self, rhs: IU) -> Self::Output {
        rhs
    }
}

impl<IU: InvertedUnsigned, U: Unsigned, B: Bit> PrivateInvert<IU> for UInt<U, B>
where
    U: PrivateInvert<InvertedUInt<IU, B>>,
{
    type Output = PrivateInvertOut<U, InvertedUInt<IU, B>>;

    #[inline]
    fn private_invert(self, rhs: IU) -> Self::Output {
        self.msb.private_invert(InvertedUInt {
            msb: rhs,
            lsb: self.lsb,
        })
    }
}

impl Invert for InvertedUTerm {
    type Output = UTerm;

    #[inline]
    fn invert(self) -> Self::Output {
        UTerm
    }
}

impl<IU: InvertedUnsigned, B: Bit> Invert for InvertedUInt<IU, B>
where
    IU: PrivateInvert<UInt<UTerm, B>>,
{
    type Output = <IU as PrivateInvert<UInt<UTerm, B>>>::Output;

    #[inline]
    fn invert(self) -> Self::Output {
        self.msb.private_invert(UInt {
            msb: UTerm,
            lsb: self.lsb,
        })
    }
}

impl<U: Unsigned> PrivateInvert<U> for InvertedUTerm {
    type Output = U;

    #[inline]
    fn private_invert(self, rhs: U) -> Self::Output {
        rhs
    }
}

impl<U: Unsigned, IU: InvertedUnsigned, B: Bit> PrivateInvert<U> for InvertedUInt<IU, B>
where
    IU: PrivateInvert<UInt<U, B>>,
{
    type Output = <IU as PrivateInvert<UInt<U, B>>>::Output;

    #[inline]
    fn private_invert(self, rhs: U) -> Self::Output {
        self.msb.private_invert(UInt {
            msb: rhs,
            lsb: self.lsb,
        })
    }
}

impl TrimTrailingZeros for InvertedUTerm {
    type Output = InvertedUTerm;

    #[inline]
    fn trim_trailing_zeros(self) -> Self::Output {
        InvertedUTerm
    }
}

impl<IU: InvertedUnsigned> TrimTrailingZeros for InvertedUInt<IU, B1> {
    type Output = Self;

    #[inline]
    fn trim_trailing_zeros(self) -> Self::Output {
        self
    }
}

impl<IU: InvertedUnsigned> TrimTrailingZeros for InvertedUInt<IU, B0>
where
    IU: TrimTrailingZeros,
{
    type Output = <IU as TrimTrailingZeros>::Output;

    #[inline]
    fn trim_trailing_zeros(self) -> Self::Output {
        self.msb.trim_trailing_zeros()
    }
}

impl<U: Unsigned> Trim for U
where
    U: Invert,
    <U as Invert>::Output: TrimTrailingZeros,
    <<U as Invert>::Output as TrimTrailingZeros>::Output: Invert,
{
    type Output = <<<U as Invert>::Output as TrimTrailingZeros>::Output as Invert>::Output;

    #[inline]
    fn trim(self) -> Self::Output {
        self.invert().trim_trailing_zeros().invert()
    }
}
