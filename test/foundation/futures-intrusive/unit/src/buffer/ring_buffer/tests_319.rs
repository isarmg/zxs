#[cfg(test)]
#[cfg(feature = "alloc")]
mod tests {
    use super::*;
    use crate::buffer::ring_buffer::if_alloc::FixedHeapBuf;

    fn test_ring_buf<Buf: RingBuf<Item = u32>>(mut buf: Buf) {
        assert_eq!(5, buf.capacity());
        assert_eq!(0, buf.len());
        assert_eq!(true, buf.is_empty());
        assert_eq!(true, buf.can_push());

        buf.push(1);
        buf.push(2);
        buf.push(3);
        assert_eq!(5, buf.capacity());
        assert_eq!(3, buf.len());
        assert_eq!(false, buf.is_empty());
        assert_eq!(true, buf.can_push());

        assert_eq!(1, buf.pop());
        assert_eq!(2, buf.pop());
        assert_eq!(1, buf.len());
        assert_eq!(false, buf.is_empty());
        assert_eq!(3, buf.pop());
        assert_eq!(0, buf.len());
        assert_eq!(true, buf.is_empty());

        for (i, val) in [4, 5, 6, 7, 8].iter().enumerate() {
            buf.push(*val);
            assert_eq!(i + 1, buf.len());
            assert_eq!(i != 4, buf.can_push());
            assert_eq!(false, buf.is_empty());
        }

        for (i, val) in [4, 5, 6, 7, 8].iter().enumerate() {
            assert_eq!(*val, buf.pop());
            assert_eq!(4 - i, buf.len());
            assert_eq!(true, buf.can_push());
            assert_eq!(i == 4, buf.is_empty());
        }
    }

    #[test]
    fn test_array_ring_buf() {
        let buf = ArrayBuf::<u32, [u32; 5]>::new();
        test_ring_buf(buf);
    }

    #[test]
    fn test_heap_ring_buf() {
        let buf = FixedHeapBuf::<u32>::with_capacity(5);
        test_ring_buf(buf);
    }

    #[test]
    fn test_growing_ring_buf() {
        let buf = GrowingHeapBuf::<u32>::with_capacity(5);
        test_ring_buf(buf);
    }

    #[test]
    #[should_panic]
    fn growing_ring_buf_rejects_full_push() {
        let mut buf = GrowingHeapBuf::with_capacity(2);
        buf.push(1);
        buf.push(2);
        buf.push(3);
    }

    #[test]
    #[should_panic]
    fn growing_ring_buf_rejects_zero_capacity_push() {
        let mut buf = GrowingHeapBuf::with_capacity(0);
        buf.push(1);
    }

    #[test]
    #[should_panic]
    fn growing_ring_buf_rejects_default_capacity_push() {
        let mut buf = GrowingHeapBuf::new();
        buf.push(1);
    }

    #[cfg(feature = "std")]
    #[test]
    fn growing_ring_buf_rejection_preserves_items_and_releases_input() {
        use std::panic::{catch_unwind, AssertUnwindSafe};
        use std::rc::Rc;

        let live = Rc::new(());
        let mut buf = GrowingHeapBuf::with_capacity(2);
        buf.push((1, Rc::clone(&live)));
        buf.push((2, Rc::clone(&live)));

        let rejected = catch_unwind(AssertUnwindSafe(|| {
            buf.push((3, Rc::clone(&live)));
        }));
        assert!(rejected.is_err());
        assert_eq!(Rc::strong_count(&live), 3);
        assert_eq!(buf.capacity(), 2);
        assert_eq!(buf.len(), 2);
        assert!(!buf.can_push());

        assert_eq!(buf.pop().0, 1);
        assert!(buf.can_push());
        buf.push((4, Rc::clone(&live)));
        assert!(!buf.can_push());
        assert_eq!(buf.pop().0, 2);
        assert_eq!(buf.pop().0, 4);
        assert!(buf.is_empty());
        assert!(buf.can_push());
        assert_eq!(Rc::strong_count(&live), 1);
    }
}
