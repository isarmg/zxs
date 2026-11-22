// Adapted from Paho Lurie-Gregg and contributors (MIT OR Apache-2.0); see ORIGIN.md.
//! Type-level comparisons used by block and digest output bounds.

use super::{B0, B1, Bit, Compare, False, True, UInt, UTerm, Unsigned, sealed::Sealed};
use super::canonical::InternalMarker;

/// A potential output from `Cmp`, this is the type equivalent to the enum variant
/// `core::cmp::Ordering::Greater`.
#[derive(Eq, PartialEq, Ord, PartialOrd, Clone, Copy, Hash, Debug, Default)]
pub struct Greater;

/// A potential output from `Cmp`, this is the type equivalent to the enum variant
/// `core::cmp::Ordering::Less`.
#[derive(Eq, PartialEq, Ord, PartialOrd, Clone, Copy, Hash, Debug, Default)]
pub struct Less;

/// A potential output from `Cmp`, this is the type equivalent to the enum variant
/// `core::cmp::Ordering::Equal`.
#[derive(Eq, PartialEq, Ord, PartialOrd, Clone, Copy, Hash, Debug, Default)]
pub struct Equal;

pub trait Ord: Sealed {}

impl Ord for Greater {}
impl Ord for Less {}
impl Ord for Equal {}

pub trait Cmp<Rhs = Self> {
    /// The result of the comparison. It should only ever be one of `Greater`, `Less`, or `Equal`.
    type Output;

    #[doc(hidden)]
    fn compare<IM: InternalMarker>(&self, _: &Rhs) -> Self::Output;
}

pub trait PrivateCmp<Rhs, SoFar> {
    type Output;

    fn private_cmp(&self, _: &Rhs, _: SoFar) -> Self::Output;
}
pub type PrivateCmpOut<A, Rhs, SoFar> = <A as PrivateCmp<Rhs, SoFar>>::Output;

/// Zero == Zero
impl Cmp<UTerm> for UTerm {
    type Output = Equal;

    #[inline]
    fn compare<IM: InternalMarker>(&self, _: &UTerm) -> Self::Output {
        Equal
    }
}

/// Nonzero > Zero
impl<U: Unsigned, B: Bit> Cmp<UTerm> for UInt<U, B> {
    type Output = Greater;

    #[inline]
    fn compare<IM: InternalMarker>(&self, _: &UTerm) -> Self::Output {
        Greater
    }
}

/// Zero < Nonzero
impl<U: Unsigned, B: Bit> Cmp<UInt<U, B>> for UTerm {
    type Output = Less;

    #[inline]
    fn compare<IM: InternalMarker>(&self, _: &UInt<U, B>) -> Self::Output {
        Less
    }
}

/// `UInt<Ul, B0>` cmp with `UInt<Ur, B0>`: `SoFar` is `Equal`
impl<Ul: Unsigned, Ur: Unsigned> Cmp<UInt<Ur, B0>> for UInt<Ul, B0>
where
    Ul: PrivateCmp<Ur, Equal>,
{
    type Output = PrivateCmpOut<Ul, Ur, Equal>;

    #[inline]
    fn compare<IM: InternalMarker>(&self, rhs: &UInt<Ur, B0>) -> Self::Output {
        self.msb.private_cmp(&rhs.msb, Equal)
    }
}

/// `UInt<Ul, B1>` cmp with `UInt<Ur, B1>`: `SoFar` is `Equal`
impl<Ul: Unsigned, Ur: Unsigned> Cmp<UInt<Ur, B1>> for UInt<Ul, B1>
where
    Ul: PrivateCmp<Ur, Equal>,
{
    type Output = PrivateCmpOut<Ul, Ur, Equal>;

    #[inline]
    fn compare<IM: InternalMarker>(&self, rhs: &UInt<Ur, B1>) -> Self::Output {
        self.msb.private_cmp(&rhs.msb, Equal)
    }
}

/// `UInt<Ul, B0>` cmp with `UInt<Ur, B1>`: `SoFar` is `Less`
impl<Ul: Unsigned, Ur: Unsigned> Cmp<UInt<Ur, B1>> for UInt<Ul, B0>
where
    Ul: PrivateCmp<Ur, Less>,
{
    type Output = PrivateCmpOut<Ul, Ur, Less>;

    #[inline]
    fn compare<IM: InternalMarker>(&self, rhs: &UInt<Ur, B1>) -> Self::Output {
        self.msb.private_cmp(&rhs.msb, Less)
    }
}

/// `UInt<Ul, B1>` cmp with `UInt<Ur, B0>`: `SoFar` is `Greater`
impl<Ul: Unsigned, Ur: Unsigned> Cmp<UInt<Ur, B0>> for UInt<Ul, B1>
where
    Ul: PrivateCmp<Ur, Greater>,
{
    type Output = PrivateCmpOut<Ul, Ur, Greater>;

    #[inline]
    fn compare<IM: InternalMarker>(&self, rhs: &UInt<Ur, B0>) -> Self::Output {
        self.msb.private_cmp(&rhs.msb, Greater)
    }
}

/// Comparing non-terimal bits, with both having bit `B0`.
/// These are `Equal`, so we propagate `SoFar`.
impl<Ul, Ur, SoFar> PrivateCmp<UInt<Ur, B0>, SoFar> for UInt<Ul, B0>
where
    Ul: Unsigned,
    Ur: Unsigned,
    SoFar: Ord,
    Ul: PrivateCmp<Ur, SoFar>,
{
    type Output = PrivateCmpOut<Ul, Ur, SoFar>;

    #[inline]
    fn private_cmp(&self, rhs: &UInt<Ur, B0>, so_far: SoFar) -> Self::Output {
        self.msb.private_cmp(&rhs.msb, so_far)
    }
}

/// Comparing non-terimal bits, with both having bit `B1`.
/// These are `Equal`, so we propagate `SoFar`.
impl<Ul, Ur, SoFar> PrivateCmp<UInt<Ur, B1>, SoFar> for UInt<Ul, B1>
where
    Ul: Unsigned,
    Ur: Unsigned,
    SoFar: Ord,
    Ul: PrivateCmp<Ur, SoFar>,
{
    type Output = PrivateCmpOut<Ul, Ur, SoFar>;

    #[inline]
    fn private_cmp(&self, rhs: &UInt<Ur, B1>, so_far: SoFar) -> Self::Output {
        self.msb.private_cmp(&rhs.msb, so_far)
    }
}

/// Comparing non-terimal bits, with `Lhs` having bit `B0` and `Rhs` having bit `B1`.
/// `SoFar`, Lhs is `Less`.
impl<Ul, Ur, SoFar> PrivateCmp<UInt<Ur, B1>, SoFar> for UInt<Ul, B0>
where
    Ul: Unsigned,
    Ur: Unsigned,
    SoFar: Ord,
    Ul: PrivateCmp<Ur, Less>,
{
    type Output = PrivateCmpOut<Ul, Ur, Less>;

    #[inline]
    fn private_cmp(&self, rhs: &UInt<Ur, B1>, _: SoFar) -> Self::Output {
        self.msb.private_cmp(&rhs.msb, Less)
    }
}

/// Comparing non-terimal bits, with `Lhs` having bit `B1` and `Rhs` having bit `B0`.
/// `SoFar`, Lhs is `Greater`.
impl<Ul, Ur, SoFar> PrivateCmp<UInt<Ur, B0>, SoFar> for UInt<Ul, B1>
where
    Ul: Unsigned,
    Ur: Unsigned,
    SoFar: Ord,
    Ul: PrivateCmp<Ur, Greater>,
{
    type Output = PrivateCmpOut<Ul, Ur, Greater>;

    #[inline]
    fn private_cmp(&self, rhs: &UInt<Ur, B0>, _: SoFar) -> Self::Output {
        self.msb.private_cmp(&rhs.msb, Greater)
    }
}

/// Got to the end of just the `Lhs`. It's `Less`.
impl<U: Unsigned, B: Bit, SoFar: Ord> PrivateCmp<UInt<U, B>, SoFar> for UTerm {
    type Output = Less;

    #[inline]
    fn private_cmp(&self, _: &UInt<U, B>, _: SoFar) -> Self::Output {
        Less
    }
}

/// Got to the end of just the `Rhs`. `Lhs` is `Greater`.
impl<U: Unsigned, B: Bit, SoFar: Ord> PrivateCmp<UTerm, SoFar> for UInt<U, B> {
    type Output = Greater;

    #[inline]
    fn private_cmp(&self, _: &UTerm, _: SoFar) -> Self::Output {
        Greater
    }
}

/// Got to the end of both! Return `SoFar`
impl<SoFar: Ord> PrivateCmp<UTerm, SoFar> for UTerm {
    type Output = SoFar;

    #[inline]
    fn private_cmp(&self, _: &UTerm, so_far: SoFar) -> Self::Output {
        so_far
    }
}

/// A **type operator** that returns `True` if `Self < Rhs`, otherwise returns `False`.
pub trait IsLess<Rhs = Self> {
    /// The type representing either `True` or `False`
    type Output: Bit;
}

impl<A, B> IsLess<B> for A
where
    A: Cmp<B> + IsLessPrivate<B, Compare<A, B>>,
{
    type Output = <A as IsLessPrivate<B, Compare<A, B>>>::Output;

}

pub trait IsLessPrivate<Rhs, Cmp> {
    type Output: Bit;

}

impl<A, B> IsLessPrivate<B, Less> for A {
    type Output = True;

}
impl<A, B> IsLessPrivate<B, Equal> for A {
    type Output = False;

}
impl<A, B> IsLessPrivate<B, Greater> for A {
    type Output = False;

}

/// A **type operator** that returns `True` if `Self <= Rhs`, otherwise returns `False`.
pub trait IsLessOrEqual<Rhs = Self> {
    /// The type representing either `True` or `False`
    type Output: Bit;
}

impl<A, B> IsLessOrEqual<B> for A
where
    A: Cmp<B> + IsLessOrEqualPrivate<B, Compare<A, B>>,
{
    type Output = <A as IsLessOrEqualPrivate<B, Compare<A, B>>>::Output;

}

pub trait IsLessOrEqualPrivate<Rhs, Cmp> {
    type Output: Bit;

}

impl<A, B> IsLessOrEqualPrivate<B, Less> for A {
    type Output = True;

}
impl<A, B> IsLessOrEqualPrivate<B, Equal> for A {
    type Output = True;

}
impl<A, B> IsLessOrEqualPrivate<B, Greater> for A {
    type Output = False;

}
