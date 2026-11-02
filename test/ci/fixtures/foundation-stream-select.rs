// Public StreamSelect termination, scheduling and size-bound regressions.
use futures::stream::Stream;
use std::cell::Cell;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll, Wake, Waker};

#[derive(Default)]
struct WakeCount(AtomicUsize);

impl Wake for WakeCount {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

fn poll_once<S: Stream + Unpin>(stream: &mut S, waker: &Waker) -> Poll<Option<S::Item>> {
    Pin::new(stream).poll_next(&mut Context::from_waker(waker))
}

#[derive(Debug, Default)]
struct EndState {
    first_polls: Cell<usize>,
    polls_after_eof: Cell<usize>,
}

#[derive(Debug)]
enum Phase {
    New,
    Yielded,
    Ended,
}

#[derive(Debug)]
struct EndBeforeItem {
    state: Rc<EndState>,
    phase: Phase,
}

impl EndBeforeItem {
    fn new(state: &Rc<EndState>) -> Self {
        Self {
            state: Rc::clone(state),
            phase: Phase::New,
        }
    }
}

impl Stream for EndBeforeItem {
    type Item = u8;

    fn poll_next(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<u8>> {
        match self.phase {
            Phase::Ended => {
                self.state
                    .polls_after_eof
                    .set(self.state.polls_after_eof.get() + 1);
                Poll::Ready(None)
            }
            Phase::Yielded => {
                self.phase = Phase::Ended;
                Poll::Ready(None)
            }
            Phase::New => {
                let previous = self.state.first_polls.get();
                self.state.first_polls.set(previous + 1);
                if previous == 0 {
                    self.phase = Phase::Ended;
                    Poll::Ready(None)
                } else {
                    self.phase = Phase::Yielded;
                    Poll::Ready(Some(7))
                }
            }
        }
    }
}

#[test]
fn a_stream_ending_before_another_item_is_not_polled_again() {
    let state = Rc::new(EndState::default());
    let mut combined =
        futures::stream_select!(EndBeforeItem::new(&state), EndBeforeItem::new(&state),);
    let wake_count = Arc::new(WakeCount::default());
    let waker = Waker::from(Arc::clone(&wake_count));

    // Whichever input is shuffled first ends. The other yields exactly one
    // item, so this covers EOF-before-item without relying on RNG order.
    assert_eq!(poll_once(&mut combined, &waker), Poll::Ready(Some(7)));
    assert_eq!(state.first_polls.get(), 2);
    assert_eq!(poll_once(&mut combined, &waker), Poll::Ready(None));
    assert_eq!(state.polls_after_eof.get(), 0);
    assert_eq!(poll_once(&mut combined, &waker), Poll::Ready(None));
    assert_eq!(state.polls_after_eof.get(), 0);
    assert_eq!(combined.size_hint(), (0, Some(0)));
    assert_eq!(wake_count.0.load(Ordering::Relaxed), 0);
}

// A finite virtual stream: it owns no collection proportional to remaining.
// The lower bound can be deliberately loose, but is always <= remaining.
#[derive(Debug)]
struct Counted {
    remaining: usize,
    lower: usize,
}

impl Counted {
    fn new(lower: usize, remaining: usize) -> Self {
        assert!(
            lower <= remaining,
            "fixture requires a legal finite size hint"
        );
        Self { remaining, lower }
    }
}

impl Stream for Counted {
    type Item = ();

    fn poll_next(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<()>> {
        if self.remaining == 0 {
            Poll::Ready(None)
        } else {
            self.remaining -= 1;
            self.lower = self.lower.saturating_sub(1);
            Poll::Ready(Some(()))
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.lower, Some(self.remaining))
    }
}

#[test]
fn finite_size_bounds_sum_without_losing_precision() {
    let combined = futures::stream_select!(Counted::new(1, 3), Counted::new(2, 4));
    assert_eq!(combined.size_hint(), (3, Some(7)));
}

#[test]
fn an_unrepresentable_lower_sum_saturates_with_unknown_upper() {
    let combined =
        futures::stream_select!(Counted::new(usize::MAX, usize::MAX), Counted::new(1, 1),);
    assert_eq!(combined.size_hint(), (usize::MAX, None));
}

#[test]
fn an_unrepresentable_upper_sum_becomes_unknown() {
    let combined = futures::stream_select!(Counted::new(0, usize::MAX), Counted::new(0, 1),);
    assert_eq!(combined.size_hint(), (0, None));
}

#[test]
fn an_exactly_maximum_upper_bound_remains_representable() {
    let combined = futures::stream_select!(Counted::new(0, usize::MAX - 1), Counted::new(0, 1),);
    assert_eq!(combined.size_hint(), (0, Some(usize::MAX)));
}

#[derive(Debug)]
struct UnknownUpper;

impl Stream for UnknownUpper {
    type Item = ();

    fn poll_next(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<()>> {
        Poll::Pending
    }
}

#[test]
fn an_unknown_input_upper_bound_stays_unknown() {
    let combined = futures::stream_select!(UnknownUpper, Counted::new(0, 0),);
    assert_eq!(combined.size_hint(), (0, None));
}

#[derive(Debug)]
struct PendingOnce {
    pending: bool,
    item: Option<u8>,
}

impl Stream for PendingOnce {
    type Item = u8;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<u8>> {
        if self.pending {
            self.pending = false;
            cx.waker().wake_by_ref();
            Poll::Pending
        } else {
            Poll::Ready(self.item.take())
        }
    }
}

#[test]
fn pending_inputs_preserve_wakes_and_both_items() {
    let mut combined = futures::stream_select!(
        PendingOnce {
            pending: true,
            item: Some(1)
        },
        PendingOnce {
            pending: true,
            item: Some(2)
        },
    );
    let wake_count = Arc::new(WakeCount::default());
    let waker = Waker::from(Arc::clone(&wake_count));
    assert_eq!(poll_once(&mut combined, &waker), Poll::Pending);
    assert_eq!(wake_count.0.load(Ordering::Relaxed), 2);

    let first = match poll_once(&mut combined, &waker) {
        Poll::Ready(Some(item)) => item,
        other => panic!("both fixture inputs are ready after one Pending: {other:?}"),
    };
    let second = match poll_once(&mut combined, &waker) {
        Poll::Ready(Some(item)) => item,
        other => panic!("one fixture item must remain: {other:?}"),
    };
    let mut actual = [first, second];
    actual.sort();
    assert_eq!(actual, [1, 2]);
    assert_eq!(poll_once(&mut combined, &waker), Poll::Ready(None));
    assert_eq!(wake_count.0.load(Ordering::Relaxed), 2);
}
