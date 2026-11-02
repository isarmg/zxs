use super::{assert_hint_contains, poll_next};
use futures::{FusedStream, Stream, StreamExt, stream};
use std::future::{pending, ready};
use std::pin::Pin;
use std::task::{Context, Poll};

struct PanicAtEnd {
    remaining: usize,
    unknown_upper: bool,
}
impl Stream for PanicAtEnd {
    type Item = u8;
    fn poll_next(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<u8>> {
        if self.remaining == 0 {
            std::panic::panic_any("original stream panic");
        }
        self.remaining -= 1;
        Poll::Ready(Some(1))
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (
            self.remaining,
            (!self.unknown_upper).then_some(self.remaining),
        )
    }
}

#[test]
fn catch_unwind_lower_allows_a_panic_before_remaining_items() {
    let mut source = stream::iter([Some(10), None, Some(11)])
        .map(|item| item.unwrap_or_else(|| std::panic::panic_any("original mapped panic")))
        .fuse()
        .catch_unwind();
    let hint = source.size_hint();
    assert!(matches!(
        poll_next(Pin::new(&mut source)),
        Poll::Ready(Some(Ok(10)))
    ));
    let Poll::Ready(Some(Err(payload))) = poll_next(Pin::new(&mut source)) else {
        panic!("expected original caught panic");
    };
    assert_eq!(
        payload.downcast_ref::<&str>(),
        Some(&"original mapped panic")
    );
    assert_eq!(source.size_hint(), (0, Some(0)));
    assert!(source.is_terminated());
    assert!(matches!(
        poll_next(Pin::new(&mut source)),
        Poll::Ready(None)
    ));
    assert_hint_contains(hint, 2);
}

#[test]
fn catch_unwind_upper_includes_a_panic_after_all_values() {
    for count in [0, 1, 2] {
        let mut source = PanicAtEnd {
            remaining: count,
            unknown_upper: false,
        }
        .catch_unwind();
        let hint = source.size_hint();
        for _ in 0..count {
            assert!(matches!(
                poll_next(Pin::new(&mut source)),
                Poll::Ready(Some(Ok(1)))
            ));
        }
        let Poll::Ready(Some(Err(payload))) = poll_next(Pin::new(&mut source)) else {
            panic!("expected terminal panic item");
        };
        assert_eq!(
            payload.downcast_ref::<&str>(),
            Some(&"original stream panic")
        );
        assert_hint_contains(hint, count + 1);
        assert_eq!(source.size_hint(), (0, Some(0)));
        assert!(matches!(
            poll_next(Pin::new(&mut source)),
            Poll::Ready(None)
        ));
    }
}

#[test]
fn catch_unwind_upper_overflow_and_unknown_remain_unknown() {
    for (remaining, unknown_upper) in [(usize::MAX, false), (2, true)] {
        let source = PanicAtEnd {
            remaining,
            unknown_upper,
        }
        .catch_unwind();
        assert_eq!(source.size_hint(), (1, None));
    }
}

#[test]
fn catch_unwind_regular_values_and_empty_source_keep_valid_bounds() {
    for values in [vec![], vec![1, 2]] {
        let count = values.len();
        let mut source = stream::iter(values).fuse().catch_unwind();
        let hint = source.size_hint();
        for _ in 0..count {
            assert!(matches!(
                poll_next(Pin::new(&mut source)),
                Poll::Ready(Some(Ok(_)))
            ));
        }
        assert!(matches!(
            poll_next(Pin::new(&mut source)),
            Poll::Ready(None)
        ));
        assert!(source.is_terminated());
        assert_hint_contains(hint, count);
    }
}

#[test]
fn take_until_ready_stop_allows_zero_items_and_preserves_result() {
    let mut source = stream::iter([1, 2, 3]).take_until(ready("reason"));
    let hint = source.size_hint();
    assert_eq!(poll_next(Pin::new(&mut source)), Poll::Ready(None));
    assert_hint_contains(hint, 0);
    assert!(source.is_stopped());
    assert!(source.is_terminated());
    assert_eq!(source.size_hint(), (0, Some(0)));
    assert_eq!(source.take_result(), Some("reason"));
    assert_eq!(source.take_result(), None);
    assert!(source.take_future().is_none());
    assert_eq!(poll_next(Pin::new(&mut source)), Poll::Ready(None));
}

#[test]
fn take_until_removed_stop_preserves_source_bounds_and_terminates_at_eof() {
    let inner = super::ImmediateHintSource {
        steps: [Poll::Ready(Some(1)), Poll::Ready(None)].into(),
    };
    let mut source = inner.take_until(pending::<()>());
    assert!(source.take_future().is_some());
    assert_eq!(source.size_hint(), (1, Some(1)));
    assert_eq!(poll_next(Pin::new(&mut source)), Poll::Ready(Some(1)));
    assert_eq!(poll_next(Pin::new(&mut source)), Poll::Ready(None));
    assert!(source.is_terminated());
    assert!(source.is_stopped());
    assert_eq!(source.size_hint(), (0, Some(0)));
    assert!(source.take_result().is_none());
    assert!(source.take_future().is_none());
    assert_eq!(poll_next(Pin::new(&mut source)), Poll::Ready(None));
}

#[test]
fn take_until_natural_eof_cancels_unresolved_stop_without_a_result() {
    let inner = super::ImmediateHintSource {
        steps: [Poll::Ready(Some(1)), Poll::Ready(None)].into(),
    };
    let mut source = inner.take_until(pending::<()>());
    assert_eq!(poll_next(Pin::new(&mut source)), Poll::Ready(Some(1)));
    assert_eq!(poll_next(Pin::new(&mut source)), Poll::Ready(None));
    assert!(source.is_terminated());
    assert!(source.take_result().is_none());
    assert!(source.take_future().is_none());
    assert_eq!(poll_next(Pin::new(&mut source)), Poll::Ready(None));
}

#[test]
fn take_until_stop_removed_after_pending_still_fuses_on_eof() {
    let inner = super::ImmediateHintSource {
        steps: [Poll::Pending, Poll::Ready(Some(1)), Poll::Ready(None)].into(),
    };
    let mut source = inner.take_until(pending::<()>());
    assert_eq!(poll_next(Pin::new(&mut source)), Poll::Pending);
    assert!(source.take_future().is_some());
    assert_eq!(source.size_hint(), (1, Some(1)));
    assert_eq!(poll_next(Pin::new(&mut source)), Poll::Ready(Some(1)));
    assert_eq!(poll_next(Pin::new(&mut source)), Poll::Ready(None));
    assert!(source.is_terminated());
    assert_eq!(poll_next(Pin::new(&mut source)), Poll::Ready(None));
}
