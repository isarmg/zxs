use futures::{FusedStream, Stream, StreamExt, TryStreamExt, stream};
use std::future::{poll_fn, ready};
use std::pin::{Pin, pin};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::task::{Context, Poll};

#[path = "foundation-stream-select.rs"]
mod stream_select;

fn poll_next<S: Stream + ?Sized>(stream: Pin<&mut S>) -> Poll<Option<S::Item>> {
    stream.poll_next(&mut Context::from_waker(futures::task::noop_waker_ref()))
}

fn assert_hint_contains(hint: (usize, Option<usize>), remaining: usize) {
    assert!(
        hint.0 <= remaining,
        "lower bound {} exceeds {remaining}",
        hint.0
    );
    assert!(hint.1.is_none_or(|upper| remaining <= upper));
}

#[test]
fn zip_reports_termination_when_left_ends() {
    let mut zipped = pin!(stream::empty::<u8>().zip(stream::pending::<u8>()));
    assert_eq!(poll_next(zipped.as_mut()), Poll::Ready(None));
    assert!(zipped.is_terminated());
    assert_eq!(poll_next(zipped.as_mut()), Poll::Ready(None));
}

#[test]
fn zip_reports_termination_when_right_ends() {
    let mut zipped = pin!(stream::pending::<u8>().zip(stream::empty::<u8>()));
    assert_eq!(poll_next(zipped.as_mut()), Poll::Ready(None));
    assert!(zipped.is_terminated());
}

#[test]
fn zip_keeps_a_queued_item_until_its_partner_arrives() {
    let mut calls = 0;
    let partner = stream::poll_fn(move |cx| {
        calls += 1;
        match calls {
            1 => {
                cx.waker().wake_by_ref();
                Poll::Pending
            }
            2 => Poll::Ready(Some(2)),
            _ => Poll::Ready(None),
        }
    });
    let mut zipped = pin!(stream::iter([1]).zip(partner));
    assert_eq!(poll_next(zipped.as_mut()), Poll::Pending);
    assert!(!zipped.is_terminated());
    assert_eq!(poll_next(zipped.as_mut()), Poll::Ready(Some((1, 2))));
    assert_eq!(poll_next(zipped.as_mut()), Poll::Ready(None));
    assert!(zipped.is_terminated());
}

#[test]
fn scan_hint_allows_the_callback_to_stop_early() {
    let mut scanned = pin!(stream::iter([1]).scan((), |_, _| ready(None::<u8>)));
    let hint = scanned.size_hint();
    assert_eq!(poll_next(scanned.as_mut()), Poll::Ready(None));
    assert_hint_contains(hint, 0);
    assert_eq!(scanned.size_hint(), (0, Some(0)));
}

#[test]
fn scan_hint_includes_the_pending_callback_output() {
    let scanned = stream::iter([7]).scan((), |_, item| {
        let mut ready = false;
        poll_fn(move |cx| {
            if ready {
                Poll::Ready(Some(item))
            } else {
                ready = true;
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        })
    });
    let mut scanned = pin!(scanned);
    assert_eq!(poll_next(scanned.as_mut()), Poll::Pending);
    let hint = scanned.size_hint();
    assert_eq!(poll_next(scanned.as_mut()), Poll::Ready(Some(7)));
    assert_eq!(poll_next(scanned.as_mut()), Poll::Ready(None));
    assert_hint_contains(hint, 1);
}

#[test]
fn try_skip_while_terminates_after_a_predicate_error_and_eof() {
    let source = stream::iter([Ok::<_, &str>(1)]).fuse();
    let mut skipped = pin!(source.try_skip_while(|_| ready(Err::<bool, _>("predicate"))));
    assert_eq!(
        poll_next(skipped.as_mut()),
        Poll::Ready(Some(Err("predicate")))
    );
    assert_eq!(poll_next(skipped.as_mut()), Poll::Ready(None));
    assert!(skipped.is_terminated());
}

#[test]
fn try_take_while_terminates_after_a_predicate_error_and_eof() {
    let source = stream::iter([Ok::<_, &str>(1)]).fuse();
    let mut taken = pin!(source.try_take_while(|_| ready(Err::<bool, _>("predicate"))));
    assert_eq!(
        poll_next(taken.as_mut()),
        Poll::Ready(Some(Err("predicate")))
    );
    assert_eq!(poll_next(taken.as_mut()), Poll::Ready(None));
    assert!(taken.is_terminated());
}

#[derive(Debug)]
struct DropNotice(Arc<AtomicUsize>);

impl Drop for DropNotice {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn predicate_errors_release_their_consumed_items_promptly() {
    let dropped = Arc::new(AtomicUsize::new(0));
    let source = stream::iter([Ok::<_, &str>(DropNotice(Arc::clone(&dropped)))]).fuse();
    let mut skipped = pin!(source.try_skip_while(|_| ready(Err::<bool, _>("predicate"))));
    assert!(matches!(
        poll_next(skipped.as_mut()),
        Poll::Ready(Some(Err("predicate")))
    ));
    assert_eq!(dropped.load(Ordering::SeqCst), 1);

    let dropped = Arc::new(AtomicUsize::new(0));
    let source = stream::iter([Ok::<_, &str>(DropNotice(Arc::clone(&dropped)))]).fuse();
    let mut taken = pin!(source.try_take_while(|_| ready(Err::<bool, _>("predicate"))));
    assert!(matches!(
        poll_next(taken.as_mut()),
        Poll::Ready(Some(Err("predicate")))
    ));
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
}

#[test]
fn predicate_errors_preserve_later_items_and_error_values() {
    let source = stream::iter([Ok::<_, &str>(1), Ok(2), Ok(3)]).fuse();
    let mut skipped = pin!(source.try_skip_while(|&item| {
        ready(if item == 1 {
            Err("predicate")
        } else {
            Ok(false)
        })
    }));
    assert_eq!(
        poll_next(skipped.as_mut()),
        Poll::Ready(Some(Err("predicate")))
    );
    assert_eq!(poll_next(skipped.as_mut()), Poll::Ready(Some(Ok(2))));
    assert_eq!(poll_next(skipped.as_mut()), Poll::Ready(Some(Ok(3))));
    assert_eq!(poll_next(skipped.as_mut()), Poll::Ready(None));

    let source = stream::iter([Ok::<_, &str>(1), Ok(2), Ok(3)]).fuse();
    let mut taken = pin!(source.try_take_while(|&item| {
        ready(if item == 1 {
            Err("predicate")
        } else {
            Ok(true)
        })
    }));
    assert_eq!(
        poll_next(taken.as_mut()),
        Poll::Ready(Some(Err("predicate")))
    );
    assert_eq!(poll_next(taken.as_mut()), Poll::Ready(Some(Ok(2))));
    assert_eq!(poll_next(taken.as_mut()), Poll::Ready(Some(Ok(3))));
    assert_eq!(poll_next(taken.as_mut()), Poll::Ready(None));
}
