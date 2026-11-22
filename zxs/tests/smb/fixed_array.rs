use super::{Array, sizes::supported_sizes};
use core::{
    cell::Cell,
    mem::{align_of_val, size_of, size_of_val},
    ptr,
};
use typenum::{U2, U3, U4, U5, Unsigned};
use crate::smb::memory_erasure::{Zeroize, ZeroizeOnDrop, Zeroizing};

macro_rules! verify_array_sizes {
    ($($len:expr => $ty:ident),+ $(,)?) => {
        $(
            assert_eq!($len, typenum::consts::$ty::USIZE);
            assert_eq!($len, size_of::<Array<u8, typenum::consts::$ty>>());
        )+
    };
}

#[test]
fn active_widths_match_core_representation() {
    supported_sizes!(verify_array_sizes);
}

#[test]
fn checked_borrows_keep_layout_and_the_original_address() {
    let mut input = [11u32, 22, 33, 44];
    let addr = input.as_ptr();
    let shared = <&Array<u32, U4>>::try_from(input.as_slice()).unwrap();
    assert_eq!(size_of_val(shared), size_of_val(&input));
    assert_eq!(align_of_val(shared), align_of_val(&input));
    assert!(ptr::eq(shared.as_ptr(), addr));
    let core: &[u32; 4] = shared.into();
    assert!(ptr::eq(ptr::from_ref(core), ptr::from_ref(&input)));
    assert!(<&Array<u32, U4>>::try_from(&input[..3]).is_err());
    assert!(<&Array<u32, U3>>::try_from(input.as_slice()).is_err());
    assert!(<&mut Array<u32, U4>>::try_from(&mut input[..3]).is_err());

    let exclusive = <&mut Array<u32, U4>>::try_from(input.as_mut_slice()).unwrap();
    assert!(ptr::eq(exclusive.as_ptr(), addr));
    exclusive[2] = 55;
    assert_eq!(input, [11, 22, 55, 44]);
}

#[test]
fn typed_splits_and_core_batches_keep_the_same_elements() {
    let serialized = Array::<u32, U5>::from([1, 2, 3, 4, 5]);
    let (head, tail) = serialized.split_ref::<U2>();
    assert!(ptr::eq(head.as_ptr(), serialized.as_ptr()));
    assert!(ptr::eq(tail.as_ptr(), serialized[2..].as_ptr()));
    assert_eq!(head.0, [1, 2]);
    assert_eq!(tail.0, [3, 4, 5]);

    let (head, tail) = serialized.split::<U2>();
    assert_eq!(head.concat(tail).0, [1, 2, 3, 4, 5]);

    let input = [1u32, 2, 3, 4, 5, 6, 7, 8, 9];
    let (blocks, tail) = Array::<u32, U4>::slice_as_chunks(&input);
    let core = Array::cast_slice_to_core(blocks);
    assert!(ptr::eq(core.as_ptr().cast(), input.as_ptr()));
    assert!(ptr::eq(ptr::from_ref(tail), ptr::from_ref(&input[8..])));
    assert_eq!(core, &[[1, 2, 3, 4], [5, 6, 7, 8]]);
    assert_eq!(tail, &[9]);
}

struct DropProbe<'a>(&'a Cell<usize>);

impl Drop for DropProbe<'_> {
    fn drop(&mut self) {
        self.0.set(self.0.get() + 1);
    }
}

#[test]
fn fallible_initialization_drops_only_the_initialized_items() {
    let drops = Cell::new(0);
    let result = Array::<DropProbe<'_>, U4>::try_from_fn(|i| {
        if i == 2 {
            Err("stop")
        } else {
            Ok(DropProbe(&drops))
        }
    });
    assert!(matches!(result, Err("stop")));
    assert_eq!(drops.get(), 2);

    let done = Array::<DropProbe<'_>, U4>::from_fn(|_| DropProbe(&drops));
    assert_eq!(drops.get(), 2);
    drop(done);
    assert_eq!(drops.get(), 6);
}

fn requires_erasure_on_drop<T: ZeroizeOnDrop>(_: &T) {}

#[test]
fn nested_arrays_use_the_same_element_erasure_and_drop_marker() {
    let mut blocks = Array::<Array<u8, U4>, U3>::from([
        Array([1, 2, 3, 4]),
        Array([5, 6, 7, 8]),
        Array([9, 10, 11, 12]),
    ]);
    blocks.zeroize();
    assert_eq!(blocks.0, [Array([0; 4]); 3]);

    let markers = Array::<Zeroizing<[u8; 4]>, U2>::from_fn(|_| Zeroizing::new([0x35; 4]));
    requires_erasure_on_drop(&markers);
}
