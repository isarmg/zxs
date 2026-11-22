use super::*;
use core::marker::PhantomData;

fn same_type<T>(_: PhantomData<T>, _: PhantomData<T>) {}

#[test]
fn typed_length_arithmetic_keeps_carry_borrow_and_snapshots() {
    same_type(PhantomData::<Sum<U255, U1>>, PhantomData::<U256>);
    same_type(PhantomData::<Diff<U256, U1>>, PhantomData::<U255>);
    same_type(PhantomData::<Diff<U64, U64>>, PhantomData::<U0>);
    same_type(PhantomData::<Sum<U40, U64>>, PhantomData::<U104>);
    same_type(PhantomData::<Sum<U80, U128>>, PhantomData::<U208>);
    same_type(PhantomData::<Sum<U24, U64>>, PhantomData::<U88>);
}

#[test]
fn typed_bounds_keep_the_exact_ordering_boundaries() {
    same_type(PhantomData::<<U1 as IsLess<U256>>::Output>, PhantomData::<True>);
    same_type(PhantomData::<<U255 as IsLess<U256>>::Output>, PhantomData::<True>);
    same_type(PhantomData::<<U256 as IsLess<U256>>::Output>, PhantomData::<False>);
    same_type(PhantomData::<<U257 as IsLess<U256>>::Output>, PhantomData::<False>);
    same_type(PhantomData::<<U31 as IsLessOrEqual<U32>>::Output>, PhantomData::<True>);
    same_type(PhantomData::<<U32 as IsLessOrEqual<U32>>::Output>, PhantomData::<True>);
    same_type(PhantomData::<<U33 as IsLessOrEqual<U32>>::Output>, PhantomData::<False>);
}

#[test]
fn unsigned_division_keeps_generic_and_simd_group_lengths() {
    same_type(PhantomData::<Quot<U0, U5>>, PhantomData::<U0>);
    same_type(PhantomData::<Quot<U1, U4>>, PhantomData::<U0>);
    same_type(PhantomData::<Quot<U9, U3>>, PhantomData::<U3>);
    same_type(PhantomData::<Quot<U30, U2>>, PhantomData::<U15>);
    same_type(PhantomData::<Quot<U31, U2>>, PhantomData::<U15>);
    same_type(PhantomData::<Quot<U64, U4>>, PhantomData::<U16>);
    same_type(PhantomData::<Quot<U144, U13>>, PhantomData::<U11>);
    assert_eq!(<Quot<U8192, U5> as Unsigned>::USIZE, 1638);
}
