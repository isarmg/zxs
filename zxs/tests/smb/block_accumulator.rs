use super::{BlockBuffer, Eager, Lazy};
use typenum::U4;
use crate::smb::memory_erasure::{Zeroize, ZeroizeOnDrop};

type EagerBuffer = BlockBuffer<U4, Eager>;
type LazyBuffer = BlockBuffer<U4, Lazy>;

#[test]
fn eager_and_lazy_keep_distinct_terminal_block_rules() {
    let mut eager = EagerBuffer::default();
    let mut lazy = LazyBuffer::default();
    let mut eager_blocks = Vec::new();
    let mut lazy_blocks = Vec::new();

    for data in [b"abcd".as_slice(), b"", b"e"] {
        eager.digest_blocks(data, |blocks| eager_blocks.extend(blocks.iter().map(|b| b.0)));
        lazy.digest_blocks(data, |blocks| lazy_blocks.extend(blocks.iter().map(|b| b.0)));
        if data.len() != 1 {
            assert_eq!(eager_blocks, [*b"abcd"]);
            assert_eq!(eager.get_data(), b"");
            assert_eq!(lazy_blocks, Vec::<[u8; 4]>::new());
            assert_eq!(lazy.get_data(), b"abcd");
        }
    }

    assert_eq!(eager_blocks, [*b"abcd"]);
    assert_eq!(lazy_blocks, [*b"abcd"]);
    assert_eq!(eager.get_data(), b"e");
    assert_eq!(lazy.get_data(), b"e");

    eager.digest_blocks(b"fghijkl", |blocks| eager_blocks.extend(blocks.iter().map(|b| b.0)));
    lazy.digest_blocks(b"fghijkl", |blocks| lazy_blocks.extend(blocks.iter().map(|b| b.0)));
    assert_eq!(eager_blocks, [*b"abcd", *b"efgh", *b"ijkl"]);
    assert_eq!(lazy_blocks, [*b"abcd", *b"efgh"]);
    assert_eq!(lazy.pad_with_zeros().0, *b"ijkl");
    assert_eq!(lazy.get_pos(), 0);
}

#[test]
fn checkpoints_preserve_size_and_reject_invalid_cursor_or_tail() {
    let mut eager = EagerBuffer::default();
    eager.digest_blocks(b"ab", |_| unreachable!("partial input"));
    let mut eager_checkpoint = eager.serialize();
    assert_eq!(eager_checkpoint.0, [2, b'a', b'b', 0]);
    assert_eq!(EagerBuffer::deserialize(&eager_checkpoint).unwrap().get_data(), b"ab");
    eager_checkpoint[3] = 1;
    assert!(EagerBuffer::deserialize(&eager_checkpoint).is_err());
    eager_checkpoint[3] = 0;
    eager_checkpoint[0] = 4;
    assert!(EagerBuffer::deserialize(&eager_checkpoint).is_err());

    let mut lazy = LazyBuffer::default();
    lazy.digest_blocks(b"abcd", |_| unreachable!("the final full block stays pending"));
    let mut lazy_checkpoint = lazy.serialize();
    assert_eq!(lazy_checkpoint.0, [4, b'a', b'b', b'c', b'd']);
    assert_eq!(LazyBuffer::deserialize(&lazy_checkpoint).unwrap().get_data(), b"abcd");
    lazy_checkpoint[0] = 5;
    assert!(LazyBuffer::deserialize(&lazy_checkpoint).is_err());

    lazy.zeroize();
    assert_eq!(lazy.serialize().0, [0; 5]);
}

#[test]
fn cloning_keeps_a_separate_pending_block_and_zeroization_resets_it() {
    fn requires_drop_erasure<T: ZeroizeOnDrop>(_: &T) {}
    let mut original = EagerBuffer::default();
    original.digest_blocks(b"abc", |_| unreachable!("partial input"));
    let mut snapshot = original.clone();
    requires_drop_erasure(&snapshot);
    original.reset();
    assert_eq!(original.get_data(), b"");
    assert_eq!(snapshot.get_data(), b"abc");

    snapshot.zeroize();
    assert_eq!(snapshot.serialize().0, [0; 4]);
}
