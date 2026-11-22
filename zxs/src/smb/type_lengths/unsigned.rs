// Adapted from Paho Lurie-Gregg and contributors (MIT OR Apache-2.0); see ORIGIN.md.
//! Unsigned representation and arithmetic.

use super::{
    Add1, B0, B1, Bit, NonZero, Sub1, Sum, Unsigned,
    canonical::{PrivateSub, PrivateSubOut, Trim, TrimOut},
};
use core::ops::{Add, Sub};

/// The terminating type for `UInt`; it always comes after the most significant
/// bit. `UTerm` by itself represents zero, which is aliased to `U0`.
#[derive(Eq, PartialEq, Ord, PartialOrd, Clone, Copy, Hash, Debug, Default)]
pub struct UTerm;

impl UTerm {
    /// Instantiates a singleton representing this unsigned integer.
    #[inline]
    pub fn new() -> UTerm {
        UTerm
    }
}

#[derive(Eq, PartialEq, Ord, PartialOrd, Clone, Copy, Hash, Debug, Default)]
pub struct UInt<U, B> {
    /// The more significant bits of `Self`.
    pub(crate) msb: U,
    /// The least significant bit of `Self`.
    pub(crate) lsb: B,
}

impl<U: Unsigned, B: Bit> UInt<U, B> {
    /// Instantiates a singleton representing this unsigned integer.
    #[inline]
    pub fn new() -> UInt<U, B> {
        UInt::default()
    }
}

impl Unsigned for UTerm {
    const U64: u64 = 0;
    const USIZE: usize = 0;
}

impl<U: Unsigned, B: Bit> Unsigned for UInt<U, B> {
    const U64: u64 = B::U8 as u64 | U::U64 << 1;
    const USIZE: usize = B::U8 as usize | U::USIZE << 1;
}

impl<U: Unsigned, B: Bit> NonZero for UInt<U, B> {}
/// `UTerm + B0 = UTerm`
impl Add<B0> for UTerm {
    type Output = UTerm;
    #[inline]
    fn add(self, _: B0) -> Self::Output {
        UTerm
    }
}

/// `U + B0 = U`
impl<U: Unsigned, B: Bit> Add<B0> for UInt<U, B> {
    type Output = UInt<U, B>;
    #[inline]
    fn add(self, _: B0) -> Self::Output {
        UInt::new()
    }
}

/// `UTerm + B1 = UInt<UTerm, B1>`
impl Add<B1> for UTerm {
    type Output = UInt<UTerm, B1>;
    #[inline]
    fn add(self, _: B1) -> Self::Output {
        UInt::new()
    }
}

/// `UInt<U, B0> + B1 = UInt<U + B1>`
impl<U: Unsigned> Add<B1> for UInt<U, B0> {
    type Output = UInt<U, B1>;
    #[inline]
    fn add(self, _: B1) -> Self::Output {
        UInt::new()
    }
}

/// `UInt<U, B1> + B1 = UInt<U + B1, B0>`
impl<U: Unsigned> Add<B1> for UInt<U, B1>
where
    U: Add<B1>,
    Add1<U>: Unsigned,
{
    type Output = UInt<Add1<U>, B0>;
    #[inline]
    fn add(self, _: B1) -> Self::Output {
        UInt::new()
    }
}

// ---------------------------------------------------------------------------------------
// Adding unsigned integers

/// `UTerm + U = U`
impl<U: Unsigned> Add<U> for UTerm {
    type Output = U;
    #[inline]
    fn add(self, rhs: U) -> Self::Output {
        rhs
    }
}

/// `UInt<U, B> + UTerm = UInt<U, B>`
impl<U: Unsigned, B: Bit> Add<UTerm> for UInt<U, B> {
    type Output = UInt<U, B>;
    #[inline]
    fn add(self, _: UTerm) -> Self::Output {
        UInt::new()
    }
}

/// `UInt<Ul, B0> + UInt<Ur, B0> = UInt<Ul + Ur, B0>`
impl<Ul: Unsigned, Ur: Unsigned> Add<UInt<Ur, B0>> for UInt<Ul, B0>
where
    Ul: Add<Ur>,
{
    type Output = UInt<Sum<Ul, Ur>, B0>;
    #[inline]
    fn add(self, rhs: UInt<Ur, B0>) -> Self::Output {
        UInt {
            msb: self.msb + rhs.msb,
            lsb: B0,
        }
    }
}

/// `UInt<Ul, B0> + UInt<Ur, B1> = UInt<Ul + Ur, B1>`
impl<Ul: Unsigned, Ur: Unsigned> Add<UInt<Ur, B1>> for UInt<Ul, B0>
where
    Ul: Add<Ur>,
{
    type Output = UInt<Sum<Ul, Ur>, B1>;
    #[inline]
    fn add(self, rhs: UInt<Ur, B1>) -> Self::Output {
        UInt {
            msb: self.msb + rhs.msb,
            lsb: B1,
        }
    }
}

/// `UInt<Ul, B1> + UInt<Ur, B0> = UInt<Ul + Ur, B1>`
impl<Ul: Unsigned, Ur: Unsigned> Add<UInt<Ur, B0>> for UInt<Ul, B1>
where
    Ul: Add<Ur>,
{
    type Output = UInt<Sum<Ul, Ur>, B1>;
    #[inline]
    fn add(self, rhs: UInt<Ur, B0>) -> Self::Output {
        UInt {
            msb: self.msb + rhs.msb,
            lsb: B1,
        }
    }
}

/// `UInt<Ul, B1> + UInt<Ur, B1> = UInt<(Ul + Ur) + B1, B0>`
impl<Ul: Unsigned, Ur: Unsigned> Add<UInt<Ur, B1>> for UInt<Ul, B1>
where
    Ul: Add<Ur>,
    Sum<Ul, Ur>: Add<B1>,
{
    type Output = UInt<Add1<Sum<Ul, Ur>>, B0>;
    #[inline]
    fn add(self, rhs: UInt<Ur, B1>) -> Self::Output {
        UInt {
            msb: self.msb + rhs.msb + B1,
            lsb: B0,
        }
    }
}

// ---------------------------------------------------------------------------------------
// Subtracting bits from unsigned integers

/// `UTerm - B0 = Term`
impl Sub<B0> for UTerm {
    type Output = UTerm;
    #[inline]
    fn sub(self, _: B0) -> Self::Output {
        UTerm
    }
}

/// `UInt - B0 = UInt`
impl<U: Unsigned, B: Bit> Sub<B0> for UInt<U, B> {
    type Output = UInt<U, B>;
    #[inline]
    fn sub(self, _: B0) -> Self::Output {
        UInt::new()
    }
}

/// `UInt<U, B1> - B1 = UInt<U, B0>`
impl<U: Unsigned, B: Bit> Sub<B1> for UInt<UInt<U, B>, B1> {
    type Output = UInt<UInt<U, B>, B0>;
    #[inline]
    fn sub(self, _: B1) -> Self::Output {
        UInt::new()
    }
}

/// `UInt<UTerm, B1> - B1 = UTerm`
impl Sub<B1> for UInt<UTerm, B1> {
    type Output = UTerm;
    #[inline]
    fn sub(self, _: B1) -> Self::Output {
        UTerm
    }
}

/// `UInt<U, B0> - B1 = UInt<U - B1, B1>`
impl<U: Unsigned> Sub<B1> for UInt<U, B0>
where
    U: Sub<B1>,
    Sub1<U>: Unsigned,
{
    type Output = UInt<Sub1<U>, B1>;
    #[inline]
    fn sub(self, _: B1) -> Self::Output {
        UInt::new()
    }
}

// ---------------------------------------------------------------------------------------
// Subtracting unsigned integers

/// `UTerm - UTerm = UTerm`
impl Sub<UTerm> for UTerm {
    type Output = UTerm;
    #[inline]
    fn sub(self, _: UTerm) -> Self::Output {
        UTerm
    }
}

/// Subtracting unsigned integers. We just do our `PrivateSub` and then `Trim` the output.
impl<Ul: Unsigned, Bl: Bit, Ur: Unsigned> Sub<Ur> for UInt<Ul, Bl>
where
    UInt<Ul, Bl>: PrivateSub<Ur>,
    PrivateSubOut<UInt<Ul, Bl>, Ur>: Trim,
{
    type Output = TrimOut<PrivateSubOut<UInt<Ul, Bl>, Ur>>;
    #[inline]
    fn sub(self, rhs: Ur) -> Self::Output {
        self.private_sub(rhs).trim()
    }
}

/// `U - UTerm = U`
impl<U: Unsigned> PrivateSub<UTerm> for U {
    type Output = U;

    #[inline]
    fn private_sub(self, _: UTerm) -> Self::Output {
        self
    }
}

/// `UInt<Ul, B0> - UInt<Ur, B0> = UInt<Ul - Ur, B0>`
impl<Ul: Unsigned, Ur: Unsigned> PrivateSub<UInt<Ur, B0>> for UInt<Ul, B0>
where
    Ul: PrivateSub<Ur>,
{
    type Output = UInt<PrivateSubOut<Ul, Ur>, B0>;

    #[inline]
    fn private_sub(self, rhs: UInt<Ur, B0>) -> Self::Output {
        UInt {
            msb: self.msb.private_sub(rhs.msb),
            lsb: B0,
        }
    }
}

/// `UInt<Ul, B0> - UInt<Ur, B1> = UInt<(Ul - Ur) - B1, B1>`
impl<Ul: Unsigned, Ur: Unsigned> PrivateSub<UInt<Ur, B1>> for UInt<Ul, B0>
where
    Ul: PrivateSub<Ur>,
    PrivateSubOut<Ul, Ur>: Sub<B1>,
{
    type Output = UInt<Sub1<PrivateSubOut<Ul, Ur>>, B1>;

    #[inline]
    fn private_sub(self, rhs: UInt<Ur, B1>) -> Self::Output {
        UInt {
            msb: self.msb.private_sub(rhs.msb) - B1,
            lsb: B1,
        }
    }
}

/// `UInt<Ul, B1> - UInt<Ur, B0> = UInt<Ul - Ur, B1>`
impl<Ul: Unsigned, Ur: Unsigned> PrivateSub<UInt<Ur, B0>> for UInt<Ul, B1>
where
    Ul: PrivateSub<Ur>,
{
    type Output = UInt<PrivateSubOut<Ul, Ur>, B1>;

    #[inline]
    fn private_sub(self, rhs: UInt<Ur, B0>) -> Self::Output {
        UInt {
            msb: self.msb.private_sub(rhs.msb),
            lsb: B1,
        }
    }
}

/// `UInt<Ul, B1> - UInt<Ur, B1> = UInt<Ul - Ur, B0>`
impl<Ul: Unsigned, Ur: Unsigned> PrivateSub<UInt<Ur, B1>> for UInt<Ul, B1>
where
    Ul: PrivateSub<Ur>,
{
    type Output = UInt<PrivateSubOut<Ul, Ur>, B0>;

    #[inline]
    fn private_sub(self, rhs: UInt<Ur, B1>) -> Self::Output {
        UInt {
            msb: self.msb.private_sub(rhs.msb),
            lsb: B0,
        }
    }
}
