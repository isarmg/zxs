use super::{Zeroize, ZeroizeOnDrop, Zeroizing, zeroize_flat_type};
use core::{cell::Cell, mem::MaybeUninit, ptr};

#[test]
fn scalar_fixed_array_and_subslice_erase_the_selected_value() {
    let mut counter = u128::MAX;
    counter.zeroize();
    assert_eq!(counter, 0);

    let mut fixed = Zeroizing::new([0xa5_u8; 240]);
    fixed.zeroize();
    assert_eq!(*fixed, [0; 240]);

    let mut bytes = [0x37_u8; 25];
    bytes[2..23].zeroize();
    assert_eq!(bytes[..2], [0x37; 2]);
    assert_eq!(bytes[2..23], [0; 21]);
    assert_eq!(bytes[23..], [0x37; 2]);
}

#[test]
fn initialized_slots_and_flat_values_erase_all_their_bytes() {
    #[repr(C)]
    struct Flat {
        words: [u64; 2],
        tail: [u8; 8],
    }

    let mut slot = MaybeUninit::new([0x5a_u8; 19]);
    slot.zeroize();
    // SAFETY: the slot already held an initialized byte array, and erasure
    // writes a valid zero value for every byte.
    assert_eq!(unsafe { slot.assume_init() }, [0; 19]);

    let mut flat = Flat { words: [u64::MAX; 2], tail: [0xa5; 8] };
    // SAFETY: both fields are plain integer arrays with valid zero values
    // and no Drop. We provide exclusive access to the whole record.
    unsafe { zeroize_flat_type(ptr::from_mut(&mut flat)) };
    assert_eq!(flat.words, [0; 2]);
    assert_eq!(flat.tail, [0; 8]);
}

struct Probe<'a> {
    bytes: [u8; 4],
    before_replace: &'a Cell<Option<[u8; 4]>>,
    at_drop: &'a Cell<Option<[u8; 4]>>,
}

impl Clone for Probe<'_> {
    fn clone(&self) -> Self {
        Self { bytes: self.bytes, before_replace: self.before_replace, at_drop: self.at_drop }
    }

    fn clone_from(&mut self, source: &Self) {
        self.before_replace.set(Some(self.bytes));
        self.bytes = source.bytes;
    }
}

impl Zeroize for Probe<'_> {
    fn zeroize(&mut self) {
        self.bytes.zeroize();
    }
}

impl Drop for Probe<'_> {
    fn drop(&mut self) {
        self.at_drop.set(Some(self.bytes));
    }
}

fn requires_drop_erasure<T: ZeroizeOnDrop>(_: &T) {}

#[test]
fn wrapper_debug_and_replacement_preserve_erasure_before_drop() {
    let before_replace = Cell::new(None);
    let destination_dropped = Cell::new(None);
    let source_dropped = Cell::new(None);
    let source = Zeroizing::new(Probe {
        bytes: [2; 4],
        before_replace: &before_replace,
        at_drop: &source_dropped,
    });
    let mut dest = Zeroizing::new(Probe {
        bytes: [1; 4],
        before_replace: &before_replace,
        at_drop: &destination_dropped,
    });
    requires_drop_erasure(&dest);
    assert_eq!(format!("{dest:?}"), "Zeroizing { .. }");

    dest.clone_from(&source);
    assert_eq!(before_replace.get(), Some([0; 4]));
    assert_eq!(dest.bytes, [2; 4]);

    drop(dest);
    assert_eq!(destination_dropped.get(), Some([0; 4]));
    assert_eq!(source.bytes, [2; 4]);
    drop(source);
    assert_eq!(source_dropped.get(), Some([0; 4]));
}
