// Adapted from Paho Lurie-Gregg and contributors (MIT OR Apache-2.0); see ORIGIN.md.
//! Unsigned quotient and its bit-level helpers.

use super::{
    Add1, B0, B1, Bit, Cmp, Compare, Diff, Equal, Greater, Less, Shleft, Sub1,
    U0, U1, UInt, UTerm, Unsigned,
    canonical::{Internal, InternalMarker, Trim, TrimOut},
};
use core::ops::{Add, Div, Shl, Sub};

/// Count the significant bits of this width.
pub trait Len {
    type Output: Unsigned;
    fn bit_len(&self) -> Self::Output;
}

type Length<T> = <T as Len>::Output;

/// Length of `UTerm` by itself is 0
impl Len for UTerm {
    type Output = U0;
    #[inline]
    fn bit_len(&self) -> Self::Output {
        UTerm
    }
}

/// Length of a bit is 1
impl<U: Unsigned, B: Bit> Len for UInt<U, B>
where
    U: Len,
    Length<U>: Add<B1>,
    Add1<Length<U>>: Unsigned,
{
    type Output = Add1<Length<U>>;
    #[inline]
    fn bit_len(&self) -> Self::Output {
        self.msb.bit_len() + B1
    }
}

/// Shifting `UTerm` by a 0 bit: `UTerm << B0 = UTerm`
impl Shl<B0> for UTerm {
    type Output = UTerm;
    #[inline]
    fn shl(self, _: B0) -> Self::Output {
        UTerm
    }
}

/// Shifting `UTerm` by a 1 bit: `UTerm << B1 = UTerm`
impl Shl<B1> for UTerm {
    type Output = UTerm;
    #[inline]
    fn shl(self, _: B1) -> Self::Output {
        UTerm
    }
}

/// Shifting left any unsigned by a zero bit: `U << B0 = U`
impl<U: Unsigned, B: Bit> Shl<B0> for UInt<U, B> {
    type Output = UInt<U, B>;
    #[inline]
    fn shl(self, _: B0) -> Self::Output {
        UInt::new()
    }
}

/// Shifting left a `UInt` by a one bit: `UInt<U, B> << B1 = UInt<UInt<U, B>, B0>`
impl<U: Unsigned, B: Bit> Shl<B1> for UInt<U, B> {
    type Output = UInt<UInt<U, B>, B0>;
    #[inline]
    fn shl(self, _: B1) -> Self::Output {
        UInt::new()
    }
}

/// Shifting left `UInt` by `UTerm`: `UInt<U, B> << UTerm = UInt<U, B>`
impl<U: Unsigned, B: Bit> Shl<UTerm> for UInt<U, B> {
    type Output = UInt<U, B>;
    #[inline]
    fn shl(self, _: UTerm) -> Self::Output {
        UInt::new()
    }
}

/// Shifting left `UTerm` by an unsigned integer: `UTerm << U = UTerm`
impl<U: Unsigned> Shl<U> for UTerm {
    type Output = UTerm;
    #[inline]
    fn shl(self, _: U) -> Self::Output {
        UTerm
    }
}

/// Shifting left `UInt` by `UInt`: `X << Y` = `UInt(X, B0) << (Y - 1)`
impl<U: Unsigned, B: Bit, Ur: Unsigned, Br: Bit> Shl<UInt<Ur, Br>> for UInt<U, B>
where
    UInt<Ur, Br>: Sub<B1>,
    UInt<UInt<U, B>, B0>: Shl<Sub1<UInt<Ur, Br>>>,
{
    type Output = Shleft<UInt<UInt<U, B>, B0>, Sub1<UInt<Ur, Br>>>;
    #[inline]
    fn shl(self, rhs: UInt<Ur, Br>) -> Self::Output {
        (UInt { msb: self, lsb: B0 }).shl(rhs.sub(B1))
    }
}
pub trait GetBit<I> {
    type Output;

    #[doc(hidden)]
    fn get_bit<IM: InternalMarker>(&self, _: &I) -> Self::Output;
}

pub type GetBitOut<N, I> = <N as GetBit<I>>::Output;

// Base case
impl<Un, Bn> GetBit<U0> for UInt<Un, Bn>
where
    Bn: Copy,
{
    type Output = Bn;

    #[inline]
    fn get_bit<IM: InternalMarker>(&self, _: &U0) -> Self::Output {
        self.lsb
    }
}

// Recursion case
impl<Un, Bn, Ui, Bi> GetBit<UInt<Ui, Bi>> for UInt<Un, Bn>
where
    UInt<Ui, Bi>: Copy + Sub<B1>,
    Un: GetBit<Sub1<UInt<Ui, Bi>>>,
{
    type Output = GetBitOut<Un, Sub1<UInt<Ui, Bi>>>;

    #[inline]
    fn get_bit<IM: InternalMarker>(&self, i: &UInt<Ui, Bi>) -> Self::Output {
        self.msb.get_bit::<Internal>(&(*i - B1))
    }
}

// Ran out of bits
impl<I> GetBit<I> for UTerm {
    type Output = B0;

    #[inline]
    fn get_bit<IM: InternalMarker>(&self, _: &I) -> Self::Output {
        B0
    }
}

/// A **type operator** that, when implemented for unsigned integer `N`, sets the bit at position
/// `I` to `B`.
pub trait SetBit<I, B> {
    type Output;

    #[doc(hidden)]
    fn set_bit<IM: InternalMarker>(self, _: I, _: B) -> Self::Output;
}
/// Alias for the result of calling `SetBit`: `SetBitOut<N, I, B> = <N as SetBit<I, B>>::Output`.
pub type SetBitOut<N, I, B> = <N as SetBit<I, B>>::Output;

// Call private one then trim it
impl<N, I, B> SetBit<I, B> for N
where
    N: PrivateSetBit<I, B>,
    PrivateSetBitOut<N, I, B>: Trim,
{
    type Output = TrimOut<PrivateSetBitOut<N, I, B>>;

    #[inline]
    fn set_bit<IM: InternalMarker>(self, i: I, b: B) -> Self::Output {
        self.private_set_bit(i, b).trim()
    }
}

// Base case
impl<Un, Bn, B> PrivateSetBit<U0, B> for UInt<Un, Bn> {
    type Output = UInt<Un, B>;

    #[inline]
    fn private_set_bit(self, _: U0, b: B) -> Self::Output {
        UInt {
            msb: self.msb,
            lsb: b,
        }
    }
}

// Recursion case
impl<Un, Bn, Ui, Bi, B> PrivateSetBit<UInt<Ui, Bi>, B> for UInt<Un, Bn>
where
    UInt<Ui, Bi>: Sub<B1>,
    Un: PrivateSetBit<Sub1<UInt<Ui, Bi>>, B>,
{
    type Output = UInt<PrivateSetBitOut<Un, Sub1<UInt<Ui, Bi>>, B>, Bn>;

    #[inline]
    fn private_set_bit(self, i: UInt<Ui, Bi>, b: B) -> Self::Output {
        UInt {
            msb: self.msb.private_set_bit(i - B1, b),
            lsb: self.lsb,
        }
    }
}

// Ran out of bits, setting B0
impl<I> PrivateSetBit<I, B0> for UTerm {
    type Output = UTerm;

    #[inline]
    fn private_set_bit(self, _: I, _: B0) -> Self::Output {
        UTerm
    }
}

// Ran out of bits, setting B1
impl<I> PrivateSetBit<I, B1> for UTerm
where
    U1: Shl<I>,
{
    type Output = Shleft<U1, I>;

    #[inline]
    fn private_set_bit(self, i: I, _: B1) -> Self::Output {
        <U1 as Shl<I>>::shl(U1::new(), i)
    }
}

// Set Bit
pub trait PrivateSetBit<I, B> {
    type Output;

    fn private_set_bit(self, _: I, _: B) -> Self::Output;
}
pub type PrivateSetBitOut<N, I, B> = <N as PrivateSetBit<I, B>>::Output;

// Div
pub trait PrivateDiv<N, D, Q, R, I> {
    type Quotient;
    type Remainder;

    fn private_div_quotient(self, _: N, _: D, _: Q, _: R, _: I) -> Self::Quotient;

}

pub type PrivateDivQuot<N, D, Q, R, I> = <() as PrivateDiv<N, D, Q, R, I>>::Quotient;
pub type PrivateDivRem<N, D, Q, R, I> = <() as PrivateDiv<N, D, Q, R, I>>::Remainder;

pub trait PrivateDivIf<N, D, Q, R, I, RcmpD> {
    type Quotient;
    type Remainder;

    fn private_div_if_quotient(self, _: N, _: D, _: Q, _: R, _: I, _: RcmpD) -> Self::Quotient;

}

pub type PrivateDivIfQuot<N, D, Q, R, I, RcmpD> =
    <() as PrivateDivIf<N, D, Q, R, I, RcmpD>>::Quotient;
pub type PrivateDivIfRem<N, D, Q, R, I, RcmpD> =
    <() as PrivateDivIf<N, D, Q, R, I, RcmpD>>::Remainder;

// 0 // N
impl<Ur: Unsigned, Br: Bit> Div<UInt<Ur, Br>> for UTerm {
    type Output = UTerm;
    #[inline]
    fn div(self, _: UInt<Ur, Br>) -> Self::Output {
        UTerm
    }
}

// M // N
impl<Ul: Unsigned, Bl: Bit, Ur: Unsigned, Br: Bit> Div<UInt<Ur, Br>> for UInt<Ul, Bl>
where
    UInt<Ul, Bl>: Len,
    Length<UInt<Ul, Bl>>: Sub<B1>,
    (): PrivateDiv<UInt<Ul, Bl>, UInt<Ur, Br>, U0, U0, Sub1<Length<UInt<Ul, Bl>>>>,
{
    type Output = PrivateDivQuot<UInt<Ul, Bl>, UInt<Ur, Br>, U0, U0, Sub1<Length<UInt<Ul, Bl>>>>;
    #[inline]
    fn div(self, rhs: UInt<Ur, Br>) -> Self::Output {
        ().private_div_quotient(self, rhs, U0::new(), U0::new(), self.bit_len().sub(B1))
    }
}

// R == 0: We set R = UInt<UTerm, N[i]>, then call out to PrivateDivIf for the if statement
impl<N, D, Q, I> PrivateDiv<N, D, Q, U0, I> for ()
where
    N: GetBit<I>,
    UInt<UTerm, GetBitOut<N, I>>: Trim,
    TrimOut<UInt<UTerm, GetBitOut<N, I>>>: Cmp<D>,
    (): PrivateDivIf<
        N,
        D,
        Q,
        TrimOut<UInt<UTerm, GetBitOut<N, I>>>,
        I,
        Compare<TrimOut<UInt<UTerm, GetBitOut<N, I>>>, D>,
    >,
{
    type Quotient = PrivateDivIfQuot<
        N,
        D,
        Q,
        TrimOut<UInt<UTerm, GetBitOut<N, I>>>,
        I,
        Compare<TrimOut<UInt<UTerm, GetBitOut<N, I>>>, D>,
    >;
    type Remainder = PrivateDivIfRem<
        N,
        D,
        Q,
        TrimOut<UInt<UTerm, GetBitOut<N, I>>>,
        I,
        Compare<TrimOut<UInt<UTerm, GetBitOut<N, I>>>, D>,
    >;

    #[inline]
    fn private_div_quotient(self, n: N, d: D, q: Q, _: U0, i: I) -> Self::Quotient
where {
        let r = (UInt {
            msb: UTerm,
            lsb: n.get_bit::<Internal>(&i),
        })
        .trim();
        let r_cmp_d = r.compare::<Internal>(&d);
        ().private_div_if_quotient(n, d, q, r, i, r_cmp_d)
    }

}

// R > 0: We perform R <<= 1 and R[0] = N[i], then call out to PrivateDivIf for the if statement
impl<N, D, Q, Ur, Br, I> PrivateDiv<N, D, Q, UInt<Ur, Br>, I> for ()
where
    N: GetBit<I>,
    UInt<UInt<Ur, Br>, GetBitOut<N, I>>: Cmp<D>,
    (): PrivateDivIf<
        N,
        D,
        Q,
        UInt<UInt<Ur, Br>, GetBitOut<N, I>>,
        I,
        Compare<UInt<UInt<Ur, Br>, GetBitOut<N, I>>, D>,
    >,
{
    type Quotient = PrivateDivIfQuot<
        N,
        D,
        Q,
        UInt<UInt<Ur, Br>, GetBitOut<N, I>>,
        I,
        Compare<UInt<UInt<Ur, Br>, GetBitOut<N, I>>, D>,
    >;
    type Remainder = PrivateDivIfRem<
        N,
        D,
        Q,
        UInt<UInt<Ur, Br>, GetBitOut<N, I>>,
        I,
        Compare<UInt<UInt<Ur, Br>, GetBitOut<N, I>>, D>,
    >;

    #[inline]
    fn private_div_quotient(self, n: N, d: D, q: Q, r: UInt<Ur, Br>, i: I) -> Self::Quotient {
        let r = UInt {
            msb: r,
            lsb: n.get_bit::<Internal>(&i),
        };
        let r_cmp_d = r.compare::<Internal>(&d);
        ().private_div_if_quotient(n, d, q, r, i, r_cmp_d)
    }

}

// -----------------------------------------
// PrivateDivIf

// R < D, I > 0, we do nothing and recurse
impl<N, D, Q, R, Ui, Bi> PrivateDivIf<N, D, Q, R, UInt<Ui, Bi>, Less> for ()
where
    UInt<Ui, Bi>: Sub<B1>,
    (): PrivateDiv<N, D, Q, R, Sub1<UInt<Ui, Bi>>>,
{
    type Quotient = PrivateDivQuot<N, D, Q, R, Sub1<UInt<Ui, Bi>>>;
    type Remainder = PrivateDivRem<N, D, Q, R, Sub1<UInt<Ui, Bi>>>;

    #[inline]
    fn private_div_if_quotient(
        self,
        n: N,
        d: D,
        q: Q,
        r: R,
        i: UInt<Ui, Bi>,
        _: Less,
    ) -> Self::Quotient
where {
        ().private_div_quotient(n, d, q, r, i - B1)
    }

}

// R == D, I > 0, we set R = 0, Q[I] = 1 and recurse
impl<N, D, Q, R, Ui, Bi> PrivateDivIf<N, D, Q, R, UInt<Ui, Bi>, Equal> for ()
where
    UInt<Ui, Bi>: Copy + Sub<B1>,
    Q: SetBit<UInt<Ui, Bi>, B1>,
    (): PrivateDiv<N, D, SetBitOut<Q, UInt<Ui, Bi>, B1>, U0, Sub1<UInt<Ui, Bi>>>,
{
    type Quotient = PrivateDivQuot<N, D, SetBitOut<Q, UInt<Ui, Bi>, B1>, U0, Sub1<UInt<Ui, Bi>>>;
    type Remainder = PrivateDivRem<N, D, SetBitOut<Q, UInt<Ui, Bi>, B1>, U0, Sub1<UInt<Ui, Bi>>>;

    #[inline]
    fn private_div_if_quotient(
        self,
        n: N,
        d: D,
        q: Q,
        _: R,
        i: UInt<Ui, Bi>,
        _: Equal,
    ) -> Self::Quotient
where {
        ().private_div_quotient(n, d, q.set_bit::<Internal>(i, B1), U0::new(), i - B1)
    }

}

// R > D, I > 0, we set R -= D, Q[I] = 1 and recurse
impl<N, D, Q, R, Ui, Bi> PrivateDivIf<N, D, Q, R, UInt<Ui, Bi>, Greater> for ()
where
    D: Copy,
    UInt<Ui, Bi>: Copy + Sub<B1>,
    R: Sub<D>,
    Q: SetBit<UInt<Ui, Bi>, B1>,
    (): PrivateDiv<N, D, SetBitOut<Q, UInt<Ui, Bi>, B1>, Diff<R, D>, Sub1<UInt<Ui, Bi>>>,
{
    type Quotient =
        PrivateDivQuot<N, D, SetBitOut<Q, UInt<Ui, Bi>, B1>, Diff<R, D>, Sub1<UInt<Ui, Bi>>>;
    type Remainder =
        PrivateDivRem<N, D, SetBitOut<Q, UInt<Ui, Bi>, B1>, Diff<R, D>, Sub1<UInt<Ui, Bi>>>;

    #[inline]
    fn private_div_if_quotient(
        self,
        n: N,
        d: D,
        q: Q,
        r: R,
        i: UInt<Ui, Bi>,
        _: Greater,
    ) -> Self::Quotient
where {
        ().private_div_quotient(n, d, q.set_bit::<Internal>(i, B1), r - d, i - B1)
    }

}

// R < D, I == 0: we do nothing, and return
impl<N, D, Q, R> PrivateDivIf<N, D, Q, R, U0, Less> for () {
    type Quotient = Q;
    type Remainder = R;

    #[inline]
    fn private_div_if_quotient(self, _: N, _: D, q: Q, _: R, _: U0, _: Less) -> Self::Quotient {
        q
    }

}

// R == D, I == 0: we set R = 0, Q[I] = 1, and return
impl<N, D, Q, R> PrivateDivIf<N, D, Q, R, U0, Equal> for ()
where
    Q: SetBit<U0, B1>,
{
    type Quotient = SetBitOut<Q, U0, B1>;
    type Remainder = U0;

    #[inline]
    fn private_div_if_quotient(self, _: N, _: D, q: Q, _: R, i: U0, _: Equal) -> Self::Quotient {
        q.set_bit::<Internal>(i, B1)
    }

}

// R > D, I == 0: We set R -= D, Q[I] = 1, and return
impl<N, D, Q, R> PrivateDivIf<N, D, Q, R, U0, Greater> for ()
where
    R: Sub<D>,
    Q: SetBit<U0, B1>,
{
    type Quotient = SetBitOut<Q, U0, B1>;
    type Remainder = Diff<R, D>;

    #[inline]
    fn private_div_if_quotient(self, _: N, _: D, q: Q, _: R, i: U0, _: Greater) -> Self::Quotient {
        q.set_bit::<Internal>(i, B1)
    }

}
